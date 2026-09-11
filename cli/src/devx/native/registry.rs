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

pub(super) fn claim(
    registry: &mut NativeRegistry,
    root: &Path,
    slot: u16,
    database: String,
    buckets: BTreeMap<String, String>,
    garage_env: BTreeMap<String, String>,
) {
    registry.claims.insert(
        key(root),
        NativeClaim {
            root: root.to_path_buf(),
            slot,
            database,
            buckets,
            garage_env,
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

/// Classify claims against currently live checkout paths. Apply receives the
/// same entries after reloading the registry under the host lock.
pub(super) fn plan_sweep(
    registry: &NativeRegistry,
    live_worktrees: &[PathBuf],
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<SweepEntry> {
    let live: BTreeSet<String> = live_worktrees.iter().map(|path| key(path)).collect();
    registry
        .claims
        .iter()
        .map(|(claim_key, claim)| SweepEntry {
            key: claim_key.clone(),
            claim: claim.clone(),
            owner: if live.contains(claim_key) || exists(&claim.root) {
                Owner::Live(claim.root.clone())
            } else {
                Owner::Orphaned
            },
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
    use super::{claim, key, load, plan_sweep, release, report, NativeRegistry, Owner};
    use crate::devx::native::supervisor::Started;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn started(label: &str, pid: u32) -> Started {
        Started {
            label: label.into(),
            pid,
            port: 18_000,
            program: label.into(),
            command: format!("/bin/{label} server"),
            start_time: "Mon Jan  1 00:00:00 2024".into(),
        }
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
        );
        claim(
            &mut registry,
            b,
            2,
            "navigator_b".into(),
            BTreeMap::from([("documents".into(), "b-documents".into())]),
            BTreeMap::new(),
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
            );
        }
        let plan = plan_sweep(&registry, &[PathBuf::from("/tmp/live")], &|path| {
            path == Path::new("/tmp/live")
        });
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
        let plan = plan_sweep(&registry, &[], &|_| false);
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
        );
        let plan = plan_sweep(&registry, &[PathBuf::from("/tmp/live")], &|_| false);
        let output = report(&plan, registry.services.len(), false);
        assert!(output.contains("nothing to reclaim"));
        assert!(!output.contains("orphaned shared service"));
    }
}
