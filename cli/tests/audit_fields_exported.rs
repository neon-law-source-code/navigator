//! The audit-export gate — every field the agent-authorization records emit
//! survives the collector's fail-closed key allow-list, and the list carries
//! nothing those records do not emit.
//!
//! ## The invariant
//!
//! The `target: "audit"` records in `portal/src/a2a.rs` are the only copy of an
//! agent-action authorization decision, and the decision recorded 2026-08-24 is
//! that the telemetry log *is* the audit record rather than a lossy view of one.
//! The collector's redaction processor is fail-closed — any attribute key absent
//! from `allowed_keys` is deleted before export — so that decision holds only
//! while the two agree, and nothing but this test makes them.
//!
//! Before ENG-320, three of those records' seventeen field names were
//! allow-listed. The exported form of an authorization record was an opaque
//! person, a step number, and an `audit` flag: it said that something
//! audit-tagged happened and nothing about what was authorized or how it was
//! decided.
//!
//! ## Why a scan rather than review
//!
//! Line coverage cannot see this. Every one of these call sites executes under
//! the existing suites, so coverage is satisfied while the invariant is entirely
//! unasserted — adding an eighteenth field that exports as a blank leaves the
//! suites green. Reading did not see it either: the issue that reported the gap
//! was written from the configuration and named four of the fourteen missing
//! fields.
//!
//! ## Why this covers `a2a.rs` alone
//!
//! `target: "audit"` is used well beyond these records — the API surface, the
//! OAuth front doors, CI and CLI token minting, the rate limiter. Those sites
//! emit `path`, `project_code`, `ip`, and `subject`, and their **absence** from
//! the allow-list is the control working: a resolved path, a Project code, and
//! an address are client-identifying, and the processor deleting them is the
//! point. Holding every audit site to this rule would demand allow-listing
//! exactly the values the allow-list exists to drop.
//!
//! The agent-authorization records are different in kind, not in degree: they
//! are content-free by construction, carrying identifiers and enums only, which
//! is what makes exporting all of them safe. So the gate is scoped to them.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// The module holding the agent-authorization records.
const SOURCE: &str = "portal/src/a2a.rs";

/// The collector whose redaction processor decides what leaves the cluster.
const COLLECTOR: &str = "examples/deploy/k8s/observability/otel-collector.yaml";

/// How many `target: "audit"` invocations that module holds.
///
/// Pinned so the scan cannot pass by finding nothing. A refactor that moves
/// these records elsewhere must move this gate with them rather than leave a
/// test that reads a file with no audit sites left in it and reports success.
const AGENT_AUTHORIZATION_SITES: usize = 4;

/// The export contract: every field name those records emit.
///
/// Written down rather than only discovered, so that adding a field to an audit
/// record is a decision taken in three places at once — the call site, this
/// contract, and the collector — instead of a diff that silently exports one
/// more thing or silently exports nothing.
const EXPORT_CONTRACT: &[&str] = &[
    // What happened, and how it was decided.
    "event",
    "decision",
    "audit",
    // What was acted on. Both spellings: the direct-dispatch record names a
    // skill where the confirmation records name a tool.
    "tool",
    "skill",
    // Who. `person_id` is the actor; the proposer pair is what lets a reader
    // tell an approval from a self-approval.
    "person_id",
    "role",
    "proposer_person_id",
    "proposer_role",
    "principal_kind",
    // Which conversation, and where in it.
    "task_id",
    "context_id",
    "step",
    // The gate's own outcome.
    "authorized",
    "confirmed",
    "confirmation_required",
    "routed_via_llm",
];

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every structured field name emitted by a `target: "audit"` invocation.
///
/// An invocation runs from the line carrying `target: "audit"` to the line that
/// closes it, and a field is either `name = <expr>` or the shorthand `name,`
/// — three of the direct-dispatch record's fields use the shorthand, so a scan
/// reading only the assigned form would miss `authorized` and
/// `confirmation_required` entirely.
fn emitted_audit_fields(source: &str) -> (BTreeSet<String>, usize) {
    let mut fields = BTreeSet::new();
    let mut sites = 0;
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if !line.contains(r#"target: "audit""#) || line.trim_start().starts_with("//") {
            continue;
        }
        sites += 1;
        for field in lines.by_ref() {
            let trimmed = field.trim();
            if trimmed == ");" {
                break;
            }
            if let Some(name) = field_name(trimmed) {
                fields.insert(name);
            }
        }
    }
    (fields, sites)
}

/// The field name on one line of an invocation, if it carries one.
fn field_name(trimmed: &str) -> Option<String> {
    let name = |candidate: &str| {
        let candidate = candidate.trim();
        (!candidate.is_empty()
            && candidate
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.'))
        .then(|| candidate.to_string())
    };
    if let Some((left, _)) = trimmed.split_once('=') {
        return name(left);
    }
    name(trimmed.strip_suffix(',')?)
}

/// The keys the redaction processor lets through.
fn allowed_keys(collector: &str) -> BTreeSet<String> {
    let block = collector
        .split_once("allowed_keys:")
        .expect("the collector declares an allowed_keys list")
        .1;
    let mut keys = BTreeSet::new();
    for line in block.lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match trimmed.strip_prefix("- ") {
            Some(key) => {
                keys.insert(key.trim().to_string());
            }
            // The next mapping key ends the list.
            None => break,
        }
    }
    keys
}

#[test]
fn every_agent_authorization_field_survives_the_export_boundary() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join(SOURCE)).expect("read the a2a module");
    let collector = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");

    let (emitted, sites) = emitted_audit_fields(&source);
    assert_eq!(
        sites, AGENT_AUTHORIZATION_SITES,
        "{SOURCE} holds {sites} `target: \"audit\"` invocations, not {AGENT_AUTHORIZATION_SITES}; \
         a scan that finds the wrong number is not reading what this gate is for"
    );

    let allowed = allowed_keys(&collector);
    let deleted: Vec<&String> = emitted.iter().filter(|f| !allowed.contains(*f)).collect();
    assert!(
        deleted.is_empty(),
        "the collector's fail-closed allow-list deletes these agent-authorization fields \
         before export, so the exported record does not carry them: {deleted:?}\nAdd each to \
         `allowed_keys` in {COLLECTOR}."
    );
}

#[test]
fn the_export_contract_matches_what_the_records_emit() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join(SOURCE)).expect("read the a2a module");
    let (emitted, _) = emitted_audit_fields(&source);
    let contract: BTreeSet<String> = EXPORT_CONTRACT.iter().map(|f| (*f).to_string()).collect();

    let undeclared: Vec<&String> = emitted.difference(&contract).collect();
    assert!(
        undeclared.is_empty(),
        "these fields are emitted but are not in the export contract: {undeclared:?}\nEach must \
         be an identifier or an enum rather than free text — the standing order in \
         `telemetry/src/lib.rs` — and a payload digest is not the privacy-preserving third \
         option it looks like; see `portal/src/audit_fields.rs`."
    );
    let stale: Vec<&String> = contract.difference(&emitted).collect();
    assert!(
        stale.is_empty(),
        "the export contract names fields no record emits any more: {stale:?}\nRemove them here \
         and from `allowed_keys`, so the allow-list does not accumulate keys nothing sends."
    );
}
