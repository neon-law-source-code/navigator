//! One Restate server per native worktree.
//!
//! Restate has no namespace boundary for registrations, so the server and its
//! journal are part of the worktree claim rather than the host-wide shared
//! service set. The configuration is written beside the journal so a second
//! worktree cannot inherit either the ports or the local state.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::supervisor::Service;

const FORMULA: &str = "restate-server";
const BINARY: &str = "restate-server";

pub(super) fn config_path(root: &Path) -> PathBuf {
    super::supervisor::service_dir(root, super::RESTATE_LABEL).join("restate.toml")
}

fn data_dir(root: &Path) -> PathBuf {
    root.join(".devx/restate/data")
}

fn config_toml(ingress: u16, admin: u16, fabric: u16) -> String {
    format!(
        "cluster-name = \"navigator-native-{fabric}\"\n\
         auto-provision = true\n\
         bind-address = \"127.0.0.1:{fabric}\"\n\
         advertised-address = \"http://127.0.0.1:{fabric}/\"\n\
         default-num-partitions = 1\n\
         [admin]\n\
         bind-address = \"127.0.0.1:{admin}\"\n\
         [ingress]\n\
         bind-address = \"127.0.0.1:{ingress}\"\n",
    )
}

pub(super) fn start_args(root: &Path) -> Vec<String> {
    let config = config_path(root).to_string_lossy().into_owned();
    let data = data_dir(root).to_string_lossy().into_owned();
    [
        "--config-file",
        config.as_str(),
        "--base-dir",
        data.as_str(),
        "--listen-mode",
        "tcp",
        "--default-num-partitions",
        "1",
        "--no-logo",
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect()
}

pub(super) fn service(root: &Path, ingress: u16, admin: u16, fabric: u16) -> Result<Service> {
    let dir = super::supervisor::service_dir(root, super::RESTATE_LABEL);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::create_dir_all(data_dir(root))
        .with_context(|| format!("create {}", data_dir(root).display()))?;
    let config = config_path(root);
    std::fs::write(&config, config_toml(ingress, admin, fabric))
        .with_context(|| format!("write {}", config.display()))?;
    Ok(Service {
        label: super::RESTATE_LABEL,
        program: super::preflight::binary(FORMULA, BINARY)?,
        args: start_args(root),
        env: Vec::new(),
        cwd: dir,
        port: ingress,
    })
}

#[cfg(test)]
mod tests {
    use super::{config_toml, start_args};
    use std::path::Path;

    #[test]
    fn config_separates_the_three_tcp_listeners_and_one_partition() {
        let config = config_toml(20_101, 20_201, 21_701);
        assert!(config.contains("bind-address = \"127.0.0.1:21701\""));
        assert!(config.contains("bind-address = \"127.0.0.1:20201\""));
        assert!(config.contains("bind-address = \"127.0.0.1:20101\""));
        assert!(config.contains("default-num-partitions = 1"));
    }

    #[test]
    fn start_recipe_forces_tcp_and_one_partition() {
        let args = start_args(Path::new("/tmp/native"));
        assert!(args.windows(2).any(|pair| pair == ["--listen-mode", "tcp"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--default-num-partitions", "1"]));
    }
}
