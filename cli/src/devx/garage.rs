//! Idempotent KIND Garage bootstrap.
//!
//! Secrets are generated in memory, applied over `kubectl` stdin, and never
//! included in command arguments or diagnostic output.

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use base64::Engine;

const CONTROL_SECRET: &str = "navigator-garage-control";
const S3_SECRET: &str = "navigator-garage-s3";
const BOOTSTRAP_LEASE: &str = "navigator-garage-bootstrap";

pub(super) struct Credentials {
    pub documents: LaneCredentials,
    pub assets: LaneCredentials,
    pub applications: LaneCredentials,
    pub archives: LaneCredentials,
    pub lfs: LaneCredentials,
}

pub(super) struct LaneCredentials {
    pub access_key: String,
    pub secret_key: String,
}

/// `devx garage-bootstrap`: mint the Garage object-storage secrets for a
/// stack whose manifests are already applied. `dev up`/`dev deploy` run
/// this sequence inline; CI's `deploy.yml` applies the manifests with a
/// raw `kubectl apply -k` and then calls this as its own step.
///
/// The order mirrors `deploy` (prepare → wait → provision), just landing
/// *after* the apply rather than bracketing it: [`prepare`] writes the
/// `navigator-garage-control` secret the Garage `StatefulSet` mounts, so
/// the pod — created by the earlier apply and stuck in `ContainerCreating`
/// on the missing secret volume — proceeds and rolls out; [`provision`]
/// then execs `garage key create` in that pod to mint the S3 credentials
/// and writes the `navigator-garage-s3` secret `navigator-web` and
/// `workflows-service` consume (both sit in `CreateContainerConfigError`
/// until it exists). Runtime-minted keys are why these can't be static
/// manifests.
pub(super) fn bootstrap(cfg: &super::KindConfig) -> Result<()> {
    super::require_tools(&["kubectl"])?;
    super::use_kind_context(cfg)?;
    prepare(&cfg.namespace)?;
    super::wait_rollout("statefulset", "garage", cfg)?;
    provision(&cfg.namespace)?;
    Ok(())
}

pub(super) fn prepare(namespace: &str) -> Result<()> {
    if secret_value(namespace, CONTROL_SECRET, "rpc_secret")?.is_some() {
        return Ok(());
    }
    let manifest = format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {CONTROL_SECRET}\n  namespace: {namespace}\ntype: Opaque\nstringData:\n  rpc_secret: {}\n  admin_token: {}\n",
        random_hex(),
        random_hex(),
    );
    apply_stdin(&manifest)
}

pub(super) fn provision(namespace: &str) -> Result<Credentials> {
    if let Some(credentials) = provisioned_credentials(namespace)? {
        return Ok(credentials);
    }

    let Some(_lease) = BootstrapLease::acquire(namespace)? else {
        for _ in 0..120 {
            if let Some(credentials) = provisioned_credentials(namespace)? {
                return Ok(credentials);
            }
            thread::sleep(Duration::from_secs(1));
        }
        bail!("timed out waiting for the Garage bootstrap lease holder");
    };

    // A previous holder may have completed immediately before this process
    // acquired the Lease.
    if let Some(credentials) = provisioned_credentials(namespace)? {
        return Ok(credentials);
    }

    let layout = garage(namespace, &["layout", "show"])?;
    let layout_version = field(&layout, &["Current cluster layout version"])
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if layout_version == 0 {
        let node = garage(namespace, &["node", "id"])?;
        let node_id = node
            .split('@')
            .next()
            .and_then(|value| value.split_whitespace().last())
            .filter(|value| !value.is_empty())
            .context("Garage node id output was empty")?;
        garage(
            namespace,
            &["layout", "assign", "-z", "kind", "-c", "5G", node_id],
        )?;
        garage(namespace, &["layout", "apply", "--version", "1"])?;
    }

    let documents = create_key(namespace, "navigator-documents")?;
    let assets = create_key(namespace, "navigator-assets")?;
    let applications = create_key(namespace, "navigator-applications")?;
    let exports = create_key(namespace, "navigator-exports")?;
    let archives = create_key(namespace, "navigator-archives")?;
    let telemetry = create_key(namespace, "navigator-telemetry")?;
    let lfs = create_key(namespace, "navigator-lfs")?;
    for (bucket, key_name) in [
        ("navigator-documents", "navigator-documents"),
        ("navigator-assets", "navigator-assets"),
        ("navigator-applications", "navigator-applications"),
        ("navigator-exports", "navigator-exports"),
        ("navigator-archives", "navigator-archives"),
        ("navigator-telemetry", "navigator-telemetry"),
        ("navigator-lfs", "navigator-lfs"),
    ] {
        garage_allow_failure(namespace, &["bucket", "create", bucket])?;
        garage(
            namespace,
            &[
                "bucket", "allow", "--read", "--write", "--owner", bucket, "--key", key_name,
            ],
        )?;
    }

    let manifest = format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {S3_SECRET}\n  namespace: {namespace}\ntype: Opaque\nstringData:\n  access_key: {}\n  secret_key: {}\n  assets_access_key: {}\n  assets_secret_key: {}\n  applications_access_key: {}\n  applications_secret_key: {}\n  exports_access_key: {}\n  exports_secret_key: {}\n  archives_access_key: {}\n  archives_secret_key: {}\n  telemetry_access_key: {}\n  telemetry_secret_key: {}\n  lfs_access_key: {}\n  lfs_secret_key: {}\n",
        documents.access_key, documents.secret_key,
        assets.access_key, assets.secret_key,
        applications.access_key, applications.secret_key,
        exports.access_key, exports.secret_key,
        archives.access_key, archives.secret_key,
        telemetry.access_key, telemetry.secret_key,
        lfs.access_key, lfs.secret_key,
    );
    apply_stdin(&manifest)?;
    Ok(Credentials {
        documents,
        assets,
        applications,
        archives,
        lfs,
    })
}

fn provisioned_credentials(namespace: &str) -> Result<Option<Credentials>> {
    let Some(documents) = existing_key(namespace, "navigator-documents")? else {
        return Ok(None);
    };
    let Some(assets) = existing_key(namespace, "navigator-assets")? else {
        return Ok(None);
    };
    let Some(applications) = existing_key(namespace, "navigator-applications")? else {
        return Ok(None);
    };
    let Some(exports) = existing_key(namespace, "navigator-exports")? else {
        return Ok(None);
    };
    let Some(archives) = existing_key(namespace, "navigator-archives")? else {
        return Ok(None);
    };
    let Some(telemetry) = existing_key(namespace, "navigator-telemetry")? else {
        return Ok(None);
    };
    let Some(lfs) = existing_key(namespace, "navigator-lfs")? else {
        return Ok(None);
    };
    let Some(access_key) = secret_value(namespace, S3_SECRET, "access_key")? else {
        return Ok(None);
    };
    let Some(secret_key) = secret_value(namespace, S3_SECRET, "secret_key")? else {
        return Ok(None);
    };
    let Some(assets_access_key) = secret_value(namespace, S3_SECRET, "assets_access_key")? else {
        return Ok(None);
    };
    let Some(assets_secret_key) = secret_value(namespace, S3_SECRET, "assets_secret_key")? else {
        return Ok(None);
    };
    let Some(applications_access_key) =
        secret_value(namespace, S3_SECRET, "applications_access_key")?
    else {
        return Ok(None);
    };
    let Some(applications_secret_key) =
        secret_value(namespace, S3_SECRET, "applications_secret_key")?
    else {
        return Ok(None);
    };
    let Some(exports_access_key) = secret_value(namespace, S3_SECRET, "exports_access_key")? else {
        return Ok(None);
    };
    let Some(exports_secret_key) = secret_value(namespace, S3_SECRET, "exports_secret_key")? else {
        return Ok(None);
    };
    let Some(archives_access_key) = secret_value(namespace, S3_SECRET, "archives_access_key")?
    else {
        return Ok(None);
    };
    let Some(archives_secret_key) = secret_value(namespace, S3_SECRET, "archives_secret_key")?
    else {
        return Ok(None);
    };
    let Some(telemetry_access_key) = secret_value(namespace, S3_SECRET, "telemetry_access_key")?
    else {
        return Ok(None);
    };
    let Some(telemetry_secret_key) = secret_value(namespace, S3_SECRET, "telemetry_secret_key")?
    else {
        return Ok(None);
    };
    let Some(lfs_access_key) = secret_value(namespace, S3_SECRET, "lfs_access_key")? else {
        return Ok(None);
    };
    let Some(lfs_secret_key) = secret_value(namespace, S3_SECRET, "lfs_secret_key")? else {
        return Ok(None);
    };
    if documents.access_key != access_key
        || documents.secret_key != secret_key
        || assets.access_key != assets_access_key
        || assets.secret_key != assets_secret_key
        || applications.access_key != applications_access_key
        || applications.secret_key != applications_secret_key
        || exports.access_key != exports_access_key
        || exports.secret_key != exports_secret_key
        || archives.access_key != archives_access_key
        || archives.secret_key != archives_secret_key
        || telemetry.access_key != telemetry_access_key
        || telemetry.secret_key != telemetry_secret_key
        || lfs.access_key != lfs_access_key
        || lfs.secret_key != lfs_secret_key
    {
        return Ok(None);
    }
    Ok(Some(Credentials {
        documents,
        assets,
        applications,
        archives,
        lfs,
    }))
}

struct BootstrapLease {
    namespace: String,
}

impl BootstrapLease {
    fn acquire(namespace: &str) -> Result<Option<Self>> {
        let manifest = format!(
            "apiVersion: coordination.k8s.io/v1\nkind: Lease\nmetadata:\n  name: {BOOTSTRAP_LEASE}\n  namespace: {namespace}\nspec:\n  holderIdentity: navigator-{}\n  leaseDurationSeconds: 180\n",
            random_hex()
        );
        if !create_stdin(&manifest)? {
            return Ok(None);
        }
        Ok(Some(Self {
            namespace: namespace.to_owned(),
        }))
    }
}

impl Drop for BootstrapLease {
    fn drop(&mut self) {
        let _ = Command::new("kubectl")
            .args([
                "--namespace",
                &self.namespace,
                "delete",
                "lease",
                BOOTSTRAP_LEASE,
                "--ignore-not-found",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn garage_allow_failure(namespace: &str, arguments: &[&str]) -> Result<()> {
    Command::new("kubectl")
        .args([
            "--namespace",
            namespace,
            "exec",
            "garage-0",
            "--",
            "/garage",
        ])
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run idempotent Garage administration command")?;
    Ok(())
}

fn create_key(namespace: &str, name: &str) -> Result<LaneCredentials> {
    if let Some(credentials) = existing_key(namespace, name)? {
        return Ok(credentials);
    }
    let output = garage(namespace, &["key", "create", name])?;
    parse_key(&output)
}

fn existing_key(namespace: &str, name: &str) -> Result<Option<LaneCredentials>> {
    garage_optional(namespace, &["key", "info", "--show-secret", name])?
        .map(|output| parse_key(&output))
        .transpose()
}

/// Read an access key and secret out of `garage key create` / `key info`
/// output.
///
/// Shared with [`super::native::garage`]: the native lane runs the same
/// commands against a local config file rather than through
/// `kubectl exec`, and parsing their output twice is how the two lanes
/// would come to disagree about what a key is.
pub(super) fn parse_key(output: &str) -> Result<LaneCredentials> {
    let access_key = field(output, &["Key ID", "Access key ID"])
        .context("Garage key-create output omitted the access key id")?;
    let secret_key = field(output, &["Secret key", "Secret access key"])
        .context("Garage key-create output omitted the secret key")?;
    Ok(LaneCredentials {
        access_key,
        secret_key,
    })
}

fn garage_optional(namespace: &str, arguments: &[&str]) -> Result<Option<String>> {
    let output = Command::new("kubectl")
        .args([
            "--namespace",
            namespace,
            "exec",
            "garage-0",
            "--",
            "/garage",
        ])
        .args(arguments)
        .output()
        .context("run optional Garage administration command")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(output.stdout).context("Garage returned non-UTF-8 output")?,
    ))
}

fn garage(namespace: &str, arguments: &[&str]) -> Result<String> {
    let output = Command::new("kubectl")
        .args([
            "--namespace",
            namespace,
            "exec",
            "garage-0",
            "--",
            "/garage",
        ])
        .args(arguments)
        .output()
        .context("run Garage administration command")?;
    if !output.status.success() {
        bail!("Garage administration command failed ({})", output.status);
    }
    String::from_utf8(output.stdout).context("Garage returned non-UTF-8 output")
}

/// The value following one of `labels` in a `key: value` output block.
/// Shared with [`super::native::garage`] for the reason [`parse_key`]
/// gives.
pub(super) fn field(output: &str, labels: &[&str]) -> Option<String> {
    output.lines().find_map(|line| {
        labels.iter().find_map(|label| {
            line.trim()
                .strip_prefix(label)
                .and_then(|value| value.strip_prefix(':'))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
    })
}

fn secret_value(namespace: &str, name: &str, key: &str) -> Result<Option<String>> {
    let output = Command::new("kubectl")
        .args(["--namespace", namespace, "get", "secret", name, "-o"])
        .arg(format!("jsonpath={{.data.{key}}}"))
        .output()
        .context("read Garage Kubernetes secret")?;
    if !output.status.success() {
        return Ok(None);
    }
    let encoded = String::from_utf8(output.stdout).context("secret value was not UTF-8")?;
    if encoded.is_empty() {
        return Ok(None);
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .context("decode Garage Kubernetes secret")?;
    Ok(Some(
        String::from_utf8(bytes).context("Garage secret was not UTF-8")?,
    ))
}

fn apply_stdin(manifest: &str) -> Result<()> {
    let mut child = Command::new("kubectl")
        .args(["apply", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("start kubectl apply for Garage secret")?;
    child
        .stdin
        .take()
        .context("kubectl stdin unavailable")?
        .write_all(manifest.as_bytes())
        .context("write Garage secret manifest")?;
    let status = child.wait().context("wait for Garage secret apply")?;
    if !status.success() {
        bail!("kubectl apply for Garage secret failed ({status})");
    }
    Ok(())
}

fn create_stdin(manifest: &str) -> Result<bool> {
    let mut child = Command::new("kubectl")
        .args(["create", "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("start kubectl create for Garage bootstrap lease")?;
    child
        .stdin
        .take()
        .context("kubectl stdin unavailable")?
        .write_all(manifest.as_bytes())
        .context("write Garage bootstrap lease manifest")?;
    Ok(child
        .wait()
        .context("wait for Garage bootstrap lease create")?
        .success())
}

/// Publish minted credentials to this process's environment, where
/// [`super::render_env_for`] reads them into `.devx/env`.
///
/// Both lanes end here. The keys are runtime-minted by Garage itself, so
/// they cannot be a static manifest or a rendered constant — which is
/// why the environment, rather than the config, is the hand-off point.
pub(super) fn export(credentials: &Credentials) {
    for (key, value) in [
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
    ] {
        std::env::set_var(key, value);
    }
}

pub(super) fn random_hex() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            use std::fmt::Write as _;
            let _ = write!(output, "{byte:02x}");
            output
        })
}

#[cfg(test)]
mod tests {
    use super::field;

    #[test]
    fn parses_garage_key_output_without_logging_it() {
        let output = "Key ID: GK0123\nSecret key: secret-value\n";
        assert_eq!(field(output, &["Key ID"]).as_deref(), Some("GK0123"));
        assert_eq!(
            field(output, &["Secret key"]).as_deref(),
            Some("secret-value")
        );
    }
}
