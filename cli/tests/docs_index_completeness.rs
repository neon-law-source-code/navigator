//! Guard that `docs/index.md` stays a real map instead of decaying back into a stub.
//!
//! ENG-382 found `docs/index.md` as a seven-line stub while `AGENTS.md` sent every agent to it
//! as "the documentation map." This test enumerates every Markdown file under `docs/` and fails,
//! naming each offender, when one has no entry in `docs/index.md` — so a new doc can land without
//! anyone remembering to link it back in, and CI catches the gap instead of the map going stale.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every `.md` file under `docs/`, as paths relative to `docs/` itself (e.g. `access-model.md`,
/// `deploy/gke-ship-example.md`), excluding `index.md` — the map does not need to list itself.
fn docs_markdown_files(docs_dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![docs_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            fs::read_dir(&dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()))
        {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                let relative = path
                    .strip_prefix(docs_dir)
                    .expect("path under docs_dir")
                    .to_string_lossy()
                    .replace('\\', "/");
                if relative != "index.md" {
                    files.push(relative);
                }
            }
        }
    }
    files.sort();
    files
}

#[test]
fn every_doc_has_an_entry_in_the_index() {
    let root = repo_root();
    let docs_dir = root.join("docs");
    let index = fs::read_to_string(docs_dir.join("index.md")).expect("read docs/index.md");

    let missing: Vec<String> = docs_markdown_files(&docs_dir)
        .into_iter()
        .filter(|relative| !index.contains(relative.as_str()))
        .collect();

    assert!(
        missing.is_empty(),
        "docs/index.md has no entry for: {}. Add a link under the right topic section.",
        missing.join(", ")
    );
}

#[test]
fn the_index_names_every_file_it_finds_in_a_synthetic_tree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let docs_dir = dir.path().join("docs");
    fs::create_dir_all(docs_dir.join("sub")).unwrap();
    fs::write(docs_dir.join("index.md"), "# Documentation\n").unwrap();
    fs::write(docs_dir.join("alpha.md"), "# Alpha\n").unwrap();
    fs::write(docs_dir.join("sub").join("beta.md"), "# Beta\n").unwrap();

    let mut files = docs_markdown_files(&docs_dir);
    files.sort();
    assert_eq!(
        files,
        vec!["alpha.md".to_string(), "sub/beta.md".to_string()]
    );
}
