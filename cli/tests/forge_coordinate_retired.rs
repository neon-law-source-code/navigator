//! No forge coordinate lives in Navigator's source.
//!
//! One invariant, two mechanical halves.
//!
//! **The six-organization vocabulary is retired.** Six deployment
//! organizations — `production-templates`, `staging-templates`, `nlf-templates`,
//! `production-applications`, `staging-applications`, `nlf-applications` —
//! collapsed to three, named for the entities they serve. Twelve files named at
//! least one of the six, and issues 02 through 11 removed them as a side effect
//! of their own work. A rename that wide is easy to half-finish and easy to
//! reintroduce: a doc paragraph pasted from an old one, a test fixture naming
//! the old organization. So the invariant is asserted rather than remembered.
//!
//! **No forge host is a *bare* literal in the files that read forge
//! configuration.** This is the sharper half, and it is the defect the collapse
//! removed: `portal::config` read `NAVIGATOR_GIT_HOST` with a **public forge as
//! the default**, so an unset variable silently pointed every Project's clone
//! URL at a namespace the Firm does not control — while `ops github setup`
//! deliberately had no such fallback and documented why. Two crates, opposite
//! rules, and the permissive one was the one serving users.
//!
//! A host default is legitimate now, and exactly one exists:
//! `cloud::workspace::DEFAULT_GIT_HOST`. What made the old fallback a defect was
//! never that a host had a default — it was that the default was anonymous,
//! undocumented, and reached by a variable nobody had set. So the rule is that
//! the default is *named*: [`is_named_default`] admits the one constant and its
//! uses, and [`the_host_default_is_declared_exactly_once`] refuses a second one.
//! Every other spelling of a forge host in these files still fails.
//!
//! **No Project repository URL is composed at all.** A Project's source is a
//! whole URL stored on the row (`store::projects::Project::repository_url`), on
//! whatever forge hosts it, so the derivation those two halves used to police is
//! gone rather than merely configured. The surviving host in configuration is
//! half of one `(host, organization)` coordinate — where a deployment's own
//! repositories are created, and the boundary `ops github setup` refuses a
//! governance write outside — and it never names a client matter's source.
//!
//! # Why the second half is scoped rather than tree-wide
//!
//! `github.com` appears legitimately all over this tree: dependency URLs in
//! `Cargo.lock`, generated third-party notices, `api.github.com` in the webhook
//! receiver, documentation links. A tree-wide refusal would be unsatisfiable and
//! would therefore be switched off. So the refusal is scoped to the files that
//! actually *compose a Project repository coordinate* — the place a stray host
//! literal becomes a wrong URL in front of a user — and [`COORDINATE_SOURCES`]
//! is asserted to exist so the scoping cannot go stale silently.
//!
//! # Scope is this repository alone
//!
//! Project repositories are separate repositories with their own CI. Nothing
//! here polices their contents; this guard walks this tree and nothing else.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The six retiring organization names. No allowlist: not one of them has a
/// legitimate surviving use, in source or in prose.
const RETIRED_ORGANIZATIONS: &[&str] = &[
    "production-templates",
    "staging-templates",
    "nlf-templates",
    "production-applications",
    "staging-applications",
    "nlf-applications",
];

/// Forge hosts that may not be spelled where a Project coordinate is composed.
///
/// One entry, and it used to be two: the second was the retired
/// `neon-law.ghe.com` tenant, which the migration rewrote to `github.com` and
/// left as a duplicate of the first.
const FORGE_HOSTS: &[&str] = &["github.com"];

/// The files that compose, render, or verify a Project repository coordinate.
///
/// Every one of these took a host or an organization from a literal before the
/// collapse. Each must now read configuration instead, so none may spell a forge
/// host at all. The list is explicit rather than a glob because that is the
/// claim: *these* are the coordinate-composing surfaces, and adding one is a
/// deliberate act.
const COORDINATE_SOURCES: &[&str] = &[
    "cloud/src/workspace.rs",
    "portal/src/config.rs",
    "portal/src/project_portal.rs",
    "cli/src/projects/doctor.rs",
    "cli/src/projects/repository.rs",
    "cli/src/devx/github_setup.rs",
    ".github/actions/validate/action.yml",
];

/// Files exempt by provenance rather than by name: only this test, whose own
/// refusal lists are written in the things it forbids.
const SKIPPED_FILES: &[&str] = &["forge_coordinate_retired.rs"];

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every file Git tracks, as a repo-relative path.
///
/// Asking Git rather than walking the filesystem answers all three traps a
/// tree-walking guard hits here at once:
///
/// - **`.git` is a *file*** inside a linked worktree checkout, not a directory,
///   so a walk that skips directories by name reads a path carrying the branch
///   name and fails on the reviewer's own branch.
/// - **`.claude/worktrees/` holds whole second copies of this tree.** One such
///   checkout contributed 1640 false hits to the previous guard. A guard that
///   fails only on the machine where someone is verifying a change is the worst
///   possible shape.
/// - **Scratch files in the working tree are not source.** A `/tmp`-style note
///   left in the checkout must not fail the run.
///
/// Git's index answers all three: it knows nothing about `target`,
/// `node_modules`, `.devx`, another worktree's files, or untracked scratch.
fn tracked_files() -> Vec<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root())
        .args(["ls-files", "-z"])
        .output()
        .expect("run `git ls-files`");
    assert!(
        output.status.success(),
        "`git ls-files` failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect();
    assert!(
        files.len() > 100,
        "expected a tracked file list, got {} entries — this guard would pass vacuously",
        files.len()
    );
    files
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether `haystack` references `retired` as a whole hyphen-delimited slug,
/// rather than embedding it inside a longer identifier.
///
/// The retired names are complete organization slugs (`staging-applications`).
/// The mandatory `<deployment>-applications` bucket lane (ENG-126) resolves to
/// names like `neon-law-stg-applications` that carry the substring while
/// being a different identifier, so a bare `contains` false-positives on the
/// bucket. Matching whole `[a-z0-9-]+` runs keeps the true invariant — no
/// retired ORGANIZATION survives — without flagging a bucket that merely shares
/// the `-applications` suffix.
fn names_retired_org(haystack: &str, retired: &str) -> bool {
    haystack
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .any(|run| run == retired)
}

/// Not one of the six organization names survives, anywhere Git tracks.
///
/// Prose counts. Two of the twelve files that named them were workshop content
/// rather than source, and a code-only grep misses exactly those.
#[test]
fn no_retired_organization_name_survives() {
    let mut hits = Vec::new();
    for path in tracked_files() {
        if SKIPPED_FILES.contains(&basename(&path)) {
            continue;
        }
        // A path can carry the name without any line doing so.
        for retired in RETIRED_ORGANIZATIONS {
            if names_retired_org(&path, retired) {
                hits.push(format!("{path}: path names a retired organization"));
            }
        }
        let Ok(body) = std::fs::read_to_string(repo_root().join(&path)) else {
            continue;
        };
        for (index, line) in body.lines().enumerate() {
            for retired in RETIRED_ORGANIZATIONS {
                if names_retired_org(line, retired) {
                    hits.push(format!("{path}:{}: {}", index + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the six deployment organizations collapsed to three, and the surviving three are \
         configuration (NAVIGATOR_GITHUB_ORG) rather than names in source. Found {} \
         occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// Whether a line is a comment rather than code.
///
/// Comments are excluded on purpose, and the exclusion is narrow rather than
/// convenient: prose *about* a forge — that the Actions App ID differs per host,
/// that a handle from one host resolves nowhere on another — is exactly the
/// context a reader needs, and a comment cannot compose a URL. What this test is
/// for is the *executable* literal: `.unwrap_or_else(|| "github.com".into())`
/// was a real default serving real users. Organization names are held to the
/// stricter rule and refused in prose too, by the test above.
fn is_comment(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with("--")
}

/// The one place a forge host may be spelled: the declaration of the named
/// default, and the code that reads it by name.
///
/// A default reached through this constant is a decision a reader can find and
/// a test can pin. The defect this guard exists for was the opposite shape — an
/// anonymous `"github.com"` inlined into an `unwrap_or_else`, which is why the
/// admission is by *name* rather than by file or by line number.
fn is_named_default(line: &str) -> bool {
    line.contains("DEFAULT_GIT_HOST")
}

/// No forge host is a bare literal in the code that composes a Project
/// coordinate.
///
/// A literal in one of these files that is not the named default is either a
/// fallback nobody chose or a fixture asserting one deployment's spelling, and
/// both are how the permissive default got there the first time.
#[test]
fn no_forge_host_is_a_bare_literal_where_a_coordinate_is_composed() {
    let mut hits = Vec::new();
    for source in COORDINATE_SOURCES {
        let path = repo_root().join(source);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (index, line) in body.lines().enumerate() {
            if is_comment(line) || is_named_default(line) {
                continue;
            }
            let lowered = line.to_lowercase();
            for host in FORGE_HOSTS {
                if lowered.contains(host) {
                    hits.push(format!("{source}:{}: {}", index + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "every forge value these files read comes from configuration — NAVIGATOR_GITHUB_ORG, \
         which has no default, and NAVIGATOR_GIT_HOST, whose one default is the named \
         DEFAULT_GIT_HOST — and a Project's own repository is a stored URL, not a composed one. \
         A forge host spelled here otherwise is either a fallback nobody chose or a fixture \
         pinning one deployment's spelling. Found {} occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// The named default is declared once, in the module that owns the coordinate.
///
/// [`is_named_default`] admits every line mentioning the constant, so a second
/// declaration elsewhere would inherit the admission and reintroduce exactly the
/// per-crate disagreement the collapse removed: two defaults, one of them the
/// permissive one, and no way to tell from a call site which was in force.
#[test]
fn the_host_default_is_declared_exactly_once() {
    let mut declarations = Vec::new();
    for path in tracked_files() {
        let is_rust = Path::new(&path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"));
        if SKIPPED_FILES.contains(&basename(&path)) || !is_rust {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(repo_root().join(&path)) else {
            continue;
        };
        if body.contains("const DEFAULT_GIT_HOST") {
            declarations.push(path);
        }
    }
    assert_eq!(
        declarations,
        vec!["cloud/src/workspace.rs".to_string()],
        "the host default is one named constant in cloud::workspace and nowhere else",
    );
}

/// The scoped list has to stay real, or the second half quietly stops meaning
/// anything.
///
/// A path that no longer exists would be skipped by a `filter_map` somewhere and
/// the guard would keep passing while checking less than it claims — the same
/// failure mode as a misspelled allowlist entry.
#[test]
fn every_coordinate_source_still_exists_and_is_tracked() {
    let tracked: BTreeSet<String> = tracked_files().into_iter().collect();
    let missing: Vec<&&str> = COORDINATE_SOURCES
        .iter()
        .filter(|source| !tracked.contains(**source))
        .collect();
    assert!(
        missing.is_empty(),
        "these coordinate-composing sources are not tracked files; a renamed or deleted entry \
         makes the host check silently narrower than it claims: {missing:?}"
    );
}

/// Both halves of the surviving forge coordinate are read from configuration,
/// each to its own rule.
///
/// This is the positive half. The negative tests above prove no host is *bare*;
/// this one proves the values are actually *read*, so the guard cannot be
/// satisfied by a file that stopped reading configuration at all.
///
/// The two keys are one coordinate resolved in one place. `NAVIGATOR_GITHUB_ORG`
/// is the organization this deployment's own automation occupies and has no
/// right answer, so a named deployment must state it. `NAVIGATOR_GIT_HOST` is
/// the host that organization lives on and has one, so absence resolves to
/// `DEFAULT_GIT_HOST` while a blank value — a configuration templated and never
/// filled in — still fails closed. Neither composes a Project's repository URL:
/// see [`no_project_repository_url_is_composed_from_a_project_code`].
#[test]
fn both_halves_of_the_forge_coordinate_are_read_from_configuration() {
    let workspace = std::fs::read_to_string(repo_root().join("cloud/src/workspace.rs"))
        .expect("read cloud/src/workspace.rs");
    for key in ["NAVIGATOR_GITHUB_ORG", "NAVIGATOR_GIT_HOST"] {
        assert!(
            workspace.contains(key),
            "cloud::workspace must read {key} rather than naming a forge coordinate",
        );
    }

    // A named deployment with no organization fails closed. Asserted against the
    // real resolver rather than against the file's text, because what matters is
    // the behaviour and not the spelling.
    let named = |key: &str| (key == "NAVIGATOR_GCP_PROJECT_ID").then(|| "neon-law".to_string());
    assert_eq!(
        cloud::workspace::WorkspaceConfig::from_lookup(named).unwrap_err(),
        cloud::workspace::WorkspaceConfigError::MissingCoordinate("NAVIGATOR_GITHUB_ORG"),
        "a named deployment with no organization must not resolve",
    );

    // The host half resolves without being named, and refuses to resolve when it
    // is named blank.
    let configured = |host: &'static str| {
        move |key: &str| match key {
            "NAVIGATOR_GCP_PROJECT_ID" => Some("neon-law".to_string()),
            "NAVIGATOR_GITHUB_ORG" => Some("an-organization".to_string()),
            "NAVIGATOR_GIT_HOST" if !host.is_empty() => Some(host.to_string()),
            _ => None,
        }
    };
    assert_eq!(
        cloud::workspace::WorkspaceConfig::from_lookup(configured(""))
            .expect("an unnamed host resolves")
            .host,
        cloud::workspace::DEFAULT_GIT_HOST,
    );
    // The one place the default's *value* is spelled. `cloud::workspace` may not
    // spell it twice — the constant is the declaration and a fixture repeating
    // it would be a second spelling to keep true — and this file is the one
    // exempt by provenance, because its own refusal lists are written in the
    // things it forbids.
    assert_eq!(cloud::workspace::DEFAULT_GIT_HOST, "github.com");
    assert_eq!(
        cloud::workspace::WorkspaceConfig::from_lookup(configured("   ")).unwrap_err(),
        cloud::workspace::WorkspaceConfigError::MissingCoordinate("NAVIGATOR_GIT_HOST"),
        "a blank host is a misconfigured deployment, not an absent one",
    );

    // And no deployment named stays the benign absence it is: the local loop and
    // this test suite operate no deployment.
    assert_eq!(
        cloud::workspace::WorkspaceConfig::from_lookup(|_| None).unwrap_err(),
        cloud::workspace::WorkspaceConfigError::MissingDeployment,
    );
}

/// A Project's repository URL is **stored data**, never composed.
///
/// This is the invariant that replaced the derivation, and it is the one a
/// future change is most likely to undo by reintroducing a convenience helper.
/// A Project's source may live on any forge in any organization, so composing
/// `{host}/{org}/{code}` would both invent a URL for a Project that has none and
/// silently override one that names somewhere else.
#[test]
fn no_project_repository_url_is_composed_from_a_project_code() {
    let workspace = std::fs::read_to_string(repo_root().join("cloud/src/workspace.rs"))
        .expect("read cloud/src/workspace.rs");
    for banned in ["project_repository", "RepositoryCoordinate"] {
        assert!(
            !workspace.contains(banned),
            "`{banned}` composes a Project repository coordinate; a Project's repository is \
             `store::projects::Project::repository_url`, a whole URL on any forge",
        );
    }

    // The positive half: the column is what carries it, and it is validated
    // rather than trusted.
    assert!(
        store::projects::is_valid_repository_url("https://gitlab.example/a-group/a-project"),
        "any forge must be storable",
    );
    assert!(
        !store::projects::is_valid_repository_url("https://forge.example"),
        "a forge root is not a repository",
    );
}
