//! Filesystem-backed [`StorageService`](crate::StorageService).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{StorageError, StorageService, StoredObject};

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Filesystem-backed storage. Object bytes live at `root/<key>.bin`;
/// content type at `root/<key>.ctype`.
#[derive(Clone)]
pub struct FsStorage {
    root: Arc<PathBuf>,
}

impl FsStorage {
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root: PathBuf = root.into();
        fs::create_dir_all(&root)
            .await
            .map_err(|e| StorageError::Io {
                key: root.display().to_string(),
                source: e,
            })?;
        Ok(Self {
            root: Arc::new(root),
        })
    }

    fn path_for(&self, key: &str, ext: &str) -> PathBuf {
        self.root.join(format!("{key}.{ext}"))
    }
}

#[async_trait]
impl StorageService for FsStorage {
    async fn put(&self, key: &str, bytes: &[u8], content_type: &str) -> Result<(), StorageError> {
        let bin = self.path_for(key, "bin");
        write_bytes(&bin, key, bytes).await?;
        let ctype = self.path_for(key, "ctype");
        write_bytes(&ctype, key, content_type.as_bytes()).await?;
        Ok(())
    }

    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        let bin = self.path_for(key, "bin");
        let bytes = read_bytes(&bin, key).await?;
        let ctype_path = self.path_for(key, "ctype");
        let content_type =
            String::from_utf8_lossy(&read_bytes(&ctype_path, key).await?).into_owned();
        Ok(StoredObject {
            key: key.to_string(),
            bytes,
            content_type,
        })
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        for ext in ["bin", "ctype"] {
            let p = self.path_for(key, ext);
            match fs::remove_file(&p).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(StorageError::Io {
                        key: key.to_string(),
                        source: e,
                    });
                }
            }
        }
        Ok(())
    }

    async fn signed_url(&self, _key: &str, _expires_in: Duration) -> Result<String, StorageError> {
        // FsStorage objects live on the local filesystem and have no
        // network address that a remote client could fetch. The
        // download handler is expected to match on `Unsupported` and
        // stream the bytes through the app instead.
        Err(StorageError::Unsupported("FsStorage has no signed URL"))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<crate::ObjectListing>, StorageError> {
        // Objects live at `root/<key>.bin`; recover the key by stripping the
        // root and the `.bin` extension, then keep those matching `prefix`.
        let root = Arc::clone(&self.root);
        let err_key = prefix.to_string();
        let prefix = prefix.to_string();
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for entry in walkdir::WalkDir::new(root.as_path())
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                    continue;
                }
                let Ok(rel) = path.strip_prefix(root.as_path()) else {
                    continue;
                };
                // `<key>.bin` -> `<key>`, with forward slashes for the key.
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                let Some(key) = rel_str.strip_suffix(".bin") else {
                    continue;
                };
                if !key.starts_with(&prefix) {
                    continue;
                }
                let size_bytes = entry.metadata().map_or(0, |m| m.len());
                out.push(crate::ObjectListing {
                    key: key.to_string(),
                    size_bytes,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| StorageError::Io {
            key: err_key,
            source: std::io::Error::other(e.to_string()),
        })?
    }
}

async fn write_bytes(path: &Path, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
    // Keys may carry `/` separators (e.g. `inbound/<timestamp>.eml`)
    // to mirror the prefix-based layout GCS uses in production.
    // Ensure the parent directory exists before creating the file —
    // GCS doesn't need it, but a posix filesystem does.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| StorageError::Io {
                key: key.to_string(),
                source: e,
            })?;
    }
    let tmp = path.with_extension(format!(
        "tmp-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos()),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut f = fs::File::create(&tmp).await.map_err(|e| StorageError::Io {
        key: key.to_string(),
        source: e,
    })?;
    f.write_all(bytes).await.map_err(|e| StorageError::Io {
        key: key.to_string(),
        source: e,
    })?;
    f.flush().await.map_err(|e| StorageError::Io {
        key: key.to_string(),
        source: e,
    })?;
    drop(f);
    fs::rename(&tmp, path).await.map_err(|e| StorageError::Io {
        key: key.to_string(),
        source: e,
    })?;
    Ok(())
}

async fn read_bytes(path: &Path, key: &str) -> Result<Vec<u8>, StorageError> {
    let mut f = match fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(StorageError::NotFound(key.to_string()));
        }
        Err(e) => {
            return Err(StorageError::Io {
                key: key.to_string(),
                source: e,
            });
        }
    };
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)
        .await
        .map_err(|e| StorageError::Io {
            key: key.to_string(),
            source: e,
        })?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::FsStorage;
    use crate::{StorageError, StorageService};
    use tempfile::TempDir;

    async fn fs() -> (FsStorage, TempDir) {
        let dir = TempDir::new().unwrap();
        let s = FsStorage::new(dir.path().to_path_buf()).await.unwrap();
        (s, dir)
    }

    #[tokio::test]
    async fn put_then_get_round_trips_bytes_and_content_type() {
        let (s, _dir) = fs().await;
        s.put("abc", b"hello", "text/plain").await.unwrap();
        let got = s.get("abc").await.unwrap();
        assert_eq!(got.bytes, b"hello");
        assert_eq!(got.content_type, "text/plain");
        assert_eq!(got.key, "abc");
    }

    #[tokio::test]
    async fn put_replaces_existing_object_with_complete_bytes() {
        let (s, _dir) = fs().await;
        s.put("abc", b"old body", "text/plain").await.unwrap();
        s.put("abc", b"new complete body", "text/markdown")
            .await
            .unwrap();

        let got = s.get("abc").await.unwrap();
        assert_eq!(got.bytes, b"new complete body");
        assert_eq!(got.content_type, "text/markdown");
    }

    #[tokio::test]
    async fn get_returns_not_found_for_missing_key() {
        let (s, _dir) = fs().await;
        match s.get("nope").await {
            Err(StorageError::NotFound(k)) => assert_eq!(k, "nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exists_reflects_presence() {
        // The default `exists` impl: `true` after a put, `false` for a
        // missing key, and `false` again after a delete.
        let (s, _dir) = fs().await;
        assert!(!s.exists("probe").await.unwrap());
        s.put("probe", b"x", "text/plain").await.unwrap();
        assert!(s.exists("probe").await.unwrap());
        s.delete("probe").await.unwrap();
        assert!(!s.exists("probe").await.unwrap());
    }

    #[tokio::test]
    async fn delete_is_idempotent() {
        let (s, _dir) = fs().await;
        s.put("d", b"x", "text/plain").await.unwrap();
        s.delete("d").await.unwrap();
        s.delete("d").await.unwrap();
        assert!(matches!(s.get("d").await, Err(StorageError::NotFound(_))));
    }

    #[tokio::test]
    async fn put_cached_falls_back_to_put_ignoring_cache_control() {
        // The Fs backend has no HTTP cache metadata; the default
        // `put_cached` impl must drop `cache_control` and store the
        // bytes + content type exactly as `put` would.
        let (s, _dir) = fs().await;
        s.put_cached(
            "img/a-400w.avif",
            b"avifbytes",
            "image/avif",
            "public, max-age=604800",
        )
        .await
        .unwrap();
        let got = s.get("img/a-400w.avif").await.unwrap();
        assert_eq!(got.bytes, b"avifbytes");
        assert_eq!(got.content_type, "image/avif");
    }

    #[tokio::test]
    async fn list_returns_keys_and_sizes_under_a_prefix() {
        let (storage, _tmp) = fs().await;
        storage
            .put(
                "telemetry/otel_logs/data/dt=2026-06-14/a.parquet",
                b"abc",
                "x",
            )
            .await
            .unwrap();
        storage
            .put(
                "telemetry/otel_logs/data/dt=2026-06-14/b.parquet",
                b"de",
                "x",
            )
            .await
            .unwrap();
        storage
            .put(
                "telemetry/otel_traces/data/dt=2026-06-14/c.parquet",
                b"f",
                "x",
            )
            .await
            .unwrap();

        let mut listed = storage
            .list("telemetry/otel_logs/data/dt=2026-06-14/")
            .await
            .unwrap();
        listed.sort_by(|a, b| a.key.cmp(&b.key));
        assert_eq!(
            listed.len(),
            2,
            "only the otel_logs prefix, not otel_traces"
        );
        assert_eq!(
            listed[0].key,
            "telemetry/otel_logs/data/dt=2026-06-14/a.parquet"
        );
        assert_eq!(listed[0].size_bytes, 3);
        assert_eq!(listed[1].size_bytes, 2);
    }

    #[tokio::test]
    async fn signed_url_is_unsupported_for_filesystem_backend() {
        let (s, _dir) = fs().await;
        match s.signed_url("any", super::Duration::from_mins(1)).await {
            Err(StorageError::Unsupported(_)) => {}
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
