//! Host-wide ownership for the native dependency tier.
//!
//! The registry records one process per shared service and one tenant claim
//! per worktree. All mutations happen while the caller holds the host
//! environment lock.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::supervisor::Started;

const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeClaim {
    pub(super) root: PathBuf,
    pub(super) slot: u16,
    pub(super) database: String,
    pub(super) buckets: BTreeMap<String, String>,
    pub(super) garage_env: BTreeMap<String, String>,
    /// The shared processes this claim attached to, recorded with the same
    /// PID, command, and process-start identity the supervisor signals on.
    ///
    /// A process record is host-wide; a tenant claim is not. Without this
    /// per-claim copy the only thing a claim knows about itself is where its
    /// checkout sits, and a checkout outlives the processes that served it.
    #[serde(default)]
    pub(super) processes: Vec<Started>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NativeRegistry {
    pub(super) version: u32,
    pub(super) services: BTreeMap<String, Started>,
    pub(super) claims: BTreeMap<String, NativeClaim>,
}

impl Default for NativeRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            services: BTreeMap::new(),
            claims: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Load {
    Absent,
    Loaded(NativeRegistry),
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Owner {
    Live(PathBuf),
    Orphaned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SweepEntry {
    pub(super) key: String,
    pub(super) claim: NativeClaim,
    pub(super) owner: Owner,
}

impl SweepEntry {
    pub(crate) fn is_orphaned(&self) -> bool {
        self.owner == Owner::Orphaned
    }
}

pub(super) fn path() -> PathBuf {
    if let Some(path) = std::env::var_os("NAVIGATOR_NATIVE_REGISTRY") {
        return PathBuf::from(path);
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".navigator/native-runtime.json"),
        None => std::env::temp_dir().join("navigator-native-runtime.json"),
    }
}

pub(super) fn state_dir(registry_path: &Path) -> PathBuf {
    registry_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("native-runtime")
}

pub(super) fn load(path: &Path) -> Load {
    match fs::read_to_string(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Load::Absent,
        Err(err) => Load::Unreadable(err.to_string()),
        Ok(raw) => match serde_json::from_str::<NativeRegistry>(&raw) {
            Ok(registry) if registry.version == REGISTRY_VERSION => Load::Loaded(registry),
            Ok(registry) => Load::Unreadable(format!(
                "unsupported registry version {} (expected {REGISTRY_VERSION})",
                registry.version
            )),
            Err(err) => Load::Unreadable(err.to_string()),
        },
    }
}

pub(super) fn save(path: &Path, registry: &NativeRegistry) -> Result<()> {
    let parent = path
        .parent()
        .context("native registry path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create native registry directory {}", parent.display()))?;
    let temp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(registry).context("serialize native registry")?;
    fs::write(&temp, body).with_context(|| format!("write native registry {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("replace native registry {}", path.display()))
}

pub(super) fn key(root: &Path) -> String {
    root.display().to_string()
}

pub(crate) fn service_count(registry: &NativeRegistry) -> usize {
    registry.services.len()
}

/// Refuse to let two worktree roots share a database or Garage bucket.
///
/// The caller holds the host lifecycle lock, so the check stays valid through
/// tenant provisioning and claim persistence. The recorded root may re-enter:
/// repeated `up` must recover its own tenant rather than treating it as a
/// collision.
pub(super) fn ensure_tenant_available(
    registry: &NativeRegistry,
    root: &Path,
    database: &str,
    buckets: &BTreeMap<String, String>,
) -> Result<()> {
    for claim in registry.claims.values() {
        if claim.root == root {
            continue;
        }
        if claim.database == database {
            anyhow::bail!(
                "native database `{database}` is already claimed by {}; refusing to share it with {}",
                claim.root.display(),
                root.display()
            );
        }
        if let Some(bucket) = buckets
            .values()
            .find(|bucket| claim.buckets.values().any(|claimed| claimed == *bucket))
        {
            anyhow::bail!(
                "native Garage bucket `{bucket}` is already claimed by {}; refusing to share it with {}",
                claim.root.display(),
                root.display()
            );
        }
    }
    Ok(())
}

pub(super) fn claim(
    registry: &mut NativeRegistry,
    root: &Path,
    slot: u16,
    database: String,
    buckets: BTreeMap<String, String>,
    garage_env: BTreeMap<String, String>,
    processes: Vec<Started>,
) {
    registry.claims.insert(
        key(root),
        NativeClaim {
            root: root.to_path_buf(),
            slot,
            database,
            buckets,
            garage_env,
            processes,
        },
    );
}

/// Remove one worktree claim. The boolean says whether it was the final
/// claimant, the only condition under which shared processes may stop.
pub(super) fn release(
    registry: &mut NativeRegistry,
    root: &Path,
) -> Option<(NativeClaim, bool, Vec<Started>)> {
    let claim = registry.claims.remove(&key(root))?;
    let final_claim = registry.claims.is_empty();
    let services = if final_claim {
        registry.services.values().cloned().collect()
    } else {
        Vec::new()
    };
    if final_claim {
        registry.services.clear();
    }
    Some((claim, final_claim, services))
}

/// Classify claims against live checkout paths and recorded process identity.
/// Apply receives the same entries after reloading the registry under the host
/// lock.
///
/// Two pieces of evidence, and a claim stays live only while both hold. A
/// checkout survives a reboot, so the path alone cannot tell a working tier
/// from a claim whose processes died with the host; the identities the claim
/// recorded answer that, and answer it the way the supervisor does — the live
/// command and process-start value, so a recycled PID reads as stale.
///
/// Evidence only ever narrows the live set. A claim that recorded no processes
/// is judged by its checkout alone, because an absent record is not proof of a
/// dead process, and `still_ours` never consults a port: a shared process that
/// is running but refusing connections is still somebody's running process.
pub(super) fn plan_sweep(
    registry: &NativeRegistry,
    live_worktrees: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
    still_ours: &dyn Fn(&Started) -> bool,
) -> Vec<SweepEntry> {
    let live: BTreeSet<String> = live_worktrees.iter().map(|path| key(path)).collect();
    registry
        .claims
        .iter()
        .map(|(claim_key, claim)| {
            let checkout_present = live.contains(claim_key) || exists(&claim.root);
            let processes_present =
                claim.processes.is_empty() || claim.processes.iter().any(still_ours);
            SweepEntry {
                key: claim_key.clone(),
                claim: claim.clone(),
                owner: if checkout_present && processes_present {
                    Owner::Live(claim.root.clone())
                } else {
                    Owner::Orphaned
                },
            }
        })
        .collect()
}

pub(super) fn report(plan: &[SweepEntry], shared_service_count: usize, apply: bool) -> String {
    let live_claims = plan
        .iter()
        .filter(|entry| matches!(&entry.owner, Owner::Live(_)))
        .count();
    let orphaned_services = usize::from(live_claims == 0) * shared_service_count;
    let mut out = format!(
        "==> native sweep: {} tenant claim(s) inspected, {} shared service record(s)\n",
        plan.len(),
        shared_service_count
    );
    for entry in plan {
        let state = match &entry.owner {
            Owner::Live(path) => format!("live ({})", path.display()),
            Owner::Orphaned => "orphaned".to_string(),
        };
        let _ = writeln!(
            out,
            "    {}  {:<10} database={} buckets={}",
            entry.key,
            state,
            entry.claim.database,
            entry.claim.buckets.len()
        );
    }
    let orphans = plan.iter().filter(|entry| entry.is_orphaned()).count();
    if orphans == 0 && orphaned_services == 0 {
        out.push_str("\n    nothing to reclaim: no native tenant outlived its worktree\n");
    } else if apply {
        let _ = writeln!(
            out,
            "\n    reclaiming {orphans} orphaned native tenant(s) and {orphaned_services} orphaned shared service record(s)"
        );
    } else {
        let _ = writeln!(
            out,
            "\n    {orphans} orphaned native tenant(s) and {orphaned_services} orphaned shared service record(s) found\n    Dry run: nothing was touched. Re-run with --apply to reclaim them."
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        claim, ensure_tenant_available, key, load, plan_sweep, release, report, NativeRegistry,
        Owner, SweepEntry,
    };
    use crate::devx::native::supervisor::{matches_identity, Started};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn started(label: &str, pid: u32) -> Started {
        started_at(label, pid, "Mon Jan  1 00:00:00 2024")
    }

    fn started_at(label: &str, pid: u32, start_time: &str) -> Started {
        Started {
            label: label.into(),
            pid,
            port: 18_000,
            program: label.into(),
            command: format!("/bin/{label} server"),
            start_time: start_time.into(),
        }
    }

    /// The supervisor's identity check against a known set of live processes:
    /// a record is ours only while its PID still carries the same command and
    /// the same process-start value. Nothing here reads a port.
    fn running(processes: &[Started]) -> impl Fn(&Started) -> bool + '_ {
        move |record| {
            processes.iter().any(|process| {
                process.pid == record.pid
                    && matches_identity(
                        &process.command,
                        &record.command,
                        &process.start_time,
                        &record.start_time,
                        &record.program,
                    )
            })
        }
    }

    fn entry<'a>(plan: &'a [SweepEntry], root: &Path) -> &'a SweepEntry {
        plan.iter()
            .find(|entry| entry.key == key(root))
            .expect("every claim is classified")
    }

    #[test]
    fn concurrent_claims_adopt_one_service_set_and_keep_tenants_distinct() {
        let mut registry = NativeRegistry::default();
        registry
            .services
            .insert("surreal".into(), started("surreal", 7));
        let a = Path::new("/tmp/worktree-a");
        let b = Path::new("/tmp/worktree-b");
        claim(
            &mut registry,
            a,
            1,
            "navigator_a".into(),
            BTreeMap::from([("documents".into(), "a-documents".into())]),
            BTreeMap::new(),
            Vec::new(),
        );
        claim(
            &mut registry,
            b,
            2,
            "navigator_b".into(),
            BTreeMap::from([("documents".into(), "b-documents".into())]),
            BTreeMap::new(),
            Vec::new(),
        );
        assert_eq!(registry.services["surreal"].pid, 7);
        assert_ne!(
            registry.claims[&key(a)].database,
            registry.claims[&key(b)].database
        );
        assert_ne!(
            registry.claims[&key(a)].buckets["documents"],
            registry.claims[&key(b)].buckets["documents"]
        );
    }

    #[test]
    fn tenant_admission_refuses_another_root_but_preserves_repeat_up_and_other_claims() {
        let mut registry = NativeRegistry::default();
        let first = Path::new("/tmp/worktree/68rwa3iq4y");
        let colliding = Path::new("/tmp/worktree/rlv3bhtqw9");
        let ordinary = Path::new("/tmp/worktree/ordinary");
        let database = "navigator_feature_632_51517b6";
        let buckets = BTreeMap::from([(
            "NAVIGATOR_STORAGE_BUCKET".into(),
            "navigator-feature-632-51517b6-documents".into(),
        )]);

        claim(
            &mut registry,
            first,
            1,
            database.into(),
            buckets.clone(),
            BTreeMap::new(),
            Vec::new(),
        );

        assert!(ensure_tenant_available(&registry, first, database, &buckets).is_ok());
        let error = ensure_tenant_available(&registry, colliding, database, &buckets)
            .expect_err("a second root cannot reuse the first root's tenant");
        assert!(error.to_string().contains("database"), "{error:#}");
        assert!(error.to_string().contains(&first.display().to_string()));
        assert!(error.to_string().contains(&colliding.display().to_string()));
        assert!(ensure_tenant_available(
            &registry,
            ordinary,
            "navigator_feature_632_ordinary",
            &BTreeMap::from([(
                "NAVIGATOR_STORAGE_BUCKET".into(),
                "navigator-feature-632-ordinary-documents".into(),
            )]),
        )
        .is_ok());
        assert_eq!(registry.claims.len(), 1, "refusal must not create a claim");

        let shared_bucket = BTreeMap::from([(
            "NAVIGATOR_STORAGE_BUCKET".into(),
            "navigator-feature-632-51517b6-documents".into(),
        )]);
        let bucket_error = ensure_tenant_available(
            &registry,
            ordinary,
            "navigator_feature_632_ordinary",
            &shared_bucket,
        )
        .expect_err("a different database cannot reuse a Garage bucket");
        assert!(bucket_error.to_string().contains("Garage bucket"));

        claim(
            &mut registry,
            ordinary,
            2,
            "navigator_feature_632_ordinary".into(),
            BTreeMap::from([(
                "NAVIGATOR_STORAGE_BUCKET".into(),
                "navigator-feature-632-ordinary-documents".into(),
            )]),
            BTreeMap::new(),
            Vec::new(),
        );
        let (_, final_claim, _) = release(&mut registry, first).expect("release first root");
        assert!(!final_claim);
        assert!(registry.claims.contains_key(&key(ordinary)));

        let (_, final_claim, _) = release(&mut registry, ordinary).expect("release ordinary root");
        assert!(final_claim);
        assert!(registry.claims.is_empty());
    }

    #[test]
    fn first_release_keeps_services_and_last_release_returns_them() {
        let mut registry = NativeRegistry::default();
        registry
            .services
            .insert("surreal".into(), started("surreal", 7));
        for root in [Path::new("/tmp/a"), Path::new("/tmp/b")] {
            claim(
                &mut registry,
                root,
                1,
                root.display().to_string(),
                BTreeMap::new(),
                BTreeMap::new(),
                Vec::new(),
            );
        }
        let (_, final_claim, stopped) = release(&mut registry, Path::new("/tmp/a")).unwrap();
        assert!(!final_claim);
        assert!(stopped.is_empty());
        let (_, final_claim, stopped) = release(&mut registry, Path::new("/tmp/b")).unwrap();
        assert!(final_claim);
        assert_eq!(stopped[0].pid, 7);
        assert!(registry.services.is_empty());
    }

    #[test]
    fn sweep_marks_missing_worktrees_orphaned_and_live_worktrees_live() {
        let mut registry = NativeRegistry::default();
        for root in [Path::new("/tmp/live"), Path::new("/tmp/gone")] {
            claim(
                &mut registry,
                root,
                1,
                root.display().to_string(),
                BTreeMap::new(),
                BTreeMap::new(),
                Vec::new(),
            );
        }
        let plan = plan_sweep(
            &registry,
            &[PathBuf::from("/tmp/live")],
            &|path| path == Path::new("/tmp/live"),
            &running(&[]),
        );
        assert_eq!(plan.len(), 2);
        assert!(matches!(
            plan.iter()
                .find(|entry| entry.key == key(Path::new("/tmp/live")))
                .unwrap()
                .owner,
            Owner::Live(_)
        ));
        assert!(plan
            .iter()
            .find(|entry| entry.key == key(Path::new("/tmp/gone")))
            .unwrap()
            .is_orphaned());
    }

    #[test]
    fn missing_and_corrupt_registry_state_are_distinct() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing.json");
        assert!(matches!(load(&missing), super::Load::Absent));
        let corrupt = temp.path().join("corrupt.json");
        std::fs::write(&corrupt, "{").unwrap();
        assert!(matches!(load(&corrupt), super::Load::Unreadable(_)));
    }

    #[test]
    fn shared_process_records_are_not_sweep_tenant_claims() {
        let mut registry = NativeRegistry::default();
        registry
            .services
            .insert("surreal".into(), started("surreal", 7));
        let plan = plan_sweep(&registry, &[], &|_| false, &running(&[]));
        assert!(plan.is_empty());
        assert!(report(&plan, registry.services.len(), false).contains("orphaned shared service"));
    }

    #[test]
    fn live_claim_keeps_shared_services_out_of_orphan_sweep_report() {
        let mut registry = NativeRegistry::default();
        registry
            .services
            .insert("surreal".into(), started("surreal", 7));
        claim(
            &mut registry,
            Path::new("/tmp/live"),
            1,
            "navigator_live".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let plan = plan_sweep(
            &registry,
            &[PathBuf::from("/tmp/live")],
            &|_| false,
            &running(&[]),
        );
        let output = report(&plan, registry.services.len(), false);
        assert!(output.contains("nothing to reclaim"));
        assert!(!output.contains("orphaned shared service"));
    }

    /// A reboot leaves every checkout on disk and no process running. Path
    /// existence therefore cannot separate a worktree still using the tier
    /// from one whose CLI died with the host — only the recorded identity
    /// can, and a PID that comes back on a fresh process comes back with a
    /// different process-start value.
    #[test]
    fn a_stale_process_record_orphans_a_claim_whose_checkout_survived() {
        let mut registry = NativeRegistry::default();
        let current = started_at("surreal", 7, "Tue Jan  2 09:00:00 2024");
        let before_reboot = started_at("surreal", 7, "Mon Jan  1 00:00:00 2024");
        registry.services.insert("surreal".into(), current.clone());
        let live = Path::new("/tmp/live");
        let stale = Path::new("/tmp/stale");
        claim(
            &mut registry,
            live,
            1,
            "navigator_live".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            vec![current.clone()],
        );
        claim(
            &mut registry,
            stale,
            2,
            "navigator_stale".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            vec![before_reboot],
        );

        let plan = plan_sweep(&registry, &[], &|_| true, &running(&[current]));
        assert!(matches!(entry(&plan, live).owner, Owner::Live(_)));
        assert!(entry(&plan, stale).is_orphaned());

        // Apply's rule, at the point the plan decides it: reclaiming the
        // orphan leaves a live claim behind, so the shared process that
        // claim is using is never a candidate to stop.
        let (_, final_claim, stopped) = release(&mut registry, stale).expect("release the orphan");
        assert!(!final_claim);
        assert!(stopped.is_empty());
        assert!(registry.claims.contains_key(&key(live)));
        assert_eq!(registry.services["surreal"].pid, 7);
        assert!(report(&plan, registry.services.len(), true).contains(
            "reclaiming 1 orphaned native tenant(s) and 0 orphaned shared service record(s)"
        ));
    }

    /// An interrupted `up` writes a claim before it can record what it
    /// attached to. Absent evidence is not evidence of a dead process, so
    /// such a claim falls back to its checkout rather than being reclaimed
    /// on a host where nothing is running at all.
    #[test]
    fn a_claim_with_no_process_evidence_is_judged_by_its_checkout_alone() {
        let mut registry = NativeRegistry::default();
        let root = Path::new("/tmp/unrecorded");
        claim(
            &mut registry,
            root,
            1,
            "navigator_unrecorded".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );

        let present = plan_sweep(&registry, &[], &|_| true, &running(&[]));
        assert!(matches!(entry(&present, root).owner, Owner::Live(_)));

        let gone = plan_sweep(&registry, &[], &|_| false, &running(&[]));
        assert!(entry(&gone, root).is_orphaned());
    }

    /// The port is the supervisor's readiness gate, never the planner's
    /// evidence. A shared dependency that is running but refusing
    /// connections is still a process a live claim depends on, so a closed
    /// port alone must not authorize reclaiming its tenant.
    #[test]
    fn a_closed_port_alone_does_not_orphan_a_claim() {
        let mut registry = NativeRegistry::default();
        let record = started("surreal", 7);
        let root = Path::new("/tmp/quiet");
        claim(
            &mut registry,
            root,
            1,
            "navigator_quiet".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            vec![record.clone()],
        );

        let plan = plan_sweep(&registry, &[], &|_| true, &running(&[record]));
        assert!(matches!(entry(&plan, root).owner, Owner::Live(_)));
    }

    /// The same PID carrying a different program is a stranger that
    /// inherited the number, not the service this claim recorded.
    #[test]
    fn a_recycled_pid_running_a_stranger_does_not_keep_a_claim_live() {
        let mut registry = NativeRegistry::default();
        let recorded = started("surreal", 7);
        let stranger = Started {
            program: "vim".into(),
            command: "/usr/bin/vim notes.txt".into(),
            ..recorded.clone()
        };
        let root = Path::new("/tmp/recycled");
        claim(
            &mut registry,
            root,
            1,
            "navigator_recycled".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            vec![recorded],
        );

        let plan = plan_sweep(&registry, &[], &|_| true, &running(&[stranger]));
        assert!(entry(&plan, root).is_orphaned());
    }

    /// One surviving identity is enough. A claim whose Rauthy process died
    /// is still using the `SurrealDB` process that did not, and reclaiming
    /// its tenant would pull the database out from under a running `web`.
    #[test]
    fn one_surviving_process_identity_keeps_a_claim_live() {
        let mut registry = NativeRegistry::default();
        let surreal = started("surreal", 7);
        let rauthy = started("rauthy", 8);
        let root = Path::new("/tmp/partial");
        claim(
            &mut registry,
            root,
            1,
            "navigator_partial".into(),
            BTreeMap::new(),
            BTreeMap::new(),
            vec![surreal.clone(), rauthy],
        );

        let plan = plan_sweep(&registry, &[], &|_| true, &running(&[surreal]));
        assert!(matches!(entry(&plan, root).owner, Owner::Live(_)));
    }
}
