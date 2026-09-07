//! Garage as a host process, with the same bootstrap the cluster gets.
//!
//! Object storage is the one dependency whose usefulness is not the
//! process but the *provisioning*: a Garage that has started serves no
//! bucket and holds no key. The cluster lane runs that bootstrap through
//! `kubectl exec garage-0 -- /garage …`
//! ([`super::super::garage::provision`]); this module runs the identical
//! command sequence against a local config file. Only the transport
//! changes — the layout assignment, the seven lane keys, the seven
//! buckets, and the grants are the same operations in the same order,
//! and the output parsing is literally the same code.
//!
//! Three ports, not one. The slot table reserves the S3 port because
//! that is the only one anything outside Garage connects to; RPC and
//! admin are internal to the process and would collide between worktrees
//! at their defaults, so they are derived from the slot here rather than
//! added to a config the rest of the workspace reads.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::super::garage::{parse_key, random_hex, Credentials, LaneCredentials};
use super::supervisor::Service;

/// The formula and executable `navigator dev install` acquires.
const FORMULA: &str = "garage";
const BINARY: &str = "garage";

/// The buckets `render_env_for` names, each with its own key. Identical
/// to the set [`super::super::garage::provision`] creates in the
/// cluster — a lane that provisioned a different set would render an
/// environment whose `NAVIGATOR_*_BUCKET` values do not exist.
const LANES: &[&str] = &[
    "navigator-documents",
    "navigator-assets",
    "navigator-applications",
    "navigator-exports",
    "navigator-archives",
    "navigator-telemetry",
    "navigator-lfs",
];

/// Zone name for the single-node layout. Matches the cluster's, which is
/// what keeps a layout dump comparable between the lanes.
const ZONE: &str = "kind";

fn state_dir(root: &Path) -> PathBuf {
    super::supervisor::service_dir(root, super::GARAGE_LABEL)
}

fn config_path(root: &Path) -> PathBuf {
    state_dir(root).join("garage.toml")
}

/// Garage's configuration file.
///
/// Mirrors `k8s/overlays/kind/garage/garage.yaml`'s embedded
/// `garage.toml` — same engine, same replication factor, same
/// consistency mode, same S3 region — with every address moved to
/// loopback and this worktree's ports.
fn config_toml(
    data: &Path,
    s3_port: u16,
    rpc_port: u16,
    admin_port: u16,
    rpc_secret: &str,
    admin_token: &str,
) -> String {
    format!(
        "metadata_dir = \"{meta}\"\n\
         data_dir = \"{blocks}\"\n\
         db_engine = \"lmdb\"\n\
         replication_factor = 1\n\
         consistency_mode = \"consistent\"\n\
         rpc_bind_addr = \"127.0.0.1:{rpc_port}\"\n\
         rpc_public_addr = \"127.0.0.1:{rpc_port}\"\n\
         rpc_secret = \"{rpc_secret}\"\n\
         \n\
         [s3_api]\n\
         s3_region = \"garage\"\n\
         api_bind_addr = \"127.0.0.1:{s3_port}\"\n\
         \n\
         [admin]\n\
         api_bind_addr = \"127.0.0.1:{admin_port}\"\n\
         admin_token = \"{admin_token}\"\n",
        meta = data.join("meta").display(),
        blocks = data.join("data").display(),
    )
}

/// Write the configuration file once and keep it.
///
/// The RPC secret must survive a restart: it is baked into the on-disk
/// layout, so regenerating it on every `up` would leave a node unable to
/// read the metadata it wrote.
fn ensure_config(root: &Path, s3_port: u16, rpc_port: u16, admin_port: u16) -> Result<PathBuf> {
    let path = config_path(root);
    if path.is_file() {
        return Ok(path);
    }
    let data = state_dir(root);
    std::fs::create_dir_all(data.join("meta"))
        .with_context(|| format!("create {}", data.join("meta").display()))?;
    std::fs::create_dir_all(data.join("data"))
        .with_context(|| format!("create {}", data.join("data").display()))?;
    std::fs::write(
        &path,
        config_toml(
            &data,
            s3_port,
            rpc_port,
            admin_port,
            &random_hex(),
            &random_hex(),
        ),
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Prepare Garage's supervised service.
pub(super) fn service(
    root: &Path,
    s3_port: u16,
    rpc_port: u16,
    admin_port: u16,
) -> Result<Service> {
    let config = ensure_config(root, s3_port, rpc_port, admin_port)?;
    Ok(Service {
        label: super::GARAGE_LABEL,
        program: super::preflight::binary(FORMULA, BINARY)?,
        args: vec![
            "-c".to_string(),
            config.display().to_string(),
            "server".to_string(),
        ],
        env: Vec::new(),
        cwd: state_dir(root),
        port: s3_port,
    })
}

/// Whether a `garage layout show` dump describes a node that has already
/// been assigned capacity.
///
/// Version zero is the state a fresh node starts in: it is serving, but
/// no bucket can be created until a layout is applied.
fn layout_applied(layout_show: &str) -> bool {
    super::super::garage::field(layout_show, &["Current cluster layout version"])
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|version| version > 0)
}

/// The node id `layout assign` takes, read from `garage node id`.
///
/// The command prints `<id>@<address>`; the assignment wants only the id.
fn node_id(node_id_output: &str) -> Option<&str> {
    node_id_output
        .split('@')
        .next()
        .and_then(|value| value.split_whitespace().last())
        .filter(|value| !value.is_empty())
}

/// Assign a layout, mint the lane keys, create the buckets, and grant
/// each key its bucket.
///
/// The same sequence — and, for the parsing, the same functions — as the
/// cluster lane's [`super::super::garage::provision`]. Idempotent: an
/// existing key is read back rather than re-minted, so the credentials
/// rendered into `.devx/env` stay stable across restarts.
pub(super) fn provision(root: &Path) -> Result<Credentials> {
    let config = config_path(root);
    let garage = super::preflight::binary(FORMULA, BINARY)?;

    if !layout_applied(&run(&garage, &config, &["layout", "show"])?) {
        let identity = run(&garage, &config, &["node", "id"])?;
        let id = node_id(&identity)
            .context("Garage `node id` output carried no node identifier")?
            .to_string();
        run(
            &garage,
            &config,
            &["layout", "assign", "-z", ZONE, "-c", "5G", &id],
        )?;
        run(&garage, &config, &["layout", "apply", "--version", "1"])?;
    }

    let mut minted = Vec::with_capacity(LANES.len());
    for lane in LANES {
        minted.push(ensure_key(&garage, &config, lane)?);
        // `bucket create` fails once the bucket exists, which is the
        // ordinary second-`up` case rather than an error.
        let _ = run(&garage, &config, &["bucket", "create", lane]);
        run(
            &garage,
            &config,
            &[
                "bucket", "allow", "--read", "--write", "--owner", lane, "--key", lane,
            ],
        )?;
    }
    let mut minted = minted.into_iter();
    let documents = minted.next().context("the documents lane key is missing")?;
    let assets = minted.next().context("the assets lane key is missing")?;
    let applications = minted
        .next()
        .context("the applications lane key is missing")?;
    let _exports = minted.next().context("the exports lane key is missing")?;
    let archives = minted.next().context("the archives lane key is missing")?;
    let _telemetry = minted.next().context("the telemetry lane key is missing")?;
    let lfs = minted.next().context("the LFS lane key is missing")?;
    Ok(Credentials {
        documents,
        assets,
        applications,
        archives,
        lfs,
    })
}

/// Read a lane's key back, minting it only when it does not exist.
fn ensure_key(garage: &Path, config: &Path, name: &str) -> Result<LaneCredentials> {
    if let Ok(existing) = run(garage, config, &["key", "info", "--show-secret", name]) {
        return parse_key(&existing);
    }
    parse_key(&run(garage, config, &["key", "create", name])?)
}

fn run(garage: &Path, config: &Path, arguments: &[&str]) -> Result<String> {
    let output = Command::new(garage)
        .arg("-c")
        .arg(config)
        .args(arguments)
        .output()
        .context("run Garage administration command")?;
    if !output.status.success() {
        bail!(
            "`garage {}` failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("Garage returned non-UTF-8 output")
}

#[cfg(test)]
mod tests {
    use super::{config_toml, layout_applied, node_id, LANES};
    use std::path::Path;

    /// Every address has to be loopback and slot-derived. A Garage that
    /// kept its default ports would bind the first worktree's numbers and
    /// refuse to start in the second — the exact failure the slot table
    /// exists to prevent.
    #[test]
    fn every_listener_is_loopback_on_a_slot_derived_port() {
        let config = config_toml(
            Path::new("/checkout/.devx/native/garage"),
            20_559,
            21_359,
            21_459,
            "rpcsecret",
            "admintoken",
        );

        assert!(
            config.contains("api_bind_addr = \"127.0.0.1:20559\""),
            "{config}"
        );
        assert!(
            config.contains("rpc_bind_addr = \"127.0.0.1:21359\""),
            "{config}"
        );
        assert!(
            config.contains("api_bind_addr = \"127.0.0.1:21459\""),
            "{config}"
        );
        assert!(!config.contains("[::]"), "{config}");
    }

    /// The cluster's `garage.toml` is the reference. A native tier on a
    /// different engine, replication factor, or S3 region would store
    /// objects the cluster lane cannot read back — the drift this
    /// two-lane design exists to avoid.
    #[test]
    fn the_storage_contract_matches_the_cluster_manifest() {
        let manifest = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("repo root is cli/'s parent")
                .join("k8s/overlays/kind/garage/garage.yaml"),
        )
        .expect("read the KIND Garage manifest");
        let config = config_toml(Path::new("/data"), 1, 2, 3, "secret", "token");

        for setting in [
            "db_engine = \"lmdb\"",
            "replication_factor = 1",
            "consistency_mode = \"consistent\"",
            "s3_region = \"garage\"",
        ] {
            assert!(config.contains(setting), "native config lacks {setting}");
            assert!(manifest.contains(setting), "manifest lacks {setting}");
        }
    }

    /// The data directories belong inside the worktree so
    /// `worktree-env down` reclaims them; Garage's own defaults are
    /// machine-wide.
    #[test]
    fn the_stores_live_under_the_directory_they_are_given() {
        let config = config_toml(
            Path::new("/checkout/.devx/native/garage"),
            1,
            2,
            3,
            "s",
            "t",
        );

        assert!(
            config.contains("metadata_dir = \"/checkout/.devx/native/garage/meta\""),
            "{config}"
        );
        assert!(
            config.contains("data_dir = \"/checkout/.devx/native/garage/data\""),
            "{config}"
        );
    }

    /// A fresh node reports version 0 and serves nothing. Reading that as
    /// "already laid out" would skip the assignment and leave every
    /// bucket creation failing.
    #[test]
    fn a_fresh_node_is_not_mistaken_for_an_assigned_one() {
        assert!(!layout_applied("Current cluster layout version: 0\n"));
        assert!(layout_applied("Current cluster layout version: 1\n"));
        assert!(!layout_applied("no layout here\n"));
    }

    /// `garage node id` prints `<id>@<address>`. Passing the whole string
    /// to `layout assign` assigns capacity to a node that does not exist.
    #[test]
    fn the_node_identifier_is_taken_without_its_address() {
        assert_eq!(
            node_id("e3f4a2@127.0.0.1:21359\n"),
            Some("e3f4a2"),
            "the address must be dropped"
        );
        assert_eq!(node_id(""), None);
    }

    /// `render_env_for` names six buckets under their own env vars (the
    /// seventh, telemetry, is provisioned for a future consumer — see
    /// ENG-206). Provisioning a different set renders an environment
    /// pointing at storage that was never created.
    #[test]
    fn every_bucket_the_environment_names_is_provisioned() {
        for bucket in [
            "navigator-documents",
            "navigator-assets",
            "navigator-applications",
            "navigator-exports",
            "navigator-archives",
            "navigator-lfs",
        ] {
            assert!(LANES.contains(&bucket), "{bucket} is not provisioned");
        }
        assert_eq!(LANES.len(), 7);
    }
}
