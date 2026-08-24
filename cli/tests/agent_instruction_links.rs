//! Prove that the agent instruction surfaces actually resolve in this checkout.
//!
//! `CLAUDE.md` is a symlink to `AGENTS.md`, and every entry under
//! `.claude/skills/` and `.codex/skills/` is a symlink into `.agents/skills/`.
//! That indirection is deliberate: one copy of each document, read by whichever
//! agent harness is pointed at the tree.
//!
//! Git only materialises a symlink as a link when `core.symlinks` is true, and
//! that is not the default on Windows. With it off, `git checkout` writes a
//! small regular file whose *contents* are the link target. `CLAUDE.md` becomes
//! nine bytes reading `AGENTS.md`, and `.claude/skills/council` becomes
//! twenty-eight bytes reading `../../.agents/skills/council`.
//!
//! Nothing reports this. The clone succeeds, the tree looks complete, `git
//! status` is clean, and the workspace builds and tests green, because no
//! compiled code reads either path. What breaks is invisible from inside the
//! repository: the harness loads `CLAUDE.md`, receives the literal string
//! `AGENTS.md` instead of the operating contract, and proceeds without it; and
//! it enumerates `.claude/skills/`, finds regular files where directories
//! holding a `SKILL.md` should be, and registers no skill. The engineering,
//! legal, and client councils then do not exist as far as that session is
//! concerned, and the only symptom is that invoking one does nothing.
//!
//! That failure mode is not hypothetical. It ran for a week across a primary
//! checkout and eight worktrees on one machine before anyone noticed, and it was
//! found by accident while investigating something else. A setup step nobody
//! knows to take is not a defence, so this guard makes the broken state loud at
//! the point where every other workspace invariant is checked.
//!
//! The assertions deliberately compare *resolved content* rather than asking
//! whether a path is a symlink. What matters is that the bytes an agent reads
//! are the document, not the route by which the filesystem delivered them, and a
//! content check states the requirement without pinning the mechanism.
//!
//! The fix, once per clone, is in [`AGENTS.md`](../../AGENTS.md).

use std::fs;
use std::path::{Path, PathBuf};

/// The workspace root, one level up from the `cli` crate.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The remedy, appended to every failure so the fix travels with the symptom.
const REMEDY: &str = "\n\nThis checkout materialised a symlink as a regular file \
     holding its own target path, which happens when `core.symlinks` is false \
     (the Windows default). Fix it once per clone:\n\
     \n    git config core.symlinks true\n\
     \n    git checkout -- CLAUDE.md .claude/skills .codex/skills\n\
     \nWindows also needs permission to create links: enable Developer Mode, or \
     run that from an elevated shell.";

/// `CLAUDE.md` must deliver the operating contract, not the path to it.
///
/// Compared by content rather than by link type: a harness reads bytes, and
/// this is the assertion that fails when those bytes are the nine-byte stub.
#[test]
fn claude_md_resolves_to_the_agent_contract() {
    let root = repo_root();
    let claude = fs::read_to_string(root.join("CLAUDE.md"))
        .unwrap_or_else(|e| panic!("read CLAUDE.md: {e}{REMEDY}"));
    let agents = fs::read_to_string(root.join("AGENTS.md"))
        .unwrap_or_else(|e| panic!("read AGENTS.md: {e}"));

    assert_eq!(
        claude.len(),
        agents.len(),
        "CLAUDE.md is {} bytes and AGENTS.md is {} bytes, so CLAUDE.md is not \
         resolving to the contract. Agents loading CLAUDE.md are running \
         without it.{REMEDY}",
        claude.len(),
        agents.len(),
    );
    assert_eq!(
        claude, agents,
        "CLAUDE.md and AGENTS.md are the same length but differ in content.{REMEDY}"
    );
}

/// Every skill entry must resolve to a directory holding a readable `SKILL.md`.
///
/// A stub checks out as a regular file, so it fails the directory test first;
/// the `SKILL.md` read then covers a link that resolves somewhere unexpected.
/// Both harness directories are checked because they carry different subsets and
/// a fix applied to one has been known to miss the other.
#[test]
fn every_skill_entry_resolves_to_a_readable_skill() {
    for harness in [".claude/skills", ".codex/skills"] {
        let dir = repo_root().join(harness);
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|e| panic!("enumerate {}: {e}", dir.display()));

        assert!(
            !entries.is_empty(),
            "{harness} holds no skills at all, which cannot be right."
        );

        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                path.is_dir(),
                "{harness}/{name} is not a directory. A skill is a directory \
                 holding SKILL.md, so nothing will register it.{REMEDY}"
            );
            assert_skill_readable(&path, harness, &name);
        }
    }
}

/// Read one skill's `SKILL.md` and require it to carry a document.
///
/// The length floor is a sanity bound rather than a style rule: it separates a
/// real skill from an empty or truncated file without asserting anything about
/// what a skill has to say.
fn assert_skill_readable(path: &Path, harness: &str, name: &str) {
    let manifest = path.join("SKILL.md");
    let body = fs::read_to_string(&manifest).unwrap_or_else(|e| {
        panic!("read {harness}/{name}/SKILL.md: {e}{REMEDY}");
    });
    assert!(
        body.len() > 200,
        "{harness}/{name}/SKILL.md is only {} bytes, which is not a skill \
         document.{REMEDY}",
        body.len(),
    );
}
