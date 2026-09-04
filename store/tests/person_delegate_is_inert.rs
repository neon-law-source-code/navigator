//! No authorization path reads `person_delegate`.
//!
//! A delegation records that one client helps another with their matters —
//! the spouse who reads the portal on behalf of a client who has no email
//! address of their own. It stores that relationship and nothing else,
//! and the point is that the table is
//! **inert**: `persons.role` is the authorization tier,
//! `person_project_roles.participation` is the scope, and a delegation is
//! neither.
//!
//! That is not a scoping convenience, it is the safety property. A
//! delegation is the one table in the store whose whole subject matter is
//! "this person may stand in for that person", so it is the table most
//! likely to be mistaken for a grant. It is not one. Two properties hold
//! only because it is inert:
//!
//! - **A delegation cannot approve anything.**
//!   [`store::access::MatterViewer::ClientDri`] is the variant that
//!   carries plan approval. An estate plan approved through a borrowed
//!   session is a forged instruction, and the record could not tell it
//!   from a real one. What a delegate may *do* is a separate decision that
//!   has not been made; until it is, the answer is nothing.
//! - **A delegation cannot cross a role.** Both sides are confined to
//!   `client`, so the mechanism can never let a clerk act as an admin or
//!   one lawyer act as another. `store::delegations::grant` refuses that
//!   at the write path and `live_for_delegate` re-checks it at the read
//!   path, but neither would matter if an authorization surface joined the
//!   table directly and skipped both.
//!
//! **Why a source guard and not a behavioural one.** "No authorization
//! decision consults this table" is a property of every future decision,
//! not of the handful that exist today, and the way it would be violated
//! is a plausible one-line convenience — widening a visibility query to
//! union a delegate's subjects, or resolving a delegation inside a policy
//! input. A behavioural test would have to be written *after* someone had
//! already written that line. This one fails in the diff that adds it.
//!
//! Widening it takes an edit to this file, in a diff a reviewer sees. That
//! is deliberate. When a surface genuinely needs to act on a delegation,
//! the change a reviewer should be asked to approve is this file, not a
//! join buried in a query.
//!
//! Mirrors `cli/tests/external_identity_is_inert.rs`, which holds the same
//! property for `person_external_identity`.

use std::fs;
use std::path::{Path, PathBuf};

/// The authorization surfaces — every place a decision about who may do
/// what is made, plus the embedded Rego that middleware evaluates.
///
/// Matched as full relative paths, never bare file names, so an exempt
/// `mod.rs` cannot quietly exempt every other one. Kept deliberately
/// identical to the list in `cli/tests/external_identity_is_inert.rs`: two
/// guards over one set of surfaces should not disagree about what the set
/// is.
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
/// Lowercased before matching, so a differently-cased spelling is caught
/// too.
///
/// **The bare words `delegate` and `delegation` are deliberately NOT on
/// this list.** `store/src/access.rs` already uses "delegate" as an
/// ordinary English verb — "it deliberately does *not* delegate the firm
/// arm to …" — so a substring guard on the bare word would be red on
/// arrival and would stay red for a reason that has nothing to do with
/// this table. Every entry below is an identifier that can only mean
/// `store::delegations`. Tightening this list to the bare word is not a
/// tightening; it is a false positive that a later maintainer will silence
/// by deleting the test.
const FORBIDDEN_NAMES: &[&str] = &[
    "person_delegate",
    "delegations",
    "newdelegation",
    "delegationstate",
    "delegationerror",
];

/// The workspace root (this test crate is `store`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read authorization surface {}: {e}", path.display()))
}

#[test]
fn no_authorization_surface_names_the_delegation_table() {
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
        "`person_delegate` carries no authorization meaning: it records that \
         one client helps another, and it is not a grant. `persons.role` is \
         the tier and `person_project_roles.participation` is the scope. An \
         authorization surface must not read it — in particular a delegation \
         must never reach `MatterViewer::ClientDri`, which carries plan \
         approval, because an estate plan approved through a borrowed session \
         is indistinguishable in the record from one the client approved \
         themselves. If a surface genuinely needs to act on a delegation, \
         change this file so a reviewer sees it. Found {} occurrence(s):\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}

/// The guard is only as good as its list. A path that no longer exists is
/// an authorization surface this test silently stopped covering — most
/// likely because it was renamed, taking its exemption from scrutiny with
/// it.
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
        "every entry in AUTHORIZATION_SURFACES must be a real file, or the \
         guard has silently stopped covering it; update the path or drop the \
         entry:\n  {}",
        missing.join("\n  ")
    );
}

/// The list has to keep naming *authorization* surfaces, or it drifts into
/// a list of files that merely happen not to mention the table. Each one
/// must still carry a role, participation, or policy decision.
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
        "every entry in AUTHORIZATION_SURFACES must still make an \
         authorization decision; a file that no longer does belongs off the \
         list, and whatever replaced it belongs on:\n  {}",
        inert.join("\n  ")
    );
}

/// And the table this guard is about must still exist, or the whole thing
/// is asserting the absence of something that is gone anyway.
#[test]
fn the_table_this_guard_is_about_still_exists() {
    let module = repo_root().join("store/src/delegations.rs");
    assert!(
        module.is_file(),
        "store/src/delegations.rs is gone; if the table was removed, remove \
         this guard with it"
    );

    let definitions = read(&repo_root().join("store/src/schema/navigator.surql"));
    assert!(
        definitions.contains("DEFINE TABLE IF NOT EXISTS person_delegate"),
        "person_delegate is not defined in navigator.surql"
    );
}

/// The forbidden list must keep matching the module it guards. A renamed
/// type would leave this test asserting the absence of a name nothing
/// uses — green, and guarding nothing.
///
/// Checked against the module source rather than by importing the types,
/// because the point is the *spelling* an authorization surface would have
/// to write.
#[test]
fn every_forbidden_name_still_appears_in_the_module_it_guards() {
    let module = read(&repo_root().join("store/src/delegations.rs")).to_lowercase();
    let mut stale = Vec::new();
    for forbidden in FORBIDDEN_NAMES {
        // `delegations` is the module's own path, which the file does not
        // have to spell inside itself.
        if *forbidden != "delegations" && !module.contains(forbidden) {
            stale.push((*forbidden).to_string());
        }
    }
    assert!(
        stale.is_empty(),
        "every FORBIDDEN_NAMES entry must still be a real spelling in \
         store/src/delegations.rs, or this guard is watching for a name that \
         no longer exists; update the entry to the new spelling:\n  {}",
        stale.join("\n  ")
    );
}
