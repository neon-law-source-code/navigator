//! No authorization path reads `person_external_identity`.
//!
//! ENG-85 stores the identifier each third-party system issues for a Person so
//! Navigator can name them in an API call — a GitHub numeric id, a Slack `U…`,
//! a Google `sub`. It stores nothing else, and the point of the issue is that
//! the table is **inert**: `persons.role` is the authorization tier and
//! `person_project_roles.participation` is the scope, and an external identity
//! is neither.
//!
//! That is not a scoping convenience, it is the safety property. Two rules in
//! the docs hold only because this table is inert:
//!
//! - a Clerk "never receives lawyer-work, advice, Git, MCP, or `/app/lawyer`
//!   authority by inheritance" ([`docs/glossary.md`](../../docs/glossary.md)),
//!   so a Clerk who *is* GitHub user `12345` gains nothing by being recorded as
//!   such; and
//! - Project participation never grants source-forge access
//!   ([`docs/project-repositories.md`](../../docs/project-repositories.md), ENG-45,
//!   ENG-49), so this table must not become the back door that reverses it.
//!
//! A row here is an address, not a key and not a permission. Looking someone up
//! does not mean they may be added to anything.
//!
//! **Why a source guard and not a behavioural one.** "No authorization decision
//! consults this table" is a property of every future decision, not of the
//! handful that exist today, and the way it would be violated is a plausible
//! one-line convenience — resolving a `github` identity inside a policy input,
//! or widening a visibility query to join it. A behavioural test would have to
//! be written *after* someone had already written that line. This one fails in
//! the diff that adds it.
//!
//! Widening it takes an edit to this file, in a diff a reviewer sees. That is
//! deliberate. If a surface here genuinely needs to *name* a Person in an
//! outbound call, the lookup belongs in the provisioning code that makes the
//! call, not in the code that decides whether the call is permitted.

use std::fs;
use std::path::{Path, PathBuf};

/// The authorization surfaces — every place a decision about who may do what is
/// made, plus the embedded Rego that middleware evaluates.
///
/// Matched as full relative paths, never bare file names, so an exempt
/// `mod.rs` cannot quietly exempt every other one.
const AUTHORIZATION_SURFACES: &[&str] = &[
    // The embedded policy and the middleware that evaluates it.
    "portal/policy/navigator.rego",
    "portal/policy/navigator_test.rego",
    "portal/src/policy.rs",
    // Session establishment: what a request's role and person come from.
    // `webhook_auth` is deliberately absent — it authenticates a provider's
    // signature over a request body and never resolves a Person, so it is
    // message authentication rather than an authorization decision.
    "portal/src/auth.rs",
    "portal/src/cli_auth.rs",
    // Row-level visibility: role + participation turned into queries.
    "store/src/access.rs",
    "store/src/participation.rs",
    // The Git transport's own authority.
    "store/src/git_access_tokens.rs",
    // The tier itself.
    "store/src/persons.rs",
];

/// The names that would mean an authorization surface had reached for this
/// table: the table, the module, and its public types.
///
/// Lowercased before matching, so a differently-cased spelling is caught too.
const FORBIDDEN_NAMES: &[&str] = &[
    "person_external_identity",
    "external_identities",
    "externalidentity",
    "externalsystem",
];

/// The workspace root (this test crate is `cli`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read authorization surface {}: {e}", path.display()))
}

#[test]
fn no_authorization_surface_names_the_external_identity_table() {
    let mut hits = Vec::new();
    for surface in AUTHORIZATION_SURFACES {
        let body = read(&repo_root().join(surface));
        for (index, line) in body.lines().enumerate() {
            let lowered = line.to_lowercase();
            for forbidden in FORBIDDEN_NAMES {
                if lowered.contains(forbidden) {
                    hits.push(format!("{surface}:{}: {}", index + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        hits.is_empty(),
        "`person_external_identity` carries no authorization meaning: it is an \
         address book, not a grant. `persons.role` is the tier and \
         `person_project_roles.participation` is the scope. An authorization \
         surface must not read it — resolve a Person to an account id in the \
         provisioning code that makes the outbound call, not in the code that \
         decides whether the call is permitted. Found {} occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// The guard is only as good as its list. A path that no longer exists is an
/// authorization surface this test silently stopped covering — most likely
/// because it was renamed, taking its exemption from scrutiny with it.
#[test]
fn every_listed_authorization_surface_still_exists() {
    let mut missing = Vec::new();
    for surface in AUTHORIZATION_SURFACES {
        if !repo_root().join(surface).is_file() {
            missing.push((*surface).to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "every entry in AUTHORIZATION_SURFACES must be a real file, or the guard \
         has silently stopped covering it; update the path or drop the entry:\n  {}",
        missing.join("\n  ")
    );
}

/// The list has to keep naming *authorization* surfaces, or it drifts into a
/// list of files that merely happen not to mention the table. Each one must
/// still carry a role, participation, or policy decision.
#[test]
fn every_listed_surface_still_makes_an_authorization_decision() {
    const AUTHORIZATION_MARKERS: &[&str] = &[
        "role",
        "participation",
        "allow",
        "policy",
        "session",
        "scope",
        "token",
    ];

    let mut inert = Vec::new();
    for surface in AUTHORIZATION_SURFACES {
        let body = read(&repo_root().join(surface)).to_lowercase();
        if !AUTHORIZATION_MARKERS
            .iter()
            .any(|marker| body.contains(marker))
        {
            inert.push((*surface).to_string());
        }
    }
    assert!(
        inert.is_empty(),
        "every entry in AUTHORIZATION_SURFACES must still make an authorization \
         decision; a file that no longer does belongs off the list, and whatever \
         replaced it belongs on:\n  {}",
        inert.join("\n  ")
    );
}

/// And the store module the guard is about must still exist, or the whole
/// thing is asserting the absence of something that is gone anyway.
#[test]
fn the_table_this_guard_is_about_still_exists() {
    let module = repo_root().join("store/src/external_identities.rs");
    assert!(
        module.is_file(),
        "store/src/external_identities.rs is gone; if the table was removed, \
         remove this guard with it"
    );

    let definitions = read(&repo_root().join("store/src/schema/navigator.surql"));
    assert!(
        definitions.contains("DEFINE TABLE IF NOT EXISTS person_external_identity"),
        "person_external_identity is not defined in navigator.surql"
    );
}
