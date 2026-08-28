//! Cloud-provider abstractions for the Neon Law Navigator workspace.
//!
//! This is the one crate that depends on a cloud-provider SDK
//! (`google-cloud-storage`). Everything else in the workspace
//! depends on the [`StorageService`] trait and stays
//! provider-agnostic.
//!
//! Two backends ship behind the trait:
//!
//! - [`FsStorage`] (in [`fs`]) writes to a filesystem directory —
//!   used by local dev, the integration test rig, and small
//!   production deployments where a single PVC is enough.
//! - [`GcsStorage`] writes to Google Cloud Storage.
//! - [`S3Storage`] writes to the object store or any conforming S3-compatible service.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use thiserror::Error;

pub mod audio;
pub mod drive;
pub mod forge;
pub mod fs;
pub mod gcloud;
pub mod gcs;
pub mod redirect;
pub mod s3;
pub mod speech;
pub mod workspace;

pub use audio::{decode_to_mono_pcm16, AudioError, DecodedAudio};
pub use drive::{
    DriveError, DriveFolder, DriveMember, DriveMemberKind, DriveRole, DriveService, DriveWorkspace,
    DriveWorkspaceConfig, FakeDrive, GoogleDrive,
};
pub use forge::{
    FakeForge, ForgeError, ForgeRepository, ForgeService, GitHubForge, GITHUB_API_URL_ENV,
    GITHUB_TOKEN_ENV, NAVIGATOR_GITHUB_TOKEN_ENV,
};
pub use fs::FsStorage;
pub use gcs::{GcsStorage, GcsStorageConfig};
pub use s3::{S3Storage, S3StorageConfig};
pub use speech::{GoogleSpeechConfig, GoogleSpeechTranscriptProvider, SpeechError};
pub use workspace::{
    documents_prefix, is_navigator_repository, is_valid_slug, DeploymentWorkspace,
    DriveCoordinates, GoogleWorkspace, WorkspaceConfig, WorkspaceConfigError, WorkspaceCustomer,
    DEFAULT_GIT_HOST, NAVIGATOR_GCP_PROJECT_ID, NAVIGATOR_GITHUB_ORG, NAVIGATOR_GIT_HOST,
    NAVIGATOR_PROJECTS_DRIVE_MOUNT, NAVIGATOR_REPOSITORY_URL, PORTAL_MOUNT_SEGMENT,
    RESERVED_PROJECT_CODES, SLUG_MAX_LEN,
};

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io error on {key}: {source}")]
    Io {
        key: String,
        #[source]
        source: std::io::Error,
    },
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("missing required env var: {0}")]
    MissingEnv(&'static str),
    #[error("gcs error on {key}: {message}")]
    Gcs { key: String, message: String },
    #[error("s3 error on {key}: {message}")]
    S3 { key: String, message: String },
    /// The backend does not support this operation. Returned by
    /// [`FsStorage::signed_url`] — local filesystem objects don't
    /// have a network address to sign. Callers fall back to
    /// proxying the bytes through the app.
    #[error("operation not supported on this storage backend: {0}")]
    Unsupported(&'static str),
}

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub key: String,
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// One object returned by [`StorageService::list`] — its key and byte size,
/// without the bytes. Enough for the nightly Iceberg authoring to build a
/// manifest entry (path + `file_size_in_bytes`) per data file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectListing {
    pub key: String,
    pub size_bytes: u64,
}

#[async_trait]
pub trait StorageService: Send + Sync {
    async fn put(&self, key: &str, bytes: &[u8], content_type: &str) -> Result<(), StorageError>;

    /// Like [`put`](Self::put), but also stamps an HTTP `Cache-Control`
    /// directive on the stored object (e.g. `public, max-age=604800`).
    ///
    /// The default implementation ignores `cache_control` and delegates
    /// to [`put`](Self::put), so backends with no notion of HTTP cache
    /// metadata — [`FsStorage`], used by dev and tests — need no change.
    /// Only [`GcsStorage`] overrides it to set the header on the
    /// uploaded object, which is what lets the public assets bucket
    /// serve photos under a bounded TTL without a cache-bust token.
    async fn put_cached(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), StorageError> {
        let _ = cache_control;
        self.put(key, bytes, content_type).await
    }

    async fn get(&self, key: &str) -> Result<StoredObject, StorageError>;
    async fn delete(&self, key: &str) -> Result<(), StorageError>;

    /// List objects whose key starts with `prefix`, with their byte sizes.
    /// Used by the nightly Iceberg authoring to discover the day's Parquet
    /// data files under `iceberg/<table>/data/dt=<date>/`. Order is
    /// unspecified. The default returns [`StorageError::Unsupported`]; the
    /// real backends ([`FsStorage`], [`GcsStorage`]) override it.
    async fn list(&self, prefix: &str) -> Result<Vec<ObjectListing>, StorageError> {
        let _ = prefix;
        Err(StorageError::Unsupported("list"))
    }

    /// Whether an object exists at `key`, without downloading it.
    ///
    /// The default implementation does a full [`get`](Self::get) and maps
    /// [`StorageError::NotFound`] to `Ok(false)`; any other error
    /// propagates. Backends override it with a metadata-only HEAD when one
    /// is cheaper than a full fetch — [`GcsStorage`] does. Used as a cheap
    /// readiness probe before a downstream step reads the object (e.g.
    /// confirming the worker has rendered + persisted a notation's PDF
    /// before dispatching it for signature).
    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        match self.get(key).await {
            Ok(_) => Ok(true),
            Err(StorageError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Generate a time-limited URL that lets a client (typically a
    /// browser) fetch an object directly from the backend without
    /// proxying through the app. `expires_in` is the validity
    /// window; the caller picks a duration short enough that link
    /// sharing isn't a concern.
    ///
    /// Backends that have no concept of a signed URL (i.e.
    /// [`FsStorage`]) return [`StorageError::Unsupported`] so the
    /// caller knows to fall back to streaming the bytes.
    async fn signed_url(&self, key: &str, expires_in: Duration) -> Result<String, StorageError>;
}

/// Pick a backend from `NAVIGATOR_STORAGE_BACKEND`, which must be set.
///
/// The GCS bucket is the documents-preferred one
/// ([`GcsStorageConfig::from_env`]): `NAVIGATOR_DOCUMENTS_BUCKET` when set,
/// else `NAVIGATOR_STORAGE_BUCKET`. This is what `web` and the worker's
/// `generate_pdf__*` render lane use. The Archives snapshot lane wants the
/// exports bucket instead — see [`exports_from_env`].
pub async fn from_env() -> Result<Arc<dyn StorageService>, StorageError> {
    backend_from_env(GcsStorageConfig::from_env, S3StorageConfig::from_env).await
}

/// Like [`from_env`], but the GCS bucket comes from
/// `NAVIGATOR_STORAGE_BUCKET` ONLY ([`GcsStorageConfig::exports_from_env`]).
///
/// For the Archives exports lane on the `workflows-service` worker, which
/// also carries `NAVIGATOR_DOCUMENTS_BUCKET` for its document-render lane:
/// the two must resolve to different buckets on the same pod. The `fs`
/// backend is identical to [`from_env`] — dev/KIND keep one storage root.
pub async fn exports_from_env() -> Result<Arc<dyn StorageService>, StorageError> {
    backend_from_env(
        GcsStorageConfig::exports_from_env,
        S3StorageConfig::exports_from_env,
    )
    .await
}

/// Resolve the operational `SurrealDB` archive lane. Production names its
/// firm-retention bucket with `NAVIGATOR_SURREAL_ARCHIVES_BUCKET`; local KIND
/// falls back to the ordinary exports bucket so the restore drill is runnable
/// without a cloud credential.
pub async fn surreal_archives_from_env() -> Result<Arc<dyn StorageService>, StorageError> {
    backend_from_env(
        GcsStorageConfig::surreal_archives_from_env,
        S3StorageConfig::surreal_archives_from_env,
    )
    .await
}

/// Like [`from_env`], but the GCS bucket comes from
/// `NAVIGATOR_ASSETS_BUCKET` (falling back to `NAVIGATOR_STORAGE_BUCKET`
/// — see [`GcsStorageConfig::assets_from_env`]).
///
/// The public-assets lane: blank government forms live only in the
/// public `<project>-assets` bucket, and `web` pulls them through this
/// handle at fill time. The `fs` backend is identical to [`from_env`] —
/// dev/KIND keep one storage root.
pub async fn assets_from_env() -> Result<Arc<dyn StorageService>, StorageError> {
    backend_from_env(
        GcsStorageConfig::assets_from_env,
        S3StorageConfig::assets_from_env,
    )
    .await
}

/// Like [`assets_from_env`], but every environment lookup goes through
/// `get`. The `navigator forms` CLI uses this to honor a `--bucket`
/// override for the assets lane while still resolving the backend
/// (`NAVIGATOR_STORAGE_BACKEND`) and the lane's own bucket fallbacks the
/// same way `web` does — so `forms sync` writes to whatever object store
/// the deps actually run (S3/Garage locally, GCS in prod), not GCS alone.
pub async fn assets_from_lookup<F>(get: F) -> Result<Arc<dyn StorageService>, StorageError>
where
    F: Fn(&str) -> Option<String>,
{
    backend_from_lookup(
        &get,
        || GcsStorageConfig::assets_from_lookup(&get),
        || S3StorageConfig::assets_from_lookup(&get),
    )
    .await
}

/// Like [`from_env`], but the GCS bucket comes from
/// `NAVIGATOR_APPLICATIONS_BUCKET` (falling back to
/// `NAVIGATOR_STORAGE_BUCKET` — see
/// [`GcsStorageConfig::applications_from_env`]).
///
/// The Project-application lane: each Project's published portal bundle
/// lives in the private, per-deployment `<project>-applications` bucket,
/// and `web` streams it through this handle at
/// `/app/projects/{code}/portal`. The bytes are streamed, never redirected
/// to a signed URL, so the session cookie and participation gate stay on
/// every request. The `fs` backend is identical to [`from_env`] — dev/KIND
/// keep one storage root.
pub async fn applications_from_env() -> Result<Arc<dyn StorageService>, StorageError> {
    backend_from_env(
        GcsStorageConfig::applications_from_env,
        S3StorageConfig::applications_from_env,
    )
    .await
}

/// Resolve the dedicated Git LFS lane. S3 deployments require their own
/// bucket and credentials; filesystem and GCS retain their existing document
/// storage behavior.
pub async fn lfs_from_env() -> Result<Arc<dyn StorageService>, StorageError> {
    backend_from_env(GcsStorageConfig::from_env, S3StorageConfig::lfs_from_env).await
}

/// Shared backend selection, driven entirely by `NAVIGATOR_STORAGE_BACKEND`;
/// when it names GCS or S3 the bucket comes from the lane's config resolver.
async fn backend_from_env<G, S>(
    gcs_config: G,
    s3_config: S,
) -> Result<Arc<dyn StorageService>, StorageError>
where
    G: FnOnce() -> Result<GcsStorageConfig, StorageError>,
    S: FnOnce() -> Result<S3StorageConfig, StorageError>,
{
    backend_from_lookup(|key| std::env::var(key).ok(), gcs_config, s3_config).await
}

/// [`backend_from_env`] with the environment read through `get` instead
/// of `std::env` directly — the backend selector and `fs` root resolve
/// through the same lookup as the lane config, so a caller can override
/// any of them (e.g. the assets bucket from a CLI `--bucket` flag).
///
/// The selector fails closed. An unset `NAVIGATOR_STORAGE_BACKEND` is
/// [`StorageError::MissingEnv`], not an implicit `fs`: a process with no
/// storage configuration would otherwise write objects to a local
/// `./var/storage` that whatever reads them (the deployed `web`, a
/// networked Garage/GCS bucket) never looks at, turning a boot-time
/// misconfiguration into a far-away "missing data" symptom (#618). Local
/// filesystem storage stays available — a caller that wants it names `fs`.
async fn backend_from_lookup<F, G, S>(
    get: F,
    gcs_config: G,
    s3_config: S,
) -> Result<Arc<dyn StorageService>, StorageError>
where
    F: Fn(&str) -> Option<String>,
    G: FnOnce() -> Result<GcsStorageConfig, StorageError>,
    S: FnOnce() -> Result<S3StorageConfig, StorageError>,
{
    let backend = get("NAVIGATOR_STORAGE_BACKEND")
        .ok_or(StorageError::MissingEnv("NAVIGATOR_STORAGE_BACKEND"))?;
    validate_backend_name(&backend)?;
    match backend.as_str() {
        "gcs" | "google" => Ok(Arc::new(GcsStorage::new_from_config(gcs_config()?).await?)),
        "s3" => Ok(Arc::new(S3Storage::new(s3_config()?)?)),
        "fs" => {
            let root =
                get("NAVIGATOR_STORAGE_FS_ROOT").unwrap_or_else(|| "./var/storage".to_string());
            Ok(Arc::new(FsStorage::new(root).await?))
        }
        _ => unreachable!("backend name validated above"),
    }
}

/// Validate an explicit backend selector. Unknown values are errors rather
/// than an accidental fallback to local filesystem storage.
pub fn validate_backend_name(value: &str) -> Result<(), StorageError> {
    match value {
        "fs" | "gcs" | "google" | "s3" => Ok(()),
        other => Err(StorageError::S3 {
            key: "<backend>".into(),
            message: format!(
                "unknown NAVIGATOR_STORAGE_BACKEND `{other}` (expected fs, gcs/google, or s3)"
            ),
        }),
    }
}

/// A key that never exists — [`wait_until_ready`] probes it so the check
/// forces a real round-trip to the backend without depending on any object
/// having been written yet.
const READINESS_PROBE_KEY: &str = "__navigator_readiness_probe__";

/// Block until the object store answers a probe, or `timeout` elapses.
///
/// Boot-time guard. In KIND the `web` pod can start before
/// the object store is reachable, and the canonical seed writes template
/// bodies as blobs to the store — so without this, the first seed fails on
/// a connection error and the pod crash-loops (with a growing backoff)
/// until the dependency happens to come up. That is exactly what made the
/// KIND e2e flake: the suite runs seconds after bring-up, while `web` is
/// still in `CrashLoopBackOff`.
///
/// [`exists`](StorageService::exists) round-trips the backend and maps a
/// missing key to `Ok(false)`, so any `Ok` means the store is reachable; an
/// `Err` means not-yet, retried with a short fixed backoff until the
/// deadline, after which the last error propagates. The filesystem backend
/// answers instantly, so this is a no-op for local/`fs` dev.
pub async fn wait_until_ready(
    storage: &Arc<dyn StorageService>,
    timeout: Duration,
) -> Result<(), StorageError> {
    wait_until_ready_with(storage, timeout, Duration::from_millis(1500)).await
}

/// [`wait_until_ready`] with an explicit retry backoff, so tests can drive
/// the retry loop without real-time sleeps.
async fn wait_until_ready_with(
    storage: &Arc<dyn StorageService>,
    timeout: Duration,
    backoff: Duration,
) -> Result<(), StorageError> {
    let deadline = Instant::now() + timeout;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match storage.exists(READINESS_PROBE_KEY).await {
            Ok(_) => {
                if attempt > 1 {
                    tracing::info!(attempt, "object storage ready");
                }
                return Ok(());
            }
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
                tracing::warn!(attempt, error = %e, "object storage not ready yet, retrying");
                tokio::time::sleep(backoff).await;
            }
        }
    }
}

#[cfg(test)]
mod ready_tests {
    use super::{wait_until_ready_with, StorageError, StorageService, StoredObject};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// A storage whose readiness probe (`exists` → `get`) errors with a
    /// connection-like `Gcs` error for the first `fail_for` probes, then
    /// reports "object absent" (`NotFound` → `exists` returns `Ok(false)`).
    /// Models the object store coming up partway through web boot.
    struct FlakyStore {
        probes: AtomicU32,
        fail_for: u32,
    }

    #[async_trait::async_trait]
    impl StorageService for FlakyStore {
        async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
            let n = self.probes.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_for {
                Err(StorageError::Gcs {
                    key: key.to_string(),
                    message: "connection refused".to_string(),
                })
            } else {
                Err(StorageError::NotFound(key.to_string()))
            }
        }
        async fn put(&self, _: &str, _: &[u8], _: &str) -> Result<(), StorageError> {
            unimplemented!()
        }
        async fn delete(&self, _: &str) -> Result<(), StorageError> {
            unimplemented!()
        }
        async fn signed_url(&self, _: &str, _: Duration) -> Result<String, StorageError> {
            unimplemented!()
        }
    }

    fn store(fail_for: u32) -> Arc<dyn StorageService> {
        Arc::new(FlakyStore {
            probes: AtomicU32::new(0),
            fail_for,
        })
    }

    #[tokio::test]
    async fn returns_ok_once_the_store_answers() {
        // Errors twice, then ready — wait should ride out the retries.
        let s = store(2);
        let r = wait_until_ready_with(&s, Duration::from_secs(5), Duration::from_millis(1)).await;
        assert!(r.is_ok(), "expected ready after retries, got {r:?}");
    }

    #[tokio::test]
    async fn ready_on_first_probe_returns_immediately() {
        let s = store(0);
        assert!(
            wait_until_ready_with(&s, Duration::from_secs(5), Duration::from_millis(1))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn times_out_with_the_last_error_when_never_ready() {
        // Never answers — the probe key is irrelevant; we just need the
        // deadline to win and the connection error to propagate.
        let s = store(u32::MAX);
        let err = wait_until_ready_with(&s, Duration::from_millis(20), Duration::from_millis(1))
            .await
            .expect_err("never-ready store must time out");
        assert!(
            matches!(err, StorageError::Gcs { .. }),
            "expected the last connection error to propagate, got {err:?}"
        );
    }
}

#[cfg(test)]
mod backend_tests {
    use super::{
        applications_from_env, assets_from_env, assets_from_lookup, exports_from_env, from_env,
        lfs_from_env, validate_backend_name, S3StorageConfig, StorageError,
    };

    #[tokio::test]
    async fn assets_from_lookup_selects_the_backend_from_the_supplied_getter() {
        // A pure injected environment — no process-global mutation, so this
        // runs in parallel with every other test. This is the seam the
        // `navigator forms` CLI uses so `forms sync` writes to whatever
        // object store the deps run (S3/Garage locally), not GCS alone.
        let s3_env = |key: &str| match key {
            "NAVIGATOR_STORAGE_BACKEND" => Some("s3".to_string()),
            "NAVIGATOR_STORAGE_ENDPOINT" => Some("http://garage:3900".to_string()),
            "NAVIGATOR_STORAGE_ACCESS_KEY" => Some("access".to_string()),
            "NAVIGATOR_STORAGE_SECRET_KEY" => Some("secret".to_string()),
            "NAVIGATOR_ASSETS_BUCKET" => Some("navigator-assets".to_string()),
            _ => None,
        };
        // The S3/Garage assets lane builds (lazily connected). The pre-fix
        // GCS-only path could not reach this arm at all.
        assert!(assets_from_lookup(s3_env).await.is_ok());

        // `fs` with a root reaches the filesystem arm through the same getter.
        let root = tempfile::tempdir().unwrap();
        let fs_root = root.path().to_str().unwrap().to_string();
        let fs_env = move |key: &str| match key {
            "NAVIGATOR_STORAGE_BACKEND" => Some("fs".to_string()),
            "NAVIGATOR_STORAGE_FS_ROOT" => Some(fs_root.clone()),
            _ => None,
        };
        assert!(assets_from_lookup(fs_env).await.is_ok());

        // An unknown selector is a hard error, not a silent fs fallback.
        let typo_env = |key: &str| (key == "NAVIGATOR_STORAGE_BACKEND").then(|| "typo".to_string());
        assert!(matches!(
            assets_from_lookup(typo_env).await,
            Err(StorageError::S3 { .. })
        ));
    }

    /// The selector fails closed: with no `NAVIGATOR_STORAGE_BACKEND` there is
    /// no backend to pick, so opening storage errors by name instead of
    /// quietly writing to a local `./var/storage` nothing else reads. A
    /// process that means to use the filesystem says so (`fs`); one that
    /// forgot to configure storage learns at boot, not when a reader 404s
    /// every blob it was handed (#618).
    #[tokio::test]
    async fn an_unset_backend_selector_is_an_error_naming_the_variable() {
        // Injected environment — nothing is set, exactly as a runtime binary
        // started with no storage configuration sees it.
        let unset_env = |_: &str| None;
        let Err(error) = assets_from_lookup(unset_env).await else {
            panic!("an unset NAVIGATOR_STORAGE_BACKEND must not open a storage backend");
        };
        assert!(matches!(
            error,
            StorageError::MissingEnv("NAVIGATOR_STORAGE_BACKEND")
        ));
        assert!(error.to_string().contains("NAVIGATOR_STORAGE_BACKEND"));

        // An `fs` root alone does not revive the old implicit default: the
        // selector still has to be named.
        let root_only = |key: &str| {
            (key == "NAVIGATOR_STORAGE_FS_ROOT").then(|| "/tmp/navigator-storage".to_string())
        };
        assert!(matches!(
            assets_from_lookup(root_only).await,
            Err(StorageError::MissingEnv("NAVIGATOR_STORAGE_BACKEND"))
        ));
    }

    #[test]
    fn backend_selector_is_strict() {
        for value in ["fs", "gcs", "google", "s3"] {
            assert!(validate_backend_name(value).is_ok());
        }
        let error = validate_backend_name("typo").unwrap_err();
        assert!(matches!(error, StorageError::S3 { .. }));
        assert!(error
            .to_string()
            .contains("unknown NAVIGATOR_STORAGE_BACKEND"));
    }

    /// Every `NAVIGATOR_STORAGE_*` var this crate's selection code reads.
    /// Cleared before and after so this test leaves the process env pristine.
    const STORAGE_VARS: &[&str] = &[
        "NAVIGATOR_STORAGE_BACKEND",
        "NAVIGATOR_STORAGE_FS_ROOT",
        "NAVIGATOR_STORAGE_ENDPOINT",
        "NAVIGATOR_STORAGE_ACCESS_KEY",
        "NAVIGATOR_STORAGE_SECRET_KEY",
        "NAVIGATOR_STORAGE_BUCKET",
        "NAVIGATOR_DOCUMENTS_BUCKET",
        "NAVIGATOR_ASSETS_BUCKET",
        "NAVIGATOR_APPLICATIONS_BUCKET",
        "NAVIGATOR_LFS_BUCKET",
    ];

    fn clear_storage_vars() {
        for key in STORAGE_VARS {
            std::env::remove_var(key);
        }
    }

    /// Exercises the env-reading selection surface end to end. Kept as one
    /// test because it mutates process-global `NAVIGATOR_STORAGE_*` vars —
    /// no other cloud test reads them, so a single serial test needs no lock
    /// (and avoids holding one across `.await`).
    #[tokio::test]
    async fn backend_and_lane_selection_reads_process_environment() {
        clear_storage_vars();
        std::env::set_var("NAVIGATOR_STORAGE_ENDPOINT", "http://garage:3900");
        std::env::set_var("NAVIGATOR_STORAGE_ACCESS_KEY", "access");
        std::env::set_var("NAVIGATOR_STORAGE_SECRET_KEY", "secret");
        std::env::set_var("NAVIGATOR_STORAGE_BUCKET", "navigator-exports");
        std::env::set_var("NAVIGATOR_DOCUMENTS_BUCKET", "navigator-documents");
        std::env::set_var("NAVIGATOR_ASSETS_BUCKET", "navigator-assets");
        std::env::set_var("NAVIGATOR_APPLICATIONS_BUCKET", "navigator-applications");
        std::env::set_var("NAVIGATOR_LFS_BUCKET", "navigator-lfs");

        // Each lane's `*_from_env` wrapper resolves its own bucket.
        assert_eq!(
            S3StorageConfig::from_env().unwrap().bucket,
            "navigator-documents"
        );
        assert_eq!(
            S3StorageConfig::exports_from_env().unwrap().bucket,
            "navigator-exports"
        );
        assert_eq!(
            S3StorageConfig::assets_from_env().unwrap().bucket,
            "navigator-assets"
        );
        assert_eq!(
            S3StorageConfig::applications_from_env().unwrap().bucket,
            "navigator-applications"
        );
        assert_eq!(
            S3StorageConfig::lfs_from_env().unwrap().bucket,
            "navigator-lfs"
        );

        // With `s3` selected, every lane reaches the `s3` match arm and
        // builds a (lazily-connected) client without a live endpoint.
        std::env::set_var("NAVIGATOR_STORAGE_BACKEND", "s3");
        assert!(from_env().await.is_ok());
        assert!(exports_from_env().await.is_ok());
        assert!(assets_from_env().await.is_ok());
        assert!(applications_from_env().await.is_ok());
        assert!(lfs_from_env().await.is_ok());

        // Explicit `fs` with a root reaches the filesystem arm.
        let root = tempfile::tempdir().unwrap();
        std::env::set_var("NAVIGATOR_STORAGE_BACKEND", "fs");
        std::env::set_var("NAVIGATOR_STORAGE_FS_ROOT", root.path().to_str().unwrap());
        assert!(from_env().await.is_ok());

        // An unrecognized selector is a hard error, not a silent fs fallback.
        std::env::set_var("NAVIGATOR_STORAGE_BACKEND", "typo");
        assert!(matches!(from_env().await, Err(StorageError::S3 { .. })));

        // And so is no selector at all — every lane fails closed.
        std::env::remove_var("NAVIGATOR_STORAGE_BACKEND");
        for opened in [
            from_env().await,
            exports_from_env().await,
            assets_from_env().await,
            applications_from_env().await,
            lfs_from_env().await,
        ] {
            assert!(matches!(
                opened,
                Err(StorageError::MissingEnv("NAVIGATOR_STORAGE_BACKEND"))
            ));
        }

        clear_storage_vars();
    }
}
