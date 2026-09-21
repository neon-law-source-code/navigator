//! Operational `SurrealDB` exports and restore drills.
//!
//! This lane is deliberately separate from `archives`: the latter writes
//! analytical Parquet snapshots, while this module emits a `SurrealQL` export a
//! responder can import to recover the operational store.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use futures::StreamExt;
use store::surreal::{SurrealConfig, SurrealDb};
use uuid::Uuid;

const ARCHIVE_PREFIX: &str = "surreal-backups";

/// Write an operational `SurrealQL` export to the configured archive lane.
pub fn export() -> Result<()> {
    runtime().block_on(async {
        let config = SurrealConfig::from_env().context("read SurrealDB coordinates")?;
        let db = connect_http(&config).await?;
        let bytes = export_bytes(&db).await?;
        let key = archive_key(&config);
        let storage = cloud::surreal_archives_from_env()
            .await
            .context("open Surreal archive storage")?;
        storage
            .put(&key, &bytes, "application/surrealql")
            .await
            .with_context(|| format!("upload SurrealDB export {key}"))?;

        println!("surreal archive written: {key} ({} bytes)", bytes.len());
        Ok(())
    })
}

/// Restore one export into a one-off namespace and prove it is recoverable.
pub fn restore_drill(key: &str) -> Result<()> {
    runtime().block_on(async {
        let config = SurrealConfig::from_env().context("read SurrealDB coordinates")?;
        let source = connect_http(&config).await?;
        let expected = table_counts(&source).await?;
        let storage = cloud::surreal_archives_from_env()
            .await
            .context("open Surreal archive storage")?;
        let export = storage
            .get(key)
            .await
            .with_context(|| format!("download SurrealDB export {key}"))?;

        let file = tempfile::NamedTempFile::new().context("create restore-drill file")?;
        std::fs::write(file.path(), &export.bytes).context("write downloaded SurrealDB export")?;

        let scratch_namespace = format!("restore_{}", Uuid::now_v7().simple());
        let scratch_config = SurrealConfig {
            namespace: scratch_namespace.clone(),
            ..config
        };
        let scratch = connect_http(&scratch_config).await?;
        let result = restore_and_verify(&scratch, file.path(), &expected).await;
        let cleanup = remove_namespace(&scratch, &scratch_namespace).await;
        result?;
        cleanup?;

        println!("surreal restore drill passed: {key}");
        Ok(())
    })
}

async fn restore_and_verify(
    scratch: &SurrealDb,
    file: &Path,
    expected: &BTreeMap<String, i64>,
) -> Result<()> {
    scratch
        .import(file)
        .await
        .context("import SurrealDB export")?;
    store::schema::apply(scratch)
        .await
        .context("apply Navigator schema to restored namespace")?;
    let version = store::schema::installed_version(scratch)
        .await
        .context("read restored schema_version:current")?;
    if version != Some(store::schema::SCHEMA_VERSION) {
        bail!(
            "restored schema_version:current is {version:?}, expected {}",
            store::schema::SCHEMA_VERSION
        );
    }

    let actual = table_counts(scratch).await?;
    if actual != *expected {
        bail!("restored row counts differ: source={expected:?}, restored={actual:?}");
    }
    Ok(())
}

async fn export_bytes(db: &SurrealDb) -> Result<Vec<u8>> {
    let mut stream = db.export(()).await.context("start SurrealDB export")?;
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.context("read SurrealDB export stream")?);
    }
    if bytes.is_empty() {
        bail!("SurrealDB export returned no bytes");
    }
    Ok(bytes)
}

async fn table_counts(db: &SurrealDb) -> Result<BTreeMap<String, i64>> {
    let mut counts = BTreeMap::new();
    for table in navigator_tables() {
        let mut response = db
            .query("SELECT VALUE count() FROM type::table($table)")
            .bind(("table", table.clone()))
            .await
            .with_context(|| format!("count rows in {table}"))?;
        let count: Option<i64> = response
            .take(0)
            .with_context(|| format!("read row count for {table}"))?;
        counts.insert(table, count.unwrap_or_default());
    }
    Ok(counts)
}

fn navigator_tables() -> Vec<String> {
    store::schema::table_names()
}

async fn remove_namespace(db: &SurrealDb, namespace: &str) -> Result<()> {
    db.query(format!("REMOVE NAMESPACE {namespace}"))
        .await
        .with_context(|| format!("remove restore-drill namespace {namespace}"))?;
    Ok(())
}

async fn connect_http(config: &SurrealConfig) -> Result<SurrealDb> {
    let endpoint = http_endpoint(&config.endpoint)?;
    store::surreal::connect(&SurrealConfig {
        endpoint,
        ..config.clone()
    })
    .await
    .context("connect to SurrealDB's HTTP backup endpoint")
}

fn http_endpoint(endpoint: &str) -> Result<String> {
    if let Some(rest) = endpoint.strip_prefix("ws://") {
        return Ok(format!("http://{rest}"));
    }
    if let Some(rest) = endpoint.strip_prefix("wss://") {
        return Ok(format!("https://{rest}"));
    }
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return Ok(endpoint.to_string());
    }
    bail!(
        "SurrealDB archive needs a ws://, wss://, http://, or https:// endpoint; got {endpoint:?}"
    )
}

fn archive_key(config: &SurrealConfig) -> String {
    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%SZ");
    format!(
        "{ARCHIVE_PREFIX}/{}/{}/{}-{}.surql",
        key_component(&config.namespace),
        key_component(&config.database),
        timestamp,
        Uuid::now_v7().simple()
    )
}

fn key_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Runtime::new().expect("create Surreal archive runtime")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{archive_key, http_endpoint, key_component, navigator_tables};
    use store::surreal::{SurrealAuth, SurrealConfig};

    #[test]
    fn backup_uses_the_http_surface_for_every_remote_scheme() {
        assert_eq!(
            http_endpoint("ws://localhost:8000").unwrap(),
            "http://localhost:8000"
        );
        assert_eq!(
            http_endpoint("wss://example.surreal.cloud").unwrap(),
            "https://example.surreal.cloud"
        );
        assert_eq!(
            http_endpoint("http://localhost:8000").unwrap(),
            "http://localhost:8000"
        );
        assert_eq!(
            http_endpoint("https://example.surreal.cloud").unwrap(),
            "https://example.surreal.cloud"
        );
        assert!(http_endpoint("mem://").is_err());
        assert!(http_endpoint("file:///tmp/surreal").is_err());
    }

    #[test]
    fn object_keys_are_timestamped_and_safe_to_select_by_point_in_time() {
        let config = SurrealConfig {
            endpoint: "ws://localhost:8000".into(),
            namespace: "firm/archive".into(),
            database: "navigator db".into(),
            auth: SurrealAuth::default(),
        };
        let key = archive_key(&config);
        assert!(key.starts_with("surreal-backups/firm_archive/navigator_db/"));
        assert!(Path::new(&key)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("surql")));
        assert_eq!(key_component("firm/archive"), "firm_archive");
    }

    #[test]
    fn row_count_reconciliation_uses_the_shipped_schema_tables() {
        let tables = navigator_tables();
        assert!(tables.contains(&"schema_version".to_string()));
        assert!(tables.contains(&"person".to_string()));
    }
}
