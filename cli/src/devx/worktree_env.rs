//! `navigator dev worktree-env` — a dev environment per git worktree.
//!
//! Agent harnesses may pre-provision a fresh git worktree per task and export
//! `NAVIGATOR_WORKTREE_PATH`. The same command also supports a normal primary
//! checkout: `--branch` creates a sibling worktree when none was supplied and
//! branches a supplied checkout in place.
//!
//! ```text
//! cargo run -p cli -- dev worktree-env up --branch <topic-branch>
//! cargo run -p cli -- dev worktree-env down
//! ```
//!
//! Two modes, both reached through the same front door:
//!
//! - **dev (default)** — an isolated KIND dependency tier per worktree. The
//!   tier owns its `SurrealDB`, Rauthy, Garage, Restate broker, and
//!   `workflows-service`; host `web` receives matching, worktree-specific
//!   forwards. A Restate journal and worker can therefore never cross from one
//!   checkout into another.
//! - **demo (`--demo`)** — the full stack running *in* KIND from the
//!   images CI published to Artifact Registry (no local build). Delegates to the
//!   pull-based [`super::deploy`]; one full stack at a time (a demo is
//!   shown, not parallelised).
//!
//! The per-worktree state (`.devx/worktree.json` + `.devx/env`) lives
//! inside the worktree, which is itself gitignored (`/.devx/`), so
//! nothing here ever lands in the tree.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use super::runtime::Runtime;
use super::KindConfig;

/// Independent local topology slots. Each slot reserves one port in every
/// worktree-only range below, so its components cannot collide with another
/// worktree or the ordinary `navigator dev up` defaults.
const WORKTREE_PORT_SPAN: u16 = 100;
/// The floor of the worktree window. No tier member binds this band, and
/// the window keeps its start regardless so every other base, and every
/// already-provisioned worktree's ports, stay where they are.
const WORKTREE_PORT_WINDOW_START: u16 = 20_000;
const WORKTREE_INGRESS_HTTP_PORT_BASE: u16 = 20_900;
const WORKTREE_INGRESS_HTTPS_PORT_BASE: u16 = 21_000;
const WORKTREE_RESTATE_INGRESS_PORT_BASE: u16 = 20_100;
const WORKTREE_RESTATE_ADMIN_PORT_BASE: u16 = 20_200;
const WORKTREE_RAUTHY_PORT_BASE: u16 = 20_400;
const WORKTREE_GARAGE_S3_PORT_BASE: u16 = 20_500;
const WORKTREE_WEB_PORT_BASE: u16 = 20_600;
/// DeleteYourData.com's own local bind port (ENG-437): locally there is no
/// DNS standing in for its real hostname, so each worktree gets it a second
/// port the same way it gets `web` one. `20_300` is the one 100-band in the
/// worktree window no other tier member claims.
const WORKTREE_DELETE_YOUR_DATA_WEB_PORT_BASE: u16 = 20_300;
const WORKTREE_OPENOBSERVE_PORT_BASE: u16 = 20_700;
const WORKTREE_OPENOBSERVE_OTLP_PORT_BASE: u16 = 20_800;
const WORKTREE_CLAMAV_PORT_BASE: u16 = 21_100;
/// `SurrealDB`, the store the workspace is porting onto (#1093). It takes
/// one port per slot like every other tier member: isolation comes from
/// the worktree's own cluster, so two checkouts hold two engines.
const WORKTREE_SURREAL_PORT_BASE: u16 = 21_200;
/// Longest slug we keep — enough to stay readable in a database name and
/// a topic branch label without risking the engine's identifier
/// limit once `navigator_` and the path fingerprint are appended.
const MAX_SLUG_LEN: usize = 40;

#[derive(Subcommand)]
pub enum WorktreeEnvCmd {
    /// Stand up this worktree's environment. Idempotent: re-running
    /// restores it (e.g. after a reboot) and keeps the same port.
    Up {
        /// Existing worktree directory. Defaults to a harness-provided
        /// worktree path, then the current directory.
        #[arg(long)]
        path: Option<PathBuf>,
        /// Prepare this topic branch before standing up its environment.
        /// A supplied worktree is branched in place; otherwise a sibling
        /// `.worktrees/<slug>` checkout is created from `origin/main`.
        #[arg(long)]
        branch: Option<String>,
        /// Full-stack demo: run `web` + `workflows-service` IN the KIND
        /// cluster from published Artifact Registry images, instead of the
        /// light host-`web` + shared-deps dev environment.
        #[arg(long)]
        demo: bool,
        /// Pin an immutable registry image tag to pull (`YY.M.D` or
        /// `YY.M.D-hotfix.N`). Only meaningful
        /// with `--demo`; omit to pull the latest published tag.
        #[arg(long)]
        tag: Option<String>,
        /// Assume this worktree's isolated KIND dependency tier is already
        /// up; don't bring it up.
        #[arg(long)]
        no_deps: bool,
        /// Which dependency tier to provision. `native` runs host
        /// processes shared across worktrees; `kind` gives this worktree
        /// its own cluster. `--demo` always uses `kind` — it needs the
        /// cluster's ingress.
        #[arg(long, default_value = "kind")]
        runtime: Runtime,
    },
    /// Tear down this worktree's isolated environment. Idempotent — exits 0
    /// even if nothing is up.
    Down {
        /// Worktree directory. Defaults to the current directory.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Show this worktree's environment (slug, mode, database, port) and
    /// whether it is reachable.
    Status {
        /// Worktree directory. Defaults to the current directory.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Reclaim abandoned worktree environments: KIND clusters whose worktree
    /// no longer exists on disk. A worktree deleted without `worktree-env
    /// down` leaks its cluster, which keeps binding its slot's host ports.
    /// Lists what it would delete and changes nothing unless `--apply` is
    /// given. Never selects the shared `dev up` cluster or a cluster a live
    /// worktree owns, and never prunes Docker volumes.
    Sweep {
        /// Delete the orphaned clusters. Without it, `sweep` is a dry run.
        #[arg(long)]
        apply: bool,
        /// Repository to enumerate worktrees from. Defaults to the current
        /// directory.
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

/// The persisted descriptor for a worktree's environment, written to
/// `<worktree>/.devx/worktree.json` so `down`/`status` act on exactly
/// what `up` created rather than re-deriving it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorktreeEnv {
    /// Sanitized worktree identity (from the branch name).
    slug: String,
    /// `"dev"` or `"demo"`.
    mode: String,
    /// The environment-owned store database (dev mode only).
    db_name: Option<String>,
    /// Host `web` port (dev mode) / ingress note (demo mode).
    web_port: u16,
    /// Isolated KIND port slot for dev mode. Demo mode uses the ordinary
    /// single-stack topology and does not reserve a slot.
    #[serde(default)]
    slot: Option<u16>,
    /// Which dependency tier `up` provisioned.
    ///
    /// Persisted so `down` and `status` reach the tier that actually
    /// exists without the caller repeating `--runtime`. Getting this
    /// wrong is not a cosmetic error: `down` against the wrong lane
    /// reports success while leaving a live cluster bound to this
    /// worktree's ports, which permanently narrows the slot pool.
    #[serde(default = "Runtime::of_existing_descriptor")]
    runtime: Runtime,
}

pub fn dispatch(cmd: WorktreeEnvCmd, base_cfg: &KindConfig) -> Result<()> {
    match cmd {
        WorktreeEnvCmd::Up {
            path,
            branch,
            demo,
            tag,
            no_deps,
            runtime,
        } => {
            let supplied = path.or_else(|| agent_worktree_path(|key| std::env::var_os(key)));
            let prepares_branch = branch.is_some();
            let root = match branch {
                Some(branch) => {
                    prepare_topic_checkout(&worktree_root(None)?, supplied.as_deref(), &branch)?
                }
                None => worktree_root(supplied.as_deref())?,
            };
            if prepares_branch {
                eprintln!("==> task checkout: {}", root.display());
            }
            if demo {
                up_demo(&root, tag.as_deref(), base_cfg)
            } else {
                up_dev(&root, no_deps, runtime, base_cfg)
            }
        }
        WorktreeEnvCmd::Down { path } => {
            let supplied = path.or_else(|| agent_worktree_path(|key| std::env::var_os(key)));
            down(&worktree_root(supplied.as_deref())?, base_cfg)
        }
        WorktreeEnvCmd::Status { path } => {
            let supplied = path.or_else(|| agent_worktree_path(|key| std::env::var_os(key)));
            status(&worktree_root(supplied.as_deref())?, base_cfg);
            Ok(())
        }
        WorktreeEnvCmd::Sweep { apply, path } => {
            let supplied = path.or_else(|| agent_worktree_path(|key| std::env::var_os(key)));
            sweep(&worktree_root(supplied.as_deref())?, apply, base_cfg)
        }
    }
}

// ---------- topic-worktree preparation ----------

/// Resolve a pre-provisioned worktree without exposing an agent-specific
/// branch ceremony in the public workflow. `NAVIGATOR_WORKTREE_PATH` is the
/// generic contract; `CODEX_WORKTREE_PATH` is a compatibility bridge for an
/// existing harness environment.
fn agent_worktree_path(get: impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    ["NAVIGATOR_WORKTREE_PATH", "CODEX_WORKTREE_PATH"]
        .into_iter()
        .filter_map(get)
        .find(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Prepare `branch` in exactly one checkout. When an agent harness or caller
/// supplies a worktree — or the command already runs in Git's linked
/// worktree — branch that checkout in place. Otherwise create a sibling under
/// the primary checkout. The returned path is the checkout that must own the
/// host `web` port and `.devx` descriptor.
fn prepare_topic_checkout(
    invocation_root: &Path,
    supplied: Option<&Path>,
    branch: &str,
) -> Result<PathBuf> {
    git_checked(
        invocation_root,
        &["check-ref-format", "--branch", branch],
        "validate topic branch name",
    )?;

    let supplied_or_current_worktree =
        supplied
            .map(git_checkout_root)
            .transpose()?
            .or(is_linked_worktree(invocation_root)?
                .then(|| git_checkout_root(invocation_root))
                .transpose()?);
    if let Some(root) = supplied_or_current_worktree {
        git_checked(&root, &["fetch", "origin", "main"], "fetch origin/main")?;
        switch_or_create_topic_branch(&root, branch)?;
        return root
            .canonicalize()
            .context("resolve supplied worktree path");
    }

    let primary = primary_worktree(invocation_root)?;
    git_checked(&primary, &["fetch", "origin", "main"], "fetch origin/main")?;
    let target = primary.join(".worktrees").join(slugify(branch));
    if target.exists() {
        let root = git_checkout_root(&target)?;
        if git_branch(&root).as_deref() != Some(branch) {
            bail!(
                "{} already exists but is not checked out on `{branch}`",
                target.display()
            );
        }
        return Ok(root);
    }

    std::fs::create_dir_all(primary.join(".worktrees"))
        .with_context(|| format!("create {}", primary.join(".worktrees").display()))?;
    let target_text = target
        .to_str()
        .context("the worktree target path was not UTF-8")?;
    if local_branch_exists(&primary, branch) {
        git_checked(
            &primary,
            &["worktree", "add", target_text, branch],
            "attach existing topic branch in a sibling worktree",
        )?;
    } else {
        git_checked(
            &primary,
            &[
                "worktree",
                "add",
                "--no-track",
                "-b",
                branch,
                target_text,
                "origin/main",
            ],
            "create topic branch in a sibling worktree",
        )?;
    }
    git_checkout_root(&target)
}

fn primary_worktree(root: &Path) -> Result<PathBuf> {
    let listing = git_worktree_listing(root)?;
    let primary = worktree_paths(&listing)
        .next()
        .context("Git reported no primary worktree")?;
    Ok(primary)
}

/// Whether `root` is a linked Git worktree rather than this repository's
/// primary checkout. Codex creates these worktrees detached by default, so
/// there is no branch name or environment variable we can rely on here.
fn is_linked_worktree(root: &Path) -> Result<bool> {
    Ok(git_checkout_root(root)? != primary_worktree(root)?.canonicalize()?)
}

fn switch_or_create_topic_branch(root: &Path, branch: &str) -> Result<()> {
    if git_branch(root).as_deref() == Some(branch) {
        return Ok(());
    }
    if local_branch_exists(root, branch) {
        git_checked(root, &["switch", branch], "switch to existing topic branch")
    } else {
        git_checked(
            root,
            &["switch", "--no-track", "-c", branch, "origin/main"],
            "create topic branch in the supplied worktree",
        )
    }
}

fn local_branch_exists(root: &Path, branch: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .status()
        .is_ok_and(|status| status.success())
}

fn git_checkout_root(path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| format!("inspect Git checkout at {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "{} exists but is not a Git worktree: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let root = String::from_utf8(output.stdout).context("Git checkout path was not UTF-8")?;
    PathBuf::from(root.trim())
        .canonicalize()
        .context("resolve Git checkout path")
}

fn git_checked(root: &Path, args: &[&str], action: &str) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| action.to_owned())?;
    if !output.status.success() {
        bail!(
            "{action} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

// ---------- dev mode ----------

fn up_dev(root: &Path, no_deps: bool, runtime: Runtime, base_cfg: &KindConfig) -> Result<()> {
    let slug = slug_for(root)?;
    // Several worktrees can start concurrently. Serialize slot reservation
    // and lifecycle work so no two environments create the same host forwards.
    let _setup_lock = acquire_worktree_env_lock(root)?;
    let _host_lock = acquire_host_worktree_env_lock()?;
    let existing = read_descriptor(root);
    let db_name = "navigator".to_string();
    let claimed_slots = claimed_worktree_slots(root, &list_kind_cluster_ports)?;
    let slot = choose_worktree_slot(
        root,
        existing.as_ref().and_then(WorktreeEnv::dev_slot),
        &claimed_slots,
        base_cfg,
        &port_listening,
    )?;
    let cfg = worktree_kind_config(base_cfg, root, slot);
    let mut sample_project_refreshed = false;
    match runtime {
        Runtime::Kind => eprintln!(
            "==> worktree-env up (dev, kind): slug={slug} cluster={} database={db_name}",
            cfg.cluster
        ),
        Runtime::Native => {
            eprintln!("==> worktree-env up (dev, native): slug={slug} database={db_name}");
        }
    }

    // Claim the cluster on the host before anything binds a port, for the same
    // reason the descriptor is written early: an interrupted setup otherwise
    // leaves a cluster that no clone admits owning, which is exactly what
    // `sweep` would then delete out from under a checkout that still exists.
    // The native lane starts no cluster, so it has none to claim.
    if runtime == Runtime::Kind {
        register_worktree_cluster(&host_cluster_registry_path(), &cfg.cluster, root)?;
    }

    // Record the claim before anything binds a port. An interrupted setup
    // otherwise leaves a cluster holding ports with no reservation behind it,
    // which is how a slot silently becomes available to a second worktree.
    write_descriptor(
        root,
        &WorktreeEnv {
            slug: slug.clone(),
            mode: "dev".into(),
            db_name: Some(db_name.clone()),
            web_port: cfg.web_port,
            slot: Some(slot),
            runtime,
        },
    )?;

    // Provision the tier. Everything below this block is lane-neutral:
    // both lanes leave the same ports listening, so the migration, the
    // schema apply, and the rendered environment are identical work.
    match runtime {
        Runtime::Kind => {
            if !no_deps && (!worktree_deps_ready(&cfg) || !super::rauthy_deployment_exists(&cfg)) {
                eprintln!(
                    "==> isolated deps need reconciliation for {}; applying its KIND fixture",
                    cfg.cluster
                );
                super::up_in(root, &cfg)?;
                sample_project_refreshed = true;
            }
            ensure_worktree_deps_ready(&cfg)?;
            // Pin this worktree's isolated KIND context before touching the cluster.
            // The fresh-boot branch above reaches it through `up_in` →
            // `configure_worktree_kubeconfig`, but a reused cluster skips `up_in`
            // entirely — and `hydrate_garage_environment` then execs `garage-0` over
            // bare `kubectl`, inheriting whatever context is ambient. On an operator
            // pointed at prod that is a `kubectl exec` in the production cluster, so
            // the pin is unconditional here rather than gated on the boot branch.
            super::configure_worktree_kubeconfig(root, &cfg)?;
            // Unconditional for the same reason as the kubeconfig pin above: the
            // fresh-boot branch aligns Rauthy inside `up_in`, but a *reused* cluster
            // skips `up_in` entirely. Re-applying an unchanged public URL starts no
            // rollout, so the reused path pays nothing.
            super::align_rauthy_public_url(&cfg)?;
            super::hydrate_garage_environment(&cfg)?;
        }
        Runtime::Native => {
            // `--no-deps` means the same thing on both lanes: the tier is
            // already up, so don't provision it. The readiness gate still
            // runs, because "already up" is a claim worth checking.
            if !no_deps {
                super::native::up(root, slot, &cfg)?;
            }
            ensure_native_deps_ready(&cfg)?;
        }
    }

    // The schema is applied, not migrated: one idempotent DEFINE file
    // re-applied on every `up` (#1093). Unconditional, because a reused
    // cluster skips `up_in` entirely and would otherwise keep whatever
    // definitions it was created with. This is the database the
    // in-cluster worker owns too, so a worktree can make the schema ready
    // without creating a private topology Restate cannot observe.
    super::surreal::apply_schema(&cfg, &db_name)?;
    eprintln!("==> applied the SurrealDB schema to {db_name}");

    if !sample_project_refreshed {
        super::sample_project::run_for_root(false, root)?;
    }

    let env_body = super::render_env_for(&cfg, &db_name, cfg.web_port, root);
    write_worktree_env(root, &env_body)?;

    print_dev_summary(&slug, &db_name, runtime, &cfg);
    Ok(())
}

fn down(root: &Path, base_cfg: &KindConfig) -> Result<()> {
    // Teardown mutates the same descriptor and database state that `up`
    // inspects. Keep the lock for the whole transaction so `down` cannot
    // remove a reservation or database while another worktree is setting up.
    let _environment_lock = acquire_worktree_env_lock(root)?;
    let _host_lock = acquire_host_worktree_env_lock()?;
    let desc = read_descriptor(root);
    if desc.as_ref().map(|d| d.mode.as_str()) == Some("demo") {
        eprintln!("==> worktree-env down (demo): deleting the in-cluster stack");
        // Removes the navigator namespace; leaves the cluster + deps.
        super::undeploy(base_cfg)?;
    } else if desc.as_ref().map(|d| d.mode.as_str()) == Some("dev") {
        let slot = desc
            .as_ref()
            .and_then(WorktreeEnv::dev_slot)
            .unwrap_or_else(|| derived_worktree_slot(root));
        let cfg = worktree_kind_config(base_cfg, root, slot);
        // The descriptor records which tier `up` actually provisioned.
        // Tearing down the wrong lane reports success while leaving this
        // worktree's ports bound, which permanently narrows the slot pool.
        match desc
            .as_ref()
            .map_or_else(Runtime::of_existing_descriptor, |d| d.runtime)
        {
            Runtime::Kind => {
                super::down_in(root, &cfg)?;
                unregister_worktree_cluster(&host_cluster_registry_path(), &cfg.cluster)?;
            }
            Runtime::Native => super::native::down(root)?,
        }
    } else {
        // A failed setup can stop before writing its descriptor. Reclaim
        // both lanes: the cluster name and port slot are path-derived, so
        // teardown stays scoped to this checkout either way, and the
        // native side only signals PIDs this worktree recorded.
        let cfg = worktree_kind_config(base_cfg, root, derived_worktree_slot(root));
        super::native::down(root)?;
        super::down_in(root, &cfg)?;
        unregister_worktree_cluster(&host_cluster_registry_path(), &cfg.cluster)?;
    }
    remove_worktree_state(root);
    eprintln!("==> worktree-env down complete");
    Ok(())
}

fn status(root: &Path, base_cfg: &KindConfig) {
    match read_descriptor(root) {
        None => println!("worktree-env: not set up (no .devx/worktree.json)"),
        Some(d) => {
            println!(
                "worktree-env: slug={} mode={} runtime={}",
                d.slug, d.mode, d.runtime
            );
            if let Some(db) = &d.db_name {
                println!("  database: {db}");
            }
            println!(
                "  web port {}: {}",
                d.web_port,
                yes_no(port_listening(d.web_port))
            );
            if let (Some(slot), Some(_db)) = (d.dev_slot(), &d.db_name) {
                let cfg = worktree_kind_config(base_cfg, root, slot);
                println!("  port slot: {slot}");
                println!(
                    "  delete-your-data web port {}: {}",
                    cfg.delete_your_data_web_port,
                    yes_no(port_listening(cfg.delete_your_data_web_port))
                );
                match d.runtime {
                    Runtime::Kind => {
                        println!("  KIND cluster: {}", cfg.cluster);
                        println!(
                            "  Restate 127.0.0.1:{}: {}",
                            cfg.restate_ingress_port,
                            yes_no(port_listening(cfg.restate_ingress_port))
                        );
                        println!(
                            "  Surreal 127.0.0.1:{}: {}",
                            cfg.surreal_port,
                            yes_no(port_listening(cfg.surreal_port))
                        );
                    }
                    // The native lane has no cluster to name, so its
                    // supervised processes are what there is to report:
                    // one line each, PID and port, live or not.
                    Runtime::Native => {
                        println!("  host processes:");
                        for line in super::native::status_lines(root) {
                            println!("{line}");
                        }
                        for line in super::native::deferred_lines() {
                            println!("{line}");
                        }
                    }
                }
            }
        }
    }
}

// ---------- demo mode ----------

fn up_demo(root: &Path, tag: Option<&str>, base_cfg: &KindConfig) -> Result<()> {
    let slug = slug_for(root)?;
    let _setup_lock = acquire_worktree_env_lock(root)?;
    eprintln!(
        "==> worktree-env up (demo): full stack in KIND from Artifact Registry (slug={slug})"
    );
    // Validate `--tag` up front so a bad tag fails before any cluster
    // work, then hand it to `deploy` as a parameter (no process-env
    // mutation).
    if let Some(tag) = tag {
        super::registry::validate_release_tag(tag)?;
    }
    super::deploy(base_cfg, tag)?;
    write_descriptor(
        root,
        &WorktreeEnv {
            slug,
            mode: "demo".into(),
            db_name: None,
            web_port: 8080,
            slot: None,
            runtime: Runtime::Kind,
        },
    )?;
    eprintln!();
    eprintln!("==> demo stack up. Reach navigator-web through the KIND ingress:");
    eprintln!("    http://localhost:8080");
    eprintln!("    (pre-seed a lawyer role with `navigator dev grant-lawyer`)");
    Ok(())
}

// ---------- slug + port derivation (pure, unit-tested) ----------

/// Derive the worktree's slug from its git branch (falling back to the
/// directory name when detached / not a repo).
fn slug_for(root: &Path) -> Result<String> {
    let branch = git_branch(root);
    let raw = match branch {
        Some(b) if b != "HEAD" => b,
        _ => root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let slug = slugify(&raw);
    if slug.is_empty() {
        bail!("could not derive a worktree slug from {}", root.display());
    }
    Ok(slug)
}

/// Current branch name for the repo at `root`, or `None` if git is
/// unavailable or the call fails.
fn git_branch(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Lowercase, replace every run of non-`[a-z0-9]` with a single `-`,
/// trim leading/trailing `-`, and truncate. The result is safe as both a
/// Kubernetes-ish label and (with `-`→`_`) a database identifier.
fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let mut s: String = trimmed.chars().take(MAX_SLUG_LEN).collect();
    // A truncation can leave a trailing dash — trim again.
    while s.ends_with('-') {
        s.pop();
    }
    s
}

/// The worktree's identity, hashed. The path is what actually
/// distinguishes two worktrees: the slug is derived from the branch and
/// falls back to the directory name when detached, and every worktree an
/// agent harness creates carries the same directory name.
fn path_hash(root: &Path) -> u32 {
    fnv1a(&root.display().to_string())
}

/// Short, stable identity for the actual worktree directory. Eight hex
/// characters keep the database name under the engine's identifier
/// limit when paired with `MAX_SLUG_LEN`.
fn worktree_fingerprint(root: &Path) -> String {
    format!("{:08x}", path_hash(root))
}

/// A small, stable, non-cryptographic hash (FNV-1a, 32-bit) so a given
/// worktree always derives the same starting port — no randomness, which
/// would re-roll the port (and the OAuth redirect) on every run.
fn fnv1a(s: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// The initial isolated-KIND slot for this worktree. The path keeps a retry
/// stable; collision handling below chooses the next free slot instead.
fn derived_worktree_slot(root: &Path) -> u16 {
    u16::try_from(path_hash(root) % u32::from(WORKTREE_PORT_SPAN)).unwrap_or(0)
}

/// Build the complete KIND configuration owned by one worktree.
fn worktree_kind_config(base: &KindConfig, root: &Path, slot: u16) -> KindConfig {
    let mut cfg = base.clone();
    cfg.cluster = worktree_cluster_name(&base.cluster, root);
    cfg.ingress_http_port = WORKTREE_INGRESS_HTTP_PORT_BASE + slot;
    cfg.ingress_https_port = WORKTREE_INGRESS_HTTPS_PORT_BASE + slot;
    cfg.restate_ingress_port = WORKTREE_RESTATE_INGRESS_PORT_BASE + slot;
    cfg.restate_admin_port = WORKTREE_RESTATE_ADMIN_PORT_BASE + slot;
    cfg.rauthy_port = WORKTREE_RAUTHY_PORT_BASE + slot;
    cfg.garage_s3_port = WORKTREE_GARAGE_S3_PORT_BASE + slot;
    cfg.web_port = WORKTREE_WEB_PORT_BASE + slot;
    cfg.delete_your_data_web_port = WORKTREE_DELETE_YOUR_DATA_WEB_PORT_BASE + slot;
    cfg.openobserve_port = WORKTREE_OPENOBSERVE_PORT_BASE + slot;
    cfg.openobserve_otlp_port = WORKTREE_OPENOBSERVE_OTLP_PORT_BASE + slot;
    cfg.clamav_port = WORKTREE_CLAMAV_PORT_BASE + slot;
    cfg.surreal_port = WORKTREE_SURREAL_PORT_BASE + slot;
    cfg
}

/// Derive a worktree cluster name exactly once.
///
/// The CLI auto-loads `.devx/env`, so a repeated `worktree-env up` can receive
/// its own previously rendered `NAVIGATOR_KIND_CLUSTER` as the base. Treat the
/// matching path fingerprint as already derived instead of appending it again.
fn worktree_cluster_name(base_cluster: &str, root: &Path) -> String {
    let fingerprint = worktree_fingerprint(root);
    let base = slugify(base_cluster);
    if base.ends_with(&format!("-{fingerprint}")) {
        base
    } else {
        format!("{base}-{fingerprint}")
    }
}

/// Every member of the dependency tier the readiness gate covers, named.
///
/// One table, read by both lanes, so neither can quietly answer for
/// fewer ports than the gate asks about. The native lane serves only
/// part of this list today; the rest is declared in
/// [`super::native::DEFERRED`] with the issue that will serve it, and a
/// test requires those two lists to partition exactly these names.
/// Dropping a port from the gate to make a lane pass is the failure that
/// arrangement exists to prevent.
fn gate_members(cfg: &KindConfig) -> [(&'static str, u16); 10] {
    [
        ("KIND ingress HTTP", cfg.ingress_http_port),
        ("KIND ingress HTTPS", cfg.ingress_https_port),
        ("Restate ingress", cfg.restate_ingress_port),
        ("Restate admin", cfg.restate_admin_port),
        ("Rauthy", cfg.rauthy_port),
        ("Garage", cfg.garage_s3_port),
        ("OpenObserve", cfg.openobserve_port),
        ("OpenObserve OTLP", cfg.openobserve_otlp_port),
        ("ClamAV", cfg.clamav_port),
        ("SurrealDB", cfg.surreal_port),
    ]
}

fn worktree_deps_ready(cfg: &KindConfig) -> bool {
    gate_members(cfg)
        .into_iter()
        .all(|(_, port)| port_listening(port))
}

/// Wait for every gate member the native lane supervises, then say which
/// ones it does not.
///
/// The cluster counterpart is [`ensure_worktree_deps_ready`]. This one
/// covers a strict subset of the same table — not a shorter table — and
/// reports the difference instead of leaving `up` looking complete.
fn ensure_native_deps_ready(cfg: &KindConfig) -> Result<()> {
    for (name, port) in gate_members(cfg) {
        if super::native::SUPERVISED.contains(&name) {
            super::wait_for_tcp("127.0.0.1", port)
                .with_context(|| format!("the native {name} process must be reachable"))?;
        }
    }
    for line in super::native::deferred_lines() {
        eprintln!("{line}");
    }
    Ok(())
}

fn ensure_worktree_deps_ready(cfg: &KindConfig) -> Result<()> {
    for (name, port) in [
        ("Restate ingress", cfg.restate_ingress_port),
        ("Restate admin", cfg.restate_admin_port),
        ("Rauthy", cfg.rauthy_port),
        ("Garage", cfg.garage_s3_port),
        ("OpenObserve", cfg.openobserve_port),
        ("OpenObserve OTLP", cfg.openobserve_otlp_port),
        ("ClamAV", cfg.clamav_port),
    ] {
        super::wait_for_tcp("127.0.0.1", port)
            .with_context(|| format!("the worktree KIND {name} forward must be reachable"))?;
    }
    Ok(())
}

fn worktree_slot_has_listener(
    base: &KindConfig,
    root: &Path,
    slot: u16,
    port_in_use: &dyn Fn(u16) -> bool,
) -> bool {
    let cfg = worktree_kind_config(base, root, slot);
    [
        cfg.ingress_http_port,
        cfg.ingress_https_port,
        cfg.restate_ingress_port,
        cfg.restate_admin_port,
        cfg.rauthy_port,
        cfg.garage_s3_port,
        cfg.web_port,
        cfg.openobserve_port,
        cfg.openobserve_otlp_port,
        cfg.clamav_port,
    ]
    .into_iter()
    .any(port_in_use)
}

/// Choose this worktree's isolated KIND slot. A valid recorded slot is reused
/// so every host coordinate stays stable across restarts. A prior interrupted
/// setup renders its KIND config before it can write a descriptor, so that
/// path-derived slot must also be reused on retry.
///
/// `claimed` is authoritative for *reservation* — it is derived from the KIND
/// clusters that hold the ports, which covers stopped and orphaned clusters
/// alike. `port_in_use` is the secondary probe that catches a non-KIND process
/// squatting on a slot; it is injected so tests never read host state.
fn choose_worktree_slot(
    root: &Path,
    recorded: Option<u16>,
    claimed: &BTreeSet<u16>,
    base_cfg: &KindConfig,
    port_in_use: &dyn Fn(u16) -> bool,
) -> Result<u16> {
    let recorded = recorded.filter(|slot| *slot < WORKTREE_PORT_SPAN);
    if let Some(slot) = recorded {
        if !claimed.contains(&slot) {
            return Ok(slot);
        }
    }
    let start = recorded.unwrap_or_else(|| derived_worktree_slot(root));
    if !claimed.contains(&start) && root.join(".devx/kind-config.yaml").is_file() {
        return Ok(start);
    }
    for i in 0..WORKTREE_PORT_SPAN {
        let candidate = (start + i) % WORKTREE_PORT_SPAN;
        if !claimed.contains(&candidate)
            && !worktree_slot_has_listener(base_cfg, root, candidate, port_in_use)
        {
            return Ok(candidate);
        }
    }
    bail!(
        "all {WORKTREE_PORT_SPAN} isolated KIND slots are reserved or occupied — stop an unused \
         worktree environment (`worktree-env down`) or free a port, then re-run"
    )
}

// ---------- database operations (async via store/SeaORM) ----------

// ---------- worktree state files (.devx/) ----------

fn devx_dir(root: &Path) -> PathBuf {
    root.join(".devx")
}

fn descriptor_path(root: &Path) -> PathBuf {
    devx_dir(root).join("worktree.json")
}

fn env_path(root: &Path) -> PathBuf {
    devx_dir(root).join("env")
}

fn read_descriptor(root: &Path) -> Option<WorktreeEnv> {
    let body = std::fs::read_to_string(descriptor_path(root)).ok()?;
    serde_json::from_str(&body).ok()
}

fn write_descriptor(root: &Path, desc: &WorktreeEnv) -> Result<()> {
    std::fs::create_dir_all(devx_dir(root))
        .with_context(|| format!("create {}", devx_dir(root).display()))?;
    let body = serde_json::to_string_pretty(desc).context("serialize worktree descriptor")?;
    std::fs::write(descriptor_path(root), body)
        .with_context(|| format!("write {}", descriptor_path(root).display()))
}

fn write_worktree_env(root: &Path, body: &str) -> Result<()> {
    std::fs::create_dir_all(devx_dir(root))
        .with_context(|| format!("create {}", devx_dir(root).display()))?;
    std::fs::write(env_path(root), body)
        .with_context(|| format!("write {}", env_path(root).display()))
}

fn remove_worktree_state(root: &Path) {
    let _ = std::fs::remove_file(descriptor_path(root));
    let _ = std::fs::remove_file(env_path(root));
    let _ = std::fs::remove_file(root.join(".devx/kind-config.yaml"));
    let _ = std::fs::remove_file(root.join(".devx/kubeconfig"));
    let _ = std::fs::remove_file(root.join(".devx/port-forwards.log"));
}

// ---------- cross-worktree environment coordination ----------

/// Acquire a process-wide environment lock shared by every worktree in this Git
/// repository. The file lives in the common Git directory, not in a disposable
/// checkout, and the OS releases the lock when the returned handle is dropped.
fn acquire_worktree_env_lock(root: &Path) -> Result<File> {
    let lock_path = git_common_dir(root)?.join("navigator-worktree-env.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open worktree environment lock {}", lock_path.display()))?;
    file.lock()
        .with_context(|| format!("lock worktree environment state {}", lock_path.display()))?;
    Ok(file)
}

/// Serialize isolated KIND setup across every local clone, not merely the
/// worktrees that share one Git common directory. Slot selection and KIND's
/// host-port binding must be one transaction on a host.
fn acquire_host_worktree_env_lock() -> Result<File> {
    let lock_path = std::env::temp_dir().join("navigator-worktree-env.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "open host worktree environment lock {}",
                lock_path.display()
            )
        })?;
    file.lock().with_context(|| {
        format!(
            "lock host worktree environment state {}",
            lock_path.display()
        )
    })?;
    Ok(file)
}

// ---------- the host cluster registry ----------

/// Where the host records which checkout owns each worktree KIND cluster.
///
/// Host-scoped, like `acquire_host_worktree_env_lock`, because the KIND
/// cluster namespace is a property of the machine rather than of one clone.
/// `$HOME` is preferred over the lock's `temp_dir` precisely because it is not
/// swept: macOS reclaims `$TMPDIR` entries untouched for a few days, and a
/// registration must outlive an environment that sits idle over a weekend.
fn host_cluster_registry_path() -> PathBuf {
    match std::env::var_os("NAVIGATOR_WORKTREE_CLUSTER_REGISTRY") {
        Some(path) => PathBuf::from(path),
        None => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(".navigator/worktree-clusters.json"),
            None => std::env::temp_dir().join("navigator-worktree-clusters.json"),
        },
    }
}

/// The outcome of loading the host cluster registry.
///
/// A registry that does not exist is not the same as one that exists but
/// cannot be parsed, and a delete command must not treat them alike. `Absent`
/// is a host from before the registry, or one where no `up` has run yet: a
/// safe fall back to the git listing. `Unreadable` means the true ownership is
/// unknown — a partial write, a disk error, a hand-edit — so a corrupt file
/// that *should* name another clone's live checkout would otherwise hide it,
/// and `sweep --apply` would delete a running environment. Loading fails
/// closed on that case instead.
enum RegistryLoad {
    Absent,
    Loaded(BTreeMap<String, PathBuf>),
    Unreadable(String),
}

/// Load the `cluster -> owning checkout` map, keeping a genuinely absent file
/// distinct from one that cannot be trusted.
fn load_cluster_registry(path: &Path) -> RegistryLoad {
    match fs::read_to_string(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => RegistryLoad::Absent,
        Err(err) => RegistryLoad::Unreadable(err.to_string()),
        Ok(raw) => match serde_json::from_str::<BTreeMap<String, PathBuf>>(&raw) {
            Ok(map) => RegistryLoad::Loaded(map),
            Err(err) => RegistryLoad::Unreadable(err.to_string()),
        },
    }
}

fn write_cluster_registry_at(path: &Path, registry: &BTreeMap<String, PathBuf>) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create host cluster registry directory {}",
                parent.display()
            )
        })?;
    }
    let rendered = serde_json::to_string_pretty(registry)
        .context("serialize the host worktree cluster registry")?;
    fs::write(path, rendered)
        .with_context(|| format!("write host cluster registry {}", path.display()))
}

/// Record that `root` owns `cluster`, so a `sweep` run from any clone on this
/// host can tell a live environment from an abandoned one.
///
/// A corrupt registry is left untouched rather than overwritten: a fresh write
/// would erase whatever other clones' registrations the unparseable file may
/// still hold, and — worse — a registry that then parses cleanly with only
/// this entry would stop `sweep --apply` from failing closed, so those other
/// clusters would look unregistered and be deleted. Leaving it corrupt keeps
/// `sweep --apply` failing closed while `up` still proceeds.
fn register_worktree_cluster(path: &Path, cluster: &str, root: &Path) -> Result<()> {
    let mut registry = match load_cluster_registry(path) {
        RegistryLoad::Loaded(map) => map,
        RegistryLoad::Absent => BTreeMap::new(),
        RegistryLoad::Unreadable(err) => {
            eprintln!(
                "==> warning: host cluster registry {} is unreadable ({err}); not recording \
                 this cluster. `sweep --apply` will refuse until the file is fixed or removed.",
                path.display()
            );
            return Ok(());
        }
    };
    registry.insert(cluster.to_string(), root.to_path_buf());
    write_cluster_registry_at(path, &registry)
}

/// Drop `cluster`'s registration. Called on teardown, where the cluster is
/// going away and a stale entry would otherwise name a checkout that no longer
/// owns anything.
///
/// An absent or unreadable registry has nothing this can safely remove: a
/// corrupt file is left as-is so `sweep --apply` keeps refusing to run against
/// unknown ownership, and a stale entry it cannot clear simply names a gone
/// checkout, which the next sweep classifies as an orphan anyway.
fn unregister_worktree_cluster(path: &Path, cluster: &str) -> Result<()> {
    let mut registry = match load_cluster_registry(path) {
        RegistryLoad::Loaded(map) => map,
        RegistryLoad::Absent | RegistryLoad::Unreadable(_) => return Ok(()),
    };
    if registry.remove(cluster).is_none() {
        return Ok(());
    }
    write_cluster_registry_at(path, &registry)
}

fn git_common_dir(root: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .context("locate the shared Git directory for worktree coordination")?;
    if !output.status.success() {
        bail!(
            "git rev-parse --git-common-dir failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let path =
        String::from_utf8(output.stdout).context("the shared Git directory path was not UTF-8")?;
    Ok(PathBuf::from(path.trim()))
}

/// A KIND cluster and the host ports it binds. Docker keeps
/// `HostConfig.PortBindings` while a container is stopped or merely created,
/// so one entry describes a running, stopped, or interrupted cluster alike.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClusterPorts {
    cluster: String,
    host_ports: Vec<u16>,
}

/// The slot a bound host port belongs to, if any. Every worktree port base is
/// a multiple of `WORKTREE_PORT_SPAN` inside the worktree window, so the
/// remainder recovers the slot regardless of which service bound the port.
/// `every_worktree_host_port_maps_back_to_its_slot` pins that invariant.
fn worktree_slot_of_host_port(port: u16) -> Option<u16> {
    // The window ends after the LAST base, so adding a tier member means
    // moving this to its base — otherwise that member's ports stop
    // registering as claims and `sweep` under-reports what a cluster holds.
    const WINDOW_END: u16 = WORKTREE_SURREAL_PORT_BASE + WORKTREE_PORT_SPAN;
    (WORKTREE_PORT_WINDOW_START..WINDOW_END)
        .contains(&port)
        .then_some(port % WORKTREE_PORT_SPAN)
}

/// Slots held by KIND clusters other than this worktree's own. Cluster names
/// carry the worktree fingerprint, so a cluster is matched to its owner by
/// name and everything else — stopped, orphaned, or from a deleted worktree —
/// counts as a reservation.
fn cluster_claimed_slots(root: &Path, clusters: &[ClusterPorts]) -> BTreeSet<u16> {
    let own = format!("-{}", worktree_fingerprint(root));
    clusters
        .iter()
        .filter(|cluster| !cluster.cluster.ends_with(&own))
        .flat_map(|cluster| cluster.host_ports.iter().copied())
        .filter_map(worktree_slot_of_host_port)
        .collect()
}

/// Every slot another environment has reserved.
///
/// The KIND clusters that actually hold the ports are the source of truth: a
/// descriptor file is absent for any interrupted setup and for every cluster
/// whose worktree was deleted, so it cannot carry the uniqueness invariant.
/// Sibling descriptors are still unioned in as a *hint*, covering the window
/// where a concurrent `up` has claimed its slot but not yet built its cluster.
fn claimed_worktree_slots(
    root: &Path,
    list_clusters: &dyn Fn() -> Result<Vec<ClusterPorts>>,
) -> Result<BTreeSet<u16>> {
    let listing = git_worktree_listing(root)?;
    let mut claimed: BTreeSet<u16> = worktree_paths(&listing)
        .filter(|path| path != root)
        .filter_map(|path| read_descriptor(&path).and_then(|desc| desc.dev_slot()))
        .collect();
    claimed.extend(cluster_claimed_slots(root, &list_clusters()?));
    Ok(claimed)
}

/// One `docker inspect` line per container: the KIND cluster it belongs to,
/// then every host port it binds. `HostConfig.PortBindings` is the configured
/// binding rather than the runtime port table, which is what keeps a stopped
/// cluster's reservation visible — `docker ps` reports no ports for one.
const KIND_PORTS_FORMAT: &str = "{{index .Config.Labels \"io.x-k8s.kind.cluster\"}}\t\
     {{range $port, $bindings := .HostConfig.PortBindings}}\
     {{range $bindings}}{{.HostPort}} {{end}}{{end}}";

/// Enumerate the KIND clusters Docker knows about with the host ports they
/// bind. `docker ps -a` covers stopped and created containers, so an orphaned
/// or interrupted cluster still registers its claim.
fn list_kind_cluster_ports() -> Result<Vec<ClusterPorts>> {
    let listing = docker_output(
        &[
            "ps",
            "-a",
            "--filter",
            "label=io.x-k8s.kind.cluster",
            "--format",
            "{{.ID}}",
        ],
        "list KIND containers for worktree slot reservation",
    )?;
    // Inspected one at a time on purpose. `docker inspect` given several ids
    // aborts with EMPTY stdout as soon as one is missing, and a container can
    // always disappear between the listing and the inspection — that would
    // silently empty the claim set and reintroduce the collision this guards
    // against. Per container, a vanished one contributes nothing and the rest
    // still register their claims.
    let mut inspected = String::new();
    for id in listing.split_whitespace() {
        if let Some(line) = inspect_kind_container_ports(id)? {
            inspected.push_str(&line);
        }
    }
    Ok(parse_kind_cluster_ports(&inspected))
}

/// The host ports one KIND container binds, or `None` if it no longer exists.
///
/// Only a vanished container is absorbed. Every other failure propagates: a
/// daemon or permission error leaves the container's reservation *unknown*,
/// and a stopped cluster has no listener for the port probe to fall back on,
/// so silently reading it as "binds no ports" is what would hand its slot to
/// the next worktree and fail that KIND startup on a host-port conflict.
fn inspect_kind_container_ports(id: &str) -> Result<Option<String>> {
    const ACTION: &str = "inspect a KIND container's host port bindings";
    let output = Command::new("docker")
        .args(["inspect", "--format", KIND_PORTS_FORMAT, id])
        .output()
        .context(ACTION)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_missing_container(&stderr) {
            return Ok(None);
        }
        bail!("{ACTION} failed: {}", stderr.trim());
    }
    String::from_utf8(output.stdout)
        .map(Some)
        .with_context(|| format!("{ACTION} returned non-UTF-8 output"))
}

/// Whether `docker inspect` failed because the container is gone rather than
/// because Docker could not answer.
fn is_missing_container(stderr: &str) -> bool {
    let stderr = stderr.to_ascii_lowercase();
    stderr.contains("no such object") || stderr.contains("no such container")
}

fn docker_output(args: &[&str], action: &str) -> Result<String> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .with_context(|| action.to_owned())?;
    if !output.status.success() {
        bail!(
            "{action} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).with_context(|| format!("{action} returned non-UTF-8 output"))
}

/// Fold `docker inspect`'s per-container lines into one entry per cluster. A
/// cluster spans several containers (control plane plus workers) and only some
/// of them bind ports, so the ports of every container must be unioned under
/// the cluster name.
fn parse_kind_cluster_ports(inspected: &str) -> Vec<ClusterPorts> {
    let mut clusters: Vec<ClusterPorts> = Vec::new();
    for (cluster, ports) in inspected.lines().filter_map(|line| line.split_once('\t')) {
        let cluster = cluster.trim();
        if cluster.is_empty() {
            continue;
        }
        let ports = ports
            .split_whitespace()
            .filter_map(|port| port.parse().ok());
        match clusters.iter_mut().find(|c| c.cluster == cluster) {
            Some(existing) => existing.host_ports.extend(ports),
            None => clusters.push(ClusterPorts {
                cluster: cluster.to_string(),
                host_ports: ports.collect(),
            }),
        }
    }
    clusters
}

// ---------- sweep (reclaiming abandoned worktree environments) ----------

/// What a KIND cluster belongs to, judged against the checkouts that exist
/// right now.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClusterOwner {
    /// Carries no worktree fingerprint: the shared `dev up` tier, or a KIND
    /// cluster that is not Navigator's at all. Never a sweep candidate.
    Unowned,
    /// A checkout that is still on disk owns this cluster.
    Live(PathBuf),
    /// The name carries a worktree fingerprint no live checkout has.
    Orphaned,
}

/// One inspected KIND cluster and the footprint `sweep` reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SweptCluster {
    cluster: String,
    owner: ClusterOwner,
    /// Worktree slots the bound host ports resolve to. Empty when KIND
    /// published no port — a setup interrupted before its cluster came up.
    slots: BTreeSet<u16>,
    host_ports: Vec<u16>,
}

impl SweptCluster {
    fn is_orphaned(&self) -> bool {
        self.owner == ClusterOwner::Orphaned
    }
}

/// The worktree fingerprint a cluster name carries, or `None` when the name
/// is not one [`worktree_kind_config`] builds.
///
/// The structure is the guard that keeps the shared `dev up` cluster out of
/// every plan: it is the bare base name, so it cannot produce a fingerprint
/// and can never fall through to "orphaned".
fn worktree_cluster_fingerprint<'a>(base_cluster: &str, cluster: &'a str) -> Option<&'a str> {
    let fingerprint = cluster.strip_prefix(&format!("{}-", slugify(base_cluster)))?;
    let well_formed = fingerprint.len() == 8
        && fingerprint
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
    well_formed.then_some(fingerprint)
}

/// Classify every KIND cluster against the checkouts that exist right now.
///
/// Pure by construction: the caller supplies the cluster inventory, the live
/// worktree paths, the host registry, and the directory predicate, so a test
/// never reads or mutates ambient Docker or the host. A cluster becomes a
/// deletion candidate only when it carries a worktree fingerprint *and* no
/// live checkout owns it — the two conditions that together spare the shared
/// tier and every live worktree.
///
/// Ownership is decided from two sources, because neither alone is enough:
///
/// * The **host registry** (`up` writes it, `down` clears it) is authoritative
///   when it knows the cluster. It is host-scoped, so it answers for clusters
///   belonging to *other clones* on this machine as well.
/// * `live_worktrees` is `git worktree list` for one clone only. The cluster
///   inventory is per-host, so this listing cannot see another clone's live
///   checkouts, and trusting it alone would classify them `Orphaned`. It stays
///   the fallback for clusters created before the registry existed, which is
///   what keeps `sweep` able to reclaim the orphans that already leaked.
fn plan_sweep(
    base_cluster: &str,
    clusters: &[ClusterPorts],
    live_worktrees: &[PathBuf],
    registry: &BTreeMap<String, PathBuf>,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<SweptCluster> {
    let live: BTreeMap<String, PathBuf> = live_worktrees
        .iter()
        .map(|path| (worktree_fingerprint(path), path.clone()))
        .collect();
    clusters
        .iter()
        .map(|entry| SweptCluster {
            cluster: entry.cluster.clone(),
            owner: match worktree_cluster_fingerprint(base_cluster, &entry.cluster) {
                None => ClusterOwner::Unowned,
                Some(fingerprint) => match registry.get(&entry.cluster) {
                    // Registered: its recorded checkout decides, whichever
                    // clone owns it.
                    Some(path) if exists(path) => ClusterOwner::Live(path.clone()),
                    Some(_) => ClusterOwner::Orphaned,
                    // Unregistered: predates the registry, so fall back to
                    // this clone's listing.
                    None => match live.get(fingerprint) {
                        Some(path) => ClusterOwner::Live(path.clone()),
                        None => ClusterOwner::Orphaned,
                    },
                },
            },
            slots: entry
                .host_ports
                .iter()
                .copied()
                .filter_map(worktree_slot_of_host_port)
                .collect(),
            host_ports: entry.host_ports.clone(),
        })
        .collect()
}

/// The `sweep` listing. Pure so the exact report is unit-tested; the caller
/// prints it and, under `--apply`, then deletes.
fn sweep_report(plan: &[SweptCluster], apply: bool) -> String {
    let mut out = format!(
        "==> worktree-env sweep: {} KIND cluster(s) inspected\n",
        plan.len()
    );
    for entry in plan {
        let detail = match &entry.owner {
            ClusterOwner::Unowned => "not a worktree cluster".to_string(),
            ClusterOwner::Live(path) => path.display().to_string(),
            ClusterOwner::Orphaned => describe_ports(&entry.host_ports),
        };
        let _ = writeln!(
            out,
            "    {:<26} {:<9} {:<10} {detail}",
            entry.cluster,
            match entry.owner {
                ClusterOwner::Unowned => "shared",
                ClusterOwner::Live(_) => "live",
                ClusterOwner::Orphaned => "orphaned",
            },
            describe_slots(&entry.slots),
        );
    }

    let orphans: Vec<&SweptCluster> = plan.iter().filter(|e| e.is_orphaned()).collect();
    if orphans.is_empty() {
        out.push_str("\n    nothing to reclaim: no cluster outlived its worktree\n");
        return out;
    }
    let slots: BTreeSet<u16> = orphans
        .iter()
        .flat_map(|e| e.slots.iter().copied())
        .collect();
    let _ = writeln!(
        out,
        "\n    {} orphaned cluster(s) holding {}",
        orphans.len(),
        describe_slots(&slots)
    );
    out.push_str(if apply {
        "    Deleting them and the port-forwards that fed them.\n"
    } else {
        "    Dry run: nothing was touched. Re-run with `--apply` to delete them.\n"
    });
    out
}

fn describe_slots(slots: &BTreeSet<u16>) -> String {
    match slots.len() {
        0 => "no slot".to_string(),
        1 => format!("slot {}", slots.iter().next().expect("one slot")),
        _ => format!(
            "slots {}",
            slots
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn describe_ports(ports: &[u16]) -> String {
    if ports.is_empty() {
        return "binds no port (interrupted setup)".to_string();
    }
    let mut sorted: Vec<u16> = ports.to_vec();
    sorted.sort_unstable();
    format!(
        "ports {}",
        sorted
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// PIDs of host processes still forwarding into `cluster`.
///
/// `spawn_port_forward` selects its cluster through the worktree kubeconfig it
/// inherits in `KUBECONFIG`, so the command line reads only
/// `kubectl --namespace navigator port-forward svc/surreal 20076:8000`: the
/// cluster name is not on it. Matching the name alone therefore finds nothing
/// and leaves the forwards running after their cluster is deleted — the stale
/// listener that keeps answering on a reclaimed port.
///
/// A forward is matched instead by the **slot** its local port resolves to.
/// That is not "killing by port": the process must be a `kubectl port-forward`
/// *and* bind a port inside this cluster's slot. A slot cannot be shared,
/// because `up` refuses any slot an existing cluster already claims and the
/// orphan is still one of those clusters, so no live environment can be
/// holding it. An unrelated process that has re-bound the raw port is not a
/// `kubectl port-forward` and is never selected.
fn port_forward_pids_for(cluster: &str, slots: &BTreeSet<u16>, processes: &str) -> Vec<u32> {
    processes
        .lines()
        .filter(|line| {
            line.contains("port-forward")
                && (line.contains(cluster) || forwards_a_slot(line, slots))
        })
        .filter_map(|line| line.split_whitespace().next()?.parse().ok())
        .collect()
}

/// Whether a `kubectl port-forward` command line binds a local port belonging
/// to one of `slots`. Only the local half of a `<host>:<service>` mapping is
/// read; the in-cluster port carries no slot.
fn forwards_a_slot(line: &str, slots: &BTreeSet<u16>) -> bool {
    line.split_whitespace()
        .filter_map(|arg| arg.split_once(':'))
        .filter_map(|(host, _)| host.parse::<u16>().ok())
        .filter_map(worktree_slot_of_host_port)
        .any(|slot| slots.contains(&slot))
}

/// Checkouts that are still on disk.
///
/// Git keeps listing a worktree whose directory was deleted until someone
/// runs `git worktree prune`, and that stale entry names exactly the
/// abandoned environment `sweep` exists to reclaim. Presence on disk, not
/// Git's listing, therefore decides.
fn live_worktree_paths_in(listing: &str, exists: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    worktree_paths(listing)
        .filter(|path| exists(path))
        .collect()
}

fn live_worktree_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let listing = git_worktree_listing(root)?;
    let mut paths = live_worktree_paths_in(&listing, &|path| path.is_dir());
    // A checkout reached through a symlink hashes differently than its real
    // path. Both spellings count as live: mistaking a live worktree for an
    // orphan would delete a running environment.
    for path in paths.clone() {
        match path.canonicalize() {
            Ok(real) if real != path => paths.push(real),
            _ => {}
        }
    }
    Ok(paths)
}

fn host_processes() -> Result<String> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,command="])
        .output()
        .context("list host processes to stop abandoned port-forwards")?;
    String::from_utf8(output.stdout).context("host process listing was not UTF-8")
}

/// Delete the confirmed orphans.
///
/// Every step tolerates an already-gone resource: `kind delete cluster` exits
/// zero for a cluster that is not there, and a port-forward that already died
/// simply is not in the process listing. Docker volumes are never pruned.
fn apply_sweep(orphans: &[&SweptCluster], processes: &str) -> Result<()> {
    for orphan in orphans {
        for pid in port_forward_pids_for(&orphan.cluster, &orphan.slots, processes) {
            eprintln!("==> stopping port-forward {pid} ({})", orphan.cluster);
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
        eprintln!("==> deleting KIND cluster '{}'", orphan.cluster);
        super::run(
            Command::new("kind")
                .arg("delete")
                .arg("cluster")
                .arg("--name")
                .arg(&orphan.cluster),
        )?;
        // Leave no registration behind naming a cluster that is gone.
        unregister_worktree_cluster(&host_cluster_registry_path(), &orphan.cluster)?;
    }
    Ok(())
}

/// The ownership map `sweep` classifies with, or an error that stops a delete
/// run. A delete cannot proceed when host-wide ownership is unknown, because
/// the git listing alone cannot see another clone's live checkout; a dry run
/// changes nothing, so it warns and falls back instead of failing.
fn resolve_sweep_registry(
    load: RegistryLoad,
    apply: bool,
    registry_path: &Path,
) -> Result<BTreeMap<String, PathBuf>> {
    match load {
        RegistryLoad::Loaded(map) => Ok(map),
        RegistryLoad::Absent => Ok(BTreeMap::new()),
        RegistryLoad::Unreadable(err) => {
            if apply {
                bail!(
                    "host cluster registry {} could not be read ({err}); refusing to delete \
                     because a live environment owned by another clone may be hidden. Fix or \
                     remove the file, then re-run.",
                    registry_path.display()
                );
            }
            eprintln!(
                "==> warning: host cluster registry {} is unreadable ({err}); this dry run falls \
                 back to the current clone's git listing and may mislabel another clone's \
                 environment. `--apply` will refuse until the file is fixed or removed.",
                registry_path.display()
            );
            Ok(BTreeMap::new())
        }
    }
}

fn sweep(root: &Path, apply: bool, base_cfg: &KindConfig) -> Result<()> {
    // Only deletion needs the host lock. It keeps a concurrent `up` from
    // building a cluster while this decides what is abandoned; a dry run
    // changes nothing, so it must not block behind a long setup.
    let _host_lock = apply.then(acquire_host_worktree_env_lock).transpose()?;
    let registry_path = host_cluster_registry_path();
    let registry =
        resolve_sweep_registry(load_cluster_registry(&registry_path), apply, &registry_path)?;
    let plan = plan_sweep(
        &base_cfg.cluster,
        &list_kind_cluster_ports()?,
        &live_worktree_paths(root)?,
        &registry,
        &|path| path.is_dir(),
    );
    print!("{}", sweep_report(&plan, apply));
    if !apply {
        return Ok(());
    }
    let orphans: Vec<&SweptCluster> = plan.iter().filter(|e| e.is_orphaned()).collect();
    if orphans.is_empty() {
        return Ok(());
    }
    apply_sweep(&orphans, &host_processes()?)?;
    eprintln!("==> worktree-env sweep complete");
    Ok(())
}

fn git_worktree_listing(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("list Git worktrees for web-port reservation")?;
    if !output.status.success() {
        bail!(
            "git worktree list --porcelain failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("Git worktree list was not UTF-8")
}

fn worktree_paths(listing: &str) -> impl Iterator<Item = PathBuf> + '_ {
    listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
}

// ---------- small helpers ----------

/// Walk up from `start` (or the current dir) to the workspace root — the
/// first ancestor holding both `Cargo.toml` and `k8s/`. In a git
/// worktree this is the worktree's own root.
fn worktree_root(start: Option<&Path>) -> Result<PathBuf> {
    let mut dir = match start {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("resolve worktree path {}", p.display()))?,
        None => std::env::current_dir().context("get current directory")?,
    };
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("k8s").is_dir() {
            return Ok(dir);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => bail!(
                "could not find the workspace root (Cargo.toml + k8s/) from the worktree path"
            ),
        }
    }
}

/// Whether something is already listening on `127.0.0.1:<port>`.
fn port_listening(port: u16) -> bool {
    format!("127.0.0.1:{port}")
        .parse()
        .ok()
        .and_then(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok())
        .is_some()
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn print_dev_summary(slug: &str, db_name: &str, runtime: Runtime, cfg: &KindConfig) {
    eprintln!("{}", dev_summary(slug, db_name, runtime, cfg));
}

/// The `worktree-env up` summary, built as a string so the wiring it
/// advertises is unit-testable. `print_dev_summary` is the thin print
/// wrapper over it, mirroring `db_agreement_lines` / `print_db_agreement`.
///
/// The tier line differs by lane because the thing an operator would
/// look at differs: a cluster name is something to `kubectl` at, and the
/// native lane has none.
fn dev_summary(slug: &str, db_name: &str, runtime: Runtime, cfg: &KindConfig) -> String {
    let rule = "===========================================================";
    let tier = match runtime {
        Runtime::Kind => format!("  KIND cluster : {}\n", cfg.cluster),
        Runtime::Native => "  tier         : native host processes\n".to_string(),
    };
    format!(
        "\n{rule}\n navigator dev worktree-env up — dev environment for `{slug}`\n{rule}\n\n\
         {tier}  database     : {db_name}\n  Restate port : {}\n  Surreal port : {}\n  web port     : {}\n\
         \x20 delete-your-data web port : {}\n\n\
         Start this worktree's web server:\n\n{}\n\n\
         Tear down this worktree's tier:\n    \
         navigator dev worktree-env down\n",
        cfg.restate_ingress_port,
        cfg.surreal_port,
        cfg.web_port,
        cfg.delete_your_data_web_port,
        web_start_instructions(cfg.web_port, cfg.delete_your_data_web_port)
    )
}

/// Both ports one `cargo run -p neon` binds locally: the default brand on
/// `web_port`, and `delete-your-data` on its own `delete_your_data_web_port`
/// — see ENG-437. `web` needs no `Host:` trickery to serve either; the port
/// alone selects the brand.
fn web_start_instructions(web_port: u16, delete_your_data_web_port: u16) -> String {
    format!(
        "    set -a; source .devx/env; set +a\n    cargo run -p neon   # neon on :{web_port}, \
         delete-your-data on :{delete_your_data_web_port}"
    )
}

impl WorktreeEnv {
    /// The recorded isolated KIND slot, but only for a dev-mode descriptor.
    fn dev_slot(&self) -> Option<u16> {
        (self.mode == "dev").then_some(self.slot).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_start_instructions_use_the_generated_local_environment() {
        // `.devx/env` carries NAVIGATOR_CI_HARNESS=1 + staging + a
        // SESSION_SECRET, which is the whole reason `web` boots from it
        // alone: the integration-tier requirements (SENDGRID_EVENTS_*,
        // DOCUSIGN_HMAC_KEY) are filtered out of
        // `enforce_deployment_invariants` under the harness. Sourcing the
        // generated env is therefore sufficient, and naming a secrets
        // provider here would advertise a dependency the binary doesn't
        // have.
        let instructions = web_start_instructions(3042, 3043);
        assert!(instructions.contains("source .devx/env"));
        assert!(instructions.contains("cargo run -p neon"));
        assert!(instructions.contains(":3042"), "must name the chosen port");
        assert!(
            instructions.contains(":3043"),
            "must name the delete-your-data port too"
        );
        assert!(
            !instructions.to_lowercase().contains("doppler"),
            "local web start is self-contained and names no secrets provider: {instructions}"
        );
    }

    #[test]
    fn dev_summary_reports_the_isolated_coordinates_and_start_instructions() {
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/pr-546"),
            51,
        );
        let summary = dev_summary("pr-546", "navigator", Runtime::Kind, &cfg);
        assert!(summary.contains("pr-546"), "names the worktree slug");
        assert!(
            summary.contains("database     : navigator"),
            "names the environment-owned database"
        );
        assert!(summary.contains(&format!("KIND cluster : {}", cfg.cluster)));
        assert!(summary.contains(&format!("Restate port : {}", cfg.restate_ingress_port)));
        assert!(summary.contains(&format!("web port     : {}", cfg.web_port)));
        assert!(summary.contains(&format!(
            "delete-your-data web port : {}",
            cfg.delete_your_data_web_port
        )));
        // The summary must carry the real start instructions, not a
        // paraphrase that could drift from `web_start_instructions`.
        assert!(summary.contains(&web_start_instructions(
            cfg.web_port,
            cfg.delete_your_data_web_port
        )));
        assert!(summary.contains("navigator dev worktree-env down"));
    }

    #[test]
    fn dev_summary_advertises_isolated_teardown() {
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/pr-546"),
            51,
        );
        let summary = dev_summary("pr-546", "navigator", Runtime::Kind, &cfg);
        assert!(
            summary.contains("Tear down this worktree's tier"),
            "teardown line must match what `down` really does: {summary}"
        );
        assert!(
            !summary.contains("shared deps"),
            "summary must not claim a shared topology: {summary}"
        );
    }

    /// The summary is where an operator learns what was built. Naming a
    /// KIND cluster on the native lane sends them to `kubectl` for a
    /// cluster that does not exist.
    #[test]
    fn the_native_summary_names_host_processes_rather_than_a_cluster() {
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/pr-546"),
            51,
        );

        let summary = dev_summary("pr-546", "navigator", Runtime::Native, &cfg);

        assert!(summary.contains("native host processes"), "{summary}");
        assert!(!summary.contains("KIND cluster"), "{summary}");
        assert!(!summary.contains(&cfg.cluster), "{summary}");
        // Everything downstream still reads the same env file, so the
        // start instructions must not differ between the lanes.
        assert!(summary.contains(&web_start_instructions(
            cfg.web_port,
            cfg.delete_your_data_web_port
        )));
    }

    #[test]
    fn print_dev_summary_emits_the_summary() {
        // Exercises the print wrapper end to end; it must not panic.
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/pr-546"),
            51,
        );
        print_dev_summary("pr-546", "navigator", Runtime::Kind, &cfg);
    }

    /// The gate covers eleven ports; the native lane supervises four.
    /// The remaining seven must be *declared* rather than dropped —
    /// otherwise a lane that serves a third of the tier reports the same
    /// "ready" as one that serves all of it. This test is what makes
    /// adding a dependency force a decision: a new gate member with no
    /// entry in either native list fails here.
    #[test]
    fn every_gated_port_is_either_supervised_natively_or_attributed_to_an_issue() {
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/pr-546"),
            51,
        );

        let gated: BTreeSet<&str> = gate_members(&cfg)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let accounted: BTreeSet<&str> = super::super::native::SUPERVISED
            .iter()
            .copied()
            .chain(super::super::native::DEFERRED.iter().map(|(name, _)| *name))
            .collect();

        assert_eq!(
            gated, accounted,
            "every readiness-gate member must be supervised on the native lane or carry the \
             issue that will supervise it"
        );
    }

    /// The names are matched as strings across two modules, so a typo
    /// would silently move a member into "deferred" — the gate would
    /// stop waiting on a port the lane actually serves.
    #[test]
    fn the_natively_supervised_members_are_the_ones_the_gate_waits_on() {
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/pr-546"),
            51,
        );

        let supervised: BTreeSet<u16> = gate_members(&cfg)
            .into_iter()
            .filter(|(name, _)| super::super::native::SUPERVISED.contains(name))
            .map(|(_, port)| port)
            .collect();

        assert_eq!(
            supervised,
            BTreeSet::from([cfg.rauthy_port, cfg.garage_s3_port, cfg.surreal_port])
        );
    }
    #[test]
    fn slugify_sanitizes_branch_names() {
        assert_eq!(slugify("codex/blog-rust-ferris"), "codex-blog-rust-ferris");
        assert_eq!(slugify("Feature/ABC_123"), "feature-abc-123");
        assert_eq!(slugify("---weird///name---"), "weird-name");
        assert_eq!(slugify("main"), "main");
        // Truncation never leaves a trailing dash.
        let long = "a".repeat(50) + "/" + &"b".repeat(50);
        let s = slugify(&long);
        assert!(s.len() <= MAX_SLUG_LEN);
        assert!(!s.ends_with('-'));
    }

    #[test]
    fn worktree_kind_config_isolated_by_path_and_slot() {
        let base = KindConfig::from_env();
        let first = worktree_kind_config(&base, Path::new("/tmp/navigator/worktrees/first"), 7);
        let second = worktree_kind_config(&base, Path::new("/tmp/navigator/worktrees/second"), 8);

        assert_ne!(first.cluster, second.cluster);
        assert_eq!(first.ingress_http_port, WORKTREE_INGRESS_HTTP_PORT_BASE + 7);
        assert_eq!(
            first.ingress_https_port,
            WORKTREE_INGRESS_HTTPS_PORT_BASE + 7
        );
        assert_eq!(
            first.restate_ingress_port,
            WORKTREE_RESTATE_INGRESS_PORT_BASE + 7
        );
        assert_eq!(
            first.restate_admin_port,
            WORKTREE_RESTATE_ADMIN_PORT_BASE + 7
        );
        assert_eq!(first.web_port, WORKTREE_WEB_PORT_BASE + 7);
        assert_eq!(
            first.delete_your_data_web_port,
            WORKTREE_DELETE_YOUR_DATA_WEB_PORT_BASE + 7
        );
        assert_eq!(first.clamav_port, WORKTREE_CLAMAV_PORT_BASE + 7);
        assert_ne!(first.restate_ingress_port, second.restate_ingress_port);
        assert_ne!(first.clamav_port, second.clamav_port);

        // ENG-437: `web` and `delete-your-data` must never share a bind
        // port — every brand a worktree can reach locally needs its own.
        assert_ne!(first.web_port, first.delete_your_data_web_port);

        let host_ports = [
            first.ingress_http_port,
            first.ingress_https_port,
            first.restate_ingress_port,
            first.restate_admin_port,
            first.clamav_port,
            first.rauthy_port,
            first.garage_s3_port,
            first.web_port,
            first.delete_your_data_web_port,
            first.openobserve_port,
            first.openobserve_otlp_port,
        ];
        assert_eq!(
            host_ports.into_iter().collect::<BTreeSet<_>>().len(),
            host_ports.len(),
            "every worktree service must receive its own host port"
        );
    }

    /// ENG-437: `worktree-env up` writes both `web` bind ports into
    /// `.devx/env`, and they are never the same port — a developer sourcing
    /// this file gets two distinct addresses to reach the two brands, not
    /// one variable silently shadowing the other.
    #[test]
    fn rendered_worktree_env_carries_two_distinct_web_ports() {
        let cfg = worktree_kind_config(
            &KindConfig::from_env(),
            Path::new("/tmp/navigator/worktrees/brand-ports"),
            22,
        );
        let env = super::super::render_env_for(&cfg, "navigator", cfg.web_port, Path::new("/ws"));
        assert!(env.contains(&format!("PORT={}", cfg.web_port)));
        assert!(env.contains(&format!(
            "NAVIGATOR_LOCAL_DELETE_YOUR_DATA_PORT={}",
            cfg.delete_your_data_web_port
        )));
        assert_ne!(cfg.web_port, cfg.delete_your_data_web_port);
    }

    #[test]
    fn worktree_cluster_derivation_is_idempotent_after_dotenv_reload() {
        let root = Path::new("/tmp/navigator/worktrees/reused");
        let first = worktree_cluster_name("navigator", root);
        assert_eq!(worktree_cluster_name(&first, root), first);
    }

    /// Two paths that collide under `fnv1a % WORKTREE_PORT_SPAN`. The
    /// collision is a property of the constants, so `derived_worktree_slot`
    /// asserts it rather than the test trusting these literals.
    const COLLIDING_A: &str = "/tmp/navigator/worktrees/w2";
    const COLLIDING_B: &str = "/tmp/navigator/worktrees/w20";

    /// Slot probe for tests: no host port is ever in use, so selection is
    /// decided purely by the injected claim set and never by ambient state.
    fn no_listeners(_port: u16) -> bool {
        false
    }

    /// A `list_clusters` fixture. Host ports are what a KIND cluster actually
    /// binds, so a fixture entry stands in for a live *or* stopped cluster —
    /// Docker keeps `HostConfig.PortBindings` across a stop.
    fn cluster_fixture(entries: &[(&str, &[u16])]) -> Vec<ClusterPorts> {
        entries
            .iter()
            .map(|(cluster, ports)| ClusterPorts {
                cluster: (*cluster).to_string(),
                host_ports: ports.to_vec(),
            })
            .collect()
    }

    #[test]
    fn worktree_slot_reuses_a_valid_recording_and_skips_claims() {
        let base = KindConfig::from_env();
        let root = Path::new("/tmp/navigator/worktrees/second");
        assert_eq!(
            choose_worktree_slot(root, Some(7), &BTreeSet::new(), &base, &no_listeners).unwrap(),
            7
        );
        assert_eq!(
            choose_worktree_slot(root, Some(7), &BTreeSet::from([7]), &base, &no_listeners)
                .unwrap(),
            8
        );
    }

    #[test]
    fn worktree_slot_reuses_an_interrupted_setup_slot() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join(".devx")).unwrap();
        std::fs::write(root.join(".devx/kind-config.yaml"), "kind: Cluster\n").unwrap();

        let slot = derived_worktree_slot(root);
        assert_eq!(
            choose_worktree_slot(
                root,
                None,
                &BTreeSet::new(),
                &KindConfig::from_env(),
                &no_listeners
            )
            .unwrap(),
            slot
        );
    }

    #[test]
    fn worktree_slot_selection_fails_when_every_slot_is_claimed() {
        let base = KindConfig::from_env();
        let claimed = (0..WORKTREE_PORT_SPAN).collect();
        let error = choose_worktree_slot(
            Path::new("/tmp/navigator/worktrees/full"),
            None,
            &claimed,
            &base,
            &no_listeners,
        )
        .unwrap_err();

        assert!(error.to_string().contains("reserved or occupied"));
    }

    #[test]
    fn every_worktree_host_port_maps_back_to_its_slot() {
        // The claim set reads slots off bound host ports, so every base must
        // sit on a `WORKTREE_PORT_SPAN` boundary for the mapping to hold.
        let cfg = worktree_kind_config(&KindConfig::from_env(), Path::new("/tmp/nav/w"), 37);
        for port in [
            cfg.ingress_http_port,
            cfg.ingress_https_port,
            cfg.restate_ingress_port,
            cfg.restate_admin_port,
            cfg.rauthy_port,
            cfg.garage_s3_port,
            cfg.web_port,
            cfg.delete_your_data_web_port,
            cfg.openobserve_port,
            cfg.openobserve_otlp_port,
            cfg.clamav_port,
            cfg.surreal_port,
        ] {
            assert_eq!(worktree_slot_of_host_port(port), Some(37));
        }

        // Ports outside the worktree window are not worktree claims: the
        // shared `dev up` tier and the testcontainer range must not register.
        for port in [3001, 15_432, 9080, 30_080, 19_999, 21_300, 32_770] {
            assert_eq!(worktree_slot_of_host_port(port), None);
        }
    }

    #[test]
    fn kind_inspect_output_folds_into_one_entry_per_cluster() {
        // Shape captured from a live host: a control plane binds the slot's
        // ports plus a random API-server port, and its workers bind nothing.
        let inspected = "navigator-02bdf258\t20976 21076 64977 20476 \n\
                         navigator-02bdf258\t\n\
                         navigator-ed964969\t20985 21085 51102 20485 \n";
        let clusters = parse_kind_cluster_ports(inspected);

        assert_eq!(
            clusters,
            vec![
                ClusterPorts {
                    cluster: "navigator-02bdf258".into(),
                    host_ports: vec![20_976, 21_076, 64_977, 20_476],
                },
                ClusterPorts {
                    cluster: "navigator-ed964969".into(),
                    host_ports: vec![20_985, 21_085, 51_102, 20_485],
                },
            ]
        );

        // The random API-server port sits outside the worktree window, so only
        // the real slots register as claims.
        let root = Path::new(COLLIDING_A);
        assert_eq!(
            cluster_claimed_slots(root, &clusters),
            BTreeSet::from([76, 85])
        );
    }

    /// A worktree cluster for `path`, binding `ports`. Mirrors what
    /// `list_kind_cluster_ports` reports for a real one.
    fn worktree_cluster(path: &Path, ports: &[u16]) -> ClusterPorts {
        ClusterPorts {
            cluster: format!("navigator-{}", worktree_fingerprint(path)),
            host_ports: ports.to_vec(),
        }
    }

    /// `plan_sweep` against a host whose registry knows nothing — every
    /// cluster falls back to this clone's git listing. That is the
    /// pre-registry behaviour, and these cases pin that it still holds so
    /// `sweep` keeps reclaiming orphans that leaked before the registry.
    fn plan_sweep_unregistered(
        base: &str,
        clusters: &[ClusterPorts],
        live: &[PathBuf],
    ) -> Vec<SweptCluster> {
        plan_sweep(base, clusters, live, &BTreeMap::new(), &|_| true)
    }

    #[test]
    fn sweep_selects_only_the_cluster_whose_worktree_is_gone() {
        let live = PathBuf::from("/tmp/navigator/worktrees/live");
        let gone = PathBuf::from("/tmp/navigator/worktrees/abandoned");
        let clusters = [
            worktree_cluster(&live, &[20_076, 20_976, 64_977]),
            worktree_cluster(&gone, &[20_085, 20_985]),
        ];

        let plan = plan_sweep_unregistered("navigator", &clusters, std::slice::from_ref(&live));

        assert_eq!(plan[0].owner, ClusterOwner::Live(live));
        assert!(!plan[0].is_orphaned(), "a live checkout keeps its cluster");
        assert!(plan[1].is_orphaned());
        // The random API-server port is outside the worktree window, so only
        // the real slots are reported as reclaimed.
        assert_eq!(plan[0].slots, BTreeSet::from([76]));
        assert_eq!(plan[1].slots, BTreeSet::from([85]));
    }

    #[test]
    fn sweep_never_selects_the_shared_dev_up_cluster() {
        // `dev up`'s cluster is the bare base name, so it carries no
        // fingerprint to match a checkout against. Classifying it as
        // "unowned" rather than letting it fall through to "orphaned" is what
        // keeps the shared tier — and every foreign KIND cluster — safe.
        let clusters = [
            ClusterPorts {
                cluster: "navigator".into(),
                host_ports: vec![15_432, 30_080],
            },
            ClusterPorts {
                cluster: "kind".into(),
                host_ports: vec![],
            },
            ClusterPorts {
                cluster: "navigator-nothexxx".into(),
                host_ports: vec![],
            },
        ];

        // No live worktrees at all: the case most likely to over-select.
        let plan = plan_sweep_unregistered("navigator", &clusters, &[]);

        assert!(plan.iter().all(|e| e.owner == ClusterOwner::Unowned));
        assert!(!plan.iter().any(SweptCluster::is_orphaned));
        assert!(sweep_report(&plan, false).contains("nothing to reclaim"));
    }

    #[test]
    fn a_stopped_cluster_is_judged_by_ownership_not_by_liveness() {
        // `list_kind_cluster_ports` reads `docker ps -a`, so a stopped
        // cluster still reports its bindings. Being stopped is not evidence
        // of abandonment: a live checkout's cluster is stopped after a
        // reboot, and deleting it would destroy a working environment.
        let live = PathBuf::from("/tmp/navigator/worktrees/stopped-but-live");
        let stopped_orphan = PathBuf::from("/tmp/navigator/worktrees/stopped-and-gone");
        let clusters = [
            worktree_cluster(&live, &[20_042]),
            worktree_cluster(&stopped_orphan, &[20_043]),
        ];

        let plan = plan_sweep_unregistered("navigator", &clusters, std::slice::from_ref(&live));

        assert_eq!(plan[0].owner, ClusterOwner::Live(live));
        assert!(plan[1].is_orphaned());
    }

    #[test]
    fn an_interrupted_setup_is_swept_even_though_it_binds_no_port() {
        // KIND created the container but published no port before setup died.
        // There is no slot to read off it, and it is still garbage holding
        // Docker memory.
        let gone = PathBuf::from("/tmp/navigator/worktrees/interrupted");
        let plan = plan_sweep_unregistered("navigator", &[worktree_cluster(&gone, &[])], &[]);

        assert!(plan[0].is_orphaned());
        assert!(plan[0].slots.is_empty());

        let report = sweep_report(&plan, false);
        assert!(report.contains("no slot"), "{report}");
        assert!(report.contains("interrupted setup"), "{report}");
    }

    #[test]
    fn a_live_environment_from_another_clone_is_never_swept() {
        // The failure this guards: the cluster inventory is host-wide, but
        // `git worktree list` only sees one clone. A second clone's live
        // checkout is therefore absent from `live_worktrees`, and without the
        // registry it classifies as `Orphaned` — so `--apply` run from this
        // clone would delete a running environment belonging to the other.
        let mine = PathBuf::from("/tmp/navigator/worktrees/mine");
        let other_clone = PathBuf::from("/tmp/other-navigator/worktrees/theirs");
        let clusters = [
            worktree_cluster(&mine, &[20_076]),
            worktree_cluster(&other_clone, &[20_085]),
        ];
        let registry = BTreeMap::from([(clusters[1].cluster.clone(), other_clone.clone())]);

        let plan = plan_sweep(
            "navigator",
            &clusters,
            std::slice::from_ref(&mine),
            &registry,
            &|path| path == other_clone,
        );

        assert_eq!(plan[0].owner, ClusterOwner::Live(mine));
        assert_eq!(
            plan[1].owner,
            ClusterOwner::Live(other_clone),
            "another clone's live checkout owns its cluster"
        );
        assert!(!plan.iter().any(SweptCluster::is_orphaned));
    }

    #[test]
    fn a_registered_cluster_is_swept_once_its_recorded_checkout_is_gone() {
        // The registry is not a blanket amnesty: a registration whose path no
        // longer exists is exactly the abandoned environment `sweep` reclaims,
        // and it must be caught even though this clone never listed it.
        let gone = PathBuf::from("/tmp/other-navigator/worktrees/deleted");
        let clusters = [worktree_cluster(&gone, &[20_085])];
        let registry = BTreeMap::from([(clusters[0].cluster.clone(), gone)]);

        let plan = plan_sweep("navigator", &clusters, &[], &registry, &|_| false);

        assert!(plan[0].is_orphaned());
        assert_eq!(plan[0].slots, BTreeSet::from([85]));
    }

    #[test]
    fn the_registry_never_promotes_the_shared_dev_up_cluster() {
        // Belt and braces: even a registry entry naming the bare base cluster
        // cannot make it a candidate, because it carries no fingerprint and is
        // classified `Unowned` before ownership is ever consulted.
        let clusters = [ClusterPorts {
            cluster: "navigator".into(),
            host_ports: vec![15_432],
        }];
        let registry = BTreeMap::from([("navigator".to_string(), PathBuf::from("/tmp/gone"))]);

        let plan = plan_sweep("navigator", &clusters, &[], &registry, &|_| false);

        assert_eq!(plan[0].owner, ClusterOwner::Unowned);
        assert!(!plan[0].is_orphaned());
    }

    /// The `Loaded` map, or an empty one for `Absent`/`Unreadable`. Only used
    /// where the distinction is not what is under test.
    fn loaded_or_empty(path: &Path) -> BTreeMap<String, PathBuf> {
        match load_cluster_registry(path) {
            RegistryLoad::Loaded(map) => map,
            RegistryLoad::Absent | RegistryLoad::Unreadable(_) => BTreeMap::new(),
        }
    }

    #[test]
    fn the_registry_round_trips_and_unregistration_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/worktree-clusters.json");
        let root = PathBuf::from("/tmp/navigator/worktrees/topic");

        // Written through a directory that does not exist yet.
        register_worktree_cluster(&path, "navigator-abc123", &root).expect("register");
        assert_eq!(
            loaded_or_empty(&path),
            BTreeMap::from([("navigator-abc123".to_string(), root)])
        );

        unregister_worktree_cluster(&path, "navigator-abc123").expect("unregister");
        assert!(loaded_or_empty(&path).is_empty());
        // Removing an absent entry is a no-op, not an error: `down` runs after
        // an interrupted setup that never registered.
        unregister_worktree_cluster(&path, "navigator-abc123").expect("idempotent unregister");
    }

    #[test]
    fn a_delete_run_refuses_an_unreadable_registry_but_a_dry_run_falls_back() {
        let path = Path::new("/tmp/does-not-matter.json");
        let unreadable = || RegistryLoad::Unreadable("partial write".into());

        // --apply must fail closed: deleting on an unknown ownership picture is
        // exactly how another clone's live cluster gets swept.
        assert!(resolve_sweep_registry(unreadable(), true, path).is_err());

        // A dry run deletes nothing, so it degrades to an empty map and warns.
        assert_eq!(
            resolve_sweep_registry(unreadable(), false, path).expect("dry run tolerates it"),
            BTreeMap::new()
        );

        // Absent and Loaded pass straight through under either flag.
        assert_eq!(
            resolve_sweep_registry(RegistryLoad::Absent, true, path).expect("absent is fine"),
            BTreeMap::new()
        );
        let loaded = BTreeMap::from([("navigator-x".to_string(), PathBuf::from("/w"))]);
        assert_eq!(
            resolve_sweep_registry(RegistryLoad::Loaded(loaded.clone()), true, path)
                .expect("loaded"),
            loaded
        );
    }

    #[test]
    fn load_distinguishes_absent_from_unreadable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let absent = dir.path().join("absent.json");
        assert!(matches!(
            load_cluster_registry(&absent),
            RegistryLoad::Absent
        ));

        let corrupt = dir.path().join("corrupt.json");
        fs::write(&corrupt, "{ not json").expect("write corrupt");
        assert!(matches!(
            load_cluster_registry(&corrupt),
            RegistryLoad::Unreadable(_)
        ));
    }

    #[test]
    fn a_corrupt_registry_is_never_overwritten_by_up_or_down() {
        // The dangerous rewrite: a corrupt file may still hold another clone's
        // registration, and replacing it with a clean one-entry file would
        // stop `sweep --apply` failing closed, so that clone's cluster would
        // look unregistered and be deleted. up and down must leave it corrupt.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("worktree-clusters.json");
        let corrupt = "{ not json";
        fs::write(&path, corrupt).expect("write corrupt");

        register_worktree_cluster(&path, "navigator-abc123", &PathBuf::from("/tmp/x"))
            .expect("register tolerates a corrupt file");
        unregister_worktree_cluster(&path, "navigator-abc123")
            .expect("unregister tolerates a corrupt file");

        assert_eq!(
            fs::read_to_string(&path).expect("still there"),
            corrupt,
            "the corrupt file must be left untouched so the delete path stays fail-closed"
        );
    }

    #[test]
    fn an_empty_inventory_sweeps_nothing() {
        let plan = plan_sweep_unregistered("navigator", &[], &[]);

        assert!(plan.is_empty());
        let report = sweep_report(&plan, false);
        assert!(report.contains("0 KIND cluster"), "{report}");
        assert!(report.contains("nothing to reclaim"), "{report}");
    }

    #[test]
    fn the_dry_run_report_names_the_footprint_and_the_flag_that_deletes() {
        let gone = PathBuf::from("/tmp/navigator/worktrees/abandoned");
        let plan = plan_sweep_unregistered(
            "navigator",
            &[worktree_cluster(&gone, &[20_985, 20_085])],
            &[],
        );

        let dry_run = sweep_report(&plan, false);
        assert!(dry_run.contains("orphaned"), "{dry_run}");
        assert!(dry_run.contains("slot 85"), "{dry_run}");
        assert!(dry_run.contains("ports 20085 20985"), "{dry_run}");
        assert!(dry_run.contains("Dry run"), "{dry_run}");
        assert!(dry_run.contains("--apply"), "{dry_run}");

        // Under `--apply` the report must stop advertising itself as a
        // no-op, or the operator cannot tell which run deleted anything.
        let applied = sweep_report(&plan, true);
        assert!(!applied.contains("Dry run"), "{applied}");
        assert!(applied.contains("Deleting"), "{applied}");
    }

    #[test]
    fn a_worktree_git_still_lists_but_disk_has_lost_is_not_live() {
        // This is the whole abandonment case: `git worktree list` keeps
        // reporting a deleted checkout until someone prunes, so trusting the
        // listing alone would classify every orphan as live and sweep
        // nothing.
        let listing = "worktree /tmp/navigator/worktrees/live\nHEAD abc123\n\n\
                       worktree /tmp/navigator/worktrees/deleted\nprunable gitdir file points \
                       to non-existent location\n\n";

        let live = live_worktree_paths_in(listing, &|path| path.ends_with("live"));

        assert_eq!(live, vec![PathBuf::from("/tmp/navigator/worktrees/live")]);
    }

    /// The command line `spawn_port_forward` really produces: the cluster is
    /// selected through the inherited `KUBECONFIG`, so only `--namespace` and
    /// the port mapping appear. Slot 76 forwards on 20076, slot 85 on
    /// 20085.
    const REAL_PORT_FORWARDS: &str = "\
  501 kubectl --namespace navigator port-forward svc/surreal 20076:8000
  502 kubectl --namespace navigator port-forward svc/surreal 20085:8000
  503 some-unrelated-server --listen 20076
";

    #[test]
    fn port_forwards_are_matched_when_the_cluster_name_is_absent() {
        // Regression: the real command line carries no cluster name, so a
        // name-only matcher found nothing and `--apply` deleted the cluster
        // while its forwards kept holding the slot's ports.
        assert_eq!(
            port_forward_pids_for(
                "navigator-02bdf258",
                &BTreeSet::from([76]),
                REAL_PORT_FORWARDS
            ),
            vec![501]
        );
        assert_eq!(
            port_forward_pids_for(
                "navigator-ed964969",
                &BTreeSet::from([85]),
                REAL_PORT_FORWARDS
            ),
            vec![502]
        );
    }

    #[test]
    fn port_forwards_of_other_slots_and_plain_port_binders_are_never_selected() {
        // An unrelated process that has since re-bound the orphan's port is
        // not a `kubectl port-forward` and is never selected — matching on the
        // raw port would kill it.
        assert!(port_forward_pids_for(
            "navigator-c45a5c02",
            &BTreeSet::from([30]),
            REAL_PORT_FORWARDS
        )
        .is_empty());
        // A cluster whose setup was interrupted binds no port and so holds no
        // slot; it must not sweep up every other environment's forwards.
        assert!(
            port_forward_pids_for("navigator-c45a5c02", &BTreeSet::new(), REAL_PORT_FORWARDS)
                .is_empty()
        );
    }

    #[test]
    fn the_shared_dev_up_port_forwards_hold_no_worktree_slot() {
        // `dev up` binds fixed ports outside the worktree window, so no slot
        // match can ever select them.
        let shared = "\
  601 kubectl --namespace navigator port-forward svc/surreal 18000:8000
  602 kubectl --namespace navigator port-forward svc/restate 9080:8080
";

        for slot in 0u16..120 {
            assert!(
                port_forward_pids_for("navigator-02bdf258", &BTreeSet::from([slot]), shared)
                    .is_empty(),
                "slot {slot} matched a shared dev up port-forward"
            );
        }
    }

    #[test]
    fn kind_inspect_output_tolerates_unlabelled_and_empty_lines() {
        assert!(parse_kind_cluster_ports("").is_empty());
        assert!(parse_kind_cluster_ports("\t20000 \n").is_empty());
        assert!(parse_kind_cluster_ports("no-tab-separator\n").is_empty());
    }

    #[test]
    fn only_a_vanished_container_is_tolerated_during_inspection() {
        assert!(is_missing_container("Error: No such object: 4f1c0d2b9a77"));
        assert!(is_missing_container(
            "Error response from daemon: No such container: navigator-c45a5c02-control-plane"
        ));

        // Anything else means the reservation is unknown, not absent. Treating
        // these as "binds no ports" is what would hand a live slot away.
        assert!(!is_missing_container(
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock."
        ));
        assert!(!is_missing_container(
            "permission denied while trying to connect to the Docker daemon socket"
        ));
        assert!(!is_missing_container(""));
    }

    #[test]
    fn colliding_paths_receive_distinct_slots() {
        let base = KindConfig::from_env();
        let first = Path::new(COLLIDING_A);
        let second = Path::new(COLLIDING_B);
        let shared = derived_worktree_slot(first);
        assert_eq!(
            shared,
            derived_worktree_slot(second),
            "fixture paths must collide for this test to mean anything"
        );

        // `first` already owns a cluster holding that slot's ports.
        let clusters = cluster_fixture(&[(
            &format!("navigator-{}", worktree_fingerprint(first)),
            &[
                WORKTREE_PORT_WINDOW_START + shared,
                WORKTREE_WEB_PORT_BASE + shared,
            ],
        )]);
        let claimed = cluster_claimed_slots(second, &clusters);
        assert_eq!(claimed, BTreeSet::from([shared]));

        let slot = choose_worktree_slot(second, None, &claimed, &base, &no_listeners).unwrap();
        assert_ne!(slot, shared, "a colliding worktree must not reuse the slot");
        assert_eq!(slot, shared + 1);
    }

    #[test]
    fn a_stopped_cluster_still_holds_its_slot() {
        // The regression: a stopped cluster answers no TCP connect, so the
        // listener probe reports its slot free while Docker still owns the
        // port binding. The claim must come from the cluster, not the probe.
        let root = Path::new(COLLIDING_A);
        let clusters =
            cluster_fixture(&[("navigator-deadbeef", &[WORKTREE_PORT_WINDOW_START + 5])]);

        assert_eq!(cluster_claimed_slots(root, &clusters), BTreeSet::from([5]));
        assert!(!no_listeners(WORKTREE_PORT_WINDOW_START + 5));
    }

    #[test]
    fn a_cluster_without_a_worktree_still_holds_its_slot() {
        // `navigator-deadbeef` matches no live worktree path, so no descriptor
        // can ever claim slot 12 — the cluster itself is the only registry.
        let root = Path::new(COLLIDING_A);
        let clusters = cluster_fixture(&[("navigator-deadbeef", &[WORKTREE_WEB_PORT_BASE + 12])]);
        let claimed = cluster_claimed_slots(root, &clusters);

        assert_eq!(claimed, BTreeSet::from([12]));
        let slot = choose_worktree_slot(
            root,
            Some(12),
            &claimed,
            &KindConfig::from_env(),
            &no_listeners,
        )
        .unwrap();
        assert_ne!(slot, 12);
    }

    #[test]
    fn a_worktree_keeps_the_slot_of_the_cluster_it_owns() {
        // Its own cluster is not a competing claim, so a repeated `up` is
        // stable — otherwise every re-run would walk to the next slot.
        let root = Path::new(COLLIDING_A);
        let clusters = cluster_fixture(&[(
            &format!("navigator-{}", worktree_fingerprint(root)),
            &[WORKTREE_PORT_WINDOW_START + 64, WORKTREE_WEB_PORT_BASE + 64],
        )]);
        let claimed = cluster_claimed_slots(root, &clusters);
        assert!(claimed.is_empty(), "a worktree cannot claim against itself");

        assert_eq!(
            choose_worktree_slot(
                root,
                Some(64),
                &claimed,
                &KindConfig::from_env(),
                &no_listeners
            )
            .unwrap(),
            64
        );
    }
    #[test]
    fn descriptor_round_trips_and_dev_slot_is_mode_gated() {
        let dev = WorktreeEnv {
            slug: "feature-x".into(),
            mode: "dev".into(),
            db_name: Some("navigator".into()),
            web_port: 20_607,
            slot: Some(7),
            runtime: Runtime::Kind,
        };
        let json = serde_json::to_string(&dev).unwrap();
        let back: WorktreeEnv = serde_json::from_str(&json).unwrap();
        assert_eq!(dev, back);
        assert_eq!(dev.dev_slot(), Some(7));

        let demo = WorktreeEnv {
            mode: "demo".into(),
            db_name: None,
            slot: None,
            runtime: Runtime::Kind,
            ..dev
        };
        assert_eq!(demo.dev_slot(), None);
    }

    #[test]
    fn worktree_environment_lock_serializes_descriptor_mutation() {
        let (_temp, repo) = git_fixture_with_origin();
        write_descriptor(
            &repo,
            &WorktreeEnv {
                slug: "main".into(),
                mode: "dev".into(),
                db_name: Some("navigator".into()),
                web_port: 20_642,
                slot: Some(42),
                runtime: Runtime::Kind,
            },
        )
        .unwrap();

        let held_lock = acquire_worktree_env_lock(&repo).unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let thread_root = repo.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _lock = acquire_worktree_env_lock(&thread_root).unwrap();
            remove_worktree_state(&thread_root);
            done_tx.send(()).unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(descriptor_path(&repo).is_file());

        drop(held_lock);
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert!(!descriptor_path(&repo).exists());
    }

    #[test]
    fn live_worktree_descriptors_reserve_slots_under_the_shared_git_lock() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let other = temp.path().join("other");
        std::fs::create_dir(&repo).unwrap();

        run_git(&repo, &["init", "--initial-branch=main"]);
        std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
        run_git(&repo, &["add", "README.md"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=Navigator Test",
                "-c",
                "user.email=dev@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "initial",
            ],
        );
        run_git(
            &repo,
            &["worktree", "add", "-b", "other", other.to_str().unwrap()],
        );

        let repo = repo.canonicalize().unwrap();
        let other = other.canonicalize().unwrap();
        write_descriptor(
            &repo,
            &WorktreeEnv {
                slug: "main".into(),
                mode: "dev".into(),
                db_name: Some("navigator".into()),
                web_port: 20_630,
                slot: Some(30),
                runtime: Runtime::Kind,
            },
        )
        .unwrap();
        write_descriptor(
            &other,
            &WorktreeEnv {
                slug: "other".into(),
                mode: "dev".into(),
                db_name: Some("navigator".into()),
                web_port: 20_642,
                slot: Some(42),
                runtime: Runtime::Kind,
            },
        )
        .unwrap();

        let lock = acquire_worktree_env_lock(&repo).unwrap();
        assert!(git_common_dir(&repo)
            .unwrap()
            .join("navigator-worktree-env.lock")
            .is_file());
        drop(lock);
        // A sibling descriptor reserves 42 even with no cluster built yet,
        // and an orphan cluster's slot 7 is unioned in beside it.
        let clusters =
            cluster_fixture(&[("navigator-deadbeef", &[WORKTREE_PORT_WINDOW_START + 7])]);
        assert_eq!(
            claimed_worktree_slots(&repo, &|| Ok(clusters.clone())).unwrap(),
            BTreeSet::from([7, 42])
        );
    }

    #[test]
    fn agent_worktree_path_prefers_generic_contract_then_codex_bridge() {
        let generic = agent_worktree_path(|key| match key {
            "NAVIGATOR_WORKTREE_PATH" => Some("/tmp/generic".into()),
            "CODEX_WORKTREE_PATH" => Some("/tmp/codex".into()),
            _ => None,
        });
        assert_eq!(generic, Some(PathBuf::from("/tmp/generic")));

        let codex =
            agent_worktree_path(|key| (key == "CODEX_WORKTREE_PATH").then(|| "/tmp/codex".into()));
        assert_eq!(codex, Some(PathBuf::from("/tmp/codex")));
    }

    #[test]
    fn topic_branch_uses_a_preprovisioned_worktree_in_place() {
        let (_temp, repo) = git_fixture_with_origin();
        let supplied = repo.parent().unwrap().join("supplied");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                supplied.to_str().unwrap(),
                "HEAD",
            ],
        );

        let selected = prepare_topic_checkout(&repo, Some(&supplied), "codex/topic").unwrap();

        assert_eq!(selected, supplied.canonicalize().unwrap());
        assert_eq!(git_branch(&selected).as_deref(), Some("codex/topic"));
        assert_eq!(worktree_paths_for_repo(&repo).len(), 2);
        assert_eq!(
            prepare_topic_checkout(&repo, Some(&supplied), "codex/topic").unwrap(),
            selected
        );
    }

    #[test]
    fn topic_branch_uses_the_current_linked_worktree_without_an_environment_path() {
        let (_temp, repo) = git_fixture_with_origin();
        let linked = repo.parent().unwrap().join("linked");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                linked.to_str().unwrap(),
                "HEAD",
            ],
        );
        let linked = linked.canonicalize().unwrap();

        let selected = prepare_topic_checkout(&linked, None, "codex/topic").unwrap();

        assert_eq!(selected, linked);
        assert_eq!(git_branch(&selected).as_deref(), Some("codex/topic"));
        assert_eq!(worktree_paths_for_repo(&repo).len(), 2);
    }

    #[test]
    fn topic_branch_creates_a_sibling_worktree_without_a_supplied_checkout() {
        let (_temp, repo) = git_fixture_with_origin();

        let selected = prepare_topic_checkout(&repo, None, "codex/topic").unwrap();

        assert_eq!(
            selected,
            repo.join(".worktrees/codex-topic").canonicalize().unwrap()
        );
        assert_eq!(git_branch(&selected).as_deref(), Some("codex/topic"));
        assert_eq!(worktree_paths_for_repo(&repo).len(), 2);
        assert_eq!(
            prepare_topic_checkout(&repo, None, "codex/topic").unwrap(),
            selected
        );
    }

    #[test]
    fn topic_branch_attaches_an_existing_local_branch() {
        let (_temp, repo) = git_fixture_with_origin();
        run_git(&repo, &["branch", "codex/existing", "origin/main"]);

        let selected = prepare_topic_checkout(&repo, None, "codex/existing").unwrap();

        assert_eq!(git_branch(&selected).as_deref(), Some("codex/existing"));
        assert_eq!(worktree_paths_for_repo(&repo).len(), 2);
    }

    #[test]
    fn topic_branch_rejects_an_invalid_branch_name() {
        let (_temp, repo) = git_fixture_with_origin();

        let error = prepare_topic_checkout(&repo, None, "not a branch").unwrap_err();

        assert!(error.to_string().contains("validate topic branch name"));
    }

    fn git_fixture_with_origin() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let remote = temp.path().join("remote.git");
        std::fs::create_dir(&repo).unwrap();
        run_git(&repo, &["init", "--initial-branch=main"]);
        std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
        run_git(&repo, &["add", "README.md"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=Navigator Test",
                "-c",
                "user.email=dev@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "initial",
            ],
        );
        run_git(temp.path(), &["init", "--bare", remote.to_str().unwrap()]);
        run_git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run_git(&repo, &["push", "-u", "origin", "main"]);
        (temp, repo.canonicalize().unwrap())
    }

    fn worktree_paths_for_repo(repo: &Path) -> Vec<PathBuf> {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(output.status.success());
        worktree_paths(&String::from_utf8(output.stdout).unwrap()).collect()
    }

    fn run_git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
