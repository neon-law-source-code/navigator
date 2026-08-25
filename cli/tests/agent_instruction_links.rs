//! Prove that the agent instruction surfaces actually resolve in this checkout.
//!
//! `CLAUDE.md` is a symlink to `AGENTS.md`. `.agents/skills/` holds the one
//! canonical skill catalog, and every entry under `.claude/skills/` and
//! `.codex/skills/` is a symlink into it. That indirection is deliberate: one
//! copy of each document, read by whichever agent harness is pointed at the tree.
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
//! it enumerates a harness catalog, finds regular files where directories
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
//! The assertions deliberately compare *resolved content* and catalog names
//! rather than asking whether a path is a symlink. What matters is that the
//! bytes an agent reads are the document and that every harness exposes the same
//! skills, not the route by which the filesystem delivered them.
//!
//! The fix, once per clone, is in [`AGENTS.md`](../../AGENTS.md).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const CANONICAL_SKILLS: &str = ".agents/skills";
const HARNESSES: [&str; 2] = [".claude/skills", ".codex/skills"];

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
    assert_contract_resolves(&repo_root()).unwrap_or_else(|error| panic!("{error}{REMEDY}"));
}

/// Every harness must expose precisely the canonical catalog, and every entry
/// must resolve to a directory holding a readable `SKILL.md`.
///
/// A stub checks out as a regular file, so it fails the directory test first;
/// the `SKILL.md` read then covers a link that resolves somewhere unexpected.
#[test]
fn harness_skill_catalogs_match_the_canonical_catalog() {
    assert_harness_catalogs_resolve(&repo_root()).unwrap_or_else(|error| panic!("{error}{REMEDY}"));
}

/// The guard itself must reject the regular-file form Git writes when it cannot
/// materialise `CLAUDE.md` as a symlink.
#[test]
fn contract_guard_rejects_a_symlink_stub() {
    let temp = tempfile::tempdir().expect("create temporary checkout");
    fs::write(temp.path().join("AGENTS.md"), "the agent contract\n").expect("write contract");
    fs::write(temp.path().join("CLAUDE.md"), "AGENTS.md").expect("write stub");

    let error = assert_contract_resolves(temp.path()).expect_err("stub must fail");
    assert!(error.contains("not resolving to the contract"), "{error}");
}

/// The catalog guard must reject the regular-file form Git writes for a skill
/// directory without mutating the repository that runs the test.
#[test]
fn catalog_guard_rejects_a_symlink_stub() {
    let temp = tempfile::tempdir().expect("create temporary checkout");
    write_skill(temp.path(), CANONICAL_SKILLS, "council");
    for harness in HARNESSES {
        fs::create_dir_all(temp.path().join(harness)).expect("create harness directory");
    }
    fs::write(
        temp.path().join(".claude/skills/council"),
        "../../.agents/skills/council",
    )
    .expect("write stub");
    write_skill(temp.path(), ".codex/skills", "council");

    let error = assert_harness_catalogs_resolve(temp.path()).expect_err("stub must fail");
    assert!(
        error.contains(".claude/skills/council is not a directory"),
        "{error}"
    );
}

fn assert_contract_resolves(root: &Path) -> Result<(), String> {
    let claude = fs::read_to_string(root.join("CLAUDE.md"))
        .map_err(|error| format!("read CLAUDE.md: {error}"))?;
    let agents = fs::read_to_string(root.join("AGENTS.md"))
        .map_err(|error| format!("read AGENTS.md: {error}"))?;

    if claude.len() != agents.len() {
        return Err(format!(
            "CLAUDE.md is {} bytes and AGENTS.md is {} bytes, so CLAUDE.md is not \
             resolving to the contract. Agents loading CLAUDE.md are running without it.",
            claude.len(),
            agents.len(),
        ));
    }
    if claude != agents {
        return Err("CLAUDE.md and AGENTS.md are the same length but differ in content.".into());
    }

    Ok(())
}

fn assert_harness_catalogs_resolve(root: &Path) -> Result<(), String> {
    let canonical = skill_names(root, CANONICAL_SKILLS)?;
    for harness in HARNESSES {
        let skills = skill_names(root, harness)?;
        if skills != canonical {
            return Err(format!(
                "{harness} does not expose the canonical skill catalog. Expected \
                 {canonical:?}; found {skills:?}."
            ));
        }
    }

    Ok(())
}

fn skill_names(root: &Path, catalog: &str) -> Result<BTreeSet<String>, String> {
    let dir = root.join(catalog);
    let entries = fs::read_dir(&dir)
        .map_err(|error| format!("read {}: {error}", dir.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("enumerate {}: {error}", dir.display()))?;

    if entries.is_empty() {
        return Err(format!(
            "{catalog} holds no skills at all, which cannot be right."
        ));
    }

    entries
        .into_iter()
        .map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() {
                return Err(format!(
                    "{catalog}/{name} is not a directory. A skill is a directory \
                     holding SKILL.md, so nothing will register it."
                ));
            }
            assert_skill_readable(&path, catalog, &name)?;
            Ok(name)
        })
        .collect()
}

/// Read one skill's `SKILL.md` and require it to carry a document.
///
/// The length floor is a sanity bound rather than a style rule: it separates a
/// real skill from an empty or truncated file without asserting anything about
/// what a skill has to say.
fn assert_skill_readable(path: &Path, harness: &str, name: &str) -> Result<(), String> {
    let manifest = path.join("SKILL.md");
    let body = fs::read_to_string(&manifest)
        .map_err(|error| format!("read {harness}/{name}/SKILL.md: {error}"))?;
    if body.len() <= 200 {
        return Err(format!(
            "{harness}/{name}/SKILL.md is only {} bytes, which is not a skill document.",
            body.len(),
        ));
    }

    Ok(())
}

fn write_skill(root: &Path, catalog: &str, name: &str) {
    let path = root.join(catalog).join(name);
    fs::create_dir_all(&path).expect("create skill directory");
    fs::write(path.join("SKILL.md"), "x".repeat(201)).expect("write skill manifest");
}
