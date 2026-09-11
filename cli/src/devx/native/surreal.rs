//! `SurrealDB` as a host process (#1093).
//!
//! The engine the cluster lane runs is a one-container Deployment with
//! `memory` as its storage argument and no volume — so the native
//! counterpart is genuinely just the same binary with the same argument,
//! bound to the host-wide native port instead of a Service.
//!
//! Memory-backed here too, and for the same reason it is memory-backed in
//! the cluster: local Surreal data resets with the process, which is a
//! decision the schema-apply step relies on rather than an oversight. The
//! schema is applied on every `up`, not migrated.

use std::path::Path;

use anyhow::{Context, Result};

use super::supervisor::Service;

/// The formula and executable `navigator dev install` acquires.
const FORMULA: &str = "surreal";
const BINARY: &str = "surreal";

/// Argv for the engine, bound to loopback on `port`.
///
/// Pure, so the flag spelling this lane depends on is asserted without a
/// running server. `--no-banner` keeps the process log to actual events;
/// the root credentials match [`super::super::surreal::host_config`],
/// which is what `store::surreal::connect` authenticates with.
fn start_args(port: u16) -> Vec<String> {
    [
        "start",
        "--no-banner",
        "--bind",
        &format!("127.0.0.1:{port}"),
        "--user",
        super::super::SURREAL_LOCAL_USER,
        "--pass",
        super::super::SURREAL_LOCAL_PASSWORD,
        "memory",
    ]
    .iter()
    .map(|argument| (*argument).to_string())
    .collect()
}

/// Prepare the engine's supervised service.
pub(super) fn service(root: &Path, port: u16) -> Result<Service> {
    Ok(Service {
        label: super::SURREAL_LABEL,
        program: super::preflight::binary(FORMULA, BINARY)?,
        args: start_args(port),
        env: Vec::new(),
        cwd: super::supervisor::service_dir(root, super::SURREAL_LABEL),
        port,
    })
}

/// Drop one worktree database from the shared in-memory engine.
pub(super) fn remove_database(cfg: &super::super::KindConfig, database: &str) -> Result<()> {
    if !database
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        anyhow::bail!("refusing to remove invalid SurrealDB database name {database:?}");
    }
    let config = super::super::surreal::host_config(cfg, database);
    runtime()?.block_on(async {
        let db = store::surreal::connect(&config)
            .await
            .with_context(|| format!("connect to SurrealDB at {}", config.endpoint))?;
        db.query(format!("REMOVE DATABASE `{database}`"))
            .await
            .with_context(|| format!("remove SurrealDB database {database}"))?;
        Ok(())
    })
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for native SurrealDB operations")
}

#[cfg(test)]
mod tests {
    use super::start_args;

    /// The fixed host port is shared by the native engine, and the loopback
    /// bind keeps it off the network. Both are in a single flag, so the flag
    /// is asserted.
    #[test]
    fn the_engine_binds_loopback_on_the_host_port() {
        let args = start_args(21_259);

        let bind = args
            .iter()
            .position(|argument| argument == "--bind")
            .expect("the start recipe binds an address");
        assert_eq!(args[bind + 1], "127.0.0.1:21259");
    }

    /// `memory` is the same storage argument
    /// `k8s/overlays/kind/surreal/surreal.yaml` passes. A native tier
    /// that quietly acquired a persistent store would diverge from the
    /// cluster lane on exactly the behavior — data resetting with the
    /// process — that the schema-apply step is written against.
    #[test]
    fn the_native_engine_is_memory_backed_like_the_cluster_one() {
        let args = start_args(8000);

        assert_eq!(
            args.last().map(String::as_str),
            Some("memory"),
            "the storage path is the trailing positional argument: {args:?}"
        );
    }

    /// `store::surreal::connect` authenticates as root/root against both
    /// lanes. A recipe that started the engine with different credentials
    /// would come up healthy and then refuse every query.
    #[test]
    fn the_root_credentials_match_the_ones_the_host_connects_with() {
        let args = start_args(8000);

        let user = args
            .iter()
            .position(|argument| argument == "--user")
            .expect("the start recipe names a root user");
        let password = args
            .iter()
            .position(|argument| argument == "--pass")
            .expect("the start recipe names a root password");
        assert_eq!(args[user + 1], super::super::super::SURREAL_LOCAL_USER);
        assert_eq!(
            args[password + 1],
            super::super::super::SURREAL_LOCAL_PASSWORD
        );
    }
}
