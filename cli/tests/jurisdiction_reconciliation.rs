//! Cross-crate reconciliation: `rules::f110::JURISDICTIONS` is the
//! validator's accepted notation-template jurisdiction vocabulary, while
//! `store/seeds/Jurisdiction.yaml` is the canonical reference data. Keep
//! them in sync without making the linter open a database.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct Seed {
    records: Vec<Record>,
}

#[derive(Debug, Deserialize)]
struct Record {
    code: String,
}

fn seeded_codes() -> std::collections::HashSet<String> {
    let seed: Seed = serde_yaml::from_str(store::seed::JURISDICTION_SEED_YAML)
        .expect("Jurisdiction.yaml parses against the schema-true shape");
    seed.records.into_iter().map(|r| r.code).collect()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate lives below the repo root")
        .to_path_buf()
}

fn template_paths(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read notation template directory") {
        let entry = entry.expect("read notation template entry");
        let path = entry.path();
        // The `github` shelf holds engineering intake (`kind: github`),
        // not legal templates: no jurisdiction, no respondent, no `code`.
        // Requiring a seeded jurisdiction of it would be a category error.
        if path
            .file_name()
            .is_some_and(|name| name == rules::GITHUB_SHELF)
        {
            continue;
        }
        if path.is_dir() {
            template_paths(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "md")
            && path.file_name().is_none_or(|name| name != "README.md")
        {
            out.push(path);
        }
    }
}

fn jurisdiction_from_frontmatter(contents: &str) -> Option<&str> {
    let mut lines = contents.lines();
    if lines.next() != Some("---") {
        return None;
    }
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some(value) = line.strip_prefix("jurisdiction:") {
            return Some(value.trim());
        }
    }
    None
}

#[test]
fn every_validator_jurisdiction_code_is_seeded() {
    let codes = seeded_codes();
    for (code, _prefix) in rules::JURISDICTIONS.iter() {
        assert!(
            codes.contains(code),
            "rules::f110 accepts jurisdiction `{code}`, but store/seeds/Jurisdiction.yaml has no row for it"
        );
    }
}

#[test]
fn every_template_jurisdiction_code_is_seeded() {
    let codes = seeded_codes();
    let root = repo_root().join("templates");
    let mut paths = Vec::new();
    template_paths(&root, &mut paths);
    assert!(!paths.is_empty(), "expected bundled notation templates");

    for path in paths {
        let contents = std::fs::read_to_string(&path).expect("read notation template");
        let jurisdiction = jurisdiction_from_frontmatter(&contents).unwrap_or_else(|| {
            panic!(
                "{} is missing required frontmatter `jurisdiction:`",
                path.strip_prefix(repo_root()).unwrap_or(&path).display()
            )
        });
        assert!(
            codes.contains(jurisdiction),
            "{} declares jurisdiction `{jurisdiction}`, but store/seeds/Jurisdiction.yaml has no row for it",
            path.strip_prefix(repo_root()).unwrap_or(&path).display()
        );
    }
}
