//! Build/restart the local Axum application and refresh browsers after source changes.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use anyhow::{bail, Context, Result};
use tokio::process::{Child, Command};

type Snapshot = BTreeMap<PathBuf, (SystemTime, u64)>;

fn snapshot(root: &Path) -> Result<(Snapshot, Snapshot)> {
    let mut source = BTreeMap::new();
    let mut assets = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !matches!(
                    entry.file_name().to_str(),
                    Some("target" | ".git" | ".devx" | ".worktrees" | "node_modules")
                )
        })
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path().strip_prefix(root)?.to_path_buf();
        let extension = path.extension().and_then(|value| value.to_str());
        let destination = if path.starts_with("server/public") {
            &mut assets
        } else if matches!(
            extension,
            Some("rs" | "toml" | "lock" | "yaml" | "yml" | "md" | "surql")
        ) {
            &mut source
        } else {
            continue;
        };
        let metadata = entry.metadata()?;
        destination.insert(path, (metadata.modified()?, metadata.len()));
    }
    Ok((source, assets))
}

async fn build(root: &Path) -> Result<bool> {
    Ok(Command::new("cargo")
        .current_dir(root)
        .args(["build", "-q", "-p", "neon"])
        .kill_on_drop(true)
        .status()
        .await
        .context("building neon")?
        .success())
}

async fn start(root: &Path, url: &str) -> Result<Child> {
    let target =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let binary = target
        .join("debug")
        .join(format!("neon{}", std::env::consts::EXE_SUFFIX));
    let mut child = Command::new(binary)
        .current_dir(root)
        .env("NAVIGATOR_DEV_RELOAD", "1")
        .kill_on_drop(true)
        .spawn()
        .context("starting neon")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    // First launch may include host executable verification before main starts.
    for _ in 0..600 {
        if let Some(status) = child.try_wait()? {
            bail!("neon exited during startup: {status}");
        }
        if client
            .get(format!("{url}/app/readyz"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            revision(root)?;
            return Ok(child);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("neon did not become ready at {url}")
}

fn revision(root: &Path) -> Result<()> {
    let value = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos();
    std::fs::write(root.join(".devx/revision"), value.to_string())?;
    Ok(())
}

async fn watch(root: &Path, url: &str) -> Result<()> {
    if !build(root).await? {
        bail!("initial neon build failed");
    }
    let mut current = snapshot(root)?;
    let mut server = start(root, url).await?;
    eprintln!(
        "Preview: {url} — watching Rust, catalogs, and static assets. Ctrl-C stops the server."
    );
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Some(status) = server.try_wait()? {
            bail!("neon exited: {status}");
        }
        let changed = snapshot(root)?;
        if changed == current {
            continue;
        }
        // Wait for a quiet interval so an editor's multi-file save builds once.
        tokio::time::sleep(Duration::from_millis(350)).await;
        let changed = snapshot(root)?;
        if changed.0 == current.0 {
            revision(root)?;
        } else if build(root).await? {
            server
                .kill()
                .await
                .context("stopping the previous preview")?;
            server = start(root, url).await?;
        } else {
            eprintln!("Build failed; the previous preview remains available. Save a fix to retry.");
        }
        // A save during compilation remains visible on the next iteration.
        current = changed;
    }
}

pub(super) fn run() -> Result<()> {
    let root = super::orchestrate::workspace_root()?;
    if !root.join(".devx/env").is_file() {
        bail!("Start this worktree with navigator dev worktree-env up first.");
    }
    dotenvy::from_path(root.join(".devx/env")).context("loading the worktree environment")?;
    if std::env::var("NAVIGATOR_ENVIRONMENT").as_deref() != Ok("dev") {
        bail!("dev serve requires NAVIGATOR_ENVIRONMENT=dev");
    }
    let url = std::env::var("NAV_BASE_URL").unwrap_or_else(|_| {
        format!(
            "http://localhost:{}",
            std::env::var("PORT").unwrap_or_else(|_| "3001".to_string())
        )
    });
    let parsed = reqwest::Url::parse(&url)?;
    if !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1")) {
        bail!("dev serve requires a loopback NAV_BASE_URL");
    }
    tokio::runtime::Runtime::new()?.block_on(async {
        tokio::select! {
            result = watch(&root, url.trim_end_matches('/')) => result,
            () = portal::hosting::shutdown_signal() => Ok(()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_separate_assets_and_detect_deletes_without_watching_build_outputs() {
        let dir = tempfile::tempdir().unwrap();
        for path in ["neon/src", "server/public/css", "target/debug", ".devx"] {
            std::fs::create_dir_all(dir.path().join(path)).unwrap();
        }
        for path in [
            "neon/src/main.rs",
            "server/public/css/home.css",
            "target/debug/output.rs",
            ".devx/revision",
        ] {
            std::fs::write(dir.path().join(path), "fixture").unwrap();
        }
        let (source, assets) = snapshot(dir.path()).unwrap();
        assert_eq!(source.len(), 1);
        assert_eq!(assets.len(), 1);
        std::fs::remove_file(dir.path().join("neon/src/main.rs")).unwrap();
        assert!(snapshot(dir.path()).unwrap().0.is_empty());
    }
}
