//! The guards for the disposable-staging lifecycle: the `kubectl` argv every
//! cluster-mutating command runs, the label round-trip that identifies a
//! managed environment, and the check that decides whether a target may be
//! destroyed.
//!
//! This is the safety boundary, so it is pure and unit-tested: nothing here
//! touches a cluster, which is what lets a test prove that a reset refuses a
//! production target and that every delete pins its context. The live-cluster
//! execution — the process plumbing that runs this argv — lives in
//! [`super::staging`], which is coverage-ignored for the same reason its
//! sibling orchestrators are.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use clap::Subcommand;
use store::DeploymentEnvironment;

pub const MANAGED_LABEL: &str = "app.kubernetes.io/part-of";
pub const ENVIRONMENT_LABEL: &str = "navigator.neonlaw.org/environment";
pub const ENVIRONMENT_ID_LABEL: &str = "navigator.neonlaw.org/environment-id";

pub const MANAGED_VALUE: &str = "navigator";
pub const STAGING_VALUE: &str = "staging";

#[derive(Subcommand, Clone, Copy)]
pub enum Action {
    /// Create or reconcile the disposable staging environment.
    Up,
    /// Delete and recreate every resource in the disposable boundary.
    Reset,
    /// Report the environment identity and lifecycle readiness.
    Status,
    /// Delete the guarded staging boundary without recreating it.
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub context: String,
    pub namespace: String,
    pub environment_id: String,
}

/// Substrings that mark a Kubernetes context as production.
///
/// Crude on purpose. This is the check that does not depend on the target
/// telling the truth about itself, so it stays dumb enough to be obviously
/// correct.
const PRODUCTION_CONTEXT_MARKERS: &[&str] = &["prod", "production", "live"];

/// Refuse a context that looks like production.
///
/// The label checks below are necessary but they are not a last line of
/// defence, because labels are data the *target* carries: a namespace that
/// happens to be labelled `environment=staging` would satisfy them even on a
/// prod cluster. The context is the operator's *intent*, supplied on the
/// command line (`dev staging --context …`), and a typo there is the realistic
/// way this command reaches production. #438 requires proving "no production
/// project/context/domain marker is present" before any delete; this is that
/// proof.
///
/// A false refusal costs an operator an env var; a false accept deletes the
/// firm's production data. It fails closed.
fn refuse_production_context(context: &str) -> Result<()> {
    let lowered = context.to_ascii_lowercase();
    if let Some(marker) = PRODUCTION_CONTEXT_MARKERS
        .iter()
        .find(|marker| lowered.contains(*marker))
    {
        bail!(
            "staging lifecycle refuses the context `{context}`: it carries the \
             production marker `{marker}`"
        );
    }
    Ok(())
}

pub fn verify_target(
    environment: DeploymentEnvironment,
    context: Option<&str>,
    namespace: &str,
    labels: &BTreeMap<String, String>,
) -> Result<Target> {
    if environment != DeploymentEnvironment::Dev {
        bail!("staging lifecycle requires NAVIGATOR_ENVIRONMENT=dev");
    }
    let context = context
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("staging lifecycle requires an explicit Kubernetes context")
        })?;
    refuse_production_context(context)?;
    if labels.get(MANAGED_LABEL).map(String::as_str) != Some(MANAGED_VALUE)
        || labels.get(ENVIRONMENT_LABEL).map(String::as_str) != Some(STAGING_VALUE)
    {
        bail!("staging lifecycle refuses an unmanaged or non-staging namespace");
    }
    let environment_id = labels
        .get(ENVIRONMENT_ID_LABEL)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("staging lifecycle requires an immutable environment ID"))?;
    Ok(Target {
        context: context.into(),
        namespace: namespace.into(),
        environment_id: environment_id.clone(),
    })
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// One step of a teardown boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    /// Must succeed; a failure aborts the teardown before anything further is
    /// deleted.
    Required(Vec<String>),
    /// May fail without aborting — releasing a finalizer on a CR that a
    /// previous run already removed is not an error, and `kubectl patch` has
    /// no `--ignore-not-found`.
    BestEffort(Vec<String>),
}

impl Step {
    pub(super) fn argv(&self) -> &[String] {
        match self {
            Step::Required(argv) | Step::BestEffort(argv) => argv,
        }
    }
}

/// The argv the KIND reset boundary deletes, in order.
///
/// Every one pins `--context`, and that is the whole guard: `inspect` proves
/// the *KIND* namespace carries the managed staging labels, so a delete that
/// fell back to the current context would verify one cluster and destroy
/// another.
pub(super) fn delete_kind_boundary_args(context: &str, namespace: &str) -> Vec<Step> {
    vec![
        // Mark the CR for deletion, but do NOT wait for the Operator here —
        // it cannot finish, and the next step is what actually releases it.
        Step::Required(argv(&[
            "--context",
            context,
            "--namespace",
            namespace,
            "delete",
            "restatedeployment",
            "workflows-service",
            "--ignore-not-found",
            "--wait=false",
        ])),
        // Release the Operator's `deployments.restate.dev` finalizer by hand.
        //
        // This looks blunt, so here is why nothing gentler works. The Operator
        // will not finalize while Restate reports the deployment live:
        //
        //     Cannot process deletion of RestateDeployment 'workflows-service'
        //     from Restate as there are 1 active deployments that rely on it
        //     reconcile failed: CleanupFailed(DeploymentInUse)
        //
        // and the thing reporting it live is the journal this very reset is
        // about to destroy — so the finalizer waits on the journal, the
        // namespace waits on the finalizer, and the journal waits on the
        // namespace. Waiting deadlocks (`delete namespace` blocks forever);
        // deferring it to the namespace delete deadlocks differently, because
        // the Operator's cleanup emits an Event and a terminating namespace
        // 403s every write into it.
        //
        // The finalizer exists to keep Restate's registry consistent. That
        // registry is deleted three steps below, so there is no consistency
        // left to protect — this is a disposable boundary, and graceful
        // deregistration from a broker being destroyed in the same breath is
        // ceremony. Best-effort: the CR is gone on a repeat run.
        Step::BestEffort(argv(&[
            "--context",
            context,
            "--namespace",
            namespace,
            "patch",
            "restatedeployment",
            "workflows-service",
            "--type=merge",
            "-p",
            r#"{"metadata":{"finalizers":[]}}"#,
        ])),
        // StatefulSet PVCs are intentionally part of the disposable staging
        // boundary. Delete them before the namespace so Garage bytes cannot
        // outlive the reset even on storage classes that retain claims during
        // termination.
        Step::Required(argv(&[
            "--context",
            context,
            "--namespace",
            namespace,
            "delete",
            "pvc",
            "--all",
            "--ignore-not-found",
            "--wait=false",
        ])),
    ]
}

pub(super) fn get_namespace_args(context: &str, namespace: &str) -> Vec<String> {
    argv(&["--context", context, "get", "namespace", namespace])
}

pub(super) fn create_namespace_args(context: &str, namespace: &str) -> Vec<String> {
    argv(&["--context", context, "create", "namespace", namespace])
}

pub(super) fn inspect_args(context: &str, namespace: &str) -> Vec<String> {
    argv(&[
        "--context",
        context,
        "get",
        "namespace",
        namespace,
        "-o",
        "json",
    ])
}

/// The argv that stamps the managed staging labels onto `namespace`.
///
/// The label keys and values come from the same constants [`verify_target`]
/// checks, so the writer and the reader cannot drift apart — a stamp that
/// wrote a key `verify_target` did not recognise would make every subsequent
/// reset refuse its own environment.
pub(super) fn stamp_args(context: &str, namespace: &str, id: &str) -> Vec<String> {
    let mut args = argv(&["--context", context, "label", "namespace", namespace]);
    args.push(format!("{MANAGED_LABEL}={MANAGED_VALUE}"));
    args.push(format!("{ENVIRONMENT_LABEL}={STAGING_VALUE}"));
    args.push(format!("{ENVIRONMENT_ID_LABEL}={id}"));
    args.push("--overwrite".into());
    args
}

#[derive(serde::Deserialize)]
struct Namespace {
    metadata: Metadata,
}

#[derive(serde::Deserialize)]
struct Metadata {
    name: Option<String>,
    labels: Option<BTreeMap<String, String>>,
}

/// Parse `kubectl get namespace -o json` output and verify it is a guarded
/// staging target. Pure so the refusal cases are provable without a cluster.
pub(super) fn parse_namespace_target(context: &str, stdout: &[u8]) -> Result<Target> {
    let namespace: Namespace = serde_json::from_slice(stdout)
        .map_err(|err| anyhow::anyhow!("parse staging namespace: {err}"))?;
    let name = namespace
        .metadata
        .name
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("staging namespace JSON has no metadata.name"))?;
    verify_target(
        DeploymentEnvironment::Dev,
        Some(context),
        &name,
        &namespace.metadata.labels.unwrap_or_default(),
    )
}

/// The live-cluster side effects the staging lifecycle needs.
///
/// The real implementation drives `kubectl` and lives in [`super::staging`];
/// tests substitute a recording fake. That seam is what makes the decisions
/// below provable without a cluster — above all that a *refused* target is
/// never deleted. It mirrors the trait seams this workspace already puts in
/// front of every other side effect (`cloud::StorageService`,
/// `workflows::StateMachineRuntime`, `Notifier`, `EmailService`); `kubectl`
/// was the only one still called inline.
pub(super) trait Cluster {
    /// Run a command that must succeed.
    fn run(&mut self, argv: &[String]) -> Result<()>;
    /// Run a probe, reporting only whether it exited zero.
    fn succeeds(&mut self, argv: &[String]) -> Result<bool>;
    /// Run a command and capture its stdout.
    fn capture(&mut self, argv: &[String]) -> Result<Vec<u8>>;
    /// Wait between readiness probes. The real impl sleeps; tests no-op.
    fn pause(&mut self);
}

/// How many times [`wait_namespace_absent`] probes before giving up.
pub(super) const NAMESPACE_DELETE_ATTEMPTS: u32 = 60;

fn require_staging(environment: DeploymentEnvironment) -> Result<()> {
    if environment != DeploymentEnvironment::Dev {
        bail!("staging lifecycle requires NAVIGATOR_ENVIRONMENT=dev");
    }
    Ok(())
}

pub(super) fn new_environment_id() -> String {
    format!("stg-{}", uuid::Uuid::now_v7().simple())
}

/// The namespace the Restate Operator reconciles the `RestateCluster` into.
///
/// It is named after the cluster CR (`restate`) and is **not** the CR's own
/// namespace, so the journal PVC and `StatefulSet` sit outside `cfg.namespace`
/// and do not go away with it. `e2e::restate_ready_target` pins the same name
/// for the readiness gate.
pub(super) const RESTATE_NAMESPACE: &str = "restate";

/// Every argv the KIND reset boundary deletes: the Restate deployment, the
/// PVCs, the namespace itself, and finally the Restate journal.
pub(super) fn kind_boundary_argv(context: &str, namespace: &str) -> Vec<Step> {
    let mut argvs = delete_kind_boundary_args(context, namespace);
    argvs.push(Step::Required(super::undeploy_args(context, namespace)));
    // The journal is NOT in `namespace`. The Operator reconciles the
    // RestateCluster into its own `restate` namespace, so a 1 GiB journal PVC
    // outlives a `navigator` teardown — and a reset that left it standing
    // would rebuild the store underneath a live journal, so the worker would
    // replay invocations against rows that no longer exist. That
    // `RecordNotFound` is the exact failure this lifecycle exists to prevent,
    // which is why the journal is inside the boundary and not beside it.
    //
    // This must run AFTER the CR's namespace is gone: the Operator watches the
    // CR, so deleting the journal while the CR still exists just has it
    // reconciled straight back.
    argvs.push(Step::Required(argv(&[
        "--context",
        context,
        "delete",
        "namespace",
        RESTATE_NAMESPACE,
        "--ignore-not-found",
        "--wait=true",
    ])));
    argvs
}

pub(super) fn inspect<C: Cluster>(
    cluster: &mut C,
    environment: DeploymentEnvironment,
    context: &str,
    namespace: &str,
) -> Result<Target> {
    require_staging(environment)?;
    let stdout = cluster.capture(&inspect_args(context, namespace))?;
    parse_namespace_target(context, &stdout)
}

pub(super) fn stamp<C: Cluster>(
    cluster: &mut C,
    environment: DeploymentEnvironment,
    context: &str,
    namespace: &str,
    id: &str,
) -> Result<()> {
    require_staging(environment)?;
    cluster.run(&stamp_args(context, namespace, id))
}

/// Whether the staging fixture can be reused instead of rebuilt.
///
/// `port_answers` alone is not proof, which is the trap: `kubectl
/// port-forward` keeps its host listener bound after the namespace it forwards
/// into is deleted, so a TCP connect still succeeds against a dead forward.
/// Reusing on that signal skips the rebuild and the very next command fails
/// with `namespaces "navigator" not found`. Ask the cluster, not the socket.
pub(super) fn fixture_is_reusable<C: Cluster>(
    cluster: &mut C,
    context: &str,
    namespace: &str,
    port_answers: bool,
) -> Result<bool> {
    if !port_answers {
        return Ok(false);
    }
    if !cluster.succeeds(&get_namespace_args(context, namespace))? {
        return Ok(false);
    }
    // Existing is not the same as usable. `kubectl get namespace` exits zero
    // for a *Terminating* namespace, so a teardown that was interrupted — or
    // one wedged on a finalizer, which is a thing that happens here — leaves a
    // namespace that answers every probe and accepts no new content. Only
    // `Active` may be reused.
    let phase = cluster.capture(&namespace_phase_args(context, namespace))?;
    Ok(String::from_utf8_lossy(&phase).trim() == NAMESPACE_ACTIVE)
}

const NAMESPACE_ACTIVE: &str = "Active";

pub(super) fn namespace_phase_args(context: &str, namespace: &str) -> Vec<String> {
    argv(&[
        "--context",
        context,
        "get",
        "namespace",
        namespace,
        "-o",
        "jsonpath={.status.phase}",
    ])
}

pub(super) fn ensure_namespace<C: Cluster>(
    cluster: &mut C,
    context: &str,
    namespace: &str,
) -> Result<()> {
    if cluster.succeeds(&get_namespace_args(context, namespace))? {
        Ok(())
    } else {
        cluster.run(&create_namespace_args(context, namespace))
    }
}

pub(super) fn wait_namespace_absent<C: Cluster>(
    cluster: &mut C,
    context: &str,
    namespace: &str,
    attempts: u32,
) -> Result<()> {
    for _ in 0..attempts {
        if !cluster.succeeds(&get_namespace_args(context, namespace))? {
            return Ok(());
        }
        cluster.pause();
    }
    bail!("timed out waiting for staging namespace `{namespace}` to delete")
}

/// Delete a guarded boundary — refusing first.
///
/// The order is the safety property: [`inspect`] must prove the target is a
/// Navigator-managed staging namespace *before* a single delete runs, so a
/// refusal leaves the cluster untouched. A teardown that deleted first and
/// checked afterwards would be indistinguishable from this one in a green
/// test suite, which is exactly why the ordering is asserted.
pub(super) fn teardown<C: Cluster>(
    cluster: &mut C,
    environment: DeploymentEnvironment,
    context: &str,
    namespace: &str,
    boundary: &[Step],
) -> Result<()> {
    inspect(cluster, environment, context, namespace)?;
    for step in boundary {
        let argv = step.argv();
        match step {
            Step::Required(_) => cluster.run(argv)?,
            // A failure here is expected on a re-run and must not abort the
            // teardown; the Required steps around it are what enforce order.
            Step::BestEffort(_) => {
                let _ = cluster.succeeds(argv);
            }
        }
    }
    Ok(())
}

/// Delete the guarded boundary and bring it back: refuse, delete, recreate,
/// re-stamp — in that order.
pub(super) fn reset<C: Cluster>(
    cluster: &mut C,
    environment: DeploymentEnvironment,
    context: &str,
    namespace: &str,
    boundary: &[Step],
    id: &str,
    mut recreate: impl FnMut(&mut C) -> Result<()>,
) -> Result<()> {
    teardown(cluster, environment, context, namespace, boundary)?;
    recreate(cluster)?;
    stamp(cluster, environment, context, namespace, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_label_keys_remain_namespaced_under_neonlaw_org() {
        assert_eq!(ENVIRONMENT_LABEL, "navigator.neonlaw.org/environment");
        assert_eq!(ENVIRONMENT_ID_LABEL, "navigator.neonlaw.org/environment-id");
    }

    fn labels() -> BTreeMap<String, String> {
        BTreeMap::from([
            (MANAGED_LABEL.into(), MANAGED_VALUE.into()),
            (ENVIRONMENT_LABEL.into(), STAGING_VALUE.into()),
            (ENVIRONMENT_ID_LABEL.into(), "stg-test".into()),
        ])
    }

    #[test]
    fn reset_requires_exactly_the_guarded_staging_target() {
        assert_eq!(
            verify_target(
                DeploymentEnvironment::Dev,
                Some("kind-navigator"),
                "navigator",
                &labels()
            )
            .unwrap()
            .environment_id,
            "stg-test"
        );
    }

    #[test]
    fn reset_rejects_production_implicit_and_unmanaged_targets() {
        assert!(verify_target(
            DeploymentEnvironment::Production,
            Some("kind-navigator"),
            "navigator",
            &labels()
        )
        .is_err());
        assert!(verify_target(DeploymentEnvironment::Dev, None, "navigator", &labels()).is_err());
        assert!(verify_target(
            DeploymentEnvironment::Dev,
            Some("kind-navigator"),
            "navigator",
            &BTreeMap::new()
        )
        .is_err());
    }

    /// The label checks cannot be the last line of defence: they read data the
    /// *target* carries, so a prod namespace labelled `environment=staging`
    /// would satisfy every one of them. `dev staging --context …` takes the
    /// context from the operator's command line, so a typo is the realistic
    /// route to production — and the guard has to refuse on the context alone.
    ///
    /// The refused string below is the real context in this repo's kubeconfig,
    /// not a hypothetical.
    #[test]
    fn reset_refuses_a_production_context_even_when_every_label_says_staging() {
        let err = verify_target(
            DeploymentEnvironment::Dev,
            Some("gke_neon-law-420305_us-west4_navigator-prod"),
            "navigator",
            // Perfectly valid managed-staging labels: the target claiming to
            // be staging must not be enough.
            &labels(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("production marker"),
            "unexpected refusal reason: {err}"
        );

        for context in [
            "gke_x_us-west4_navigator-production",
            "PROD-cluster",
            "navigator-live",
        ] {
            assert!(
                verify_target(
                    DeploymentEnvironment::Dev,
                    Some(context),
                    "navigator",
                    &labels()
                )
                .is_err(),
                "{context} must be refused",
            );
        }

        // The KIND context is still accepted — the guard must not refuse the
        // one environment the lifecycle exists to manage.
        assert!(verify_target(
            DeploymentEnvironment::Dev,
            Some("kind-navigator"),
            "navigator",
            &labels()
        )
        .is_ok());
    }

    #[test]
    fn reset_rejects_a_blank_context_and_a_blank_environment_id() {
        assert!(verify_target(
            DeploymentEnvironment::Dev,
            Some("   "),
            "navigator",
            &labels()
        )
        .is_err());
        let mut blank = labels();
        blank.insert(ENVIRONMENT_ID_LABEL.into(), String::new());
        assert!(verify_target(
            DeploymentEnvironment::Dev,
            Some("kind-navigator"),
            "navigator",
            &blank
        )
        .is_err());
    }

    /// `verify_target` guards the *labels*; this guards the *cluster*. The two
    /// are separate: reset inspects the KIND namespace by explicit context, so
    /// any delete that omits `--context` would pass every label check and then
    /// destroy whatever cluster happened to be current — prod included.
    #[test]
    fn every_destructive_kind_delete_pins_the_context() {
        let steps = delete_kind_boundary_args("kind-navigator", "navigator");
        assert_eq!(
            steps.len(),
            3,
            "delete the CR, release its finalizer, delete the pvcs"
        );
        for argv in steps.iter().map(Step::argv) {
            assert_eq!(
                argv.iter()
                    .position(|arg| arg == "--context")
                    .map(|i| argv[i + 1].as_str()),
                Some("kind-navigator"),
                "an unpinned command lands on the current context, which \
                 `inspect` never checked: {argv:?}",
            );
            assert_eq!(
                argv.iter()
                    .position(|arg| arg == "--namespace")
                    .map(|i| argv[i + 1].as_str()),
                Some("navigator"),
                "{argv:?} must stay inside the staging namespace",
            );
        }
    }

    /// Every cluster-mutating argv this module builds pins the context, not
    /// just the destructive pair above.
    #[test]
    fn every_lifecycle_argv_pins_the_context() {
        let built = [
            get_namespace_args("kind-navigator", "navigator"),
            create_namespace_args("kind-navigator", "navigator"),
            inspect_args("kind-navigator", "navigator"),
            stamp_args("kind-navigator", "navigator", "stg-test"),
        ];
        for argv in &built {
            assert_eq!(
                argv.first().map(String::as_str),
                Some("--context"),
                "{argv:?} must lead with the context pin",
            );
            assert_eq!(argv.get(1).map(String::as_str), Some("kind-navigator"));
        }
    }

    /// The stamp writes what the guard reads. Hardcoding either side lets them
    /// drift, which would make a reset refuse the very environment it stamped.
    #[test]
    fn what_stamp_writes_is_what_verify_target_accepts() {
        let args = stamp_args("kind-navigator", "navigator", "stg-roundtrip");
        let written: BTreeMap<String, String> = args
            .iter()
            .filter_map(|arg| arg.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let target = verify_target(
            DeploymentEnvironment::Dev,
            Some("kind-navigator"),
            "navigator",
            &written,
        )
        .expect("labels written by stamp_args must satisfy verify_target");
        assert_eq!(target.environment_id, "stg-roundtrip");
    }

    #[test]
    fn parse_namespace_target_accepts_a_managed_staging_namespace() {
        let json = br#"{"metadata":{"name":"navigator","labels":{
            "app.kubernetes.io/part-of":"navigator",
            "navigator.neonlaw.org/environment":"staging",
            "navigator.neonlaw.org/environment-id":"stg-parsed"}}}"#;
        let target = parse_namespace_target("kind-navigator", json).unwrap();
        assert_eq!(target.environment_id, "stg-parsed");
        assert_eq!(target.namespace, "navigator");
        assert_eq!(target.context, "kind-navigator");
    }

    const MANAGED_JSON: &[u8] = br#"{"metadata":{"name":"navigator","labels":{
        "app.kubernetes.io/part-of":"navigator",
        "navigator.neonlaw.org/environment":"staging",
        "navigator.neonlaw.org/environment-id":"stg-live"}}}"#;

    /// A namespace that is real but is not ours — the production `navigator`
    /// namespace looks exactly like this.
    const UNMANAGED_JSON: &[u8] = br#"{"metadata":{"name":"navigator","labels":{}}}"#;

    /// Records every argv and replays canned probe/capture results, so a test
    /// can assert what the lifecycle *did* — and, more importantly, what it
    /// did not do.
    struct FakeCluster {
        calls: Vec<Vec<String>>,
        namespace_json: Vec<u8>,
        /// `succeeds` answers, popped front to back; then `exists_default`.
        probes: Vec<bool>,
        exists_default: bool,
        pauses: u32,
        /// What `get namespace -o jsonpath={.status.phase}` reports.
        phase: String,
    }

    impl FakeCluster {
        fn managed() -> Self {
            Self::with_json(MANAGED_JSON)
        }

        fn with_json(json: &[u8]) -> Self {
            Self {
                calls: Vec::new(),
                namespace_json: json.to_vec(),
                probes: Vec::new(),
                exists_default: false,
                pauses: 0,
                phase: "Active".into(),
            }
        }

        fn verbs(&self) -> Vec<String> {
            self.calls.iter().map(|c| c.join(" ")).collect()
        }

        fn deletes(&self) -> Vec<&Vec<String>> {
            self.calls
                .iter()
                .filter(|c| c.contains(&"delete".to_string()))
                .collect()
        }
    }

    impl Cluster for FakeCluster {
        fn run(&mut self, argv: &[String]) -> Result<()> {
            self.calls.push(argv.to_vec());
            Ok(())
        }
        fn succeeds(&mut self, argv: &[String]) -> Result<bool> {
            self.calls.push(argv.to_vec());
            Ok(if self.probes.is_empty() {
                self.exists_default
            } else {
                self.probes.remove(0)
            })
        }
        fn capture(&mut self, argv: &[String]) -> Result<Vec<u8>> {
            self.calls.push(argv.to_vec());
            if argv.iter().any(|a| a.contains("jsonpath={.status.phase}")) {
                return Ok(self.phase.clone().into_bytes());
            }
            Ok(self.namespace_json.clone())
        }
        fn pause(&mut self) {
            self.pauses += 1;
        }
    }

    /// THE safety invariant of the whole feature: a target the guard refuses
    /// must not be touched. `verify_target` returning `Err` proves the guard
    /// decides correctly; only this proves the decision actually stops the
    /// deletes.
    #[test]
    fn teardown_refuses_before_it_deletes_anything() {
        let mut cluster = FakeCluster::with_json(UNMANAGED_JSON);
        let boundary = kind_boundary_argv("kind-navigator", "navigator");

        let err = teardown(
            &mut cluster,
            DeploymentEnvironment::Dev,
            "kind-navigator",
            "navigator",
            &boundary,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unmanaged or non-staging"));
        assert!(
            cluster.deletes().is_empty(),
            "a refused target must not be deleted, but ran: {:?}",
            cluster.verbs(),
        );
    }

    /// The same refusal, reached the other way: the wrong typed environment.
    #[test]
    fn teardown_refuses_a_production_environment_before_it_deletes_anything() {
        let mut cluster = FakeCluster::managed();
        let boundary = kind_boundary_argv("kind-navigator", "navigator");

        assert!(teardown(
            &mut cluster,
            DeploymentEnvironment::Production,
            "kind-navigator",
            "navigator",
            &boundary,
        )
        .is_err());
        assert!(
            cluster.calls.is_empty(),
            "a production environment must not even be inspected: {:?}",
            cluster.verbs(),
        );
    }

    #[test]
    fn teardown_inspects_first_then_deletes_the_whole_boundary_in_order() {
        let mut cluster = FakeCluster::managed();
        let boundary = kind_boundary_argv("kind-navigator", "navigator");

        teardown(
            &mut cluster,
            DeploymentEnvironment::Dev,
            "kind-navigator",
            "navigator",
            &boundary,
        )
        .unwrap();

        let verbs = cluster.verbs();
        assert!(
            verbs[0].contains("get namespace"),
            "inspect runs first: {verbs:?}"
        );
        assert_eq!(
            cluster.deletes().len(),
            4,
            "restatedeployment, pvc, namespace, journal"
        );
        assert!(verbs[1].contains("delete restatedeployment"));
        assert!(verbs[2].contains("patch restatedeployment"));
        assert!(verbs[3].contains("delete pvc"));
        assert!(verbs[4].contains("delete --ignore-not-found namespace navigator"));
        assert!(verbs[5].contains(&format!("delete namespace {RESTATE_NAMESPACE}")));
    }

    /// The Operator never releases this finalizer on its own: it refuses while
    /// Restate reports the deployment in use — `CleanupFailed(DeploymentInUse)`
    /// — and the thing reporting it is the journal this same reset destroys
    /// two steps later. So the finalizer waits on the journal, the namespace
    /// waits on the finalizer, and the journal waits on the namespace.
    /// Verified live against KIND: waiting on the Operator blocks forever.
    #[test]
    fn the_restate_finalizer_is_released_before_the_namespace_dies() {
        let steps = kind_boundary_argv("kind-navigator", "navigator");
        let verbs: Vec<String> = steps.iter().map(|s| s.argv().join(" ")).collect();

        let release = verbs
            .iter()
            .position(|v| v.contains("patch restatedeployment"))
            .expect("the boundary releases the finalizer itself");
        let namespace = verbs
            .iter()
            .position(|v| v.contains("delete --ignore-not-found namespace navigator"))
            .expect("the boundary deletes its namespace");

        assert!(
            verbs[release].contains(r#"{"metadata":{"finalizers":[]}}"#),
            "the release must actually empty the finalizer list: {:?}",
            verbs[release],
        );
        assert!(
            release < namespace,
            "a terminating namespace 403s the Event the Operator's cleanup \
             writes, so the finalizer must go while it is still Active: \
             {verbs:?}",
        );
        assert!(
            matches!(steps[release], Step::BestEffort(_)),
            "a repeat run has no CR left to patch; that is not a failure",
        );
    }

    /// The Restate journal lives in its own namespace, so a boundary scoped to
    /// `navigator` leaves it standing — a fresh store under a live journal,
    /// which is the `RecordNotFound` this lifecycle exists to prevent.
    #[test]
    fn the_kind_boundary_deletes_the_restate_journal_after_the_cr() {
        let verbs: Vec<String> = kind_boundary_argv("kind-navigator", "navigator")
            .iter()
            .map(|step| step.argv().join(" "))
            .collect();

        let journal = verbs
            .iter()
            .position(|v| v.contains(&format!("namespace {RESTATE_NAMESPACE}")))
            .expect("the journal namespace must be inside the reset boundary");
        let cluster_cr = verbs
            .iter()
            .position(|v| v.contains("delete --ignore-not-found namespace navigator"))
            .expect("the CR's namespace is deleted too");

        assert!(
            journal > cluster_cr,
            "deleting the journal before the CR just has the Operator \
             reconcile it straight back: {verbs:?}",
        );
        assert!(
            verbs[journal].contains("--context kind-navigator"),
            "the journal delete pins the context like every other: {verbs:?}",
        );
    }

    #[test]
    fn reset_recreates_and_restamps_only_after_a_successful_teardown() {
        let mut cluster = FakeCluster::managed();
        let boundary = kind_boundary_argv("kind-navigator", "navigator");
        let mut recreated = 0;

        reset(
            &mut cluster,
            DeploymentEnvironment::Dev,
            "kind-navigator",
            "navigator",
            &boundary,
            "stg-new",
            |_| {
                recreated += 1;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(recreated, 1);
        let verbs = cluster.verbs();
        let stamp_at = verbs
            .iter()
            .position(|v| v.contains("label namespace"))
            .unwrap();
        let last_delete = verbs.iter().rposition(|v| v.contains("delete")).unwrap();
        assert!(
            stamp_at > last_delete,
            "the re-stamp lands after the deletes: {verbs:?}"
        );
        assert!(verbs[stamp_at].contains("stg-new"));
    }

    #[test]
    fn reset_does_not_recreate_when_the_target_is_refused() {
        let mut cluster = FakeCluster::with_json(UNMANAGED_JSON);
        let boundary = kind_boundary_argv("kind-navigator", "navigator");
        let mut recreated = 0;

        assert!(reset(
            &mut cluster,
            DeploymentEnvironment::Dev,
            "kind-navigator",
            "navigator",
            &boundary,
            "stg-new",
            |_| {
                recreated += 1;
                Ok(())
            },
        )
        .is_err());

        assert_eq!(recreated, 0, "a refused reset must not rebuild anything");
        assert!(cluster.deletes().is_empty());
    }

    /// Regression: a stale `kubectl port-forward` answers on its host port
    /// long after its namespace is deleted, so reusing the fixture on a TCP
    /// probe alone skipped the rebuild and the next command died with
    /// `namespaces "navigator" not found`. Hit live against KIND.
    #[test]
    fn a_live_host_port_alone_does_not_make_the_fixture_reusable() {
        let mut gone = FakeCluster::managed();
        gone.exists_default = false; // namespace deleted, forward still bound
        assert!(
            !fixture_is_reusable(&mut gone, "kind-navigator", "navigator", true).unwrap(),
            "a dead forward still answers; the namespace is the real signal",
        );

        let mut live = FakeCluster::managed();
        live.exists_default = true;
        assert!(fixture_is_reusable(&mut live, "kind-navigator", "navigator", true).unwrap());

        // A dead port short-circuits without asking the cluster at all.
        let mut unreachable = FakeCluster::managed();
        unreachable.exists_default = true;
        assert!(
            !fixture_is_reusable(&mut unreachable, "kind-navigator", "navigator", false).unwrap()
        );
        assert!(unreachable.calls.is_empty());
    }

    /// Existing is not usable. `kubectl get namespace` exits zero for a
    /// *Terminating* namespace, so an interrupted teardown — or one wedged on
    /// a finalizer, which happens here — leaves a namespace that answers every
    /// probe and accepts no new content. Reusing it means the next command
    /// runs against a namespace being deleted.
    #[test]
    fn a_terminating_namespace_is_not_a_reusable_fixture() {
        let mut dying = FakeCluster::managed();
        dying.exists_default = true; // `get namespace` still exits zero
        dying.phase = "Terminating".into();
        assert!(
            !fixture_is_reusable(&mut dying, "kind-navigator", "navigator", true).unwrap(),
            "a Terminating namespace exists but cannot be built into",
        );

        let mut active = FakeCluster::managed();
        active.exists_default = true;
        active.phase = "Active".into();
        assert!(fixture_is_reusable(&mut active, "kind-navigator", "navigator", true).unwrap());
    }

    #[test]
    fn ensure_namespace_creates_only_when_it_is_absent() {
        let mut present = FakeCluster::managed();
        present.exists_default = true;
        ensure_namespace(&mut present, "kind-navigator", "navigator").unwrap();
        assert_eq!(
            present.calls.len(),
            1,
            "an existing namespace is left alone"
        );

        let mut absent = FakeCluster::managed();
        absent.exists_default = false;
        ensure_namespace(&mut absent, "kind-navigator", "navigator").unwrap();
        assert!(absent.verbs()[1].contains("create namespace"));
    }

    #[test]
    fn wait_namespace_absent_returns_once_gone_and_times_out_otherwise() {
        // Present for two probes, then gone.
        let mut eventual = FakeCluster::managed();
        eventual.probes = vec![true, true, false];
        wait_namespace_absent(&mut eventual, "kind-navigator", "navigator", 5).unwrap();
        assert_eq!(eventual.pauses, 2, "it pauses only between probes");

        let mut stuck = FakeCluster::managed();
        stuck.exists_default = true;
        let err = wait_namespace_absent(&mut stuck, "kind-navigator", "navigator", 3).unwrap_err();
        assert!(err.to_string().contains("timed out"));
        assert_eq!(stuck.pauses, 3);
    }

    #[test]
    fn inspect_and_stamp_refuse_a_non_staging_environment() {
        let mut cluster = FakeCluster::managed();
        assert!(inspect(
            &mut cluster,
            DeploymentEnvironment::Production,
            "kind-navigator",
            "navigator"
        )
        .is_err());
        assert!(stamp(
            &mut cluster,
            DeploymentEnvironment::Production,
            "kind-navigator",
            "navigator",
            "stg-x"
        )
        .is_err());
        assert!(cluster.calls.is_empty(), "neither may touch the cluster");
    }

    #[test]
    fn a_fresh_environment_id_is_prefixed_and_unique() {
        let a = new_environment_id();
        let b = new_environment_id();
        assert!(a.starts_with("stg-"), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn parse_namespace_target_refuses_unlabelled_nameless_and_invalid_json() {
        // A real namespace that simply is not ours — the prod `navigator`
        // namespace looks exactly like this.
        let unmanaged = br#"{"metadata":{"name":"navigator","labels":{}}}"#;
        assert!(parse_namespace_target("kind-navigator", unmanaged).is_err());

        let nameless = br#"{"metadata":{"labels":{
            "app.kubernetes.io/part-of":"navigator",
            "navigator.neonlaw.org/environment":"staging",
            "navigator.neonlaw.org/environment-id":"stg-x"}}}"#;
        assert!(parse_namespace_target("kind-navigator", nameless).is_err());

        assert!(parse_namespace_target("kind-navigator", b"not json").is_err());
    }
}
