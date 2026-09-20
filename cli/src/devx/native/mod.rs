//! The native dependency tier — every local dependency as a host
//! process instead of a pod.
//!
//! This is the opt-in host lane; `--runtime kind` remains the default local
//! lane in [`super::orchestrate`]. Both write the same `.devx/env` and
//! `.devx/worktree.json`, so everything downstream — `dev grant-lawyer`,
//! `dev browser-e2e`, `cargo run -p neon` — is identical under either.
//! The env file, not the topology, is the contract.
//!
//! Isolation is logical rather than physical. Most dependencies are one
//! long-lived shared process serving every worktree through a tenant
//! key, which is what makes a second `worktree-env up` fast: it connects
//! to processes that are already running instead of building a cluster.
//!
//! | Dependency | Process | Per-worktree tenant |
//! |---|---|---|
//! | `SurrealDB` | shared | database inside the `navigator` namespace |
//! | Rauthy | shared | none needed — one client, wildcard localhost redirect |
//! | Garage | shared | bucket pair |
//! | Restate | **per worktree** | — |
//! | `workflows-service` | **per worktree** | — |
//!
//! Restate is the exception, and not by oversight. Its OSS server has no
//! namespace or environment concept, and a service name resolves to one
//! active deployment endpoint — so a second worktree registering
//! `workflows-service` would silently take over the first worktree's
//! invocations and execute them against its own store handle. The SDK does
//! expose a doc-hidden runtime rename seam through
//! `service::macro_support::service_definition` and the public discovery
//! module, but it varies the declared service names, depends on non-public
//! API under a caret pin, and fails open on a typo. One `restate-server` per
//! worktree is the fix, and it is cheap next to the cluster it replaces.
//!
//! `OpenObserve` and `ClamAV` remain deferred to ENG-131. That gap is
//! declared in [`DEFERRED`] rather than dropped from the readiness gate —
//! see [`super::worktree_env`]'s gate table and the test that forces every
//! gated port to be either supervised here or attributed to the issue that
//! will supervise it.

mod garage;
mod preflight;
mod rauthy;
mod registry;
mod restate;
mod supervisor;
mod surreal;
mod worker;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::KindConfig;

/// Stable service labels and the host-native process ports.
const RAUTHY_LABEL: &str = "rauthy";
const GARAGE_LABEL: &str = "garage";
const SURREAL_LABEL: &str = "surreal";
const RESTATE_LABEL: &str = "restate-server";
const WORKFLOWS_LABEL: &str = "workflows-service";

/// Ports the tier binds that nothing outside it connects to.
///
/// The slot table in [`super::worktree_env`] reserves ports for each
/// worktree's processes. These listeners are shared and therefore use one
/// host-wide fixed port set outside that reservation window.
const GARAGE_RPC_PORT_BASE: u16 = 21_300;
const GARAGE_ADMIN_PORT_BASE: u16 = 21_400;
const RAUTHY_RAFT_PORT_BASE: u16 = 21_500;
const RAUTHY_API_PORT_BASE: u16 = 21_600;
const RESTATE_FABRIC_PORT_BASE: u16 = 21_700;
const WORKFLOWS_SERVICE_LISTEN_PORT_BASE: u16 = 21_800;
const WORKFLOWS_SERVICE_HEALTH_PORT_BASE: u16 = 21_900;

fn is_worktree_process(record: &supervisor::Started) -> bool {
    matches!(record.label.as_str(), RESTATE_LABEL | WORKFLOWS_LABEL)
}

/// Readiness-gate members this lane starts and health-gates.
///
/// Spelled exactly as [`super::worktree_env`]'s gate table spells them —
/// the two lists are compared by a test, so a typo here is a failing
/// build rather than a port that silently answers for nothing.
pub(super) const SUPERVISED: &[&str] = &[
    "Rauthy",
    "Garage",
    "SurrealDB",
    "Restate ingress",
    "Restate admin",
];

/// Readiness-gate members this lane has no host process for yet, and the
/// issue that gives each one.
///
/// Declared rather than dropped. A gate that quietly shrinks to the
/// ports a lane happens to serve stops being a gate: it reports ready
/// while half the tier is missing, and the next person to add a
/// dependency has no signal that one lane never got it. Every entry here
/// is a port `worktree-env up --runtime native` will *not* satisfy, said
/// out loud at the end of the run.
pub(super) const DEFERRED: &[(&str, &str)] = &[
    ("KIND ingress HTTP", "ENG-132"),
    ("KIND ingress HTTPS", "ENG-132"),
    ("OpenObserve", "ENG-131"),
    ("OpenObserve OTLP", "ENG-131"),
    ("ClamAV", "ENG-131"),
];

/// Converge this host's toolchain on the pinned native dependencies.
///
/// The entry point behind `navigator dev install`. The platform string
/// is injected into [`preflight::ensure`] rather than read inside it, so
/// the non-macOS refusal stays testable without a non-macOS machine.
///
/// Rauthy comes last because it is the only step that can take minutes:
/// a Homebrew failure should surface before a developer waits on a
/// source build.
pub(super) fn install() -> Result<()> {
    preflight::ensure(std::env::consts::OS)?;
    rauthy::resolve()?;
    Ok(())
}

/// Start this worktree's native dependency tier and bootstrap it.
///
/// Idempotent, like the cluster lane's `up`: a process that is already
/// serving its port is left alone, and an existing Garage key is read
/// back rather than re-minted. That is what makes a repeated
/// `worktree-env up` — after a reboot, or just as a re-check — cheap.
///
/// The bootstrap that follows is the same work the cluster lane does,
/// reached differently: Garage's layout, keys, and buckets come from the
/// local binary instead of `kubectl exec`. The Surreal schema apply is
/// not repeated — it is already lane-neutral and stays with the caller.
pub(super) fn database_name(root: &Path, slug: &str) -> String {
    format!("navigator_{}_{}", slug.replace('-', "_"), fingerprint(root))
}

pub(super) fn restate_fabric_port(slot: u16) -> u16 {
    RESTATE_FABRIC_PORT_BASE + slot
}

pub(super) fn worker_listen_port(slot: u16) -> u16 {
    worker::ports(slot).listen
}

pub(super) fn worker_health_port(slot: u16) -> u16 {
    worker::ports(slot).health
}

pub(super) fn append_environment(body: &mut String, slot: u16) {
    use std::fmt::Write as _;

    let ports = worker::ports(slot);
    let _ = writeln!(body, "WORKFLOWS_SERVICE_LISTEN=127.0.0.1:{}", ports.listen);
    let _ = writeln!(
        body,
        "WORKFLOWS_SERVICE_HEALTH_LISTEN=127.0.0.1:{}",
        ports.health
    );
}

pub(super) fn claimed_slots(root: &Path) -> Result<std::collections::BTreeSet<u16>> {
    let state = loaded(&registry::path())?;
    Ok(state
        .claims
        .values()
        .filter(|claim| claim.root != root)
        .map(|claim| claim.slot)
        .collect())
}

fn fingerprint(root: &Path) -> String {
    let mut hash = 0x811c_9dc5_u32;
    for byte in root.display().to_string().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{hash:08x}")
}

fn loaded(path: &Path) -> Result<registry::NativeRegistry> {
    match registry::load(path) {
        registry::Load::Loaded(value) => Ok(value),
        registry::Load::Absent => Ok(registry::NativeRegistry::default()),
        registry::Load::Unreadable(error) => anyhow::bail!(
            "native registry {} is unreadable ({error}); refusing to start or alter shared processes until it is repaired",
            path.display()
        ),
    }
}

fn apply_environment(claim: &registry::NativeClaim) {
    for (key, value) in &claim.garage_env {
        std::env::set_var(key, value);
    }
}

fn remove_worktree_process_state(root: &Path) {
    let _ = std::fs::remove_file(supervisor::ledger_path(root));
    let _ = std::fs::remove_dir_all(root.join(".devx/native"));
    let _ = std::fs::remove_dir_all(root.join(".devx/restate"));
}

pub(super) fn up(root: &Path, slot: u16, cfg: &KindConfig, database: &str) -> Result<()> {
    preflight::ensure(std::env::consts::OS)?;
    let registry_path = registry::path();
    let shared_root = registry::state_dir(&registry_path);
    let mut state = loaded(&registry_path)?;
    let buckets = garage::bucket_names(database);
    registry::ensure_tenant_available(&state, root, database, &buckets)?;
    let shared_services = [
        surreal::service(&shared_root, cfg.surreal_port)?,
        garage::service(
            &shared_root,
            cfg.garage_s3_port,
            GARAGE_RPC_PORT_BASE,
            GARAGE_ADMIN_PORT_BASE,
        )?,
        rauthy::service(
            &shared_root,
            cfg.rauthy_port,
            RAUTHY_RAFT_PORT_BASE,
            RAUTHY_API_PORT_BASE,
        )?,
    ];
    let registered: Vec<_> = state.services.values().cloned().collect();
    let shared_records =
        supervisor::ensure_all_with_existing(&shared_root, &shared_services, &registered)?;
    let previous_worktree_processes = state
        .claims
        .get(&registry::key(root))
        .map(|claim| claim.processes.clone())
        .unwrap_or_default();
    let restate_service = restate::service(
        root,
        cfg.restate_ingress_port,
        cfg.restate_admin_port,
        restate_fabric_port(slot),
    )?;
    let restate_record = supervisor::ensure_all_with_existing(
        root,
        &[restate_service],
        &previous_worktree_processes,
    )?;
    super::wait_for_tcp("127.0.0.1", cfg.restate_admin_port)
        .context("the native Restate admin listener must be reachable")?;
    super::wait_for_tcp("127.0.0.1", restate_fabric_port(slot))
        .context("the native Restate fabric listener must be reachable")?;
    let mut records = shared_records.clone();
    records.extend(restate_record);
    state.services = shared_records
        .iter()
        .map(|record| (record.label.clone(), record.clone()))
        .collect();
    // Publish the process identities before tenant provisioning. If the CLI
    // is interrupted in the gap, sweep can still see and reclaim a shared
    // process that has no tenant claim yet.
    registry::save(&registry_path, &state)?;
    let tenant = garage::provision(&shared_root, database)
        .context("provision native object storage tenant")?;
    registry::claim(
        &mut state,
        root,
        slot,
        database.to_string(),
        tenant.buckets,
        tenant.env,
        records,
    );
    registry::save(&registry_path, &state)?;
    let claim = state
        .claims
        .get(&registry::key(root))
        .context("native claim disappeared after registration")?;
    apply_environment(claim);
    Ok(())
}

/// Build, start, and register this worktree's worker after `.devx/env` exists.
/// Registration goes through the same admin API machinery as cloud re-register,
/// but targets this claim's private Restate admin and worker ports.
pub(super) fn start_workflows(root: &Path, slot: u16, admin_port: u16) -> Result<()> {
    let binary = worker::build(root)?;
    let path = registry::path();
    let mut state = loaded(&path)?;
    let previous = state
        .claims
        .get(&registry::key(root))
        .map(|claim| {
            claim
                .processes
                .iter()
                .filter(|record| is_worktree_process(record))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let service = worker::service(root, slot, binary);
    let record = supervisor::ensure_all_with_existing(root, &[service], &previous)?;
    let record = record.into_iter().next().context("worker record missing")?;
    let mut ledger = previous;
    ledger.retain(|existing| existing.label != WORKFLOWS_LABEL);
    ledger.push(record.clone());
    supervisor::write_ledger(root, &ledger)?;
    let claim = state
        .claims
        .get_mut(&registry::key(root))
        .context("native worktree has no claim for workflows-service")?;
    claim
        .processes
        .retain(|existing| existing.label != WORKFLOWS_LABEL);
    claim.processes.push(record);
    registry::save(&path, &state)?;
    let ports = worker::ports(slot);
    super::wait_for_tcp("127.0.0.1", ports.health)
        .context("the native workflows-service health listener must be reachable")?;
    super::restate_register_local(admin_port, ports.listen)
}

pub(super) fn reload_workflows(root: &Path, slot: u16, admin_port: u16) -> Result<()> {
    let path = registry::path();
    let state = loaded(&path)?;
    if let Some(record) = state.claims.get(&registry::key(root)).and_then(|claim| {
        claim
            .processes
            .iter()
            .find(|record| record.label == WORKFLOWS_LABEL)
    }) {
        supervisor::stop(record);
    }
    start_workflows(root, slot, admin_port)
}

pub(super) fn restore_environment(root: &Path) -> Result<()> {
    let path = registry::path();
    let state = loaded(&path)?;
    let claim = state
        .claims
        .get(&registry::key(root))
        .context("native worktree has no host claim; run up without --no-deps")?;
    apply_environment(claim);
    Ok(())
}

/// Remove this worktree's tenants and claim. Shared processes are stopped only
/// when this was the final claim, and only after PID identity re-checks.
///
/// Idempotent and scoped: only PIDs this worktree recorded, and only
/// after re-identifying each one, so a teardown can never reach another
/// checkout's tier or a stranger that inherited a PID.
pub(super) fn down(root: &Path, cfg: &KindConfig) -> Result<()> {
    let path = registry::path();
    let mut state = loaded(&path)?;
    let Some(claim) = state.claims.get(&registry::key(root)).cloned() else {
        return Ok(());
    };
    let shared_root = registry::state_dir(&path);
    let tenant = garage::Tenant {
        buckets: claim.buckets.clone(),
        env: claim.garage_env.clone(),
    };
    garage::remove_tenant(&shared_root, &tenant).context("remove native Garage tenant")?;
    surreal::remove_database(cfg, &claim.database)?;
    for process in &claim.processes {
        if is_worktree_process(process) {
            supervisor::stop(process);
        }
    }
    let Some((_, final_claim, services)) = registry::release(&mut state, root) else {
        return Ok(());
    };
    registry::save(&path, &state)?;
    if final_claim {
        for service in services {
            supervisor::stop(&service);
        }
        let _ = std::fs::remove_dir_all(&shared_root);
    }
    remove_worktree_process_state(root);
    Ok(())
}

pub(super) fn sweep_plan(
    live_worktrees: &[PathBuf],
) -> Result<(PathBuf, registry::NativeRegistry, Vec<registry::SweepEntry>)> {
    let path = registry::path();
    let state = loaded(&path)?;
    let plan = registry::plan_sweep(
        &state,
        live_worktrees,
        &|path| path.is_dir(),
        &supervisor::still_ours,
    );
    Ok((path, state, plan))
}

pub(super) fn sweep_report(
    plan: &[registry::SweepEntry],
    shared_service_count: usize,
    apply: bool,
) -> String {
    registry::report(plan, shared_service_count, apply)
}

pub(super) fn service_count(state: &registry::NativeRegistry) -> usize {
    registry::service_count(state)
}

pub(super) fn apply_sweep(
    path: &Path,
    mut state: registry::NativeRegistry,
    orphans: &[registry::SweepEntry],
    cfg: &KindConfig,
) -> Result<()> {
    let shared_root = registry::state_dir(path);
    let services: Vec<_> = state.services.values().cloned().collect();
    for orphan in orphans {
        if !orphan.is_orphaned() {
            continue;
        }
        let Some((claim, _, _)) = registry::release(&mut state, &orphan.claim.root) else {
            continue;
        };
        for process in &claim.processes {
            if is_worktree_process(process) {
                supervisor::stop(process);
            }
        }
        let tenant = garage::Tenant {
            buckets: claim.buckets.clone(),
            env: claim.garage_env.clone(),
        };
        garage::remove_tenant(&shared_root, &tenant)?;
        surreal::remove_database(cfg, &claim.database)?;
        remove_worktree_process_state(&claim.root);
    }
    if state.claims.is_empty() {
        for service in &services {
            supervisor::stop(service);
        }
        state.services.clear();
        let _ = std::fs::remove_dir_all(&shared_root);
    }
    registry::save(path, &state)
}

/// `status` lines for the processes this worktree started.
pub(super) fn status_lines(root: &Path) -> Vec<String> {
    let state = match registry::load(&registry::path()) {
        registry::Load::Loaded(value) => value,
        registry::Load::Absent => return Vec::new(),
        registry::Load::Unreadable(error) => {
            return vec![format!("  native registry: unreadable ({error})")]
        }
    };
    let claim_processes = state
        .claims
        .get(&registry::key(root))
        .map(|claim| claim.processes.clone())
        .unwrap_or_default();
    let mut records = state.services.into_values().collect::<Vec<_>>();
    records.extend(claim_processes.into_iter().filter(is_worktree_process));
    records
        .into_iter()
        .map(|record| {
            let live = supervisor::is_live(&record);
            let label = record.label;
            let pid = record.pid;
            let port = record.port;
            format!(
                "  {label} 127.0.0.1:{port} (pid {pid}): {}",
                if live { "yes" } else { "no" }
            )
        })
        .collect()
}

/// The lines naming what this lane does not serve yet.
///
/// Printed at the end of `up` so an operator reads the gap from the run
/// that produced it rather than from an issue tracker.
pub(super) fn deferred_lines() -> Vec<String> {
    let mut lines = vec![
        "==> not on the native lane yet — these dependency-tier ports stay unserved:".to_string(),
    ];
    lines.extend(
        DEFERRED
            .iter()
            .map(|(member, issue)| format!("    {member} ({issue})")),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::{
        database_name, deferred_lines, garage, registry, remove_worktree_process_state, restate,
        supervisor, worker, DEFERRED, GARAGE_ADMIN_PORT_BASE, GARAGE_RPC_PORT_BASE,
        RAUTHY_API_PORT_BASE, RAUTHY_RAFT_PORT_BASE, RESTATE_FABRIC_PORT_BASE, SUPERVISED,
        WORKFLOWS_SERVICE_HEALTH_PORT_BASE, WORKFLOWS_SERVICE_LISTEN_PORT_BASE,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    #[test]
    fn native_databases_are_stable_per_worktree_and_distinct_between_worktrees() {
        let first = database_name(Path::new("/tmp/worktree-a"), "feature-129");
        assert_eq!(
            first,
            database_name(Path::new("/tmp/worktree-a"), "feature-129")
        );
        assert_ne!(
            first,
            database_name(Path::new("/tmp/worktree-b"), "feature-129")
        );
        assert!(first.starts_with("navigator_feature_129_"));
    }

    #[test]
    fn colliding_legacy_database_names_are_refused_before_garage_provisioning() {
        let first = Path::new("/tmp/worktree/68rwa3iq4y");
        let second = Path::new("/tmp/worktree/rlv3bhtqw9");
        let database = database_name(first, "feature-632");
        assert_eq!(database, database_name(second, "feature-632"));

        let buckets = garage::bucket_names(&database);
        let mut state = registry::NativeRegistry::default();
        registry::claim(
            &mut state,
            first,
            1,
            database.clone(),
            buckets.clone(),
            BTreeMap::new(),
            Vec::new(),
        );

        let error = registry::ensure_tenant_available(&state, second, &database, &buckets)
            .expect_err("another root must not reach Garage provisioning with this tenant");
        assert!(error.to_string().contains("database"), "{error:#}");
        assert_eq!(state.claims.len(), 1, "the rejected root has no claim");
    }

    /// The slot table's last range starts at `21_200` and spans 100. An
    /// internal port base below `21_300` would hand a worktree a number
    /// another worktree's `SurrealDB` already holds — a collision the slot
    /// machinery cannot see, because these ports never reach it.
    #[test]
    fn every_internal_port_base_sits_past_the_slot_tables_last_range() {
        for base in [
            GARAGE_RPC_PORT_BASE,
            GARAGE_ADMIN_PORT_BASE,
            RAUTHY_RAFT_PORT_BASE,
            RAUTHY_API_PORT_BASE,
            RESTATE_FABRIC_PORT_BASE,
            WORKFLOWS_SERVICE_LISTEN_PORT_BASE,
            WORKFLOWS_SERVICE_HEALTH_PORT_BASE,
        ] {
            assert!(base >= 21_300, "{base} overlaps the slot table");
        }
    }

    /// Four bases, four ranges. Two sharing a base would put two
    /// listeners of the same worktree on one port, which presents as one
    /// of them failing to start for no visible reason.
    #[test]
    fn the_internal_port_ranges_do_not_overlap_each_other() {
        let bases = BTreeSet::from([
            GARAGE_RPC_PORT_BASE,
            GARAGE_ADMIN_PORT_BASE,
            RAUTHY_RAFT_PORT_BASE,
            RAUTHY_API_PORT_BASE,
            RESTATE_FABRIC_PORT_BASE,
            WORKFLOWS_SERVICE_LISTEN_PORT_BASE,
            WORKFLOWS_SERVICE_HEALTH_PORT_BASE,
        ]);

        assert_eq!(bases.len(), 7);
        let ordered: Vec<u16> = bases.into_iter().collect();
        for pair in ordered.windows(2) {
            assert!(
                pair[1] - pair[0] >= 100,
                "{pair:?} are closer together than one slot span"
            );
        }
    }

    /// A member cannot be both served and deferred: that reads as
    /// "supervised" to the gate and as "known gap" to the operator, and
    /// only one of them can be true.
    #[test]
    fn no_gate_member_is_both_supervised_and_deferred() {
        for (member, _) in DEFERRED {
            assert!(
                !SUPERVISED.contains(member),
                "{member} is claimed by both lists"
            );
        }
    }

    /// Every gap names the issue that closes it. An unattributed entry
    /// is indistinguishable from something nobody intends to do.
    #[test]
    fn every_deferred_member_names_the_issue_that_will_serve_it() {
        for (member, issue) in DEFERRED {
            assert!(
                issue.starts_with("ENG-"),
                "{member} defers to `{issue}`, which is not an issue identifier"
            );
        }
    }

    /// The gap is reported by the run that has it, not left for someone
    /// to look up. The issue identifiers are the actionable part, so
    /// they have to reach the output.
    #[test]
    fn the_reported_gap_names_each_member_and_its_issue() {
        let printed = deferred_lines().join("\n");

        for (member, issue) in DEFERRED {
            assert!(printed.contains(member), "{printed}");
            assert!(printed.contains(issue), "{printed}");
        }
    }

    #[test]
    fn restate_recipe_is_tcp_only_and_tuned_for_one_partition() {
        let args = restate::start_args(std::path::Path::new("/tmp/native"));
        assert!(args.windows(2).any(|pair| pair == ["--listen-mode", "tcp"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--default-num-partitions", "1"]));
    }

    #[test]
    fn worker_ports_are_distinct_for_two_worktrees() {
        let first = worker::ports(3);
        let second = worker::ports(4);
        assert_ne!(first, second);
        assert_ne!(first.listen, first.health);
        assert_ne!(second.listen, second.health);
    }

    #[test]
    fn private_process_state_is_removed_without_touching_shared_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join(".devx/native/workflows-service")).unwrap();
        std::fs::create_dir_all(root.join(".devx/restate/data")).unwrap();
        std::fs::write(supervisor::ledger_path(root), "[]").unwrap();

        remove_worktree_process_state(root);

        assert!(!supervisor::ledger_path(root).exists());
        assert!(!root.join(".devx/native").exists());
        assert!(!root.join(".devx/restate").exists());
    }
}
