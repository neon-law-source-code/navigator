//! Grounding test for the third-party integration catalog.
//!
//! `docs/third-party-integrations.md` carries a table of every external
//! service the app dials — service, purpose, kind (binding/platform),
//! and the `<VENDOR>_*` env prefix that activates it. That table is
//! prose, so nothing stops it drifting from reality: a renamed env
//! scheme, a vendor added to the code but not the doc, a stale row.
//!
//! This test pins the table to the code the same way
//! `cli`'s `devx::gcp::deploy_workshop_prose_matches_the_dry_run_pipeline` pins
//! the deploy workshop: every env prefix the catalog names must exist in
//! `.env.example`, the kind column must stay a closed vocabulary over exactly
//! the services we ship, and the stub-fallback claim the doc makes for the
//! feature vendors must be backed by a real stub the code constructs.
//!
//! A row's kind decides how its credential is configured, so it decides what
//! there is to ground. `binding` and `platform` are deployment-wide: one set
//! of `<VENDOR>_*` variables per deployment, which must appear in
//! `.env.example`. `firm-owned` is not — the credential is a typed row the
//! Firm's Admin DRI writes, resolved from the Project's `firm_id`, so there
//! is no environment variable to find and asserting one would be asserting
//! the wrong contract. Naming an env prefix on a firm-owned row is therefore
//! itself a failure: it would mean a Firm credential had acquired a
//! deployment-wide fallback.

use std::path::Path;

/// One parsed catalog row.
struct Row {
    service: String,
    kind: String,
    env_tokens: Vec<String>,
}

/// Read a repo-root file relative to this crate (`web/` → workspace root
/// is one level up), matching the path convention the docs loader uses.
fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} — {e}", path.display()))
}

/// Pull the catalog table out of the doc: the `|`-rows that follow the
/// `## Current integrations` heading, minus the header and separator.
fn catalog_rows() -> Vec<Row> {
    let doc = repo_file("docs/third-party-integrations.md");
    let after = doc
        .split_once("## Current integrations")
        .expect("doc must have a `## Current integrations` section")
        .1;

    let mut rows = Vec::new();
    for line in after.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            // The table is a contiguous block; stop at the first
            // non-table line after we've started collecting.
            if !rows.is_empty() {
                break;
            }
            continue;
        }
        let cells: Vec<String> = line
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().to_string())
            .collect();
        if cells.len() != 4 {
            continue;
        }
        // Skip the header row and the `---` separator row.
        if cells[0] == "Service" || cells[0].starts_with("---") {
            continue;
        }
        let env_tokens = cells[3]
            .split(',')
            .map(|t| t.trim().trim_matches('`').to_string())
            .filter(|t| !t.is_empty())
            .collect();
        rows.push(Row {
            service: cells[0].clone(),
            kind: cells[2].clone(),
            env_tokens,
        });
    }
    rows
}

/// Every kind a catalog row may declare, and whether its credential is
/// configured per deployment.
const KINDS: [(&str, bool); 3] = [("binding", true), ("platform", true), ("firm-owned", false)];

fn deployment_configured(kind: &str) -> Option<bool> {
    KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, configured)| *configured)
}

#[test]
fn catalog_lists_exactly_the_services_we_ship() {
    let rows = catalog_rows();
    let mut services: Vec<&str> = rows.iter().map(|r| r.service.as_str()).collect();
    services.sort_unstable();
    assert_eq!(
        services,
        [
            "DocuSign",
            "Google Cloud",
            "Notion",
            "Restate Cloud",
            "SendGrid",
            "Slack",
            "Vertex AI",
            "Xero",
        ],
        "catalog must name exactly the external services the app dials",
    );
}

#[test]
fn catalog_kind_split_is_correct() {
    let rows = catalog_rows();
    for row in &rows {
        assert!(
            deployment_configured(&row.kind).is_some(),
            "{} has kind `{}` — must be one of {:?}",
            row.service,
            row.kind,
            KINDS.map(|(name, _)| name),
        );
    }
    let binding: Vec<&str> = rows
        .iter()
        .filter(|r| r.kind == "binding")
        .map(|r| r.service.as_str())
        .collect();
    // Binding vendors take legally/financially weighty action and follow
    // the two-account convention.
    assert_eq!(
        binding,
        ["DocuSign", "Xero"],
        "only DocuSign and Xero are binding vendors",
    );
    let firm_owned: Vec<&str> = rows
        .iter()
        .filter(|r| r.kind == "firm-owned")
        .map(|r| r.service.as_str())
        .collect();
    // A firm-owned vendor's credential is a Firm row resolved from the
    // Project's `firm_id`, never a deployment-wide variable.
    assert_eq!(
        firm_owned,
        ["Notion", "Slack"],
        "only Notion and Slack are firm-owned vendors",
    );
}

#[test]
fn every_catalog_env_prefix_exists_in_env_example() {
    let env_example = repo_file(".env.example");
    let rows = catalog_rows();
    for row in &rows {
        if deployment_configured(&row.kind) == Some(false) {
            // An env prefix is a screaming-snake `<VENDOR>_*` name, so an
            // uppercase letter in this cell is the tell.
            assert!(
                row.env_tokens
                    .iter()
                    .all(|token| !token.chars().any(char::is_uppercase)),
                "{} is firm-owned, so its credential is resolved from the \
                 Project's `firm_id` — naming an env prefix in `{:?}` would \
                 mean a Firm credential had gained a deployment-wide fallback",
                row.service,
                row.env_tokens,
            );
            continue;
        }
        assert!(
            !row.env_tokens.is_empty(),
            "{} names no env prefix in the catalog",
            row.service,
        );
        for token in &row.env_tokens {
            // A `<VENDOR>_*` prefix grounds on its stem. Every catalog row
            // names a prefix today, but a row naming a bare variable with no
            // `*` grounds on the whole name instead.
            let stem = token.trim_end_matches('*');
            assert!(
                env_example.contains(stem),
                "catalog row `{}` names `{}`, but `.env.example` has no `{}` — \
                 the doc has drifted from the env contract",
                row.service,
                token,
                stem,
            );
        }
    }
}

#[test]
fn feature_vendor_stub_fallback_is_real() {
    // The catalog promises every feature vendor "stubs until configured"
    // so a fresh checkout boots with no cloud account. Ground that claim
    // for the billing seam by constructing the stub the code falls back
    // to — if `StubBillingProvider` were removed or renamed, this fails
    // to compile, catching the drift at the source.
    let _stub = portal::billing::StubBillingProvider::new();
}
