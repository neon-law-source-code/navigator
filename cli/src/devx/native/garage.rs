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
//! admin are internal to the shared process and use fixed host ports
//! outside the per-worktree slot range.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::super::garage::{parse_key, random_hex, Credentials, LaneCredentials};
use super::supervisor::Service;

pub(super) struct Tenant {
    pub(super) buckets: BTreeMap<String, String>,
    pub(super) env: BTreeMap<String, String>,
}

/// The formula and executable `navigator dev install` acquires.
const FORMULA: &str = "garage";
const BINARY: &str = "garage";

/// The buckets `render_env_for` names, each with its own key. Identical
/// to the set [`super::super::garage::provision`] creates in the
/// cluster — a lane that provisioned a different set would render an
/// environment whose `NAVIGATOR_*_BUCKET` values do not exist.
const LANES: &[(&str, &str)] = &[
    ("documents", "NAVIGATOR_STORAGE_BUCKET"),
    ("assets", "NAVIGATOR_ASSETS_BUCKET"),
    ("applications", "NAVIGATOR_APPLICATIONS_BUCKET"),
    ("exports", "NAVIGATOR_SURREAL_ARCHIVES_BUCKET"),
    ("archives", "NAVIGATOR_ARCHIVES_BUCKET"),
    ("telemetry", "NAVIGATOR_TELEMETRY_BUCKET"),
    ("lfs", "NAVIGATOR_LFS_BUCKET"),
];

/// Zone name for the single-node layout. Matches the cluster's, which is
/// what keeps a layout dump comparable between the lanes.
const ZONE: &str = "kind";

/// The Garage bucket names one native database tenant owns.
///
/// Kept separate from provisioning so the host registry can reject a bucket
/// collision before any Garage administration command can alter shared state.
pub(super) fn bucket_names(tenant: &str) -> BTreeMap<String, String> {
    LANES
        .iter()
        .map(|(suffix, env_name)| ((*env_name).to_string(), bucket_name(tenant, suffix)))
        .collect()
}

fn state_dir(root: &Path) -> PathBuf {
    super::supervisor::service_dir(root, super::GARAGE_LABEL)
}

fn config_path(root: &Path) -> PathBuf {
    state_dir(root).join("garage.toml")
}

/// Garage's configuration file for the shared native process.
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

/// Assign a layout, mint one set of tenant keys, create the buckets, and
/// grant each key its bucket.
///
/// The same sequence — and, for the parsing, the same functions — as the
/// cluster lane's [`super::super::garage::provision`]. Idempotent: an
/// existing key is read back rather than re-minted, so the credentials
/// rendered into `.devx/env` stay stable across restarts.
pub(super) fn provision(root: &Path, tenant: &str) -> Result<Tenant> {
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
    let buckets = bucket_names(tenant);
    for (_, env_name) in LANES {
        let name = &buckets[*env_name];
        minted.push(ensure_key(&garage, &config, name)?);
        // `bucket create` fails once the bucket exists, which is the
        // ordinary second-`up` case rather than an error.
        let _ = run(&garage, &config, &["bucket", "create", name]);
        run(
            &garage,
            &config,
            &[
                "bucket", "allow", "--read", "--write", "--owner", name, "--key", name,
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
    let credentials = Credentials {
        documents,
        assets,
        applications,
        archives,
        lfs,
    };
    let mut env = BTreeMap::new();
    for (key, value) in credential_env(&credentials) {
        env.insert(key.to_string(), value.to_string());
    }
    env.extend(
        buckets
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    Ok(Tenant { buckets, env })
}

fn bucket_name(tenant: &str, suffix: &str) -> String {
    format!("{}-{suffix}", tenant.replace('_', "-"))
}

fn credential_env(credentials: &Credentials) -> [(&'static str, &str); 10] {
    [
        (
            "NAVIGATOR_GARAGE_ACCESS_KEY",
            &credentials.documents.access_key,
        ),
        (
            "NAVIGATOR_GARAGE_SECRET_KEY",
            &credentials.documents.secret_key,
        ),
        (
            "NAVIGATOR_GARAGE_ASSETS_ACCESS_KEY",
            &credentials.assets.access_key,
        ),
        (
            "NAVIGATOR_GARAGE_ASSETS_SECRET_KEY",
            &credentials.assets.secret_key,
        ),
        (
            "NAVIGATOR_GARAGE_APPLICATIONS_ACCESS_KEY",
            &credentials.applications.access_key,
        ),
        (
            "NAVIGATOR_GARAGE_APPLICATIONS_SECRET_KEY",
            &credentials.applications.secret_key,
        ),
        (
            "NAVIGATOR_GARAGE_ARCHIVES_ACCESS_KEY",
            &credentials.archives.access_key,
        ),
        (
            "NAVIGATOR_GARAGE_ARCHIVES_SECRET_KEY",
            &credentials.archives.secret_key,
        ),
        (
            "NAVIGATOR_GARAGE_LFS_ACCESS_KEY",
            &credentials.lfs.access_key,
        ),
        (
            "NAVIGATOR_GARAGE_LFS_SECRET_KEY",
            &credentials.lfs.secret_key,
        ),
    ]
}

pub(super) fn remove_tenant(root: &Path, tenant: &Tenant) -> Result<()> {
    let config = config_path(root);
    let garage = super::preflight::binary(FORMULA, BINARY)?;
    for bucket in tenant.buckets.values() {
        remove_resource(&garage, &config, &["bucket", "delete", "--yes", bucket])?;
        remove_resource(&garage, &config, &["key", "delete", "--yes", bucket])?;
    }
    Ok(())
}

fn remove_resource(garage: &Path, config: &Path, args: &[&str]) -> Result<()> {
    match run(garage, config, args) {
        Ok(_) => Ok(()),
        Err(error) if error.to_string().contains("not found") => Ok(()),
        Err(error) if error.to_string().contains("does not exist") => Ok(()),
        Err(error) => Err(error),
    }
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
    use super::{bucket_name, bucket_names, config_toml, layout_applied, node_id, LANES};
    use std::path::Path;

    /// Every address has to be loopback. The shared process uses fixed
    /// internal ports while the S3 port remains supplied by the caller.
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

    #[test]
    fn tenant_bucket_names_are_isolated_and_stable() {
        assert_eq!(
            bucket_name("navigator_alpha_1234", "documents"),
            "navigator-alpha-1234-documents"
        );
        assert_ne!(
            bucket_name("navigator_alpha_1234", "documents"),
            bucket_name("navigator_beta_5678", "documents")
        );
    }

    #[test]
    fn every_tenant_bucket_is_known_before_provisioning() {
        let buckets = bucket_names("navigator_alpha_1234");

        assert_eq!(buckets.len(), LANES.len());
        assert_eq!(
            buckets["NAVIGATOR_STORAGE_BUCKET"],
            "navigator-alpha-1234-documents"
        );
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

    /// The shared process data belongs inside the host runtime directory,
    /// rather than a disposable worktree.
    #[test]
    fn the_stores_live_under_the_directory_they_are_given() {
        let root = Path::new("/host/.navigator/native-runtime/garage");
        let config = config_toml(root, 1, 2, 3, "s", "t");

        assert!(
            config.contains(&format!(
                "metadata_dir = \"{}\"",
                root.join("meta").display()
            )),
            "{config}"
        );
        assert!(
            config.contains(&format!("data_dir = \"{}\"", root.join("data").display())),
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

    /// `render_env_for` names seven buckets under their own env vars.
    /// Provisioning a different set renders an environment pointing at
    /// storage that was never created.
    #[test]
    fn every_bucket_the_environment_names_is_provisioned() {
        for bucket in [
            "documents",
            "assets",
            "applications",
            "exports",
            "archives",
            "telemetry",
            "lfs",
        ] {
            assert!(
                LANES.iter().any(|(suffix, _)| *suffix == bucket),
                "{bucket} is not provisioned"
            );
        }
        assert_eq!(LANES.len(), 7);
    }
}
