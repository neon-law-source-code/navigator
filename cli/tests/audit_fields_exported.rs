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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

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
    // Which server-generated task to join, and where in it.
    "task_id",
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
            && !candidate.starts_with('.')
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

#[test]
fn a_multiline_expression_does_not_become_an_exported_field() {
    let source = r#"
        emit_audit({
            target: "audit",
            .tool_name,
            authorized,
        });
    "#;

    let (fields, sites) = emitted_audit_fields(source);

    assert_eq!(sites, 1);
    assert_eq!(fields, BTreeSet::from(["authorized".to_string()]));
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
fn every_telemetry_recorder_field_survives_the_export_boundary() {
    let root = workspace_root();
    let collector = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    let allowed = allowed_keys(&collector);
    let contracts = [
        ("record_web_visit", telemetry::WEB_VISIT_ATTRIBUTE_KEYS),
        (
            "record_funnel_event",
            telemetry::FUNNEL_EVENT_ATTRIBUTE_KEYS,
        ),
        (
            "record_funnel_step_with_meter",
            telemetry::FUNNEL_STEP_ATTRIBUTE_KEYS,
        ),
        ("record_auth_event", telemetry::AUTH_EVENT_ATTRIBUTE_KEYS),
        (
            "record_auth_sign_in_with_meter",
            telemetry::AUTH_SIGN_IN_ATTRIBUTE_KEYS,
        ),
    ];
    let missing: Vec<(&str, &str)> = contracts
        .iter()
        .flat_map(|(recorder, keys)| {
            keys.iter()
                .filter(|key| !allowed.contains(**key))
                .map(move |key| (*recorder, *key))
        })
        .collect();

    assert!(
        missing.is_empty(),
        "the collector's fail-closed allow-list deletes telemetry recorder fields before export: \
         {missing:?}; add each key to `allowed_keys` in {COLLECTOR}"
    );
}

#[test]
fn caller_context_is_not_exported_but_server_task_id_is_required() {
    let root = workspace_root();
    let collector = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    let allowed = allowed_keys(&collector);
    let contract: BTreeSet<String> = EXPORT_CONTRACT.iter().map(|f| (*f).to_string()).collect();

    assert!(
        contract.contains("task_id") && allowed.contains("task_id"),
        "server-generated task_id must remain in the static and collector export contracts"
    );
    assert!(
        !contract.contains("context_id") && !allowed.contains("context_id"),
        "caller-selected context_id must be excluded from both export contracts"
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

/// ENG-710: the OIDC/API/CI/rate-limit `target: "audit"` sites outside
/// `a2a.rs` (`portal/src/oauth.rs`, `portal/src/api_audit.rs`,
/// `portal/src/rate_limit.rs`, `portal/src/ci_auth.rs`) are deliberately NOT
/// pinned by [`every_agent_authorization_field_survives_the_export_boundary`]
/// — that gate is scoped to the content-free a2a.rs records alone (see the
/// module docs on "Why this covers `a2a.rs` alone"). But the *safe* fields
/// those other sites emit still need to survive redaction for a Dash0 log
/// query grouped by `event` to return the OIDC/API/CI/rate-limit events at
/// all, and nothing else guards that. A plain membership check against the
/// allow-list, not a source scan: the fields these sites deliberately do NOT
/// export (`path`, `project_code`, `ip`, `subject`, `repository`, `kid`,
/// `user_id`, `method`, `status`) are excluded on purpose, so asserting
/// equality against everything a scan finds would fight that design instead
/// of guarding it.
#[test]
fn oidc_api_ci_and_rate_limit_events_still_survive_redaction() {
    let root = workspace_root();
    let collector = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    let allowed = allowed_keys(&collector);

    for key in ["event", "role", "person_id"] {
        assert!(
            allowed.contains(key),
            "`{key}` must stay on the collector's allow-list: portal/src/oauth.rs, \
             portal/src/api_audit.rs, portal/src/rate_limit.rs, and portal/src/ci_auth.rs \
             all emit `target: \"audit\"` records carrying it, and losing it here silently \
             empties a Dash0 log query grouped by that key."
        );
    }
}

#[test]
fn dash0_is_an_additive_exporter_after_redaction_for_every_signal() {
    let root = workspace_root();
    let collector = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    // Dash0 routes on the `Dash0-Dataset` header exactly as spelled here. An
    // unrecognized routing header is ignored rather than rejected, so a typo
    // (or an `X-` prefix RFC 6648 retired) exports successfully into the
    // wrong dataset — a failure no exporter metric reports.
    assert!(collector.contains(
        "otlp/dash0:\n        endpoint: ${env:DASH0_ENDPOINT}\n        headers:\n          Authorization: \"Bearer ${env:DASH0_TOKEN}\"\n          Dash0-Dataset: \"${env:DASH0_DATASET}\""
    ));

    for signal in ["traces", "metrics", "logs"] {
        let marker = if signal == "traces" {
            "        traces:\n          receivers:".to_string()
        } else {
            format!("        {signal}:\n")
        };
        let start = collector
            .find(&marker)
            .unwrap_or_else(|| panic!("collector is missing the {signal} pipeline"));
        let pipeline = &collector[start..];
        let end = if signal == "traces" {
            pipeline
                .find("\n        traces/dash0:\n")
                .unwrap_or_else(|| {
                    ["metrics", "logs"]
                        .into_iter()
                        .filter_map(|candidate| pipeline.find(&format!("\n        {candidate}:\n")))
                        .min()
                        .unwrap_or(pipeline.len())
                })
        } else {
            ["traces", "metrics", "logs"]
                .into_iter()
                .filter(|candidate| *candidate != signal)
                .filter_map(|candidate| pipeline.find(&format!("\n        {candidate}:\n")))
                .min()
                .unwrap_or(pipeline.len())
        };
        let pipeline = &pipeline[..end];
        let redaction = pipeline
            .find("redaction")
            .expect("each signal pipeline runs redaction");
        let batch = pipeline
            .find("batch")
            .expect("each signal pipeline batches after redaction");
        assert!(redaction < batch, "{signal} redaction must precede batch");
        if signal == "traces" {
            assert!(
                pipeline.contains("exporters: [googlecloud]"),
                "Google Cloud keeps its existing trace lane"
            );
            assert!(
                collector.contains("traces/dash0:\n          receivers: [otlp]"),
                "Dash0 has a separate trace lane"
            );
            assert!(
                collector.contains("exporters: [otlp/dash0]"),
                "Dash0 receives the filtered trace lane"
            );
        } else {
            assert!(
                pipeline.contains("exporters: [googlecloud, otlp/dash0]"),
                "{signal} keeps googlecloud and fans out to Dash0"
            );
        }
    }
}

#[test]
fn metrics_aggregate_after_redaction_before_export() {
    let root = workspace_root();
    let collector = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    let start = collector
        .find("        metrics:\n          receivers:")
        .expect("collector is missing the metrics pipeline");
    let pipeline = &collector[start..];
    let end = pipeline
        .find("\n        logs:\n")
        .expect("metrics pipeline must end before the logs pipeline");
    let pipeline = &pipeline[..end];
    let processors = pipeline
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("processors: [")
                .and_then(|list| list.strip_suffix(']'))
        })
        .expect("metrics pipeline must declare processors")
        .split(',')
        .map(str::trim)
        .collect::<Vec<_>>();
    let redaction = processors
        .iter()
        .position(|processor| *processor == "redaction")
        .expect("metrics pipeline must redact");
    let aggregation = processors
        .iter()
        .position(|processor| *processor == "metricstransform")
        .expect("metrics pipeline must aggregate with metricstransform");
    let batch = processors
        .iter()
        .position(|processor| *processor == "batch")
        .expect("metrics pipeline must batch");
    assert!(
        redaction < aggregation && aggregation < batch,
        "metrics must aggregate after redaction and before batch: {processors:?}"
    );
    assert!(
        pipeline
            .find("processors:")
            .expect("metrics must declare processors")
            < pipeline
                .find("exporters:")
                .expect("metrics must declare exporters"),
        "metrics aggregation must be configured before export"
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SyntheticMetricPoint {
    resource: BTreeSet<String>,
    labels: BTreeMap<String, String>,
    value: u64,
}

fn aggregate_redacted_metric_points(
    mut points: Vec<SyntheticMetricPoint>,
    redacted_labels: &[&str],
) -> Vec<SyntheticMetricPoint> {
    for point in &mut points {
        if let Some(instance) = point.labels.remove("service_instance_id") {
            point
                .resource
                .insert(format!("service.instance.id={instance}"));
        }
        for label in redacted_labels {
            point.labels.remove(*label);
        }
        point.labels.remove("redaction_redacted_keys");
    }

    let mut grouped: BTreeMap<(BTreeSet<String>, BTreeMap<String, String>), u64> = BTreeMap::new();
    for point in points {
        let key = (point.resource, point.labels);
        *grouped.entry(key).or_default() += point.value;
    }
    grouped
        .into_iter()
        .map(|((resource, labels), value)| SyntheticMetricPoint {
            resource,
            labels,
            value,
        })
        .collect()
}

fn collector_config(collector: &str) -> serde_json::Value {
    serde_yaml::Deserializer::from_str(collector)
        .map(|document| serde_yaml::Value::deserialize(document).expect("YAML parses"))
        .find(|document: &serde_yaml::Value| {
            document.get("kind").and_then(serde_yaml::Value::as_str) == Some("ConfigMap")
                && document
                    .get("metadata")
                    .and_then(|metadata| metadata.get("name"))
                    .and_then(serde_yaml::Value::as_str)
                    == Some("otel-collector-config")
        })
        .and_then(|document| {
            document
                .get("data")
                .and_then(|data| data.get("config.yaml"))
                .and_then(serde_yaml::Value::as_str)
                .and_then(|config| serde_yaml::from_str::<serde_yaml::Value>(config).ok())
        })
        .and_then(|config| serde_json::to_value(config).ok())
        .expect("collector config parses")
}

#[test]
fn redaction_aggregates_colliding_points_and_preserves_instance_resource_identity() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    let config = collector_config(&source);
    let processors = config
        .pointer("/service/pipelines/metrics/processors")
        .and_then(serde_json::Value::as_array)
        .expect("metrics pipeline declares processors");
    let processors: Vec<&str> = processors
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let identity = processors
        .iter()
        .position(|processor| *processor == "transform/metric_identity")
        .expect("metrics moves service_instance_id into resource identity");
    let redaction = processors
        .iter()
        .position(|processor| *processor == "redaction")
        .expect("metrics pipeline redacts");
    let drop_summary = processors
        .iter()
        .position(|processor| *processor == "transform/drop_redaction_keys")
        .expect("metrics drops redaction_redacted_keys");
    let aggregation = processors
        .iter()
        .position(|processor| *processor == "metricstransform")
        .expect("metrics pipeline aggregates");
    let batch = processors
        .iter()
        .position(|processor| *processor == "batch")
        .expect("metrics pipeline batches");
    assert!(identity < redaction && redaction < drop_summary && drop_summary < aggregation);
    assert!(aggregation < batch);

    let label_set = config
        .pointer("/processors/metricstransform/transforms/0/operations/0/label_set")
        .and_then(serde_json::Value::as_array)
        .expect("metricstransform declares an aggregation label set");
    let label_set: BTreeSet<&str> = label_set
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(!label_set.contains("service_instance_id"));
    assert!(!label_set.contains("redaction_redacted_keys"));
    assert!(label_set.contains("service.instance.id"));

    let points = vec![
        SyntheticMetricPoint {
            resource: BTreeSet::new(),
            labels: BTreeMap::from([
                ("service_instance_id".into(), "otel-a".into()),
                ("country".into(), "US".into()),
                ("redaction_redacted_keys".into(), "country".into()),
            ]),
            value: 2,
        },
        SyntheticMetricPoint {
            resource: BTreeSet::new(),
            labels: BTreeMap::from([
                ("service_instance_id".into(), "otel-a".into()),
                ("country".into(), "CA".into()),
                ("redaction_redacted_keys".into(), "country".into()),
            ]),
            value: 3,
        },
    ];
    let output = aggregate_redacted_metric_points(points, &["country"]);
    assert_eq!(output.len(), 1, "redaction collisions become one series");
    assert_eq!(output[0].value, 5, "redaction preserves the total value");
    assert_eq!(
        output[0].resource,
        BTreeSet::from(["service.instance.id=otel-a".into()])
    );
    assert!(output[0].labels.is_empty());
}

#[test]
fn self_scrape_component_labels_survive_redaction_without_collapsing_series() {
    let root = workspace_root();
    let source = fs::read_to_string(root.join(COLLECTOR)).expect("read the collector config");
    let config = collector_config(&source);
    let component_labels = [
        "exporter",
        "processor",
        "data_type",
        "otel_signal",
        "receiver",
        "transport",
        "grpc_method",
        "grpc_status",
        "grpc_target",
    ];
    let allowed = allowed_keys(&source);
    for label in component_labels {
        assert!(
            allowed.contains(label),
            "self-scrape label {label:?} must survive fail-closed redaction"
        );
    }

    let label_set = config
        .pointer("/processors/metricstransform/transforms/0/operations/0/label_set")
        .and_then(serde_json::Value::as_array)
        .expect("metricstransform declares an aggregation label set");
    let label_set: BTreeSet<&str> = label_set
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for label in component_labels {
        assert!(
            label_set.contains(label),
            "self-scrape label {label:?} must remain a series dimension"
        );
    }

    let points = vec![
        SyntheticMetricPoint {
            resource: BTreeSet::from(["service.instance.id=otel-a".into()]),
            labels: BTreeMap::from([
                ("processor".into(), "batch".into()),
                ("redaction_redacted_keys".into(), String::new()),
            ]),
            value: 2,
        },
        SyntheticMetricPoint {
            resource: BTreeSet::from(["service.instance.id=otel-a".into()]),
            labels: BTreeMap::from([
                ("processor".into(), "memory_limiter".into()),
                ("redaction_redacted_keys".into(), String::new()),
            ]),
            value: 3,
        },
    ];
    let output = aggregate_redacted_metric_points(points, &[]);
    assert_eq!(
        output.len(),
        2,
        "distinct collector components remain distinct series"
    );
    assert_eq!(
        output.iter().map(|point| point.value).sum::<u64>(),
        5,
        "preserving component labels does not lose self-scrape points"
    );
}
