//! Prove that the one agent catalog actually resolves in this checkout.
//!
//! [`AGENTS.md`](../../AGENTS.md) is the single operating contract and
//! `.agents/skills/` the single skill catalog. There is no `CLAUDE.md`, no
//! `.claude/`, and no `.codex/`: the harness-specific mirrors this repository
//! used to carry are gone, and with them the symlinks that made them work.
//!
//! That arrangement failed in a way nothing reported. Git materialises a
//! symlink as a link only when `core.symlinks` is true, which is not the
//! Windows default, and with it off `git checkout` wrote a small regular file
//! whose *contents* were the link target. `CLAUDE.md` became nine bytes reading
//! `AGENTS.md`; `.claude/skills/council` became twenty-eight bytes reading
//! `../../.agents/skills/council`. The clone succeeded, the tree looked
//! complete, `git status` was clean, and the workspace built and tested green,
//! because no compiled code read either path. What broke was invisible from
//! inside the repository: a harness loaded `CLAUDE.md`, received the literal
//! string `AGENTS.md` instead of the operating contract, and proceeded without
//! it; it enumerated a catalog, found regular files where directories holding a
//! `SKILL.md` should be, and registered no skill. The engineering, legal, and
//! client councils then did not exist as far as that session was concerned, and
//! the only symptom was that invoking one did nothing. It ran that way for a
//! week across a primary checkout and eight worktrees before anyone noticed,
//! and it was found by accident while investigating something else.
//!
//! Deleting the mirrors removed the failure rather than guarding it: with no
//! symlink anywhere in the tree, `core.symlinks` cannot produce a stub. So these
//! assertions hold the shape that makes that true — one catalog that resolves,
//! no mirror committed beside it, and no symlink to mis-materialise.
//!
//! Committed-ness is the right question for the mirrors, not existence. A
//! developer's own `.claude/` is local state now (it holds the harness's linked
//! checkouts), and personal skills kept there are their business; what must not
//! come back is a mirror in the tree everyone clones.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CANONICAL_SKILLS: &str = ".agents/skills";

/// Paths that must never be tracked again, each the root of a retired mirror.
const RETIRED_MIRRORS: [&str; 3] = ["CLAUDE.md", ".claude/", ".codex/"];

/// The workspace root, one level up from the `cli` crate.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every tracked path, with its mode, as `git ls-files -s` reports it.
///
/// Read from the index rather than the filesystem: the invariant is about what
/// a clone receives, and the working tree also carries ignored local state that
/// is nobody else's concern.
fn tracked_entries() -> Vec<(String, String)> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["ls-files", "-s", "-z"])
        .output()
        .expect("run `git ls-files`");
    assert!(
        output.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|record| !record.is_empty())
        .map(|record| {
            // `<mode> <object> <stage>\t<path>`
            let (meta, path) = record
                .split_once('\t')
                .unwrap_or_else(|| panic!("malformed `git ls-files -s` record: {record:?}"));
            let mode = meta
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("no mode in record: {record:?}"));
            (mode.to_string(), path.to_string())
        })
        .collect()
}

/// The canonical catalog must resolve: every entry a directory holding a
/// readable `SKILL.md`.
#[test]
fn the_canonical_catalog_resolves() {
    let names = skill_names(&repo_root(), CANONICAL_SKILLS)
        .unwrap_or_else(|error| panic!("{error}\n\n{REMEDY}"));
    assert!(
        names.contains("council"),
        "the canonical catalog is missing skills it is known to hold; found {names:?}"
    );
}

/// No harness mirror may be committed beside the catalog.
///
/// A mirror is how the symlink breakage got in, and re-adding one is the easy
/// mistake: it looks like making a skill work in one more tool.
#[test]
fn no_retired_mirror_is_tracked() {
    let offenders: Vec<String> = tracked_entries()
        .into_iter()
        .map(|(_, path)| path)
        .filter(|path| {
            RETIRED_MIRRORS
                .iter()
                .any(|mirror| path == mirror || path.starts_with(mirror))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "the agent contract is `AGENTS.md` and the catalog is `{CANONICAL_SKILLS}`; \
         these {} tracked path(s) re-introduce a retired mirror:\n  {}",
        offenders.len(),
        offenders.join("\n  ")
    );
}

/// Nothing in the tree may be a symlink.
///
/// This is the assertion that keeps the fix a fix. Mode `120000` is Git's
/// symlink mode, and it is the only thing `core.symlinks` can fail to
/// materialise — so while no tracked entry carries it, a clone cannot receive a
/// stub where a document should be, whatever the cloner has configured.
#[test]
fn nothing_tracked_is_a_symlink() {
    let links: Vec<String> = tracked_entries()
        .into_iter()
        .filter(|(mode, _)| mode == "120000")
        .map(|(_, path)| path)
        .collect();

    assert!(
        links.is_empty(),
        "a clone with `core.symlinks` off materialises each of these as a regular \
         file holding its own target path, silently and with a clean `git status`. \
         Store the document once and reference it by path instead. Found {} \
         symlink(s):\n  {}",
        links.len(),
        links.join("\n  ")
    );
}

/// The catalog guard must reject a non-directory entry without mutating the
/// repository that runs the test.
///
/// A stub checks out as a regular file, so the directory test is what catches
/// the shape this file exists to prevent.
#[test]
fn catalog_guard_rejects_a_non_directory_entry() {
    let temp = tempfile::tempdir().expect("create temporary checkout");
    write_skill(temp.path(), CANONICAL_SKILLS, "council");
    fs::write(
        temp.path().join(CANONICAL_SKILLS).join("stub"),
        "../../.agents/skills/council",
    )
    .expect("write stub");

    let error = skill_names(temp.path(), CANONICAL_SKILLS).expect_err("stub must fail");
    assert!(
        error.contains(".agents/skills/stub is not a directory"),
        "{error}"
    );
}

/// The catalog guard must reject a skill whose `SKILL.md` is empty or missing.
#[test]
fn catalog_guard_rejects_an_unreadable_skill() {
    let temp = tempfile::tempdir().expect("create temporary checkout");
    fs::create_dir_all(temp.path().join(CANONICAL_SKILLS).join("hollow"))
        .expect("create skill directory");

    let error = skill_names(temp.path(), CANONICAL_SKILLS).expect_err("hollow skill must fail");
    assert!(error.contains("hollow/SKILL.md"), "{error}");
}

/// The remedy, appended to a catalog failure so the fix travels with the
/// symptom.
const REMEDY: &str = "The canonical catalog is `.agents/skills/`: one directory \
     per skill, each holding a `SKILL.md`. Nothing mirrors it and nothing in the \
     tree is a symlink, so a missing skill is a missing directory rather than a \
     checkout problem.";

/// The redline skill must establish that its native Word capabilities exist
/// before it describes how to construct or verify tracked changes.
#[test]
fn redline_skill_preflights_native_word_capabilities() {
    let root = repo_root();
    let skill_path = root.join(CANONICAL_SKILLS).join("redline/SKILL.md");
    let skill = fs::read_to_string(&skill_path).expect("read canonical redline skill");

    let preflight_start = skill
        .find("## Capability preflight")
        .expect("redline skill must have a capability preflight");
    let construction_start = skill
        .find("## Build true tracked changes")
        .expect("redline skill must describe tracked-change construction");
    assert!(
        preflight_start < construction_start,
        "capability preflight must precede tracked-change construction"
    );

    let preflight = &skill[preflight_start..construction_start];
    let preflight_words = preflight.split_whitespace().collect::<Vec<_>>().join(" ");
    for capability in [
        "supported native Word writer",
        "accept/reject verification",
        "render-and-inspect path",
    ] {
        assert!(
            preflight.contains(capability),
            "capability preflight must name {capability}"
        );
    }
    assert!(
        preflight.contains("stop, explain the missing capability"),
        "capability preflight must stop when a required capability is unavailable"
    );
    assert!(
        preflight_words.contains("must not claim"),
        "capability preflight must prohibit a fabricated completion claim"
    );

    let routing = fs::read_to_string(
        root.join(CANONICAL_SKILLS)
            .join("redline/agents/openai.yaml"),
    )
    .expect("read redline OpenAI routing metadata");
    assert!(
        routing.contains("capability preflight first"),
        "OpenAI routing must require the capability preflight"
    );
    assert!(
        routing.contains("do not claim that a true Word redline was produced"),
        "OpenAI routing must preserve the preflight stop condition"
    );
}

/// The random-refactor skill must pick one tracked Rust file, compare it to
/// the workspace Rust conventions, The Rust Book, a local standard-library
/// clone, and similar repository patterns, then answer the review questions
/// before any edit.
#[test]
fn random_refactor_skill_grounds_a_file_against_book_stdlib_and_repo() {
    let root = repo_root();
    let skill_path = root.join(CANONICAL_SKILLS).join("random-refactor/SKILL.md");
    let skill = fs::read_to_string(&skill_path).expect("read canonical random-refactor skill");

    for required in [
        "/random-refactor",
        "git ls-files",
        ".agents/skills/rust/SKILL.md",
        "docs/rust-programming.md",
        "rust-lang/book",
        "library/std",
        "/tmp/navigator-rust-library",
        "/tmp/navigator-random-refactor/",
        "Is this actually needed?",
        "describe only the present",
        "rejected alternatives",
        "Is it tested?",
        "Is it documented?",
        "Presentations",
        "Workshops",
        "RUST_IN_PEACE.md",
        "similar patterns",
        "api-guidelines",
        "microsoft.github.io/rust-guidelines",
        "rust-by-example",
        "Klabnik",
    ] {
        assert!(
            skill.contains(required),
            "random-refactor skill must contain {required:?}"
        );
    }
}

/// Deliberate per-skill line-count ceilings, not measurements: raising one is
/// a conscious choice made when the catalog owner decides that skill should
/// grow. Add an entry here to put a new skill under the same policy instead
/// of writing another bespoke size-gate test.
const SKILL_DOCUMENTATION_SIZE_POLICIES: &[(&str, usize)] = &[("random-refactor", 81)];

/// Every skill named in [`SKILL_DOCUMENTATION_SIZE_POLICIES`] must stay at or
/// under its ceiling. One shared test enforces every policy so the check
/// does not need to be re-implemented per skill.
#[test]
fn skills_respect_documentation_size_policy() {
    let root = repo_root();
    let violations: Vec<String> = SKILL_DOCUMENTATION_SIZE_POLICIES
        .iter()
        .filter_map(|(name, max_lines)| {
            let relative_path = format!("{CANONICAL_SKILLS}/{name}/SKILL.md");
            let skill = fs::read_to_string(root.join(&relative_path))
                .unwrap_or_else(|error| panic!("read canonical {name} skill: {error}"));
            let lines = skill.lines().count();
            (lines > *max_lines).then(|| {
                format!(
                    "{relative_path} has {lines} lines; allowed maximum is {max_lines}. \
                     Trim the file or deliberately raise the budget."
                )
            })
        })
        .collect();

    assert!(
        violations.is_empty(),
        "documentation size policy violated:\n  {}",
        violations.join("\n  ")
    );
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
fn assert_skill_readable(path: &Path, catalog: &str, name: &str) -> Result<(), String> {
    let manifest = path.join("SKILL.md");
    let body = fs::read_to_string(&manifest)
        .map_err(|error| format!("read {catalog}/{name}/SKILL.md: {error}"))?;
    if body.len() <= 200 {
        return Err(format!(
            "{catalog}/{name}/SKILL.md is only {} bytes, which is not a skill document.",
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
