//! The brand identifier is `neon` and the host is `neonlaw.com`. This asserts
//! that over the whole tree.
//!
//! The practice is Neon Law, and `neon` is the one identifier its crates,
//! images, binaries, content root, tests, and telemetry carry. The invariant is
//! asserted rather than remembered because a crate directory, a Containerfile
//! `COPY` list, and a deployment name all have to agree, and they are edited by
//! different people at different times — so the check reads the whole tree
//! rather than a list of files someone has to keep current.
//!
//! `fiat` is the identifier this guards against, and the reason the match reads
//! a word boundary rather than a substring. It is ordinary English — "fiat
//! currency", "by fiat" — and it appears inside unrelated identifiers, so a
//! naive substring search would flag `fiat_shamir` in a vendored crypto crate
//! and every generated licence text that happens to use the word. What the
//! guard holds to `neon` is the identifier position: a crate, an image, a path
//! segment, a deployment, a GCP project.

use std::fs;
use std::path::{Path, PathBuf};

/// Identifiers that may not stand where `neon` belongs, lowercased. Each is
/// matched on a word boundary — see the module docs for why `fiat` cannot be a
/// substring search.
const NOT_NEON: &[&str] = &["fiat"];

/// Substrings that may legitimately contain a non-`neon` identifier,
/// lowercased. A line is cleared of every one of these before it is checked, so
/// a line carrying both an allowed name and a stray occurrence still fails.
///
/// Empty, and that is the point: `neon` holds every identifier position with no
/// carve-out. An entry earns its place here only by naming a value that still
/// has to exist somewhere in the tree, and
/// [`every_allowed_name_still_occurs_in_the_workspace`] holds it to that — an
/// exclusion that stops matching is an exclusion that has silently widened the
/// guard.
const ALLOWED: &[&str] = &[];

/// Directories the walk never descends into: build output, VCS metadata, and
/// the local dev-environment scratch dir.
/// `worktrees` (no dot) is the directory the harness creates linked checkouts
/// under — `.claude/worktrees/<branch>/` is a whole second copy of the tree.
/// Without it the walk reads another branch's files and reports them against
/// this one: a checkout of the rename branch alone contributed 1640 hits. CI
/// has no such directory, so the failure is local-only, which is the worst
/// shape for a guard — it cries wolf exactly where someone is trying to verify
/// a change and never where it would catch one.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    ".git",
    "node_modules",
    ".worktrees",
    "worktrees",
    ".devx",
    // Third-party source we vendor rather than author, and build output. A
    // minified bundle is one enormous line, so a single incidental match dumps
    // the whole file into the failure message and buries every real hit.
    "vendor",
    "dist",
];

/// File extensions the walk never reads: minified or compiled artifacts that
/// are generated rather than written, and unreadable as a diff either way.
const SKIPPED_EXTENSIONS: &[&str] = &["woff2", "woff", "png", "jpg", "jpeg", "svg", "ico", "wasm"];

/// Path fragments that mark generated web assets. These are emitted by
/// `esbuild` and `dx` into the served public tree, so they sit beside authored
/// files rather than under a directory of their own.
const SKIPPED_PATH_FRAGMENTS: &[&str] = &[
    "public/js/",
    "public/dioxus/",
    "public/swagger-ui/",
    ".min.js",
];

/// Files exempt by provenance rather than by name.
const SKIPPED_FILES: &[&str] = &[
    // This test, whose own table is written in the identifier it forbids.
    "brand_identifier_is_neon.rs",
    // Generated from the dependency tree, so `fiat` in them belongs to somebody
    // else's crate and no edit here would remove it.
    "THIRD-PARTY-NOTICES.txt",
    "Cargo.lock",
    // A developer's machine-local agent permissions, gitignored and never
    // committed. It accumulates the literal command strings someone once
    // approved, so a path they ran a year ago lives on in it. CI has no such
    // file, so a hit here fails only on the machine trying to verify a change
    // and never where it would catch one — the same shape as `worktrees` above.
    "settings.local.json",
];

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The line with every allowed name removed. Whatever remains is a genuine
/// leftover.
fn strip_allowed(line: &str) -> String {
    let mut out = line.to_lowercase();
    for allowed in ALLOWED {
        out = out.replace(allowed, " ");
    }
    out
}

/// Every [`NOT_NEON`] identifier occurring in `haystack` as a whole word.
///
/// "Whole word" means neither neighbour is alphanumeric, `_`, or `-`. That is
/// what separates an identifier position — `fiat-server`, `fiat/Entity.yaml`,
/// `--deployment fiat-staging` — from `fiat_shamir` in a vendored crate or
/// `ratify` in ordinary prose. The `-` is deliberately a boundary rather than a
/// word character: `fiat-registry` and `fiat-law` are exactly the shapes that
/// must read `neon`, and treating `-` as interior would let every one through.
fn non_neon_identifiers(haystack: &str) -> Vec<&'static str> {
    fn interior(byte: Option<u8>) -> bool {
        byte.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
    }
    let bytes = haystack.as_bytes();
    let mut hits = Vec::new();
    for needle in NOT_NEON {
        let mut from = 0;
        while let Some(offset) = haystack[from..].find(needle) {
            let at = from + offset;
            let before = at.checked_sub(1).map(|i| bytes[i]);
            let after = bytes.get(at + needle.len()).copied();
            if !interior(before) && !interior(after) {
                hits.push(*needle);
                break;
            }
            from = at + needle.len();
        }
    }
    hits
}

fn walk(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `.git` is a directory in a normal checkout and a *file* holding a
        // `gitdir:` pointer inside a worktree, so it has to be skipped by name
        // before the directory test — otherwise the walk reads a path carrying
        // the branch name and the guard fails on the reviewer's branch.
        if SKIPPED_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
            continue;
        }
        if SKIPPED_FILES.contains(&name.as_ref()) {
            continue;
        }
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| SKIPPED_EXTENSIONS.contains(&ext))
        {
            continue;
        }
        let as_posix = path.to_string_lossy().replace('\\', "/");
        if SKIPPED_PATH_FRAGMENTS
            .iter()
            .any(|fragment| as_posix.contains(fragment))
        {
            continue;
        }
        // A path can carry the identifier without any line doing so — a file
        // named `Containerfile.fiat`, or a `fiat/` crate directory.
        let relative = path.strip_prefix(repo_root()).unwrap_or(&path);
        let displayed = relative.to_string_lossy().to_string();
        for hit in non_neon_identifiers(&strip_allowed(&displayed)) {
            out.push(format!(
                "{displayed}: path names `{hit}` where `neon` belongs"
            ));
        }
        let Ok(body) = fs::read_to_string(&path) else {
            continue;
        };
        for (i, line) in body.lines().enumerate() {
            for hit in non_neon_identifiers(&strip_allowed(line)) {
                // Truncate: a generated or minified file is one enormous line,
                // and printing it whole buries every other hit in the report.
                let trimmed = line.trim();
                let excerpt: String = trimmed.chars().take(120).collect();
                let ellipsis = if trimmed.chars().count() > 120 {
                    "…"
                } else {
                    ""
                };
                out.push(format!(
                    "{displayed}:{}: `{hit}` in {excerpt}{ellipsis}",
                    i + 1
                ));
            }
        }
    }
}

#[test]
fn the_brand_identifier_is_neon_across_the_workspace() {
    let mut hits = Vec::new();
    walk(&repo_root(), &mut hits);
    assert!(
        hits.is_empty(),
        "the brand identifier is `neon` and the host is `neonlaw.com`. \
         Found {} occurrence(s) standing where `neon` belongs:\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// The exclusions have to stay reachable, or the guard silently widens: a
/// typo'd entry in `ALLOWED` would match nothing and the test would keep
/// passing while permitting less than it claims.
#[test]
fn every_allowed_name_still_occurs_in_the_workspace() {
    let mut unused = Vec::new();
    for allowed in ALLOWED {
        let mut found = false;
        let mut hits = Vec::new();
        // Reuse the walk by temporarily treating this name as the only
        // forbidden one: a name still present in the tree shows up as a hit
        // for every *other* allowed pattern's strip, so search directly.
        search(&repo_root(), allowed, &mut found, &mut hits);
        if !found {
            unused.push(*allowed);
        }
    }
    assert!(
        unused.is_empty(),
        "these exclusions match nothing in the workspace and should be dropped \
         (or are misspelled, which would let the identifier back in): {unused:?}"
    );
}

fn search(dir: &Path, needle: &str, found: &mut bool, hits: &mut Vec<String>) {
    if *found {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIPPED_DIRS.contains(&name.as_ref()) {
            continue;
        }
        if path.is_dir() {
            search(&path, needle, found, hits);
            continue;
        }
        if SKIPPED_FILES.contains(&name.as_ref()) {
            continue;
        }
        if let Ok(body) = fs::read_to_string(&path) {
            if body.to_lowercase().contains(needle) {
                *found = true;
                hits.push(path.display().to_string());
                return;
            }
        }
    }
}
