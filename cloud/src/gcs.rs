//! Google Cloud Storage backend for [`StorageService`](crate::StorageService).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use google_cloud_storage::client::{Client as GcsClient, ClientConfig};
use google_cloud_storage::http::objects::delete::DeleteObjectRequest;
use google_cloud_storage::http::objects::download::Range;
use google_cloud_storage::http::objects::get::GetObjectRequest;
use google_cloud_storage::http::objects::list::ListObjectsRequest;
use google_cloud_storage::http::objects::patch::PatchObjectRequest;
use google_cloud_storage::http::objects::upload::{Media, UploadObjectRequest, UploadType};
use google_cloud_storage::http::objects::Object;
use google_cloud_storage::http::Error as GcsHttpError;
use google_cloud_storage::sign::SignedURLOptions;

use crate::{StorageError, StorageService, StoredObject};

/// Configuration for the GCS backend. The bucket name is resolved from
/// `NAVIGATOR_DOCUMENTS_BUCKET` (preferred) falling back to
/// `NAVIGATOR_STORAGE_BUCKET`; the endpoint override is
/// `NAVIGATOR_STORAGE_ENDPOINT`. ADC-based auth picks up
/// `GOOGLE_APPLICATION_CREDENTIALS` (the GCP convention) automatically.
#[derive(Debug, Clone)]
pub struct GcsStorageConfig {
    pub bucket: String,
    /// Override endpoint for emulators (a Rust-local test endpoint). `None`
    /// uses the real GCS endpoint and ADC auth.
    pub endpoint: Option<String>,
}

impl GcsStorageConfig {
    pub fn from_env() -> Result<Self, StorageError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Exports-lane variant: resolves the bucket from
    /// `NAVIGATOR_STORAGE_BUCKET` ONLY, never `NAVIGATOR_DOCUMENTS_BUCKET`.
    ///
    /// The Archives snapshot workflow writes to the dedicated exports
    /// bucket and must stay there even on a pod that also carries
    /// `NAVIGATOR_DOCUMENTS_BUCKET` for its document-render lane (the
    /// `workflows-service` worker does both). Using [`from_env`] there
    /// would silently follow the documents-bucket preference and land
    /// nightly Parquet in the documents bucket.
    pub fn exports_from_env() -> Result<Self, StorageError> {
        Self::exports_from_lookup(|k| std::env::var(k).ok())
    }

    /// Operational-Surreal-archive variant. Every deployment names the
    /// firm-controlled retention bucket explicitly.
    pub fn surreal_archives_from_env() -> Result<Self, StorageError> {
        Self::surreal_archives_from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, StorageError> {
        // Bucket name resolution has a precedence chain so a single
        // workload can name its bucket specifically without disturbing
        // the others that share `cloud::from_env()`:
        //
        // 1. `NAVIGATOR_DOCUMENTS_BUCKET` — the private documents bucket
        //    `web` (and the worker's `generate_pdf__*` render lane) write
        //    client documents + `blobs/<sha>` to. Set on the `web` pod and
        //    the `workflows-service` worker.
        // 2. `NAVIGATOR_STORAGE_BUCKET` — the generic fallback. The
        //    `archives` exports lane (via `exports_from_env`) points it at
        //    the exports bucket; tests may point it at a Rust-local endpoint.
        //
        // The split keeps client documents out of the public `-assets`
        // bucket: the documents var gives `web` + the worker's render lane
        // their own private bucket, and the fallback serves every other
        // caller.
        let bucket = get("NAVIGATOR_DOCUMENTS_BUCKET")
            .or_else(|| get("NAVIGATOR_STORAGE_BUCKET"))
            .ok_or(StorageError::MissingEnv(
                "NAVIGATOR_DOCUMENTS_BUCKET or NAVIGATOR_STORAGE_BUCKET",
            ))?;
        Ok(Self {
            bucket,
            endpoint: Self::endpoint(&get),
        })
    }

    /// Resolve the bucket from `NAVIGATOR_STORAGE_BUCKET` only — the
    /// exports lane. See [`exports_from_env`](Self::exports_from_env).
    pub fn exports_from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, StorageError> {
        let bucket = get("NAVIGATOR_STORAGE_BUCKET")
            .ok_or(StorageError::MissingEnv("NAVIGATOR_STORAGE_BUCKET"))?;
        Ok(Self {
            bucket,
            endpoint: Self::endpoint(&get),
        })
    }

    pub fn surreal_archives_from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, StorageError> {
        let bucket = get("NAVIGATOR_SURREAL_ARCHIVES_BUCKET")
            .filter(|value| !value.is_empty())
            .ok_or(StorageError::MissingEnv(
                "NAVIGATOR_SURREAL_ARCHIVES_BUCKET",
            ))?;
        Ok(Self {
            bucket,
            endpoint: Self::endpoint(&get),
        })
    }

    /// Assets-lane variant: resolves the bucket from
    /// `NAVIGATOR_ASSETS_BUCKET` (the public `<project>-assets` bucket)
    /// falling back to `NAVIGATOR_STORAGE_BUCKET` — the single-bucket
    /// test topology, where one bucket may carry every lane.
    /// `NAVIGATOR_DOCUMENTS_BUCKET` is deliberately NOT in
    /// this chain: the private documents bucket must never shadow the
    /// public assets one.
    pub fn assets_from_env() -> Result<Self, StorageError> {
        Self::assets_from_lookup(|k| std::env::var(k).ok())
    }

    /// See [`assets_from_env`](Self::assets_from_env).
    pub fn assets_from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, StorageError> {
        let bucket = get("NAVIGATOR_ASSETS_BUCKET")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| get("NAVIGATOR_STORAGE_BUCKET"))
            .ok_or(StorageError::MissingEnv(
                "NAVIGATOR_ASSETS_BUCKET or NAVIGATOR_STORAGE_BUCKET",
            ))?;
        Ok(Self {
            bucket,
            endpoint: Self::endpoint(&get),
        })
    }

    /// Applications-lane variant: resolves the bucket from
    /// `NAVIGATOR_APPLICATIONS_BUCKET` (the private, per-deployment
    /// `<project>-applications` bucket) falling back to
    /// `NAVIGATOR_STORAGE_BUCKET` — the single-bucket test/KIND topology.
    ///
    /// The bundle lane is private, but `web` streams its bytes rather than
    /// signing a URL, so this handle stays a plain read lane like the
    /// others. `NAVIGATOR_DOCUMENTS_BUCKET` is deliberately NOT in this
    /// chain: a Project's published portal must never shadow the private
    /// matter-documents bucket.
    pub fn applications_from_env() -> Result<Self, StorageError> {
        Self::applications_from_lookup(|k| std::env::var(k).ok())
    }

    /// See [`applications_from_env`](Self::applications_from_env).
    pub fn applications_from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, StorageError> {
        let bucket = get("NAVIGATOR_APPLICATIONS_BUCKET")
            .filter(|s| !s.trim().is_empty())
            .or_else(|| get("NAVIGATOR_STORAGE_BUCKET"))
            .ok_or(StorageError::MissingEnv(
                "NAVIGATOR_APPLICATIONS_BUCKET or NAVIGATOR_STORAGE_BUCKET",
            ))?;
        Ok(Self {
            bucket,
            endpoint: Self::endpoint(&get),
        })
    }

    /// The emulator endpoint override, treating an empty string as unset.
    ///
    /// A Kubernetes env var declared with no `value:` arrives as `""`, and
    /// `std::env::var` returns `Ok("")` for it — not absent. If we kept
    /// that as `Some("")`, `new_from_config` would take the emulator
    /// branch with a host-less `storage_endpoint`, and every GCS request
    /// would fail to build a URL ("builder error") before reaching the
    /// network. Only a real, non-empty test endpoint selects that branch.
    fn endpoint<F: Fn(&str) -> Option<String>>(get: &F) -> Option<String> {
        get("NAVIGATOR_STORAGE_ENDPOINT").filter(|s| !s.is_empty())
    }
}

/// Google Cloud Storage backend.
#[derive(Clone)]
pub struct GcsStorage {
    client: Arc<GcsClient>,
    bucket: Arc<String>,
    /// True when an endpoint override (emulator) is configured. The
    /// anonymous emulator client has no signing identity, so
    /// [`StorageService::signed_url`] reports `Unsupported` and callers
    /// fall back to streaming the bytes through the app.
    emulator: bool,
}

/// Which identity the GCS client presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GcsCredentials {
    /// An endpoint override is configured (emulator): no auth at all, and
    /// no signing identity.
    Anonymous,
    /// Application Default Credentials, discovered by `google-cloud-auth`.
    Adc,
    /// `gcloud auth print-access-token`, because `google-cloud-auth`
    /// refused the ambient credential shape. See [`crate::gcloud`].
    GcloudCli,
}

impl GcsStorage {
    pub async fn new_from_config(cfg: GcsStorageConfig) -> Result<Self, StorageError> {
        // If an endpoint override is set (emulator), skip auth entirely —
        // this branch must never touch ADC discovery or the metadata
        // server. Otherwise authenticate, ADC first.
        let (client_config, credentials) = if let Some(endpoint) = cfg.endpoint.clone() {
            (
                ClientConfig {
                    storage_endpoint: endpoint,
                    ..ClientConfig::default()
                }
                .anonymous(),
                GcsCredentials::Anonymous,
            )
        } else {
            let adc = ClientConfig::default()
                .with_auth()
                .await
                .map_err(|e| e.to_string());
            authenticated_config(adc, crate::gcloud::probe)?
        };

        Ok(Self {
            client: Arc::new(GcsClient::new(client_config)),
            bucket: Arc::new(cfg.bucket),
            emulator: credentials == GcsCredentials::Anonymous,
        })
    }
}

/// Pick the authenticated client config: ADC first, the `gcloud` CLI second.
///
/// `google-cloud-auth` rejects the `external_account` credential file that
/// Workload Identity Federation writes, which is how every
/// `publish-cli-archives` job in release 26.8.12 failed with `unsupported
/// account external_account` — in a job that had already authenticated
/// `gcloud` from that same file. When ADC refuses, ask `gcloud` for the
/// bearer it is holding instead of failing the command.
///
/// Both inputs are parameters rather than calls so the selection is
/// testable without ambient GCP credentials: `adc` is the outcome of
/// `ClientConfig::with_auth`, and `gcloud_probe` reports whether the CLI
/// can mint a token right now.
fn authenticated_config(
    adc: Result<ClientConfig, String>,
    gcloud_probe: impl FnOnce() -> anyhow::Result<()>,
) -> Result<(ClientConfig, GcsCredentials), StorageError> {
    let adc_error = match adc {
        Ok(config) => return Ok((config, GcsCredentials::Adc)),
        Err(adc_error) => adc_error,
    };
    // Probe rather than trust: an absent or logged-out `gcloud` must
    // surface here, with the ADC error still attached, instead of as an
    // opaque 401 on the first upload.
    gcloud_probe().map_err(|gcloud_error| StorageError::Gcs {
        key: "<auth>".into(),
        message: format!(
            "{adc_error}; `gcloud auth print-access-token` is not usable either: {gcloud_error:#}"
        ),
    })?;
    eprintln!(
        "==> Application Default Credentials unavailable ({adc_error}); \
         using `gcloud auth print-access-token` instead"
    );
    Ok((
        ClientConfig {
            // A token-only identity: `default_sign_by` and
            // `default_google_access_id` stay unset, so this client can call
            // the JSON API but cannot locally sign a V4 URL. Every caller on
            // this path is an upload, and the alternative here is no client at
            // all.
            token_source_provider: Some(Box::new(crate::gcloud::GcloudTokenSourceProvider)),
            ..ClientConfig::default()
        },
        GcsCredentials::GcloudCli,
    ))
}

#[async_trait]
impl StorageService for GcsStorage {
    async fn put(&self, key: &str, bytes: &[u8], content_type: &str) -> Result<(), StorageError> {
        let mut media = Media::new(key.to_string());
        media.content_type = content_type.to_string().into();
        self.client
            .upload_object(
                &UploadObjectRequest {
                    bucket: (*self.bucket).clone(),
                    ..Default::default()
                },
                bytes.to_vec(),
                &UploadType::Simple(media),
            )
            .await
            .map_err(|e| StorageError::Gcs {
                key: key.to_string(),
                message: e.to_string(),
            })?;
        Ok(())
    }

    async fn put_cached(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: &str,
        cache_control: &str,
    ) -> Result<(), StorageError> {
        // A `Simple` upload (`Media`) carries no place for the
        // `Cache-Control` header. The crate's `Multipart` upload would,
        // but in google-cloud-storage 0.24 it sends the request as
        // `multipart/form-data` (reqwest's default), which the GCS JSON
        // upload API rejects — verified against a real bucket. So upload
        // the bytes via the proven simple path, then PATCH the object's
        // metadata to set `Cache-Control`. The brief window where the
        // object carries GCS's default cache directive is harmless for a
        // deploy that re-uploads the whole tree.
        self.put(key, bytes, content_type).await?;
        match self
            .client
            .patch_object(&PatchObjectRequest {
                bucket: (*self.bucket).clone(),
                object: key.to_string(),
                metadata: Some(Object {
                    cache_control: Some(cache_control.to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
        {
            Ok(_) => Ok(()),
            // The crate decodes the PATCH body only after a 2xx status
            // check, so a decode failure means the server accepted the
            // request — only the response shape was unexpected.
            // a test endpoint answers with its internal object shape
            // (no `etag`, `selfLink`, …), which the typed `Object`
            // decode rejects; real GCS returns a full `storage#object`
            // and never takes this branch. "Accepted" is not proof the
            // directive landed, so read the metadata back (GET
            // responses decode against both servers) and require the
            // requested `Cache-Control` before reporting success.
            Err(GcsHttpError::HttpClient(e)) if e.is_decode() => {
                let object = self
                    .client
                    .get_object(&GetObjectRequest {
                        bucket: (*self.bucket).clone(),
                        object: key.to_string(),
                        ..Default::default()
                    })
                    .await
                    .map_err(|e| map_gcs_error(&e, key))?;
                if object.cache_control.as_deref() == Some(cache_control) {
                    Ok(())
                } else {
                    Err(StorageError::Gcs {
                        key: key.to_string(),
                        message: format!(
                            "metadata PATCH returned 2xx with an undecodable body and the \
                             read-back shows cache-control {:?}, not the requested {cache_control:?}",
                            object.cache_control
                        ),
                    })
                }
            }
            Err(e) => Err(StorageError::Gcs {
                key: key.to_string(),
                message: e.to_string(),
            }),
        }
    }

    async fn get(&self, key: &str) -> Result<StoredObject, StorageError> {
        let metadata = self
            .client
            .get_object(&GetObjectRequest {
                bucket: (*self.bucket).clone(),
                object: key.to_string(),
                ..Default::default()
            })
            .await
            .map_err(|e| map_gcs_error(&e, key))?;
        let bytes = self
            .client
            .download_object(
                &GetObjectRequest {
                    bucket: (*self.bucket).clone(),
                    object: key.to_string(),
                    ..Default::default()
                },
                &Range::default(),
            )
            .await
            .map_err(|e| map_gcs_error(&e, key))?;
        Ok(StoredObject {
            key: key.to_string(),
            bytes,
            content_type: metadata
                .content_type
                .unwrap_or_else(|| "application/octet-stream".into()),
        })
    }

    async fn exists(&self, key: &str) -> Result<bool, StorageError> {
        // Metadata-only HEAD: `get_object` fetches just the object's
        // metadata (no `download_object`), so the readiness probe never
        // streams the PDF bytes back. A `NotFound` is the negative answer;
        // any other error propagates.
        match self
            .client
            .get_object(&GetObjectRequest {
                bucket: (*self.bucket).clone(),
                object: key.to_string(),
                ..Default::default()
            })
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => match map_gcs_error(&e, key) {
                StorageError::NotFound(_) => Ok(false),
                other => Err(other),
            },
        }
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.client
            .delete_object(&DeleteObjectRequest {
                bucket: (*self.bucket).clone(),
                object: key.to_string(),
                ..Default::default()
            })
            .await
            .map_err(|e| StorageError::Gcs {
                key: key.to_string(),
                message: e.to_string(),
            })?;
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<crate::ObjectListing>, StorageError> {
        // Page through every object under `prefix` (GCS caps a page at ~1000,
        // and a busy day's telemetry can exceed that).
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let resp = self
                .client
                .list_objects(&ListObjectsRequest {
                    bucket: (*self.bucket).clone(),
                    prefix: Some(prefix.to_string()),
                    page_token: page_token.clone(),
                    ..Default::default()
                })
                .await
                .map_err(|e| StorageError::Gcs {
                    key: prefix.to_string(),
                    message: e.to_string(),
                })?;
            if let Some(items) = resp.items {
                out.extend(items.into_iter().map(|o| crate::ObjectListing {
                    key: o.name,
                    size_bytes: u64::try_from(o.size).unwrap_or(0),
                }));
            }
            match resp.next_page_token {
                Some(t) if !t.is_empty() => page_token = Some(t),
                _ => break,
            }
        }
        Ok(out)
    }

    async fn signed_url(&self, key: &str, expires_in: Duration) -> Result<String, StorageError> {
        // The anonymous emulator client (a test endpoint) has no
        // signing identity; report `Unsupported` so callers stream
        // the bytes through the app instead of failing the request.
        if self.emulator {
            return Err(StorageError::Unsupported(
                "signed URLs against an emulator endpoint",
            ));
        }
        // V4 signed URL caps at 7 days; the caller picks the window.
        let opts = SignedURLOptions {
            expires: expires_in,
            ..SignedURLOptions::default()
        };
        self.client
            .signed_url(&self.bucket, key, None, None, opts)
            .await
            .map_err(|e| StorageError::Gcs {
                key: key.to_string(),
                message: e.to_string(),
            })
    }
}

/// Translate a `google_cloud_storage::http::Error` into `StorageError`.
/// 404 / "No such object" → `NotFound`; everything else → `Gcs`.
/// The string fallback covers proxy / emulator paths that may surface
/// the response code only in the `message` field of the
/// `ErrorResponse` rather than the structured `.code` field.
fn map_gcs_error(e: &GcsHttpError, key: &str) -> StorageError {
    if let GcsHttpError::Response(resp) = e {
        if resp.code == 404 {
            return StorageError::NotFound(key.to_string());
        }
    }
    let msg = e.to_string();
    if msg.contains("No such object") || msg.contains("404") {
        return StorageError::NotFound(key.to_string());
    }
    StorageError::Gcs {
        key: key.to_string(),
        message: msg,
    }
}

#[cfg(test)]
mod tests {
    use super::{authenticated_config, ClientConfig, GcsCredentials, GcsStorage, GcsStorageConfig};
    use crate::{StorageError, StorageService};
    use std::time::Duration;

    /// The exact message `google-cloud-auth` 0.17 produces for the
    /// `external_account` credential file `google-github-actions/auth`
    /// writes for Workload Identity Federation.
    const WIF_REJECTION: &str = "unsupported account external_account";

    #[test]
    fn working_adc_is_used_as_is() {
        // The ordinary path — a GKE pod's metadata server, a developer's
        // `application-default login`. Nothing shells out.
        let (_config, credentials) = authenticated_config(Ok(ClientConfig::default()), || {
            panic!("gcloud must not be probed when ADC succeeded")
        })
        .unwrap();
        assert_eq!(credentials, GcsCredentials::Adc);
    }

    #[test]
    fn a_wif_credential_falls_back_to_the_gcloud_cli() {
        // Every `publish-cli-archives` job in release 26.8.12 died here.
        // `gcloud` in that job is authenticated from the very file ADC
        // refused, so the fallback is a credential that already works.
        let (config, credentials) =
            authenticated_config(Err(WIF_REJECTION.into()), || Ok(())).unwrap();
        assert_eq!(credentials, GcsCredentials::GcloudCli);
        assert!(
            config.token_source_provider.is_some(),
            "the fallback client must carry a token source, not go anonymous"
        );
        assert_eq!(
            config.storage_endpoint,
            ClientConfig::default().storage_endpoint,
            "the fallback changes the credential only, never the endpoint"
        );
    }

    #[test]
    fn both_credentials_failing_names_both_causes() {
        // A developer box with neither ADC nor a logged-in `gcloud`. The
        // error has to name both, or the fallback turns a legible ADC
        // failure into a mystery.
        let err = authenticated_config(Err(WIF_REJECTION.into()), || {
            Err(anyhow::anyhow!(
                "You do not currently have an active account"
            ))
        })
        .unwrap_err();
        let StorageError::Gcs { key, message } = &err else {
            panic!("got {err:?}")
        };
        assert_eq!(key, "<auth>");
        assert!(message.contains(WIF_REJECTION), "got {message}");
        assert!(
            message.contains("You do not currently have an active account"),
            "got {message}"
        );
    }

    #[tokio::test]
    async fn an_endpoint_override_stays_anonymous() {
        // The emulator branch must not consult ADC or `gcloud` at all —
        // the KIND tier has no GCP credentials and must not wait on a
        // metadata-server probe.
        let storage = GcsStorage::new_from_config(GcsStorageConfig {
            bucket: "navigator".into(),
            endpoint: Some("http://localhost:30443".into()),
        })
        .await
        .unwrap();
        assert!(
            storage.emulator,
            "an endpoint override selects the anonymous, unsigned client"
        );
    }

    #[test]
    fn gcs_config_reports_missing_bucket() {
        let err = GcsStorageConfig::from_lookup(|_| None).unwrap_err();
        assert!(
            matches!(
                err,
                StorageError::MissingEnv("NAVIGATOR_DOCUMENTS_BUCKET or NAVIGATOR_STORAGE_BUCKET")
            ),
            "got {err:?}",
        );
    }

    #[test]
    fn gcs_config_reads_endpoint_override() {
        use std::collections::HashMap;
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_STORAGE_BUCKET", "navigator"),
            ("NAVIGATOR_STORAGE_ENDPOINT", "http://test-storage:4443"),
        ]);
        let cfg = GcsStorageConfig::from_lookup(|k| map.get(k).map(|s| (*s).to_string())).unwrap();
        assert_eq!(cfg.bucket, "navigator");
        assert_eq!(cfg.endpoint.as_deref(), Some("http://test-storage:4443"));
    }

    #[test]
    fn empty_endpoint_is_treated_as_unset() {
        use std::collections::HashMap;
        // A K8s env var declared with no `value:` arrives as `""`. It
        // must NOT select the emulator branch — otherwise the GCS
        // backend builds host-less URLs and every request fails with a
        // reqwest "builder error" before hitting the network.
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_DOCUMENTS_BUCKET", "proj-documents"),
            ("NAVIGATOR_STORAGE_ENDPOINT", ""),
        ]);
        let cfg = GcsStorageConfig::from_lookup(|k| map.get(k).map(|s| (*s).to_string())).unwrap();
        assert_eq!(cfg.endpoint, None, "empty endpoint must resolve to None");
    }

    #[test]
    fn documents_bucket_takes_precedence_over_storage_bucket() {
        use std::collections::HashMap;
        // `web` sets both vars (the generic one may linger from a
        // previous config); the documents-specific one must win so
        // client blobs never land in whatever `STORAGE_BUCKET` named.
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_DOCUMENTS_BUCKET", "proj-documents"),
            ("NAVIGATOR_STORAGE_BUCKET", "proj-assets"),
        ]);
        let cfg = GcsStorageConfig::from_lookup(|k| map.get(k).map(|s| (*s).to_string())).unwrap();
        assert_eq!(cfg.bucket, "proj-documents");
    }

    #[test]
    fn falls_back_to_storage_bucket_when_documents_unset() {
        use std::collections::HashMap;
        // `archives` / KIND / the `navigator` CLI sets only the generic var; the
        // fallback keeps them resolving their own bucket unchanged.
        let map: HashMap<&str, &str> =
            HashMap::from([("NAVIGATOR_STORAGE_BUCKET", "proj-exports")]);
        let cfg = GcsStorageConfig::from_lookup(|k| map.get(k).map(|s| (*s).to_string())).unwrap();
        assert_eq!(cfg.bucket, "proj-exports");
    }

    #[test]
    fn assets_lane_prefers_assets_bucket_and_ignores_documents_bucket() {
        use std::collections::HashMap;
        // The fill path pulls blank government forms from the PUBLIC
        // assets bucket; the private documents bucket must never shadow
        // it, even on a pod that sets all three vars.
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_ASSETS_BUCKET", "proj-assets"),
            ("NAVIGATOR_DOCUMENTS_BUCKET", "proj-documents"),
            ("NAVIGATOR_STORAGE_BUCKET", "navigator"),
        ]);
        let cfg =
            GcsStorageConfig::assets_from_lookup(|k| map.get(k).map(|s| (*s).to_string())).unwrap();
        assert_eq!(cfg.bucket, "proj-assets");
        // KIND/dev single-bucket topology: only the generic var is set.
        let map: HashMap<&str, &str> = HashMap::from([("NAVIGATOR_STORAGE_BUCKET", "navigator")]);
        let cfg =
            GcsStorageConfig::assets_from_lookup(|k| map.get(k).map(|s| (*s).to_string())).unwrap();
        assert_eq!(cfg.bucket, "navigator");
        let err = GcsStorageConfig::assets_from_lookup(|_| None).unwrap_err();
        assert!(matches!(
            err,
            StorageError::MissingEnv("NAVIGATOR_ASSETS_BUCKET or NAVIGATOR_STORAGE_BUCKET")
        ));
    }

    #[test]
    fn exports_lane_ignores_documents_bucket_on_a_shared_pod() {
        use std::collections::HashMap;
        // The worker carries BOTH vars: DOCUMENTS for the render lane,
        // STORAGE for the exports lane. The exports resolver must pin to
        // STORAGE_BUCKET so nightly Parquet never follows the document
        // preference into the documents bucket. (The default `from_lookup`
        // on the same map resolves to documents — proving the two lanes
        // split.)
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_DOCUMENTS_BUCKET", "proj-documents"),
            ("NAVIGATOR_STORAGE_BUCKET", "proj-exports"),
        ]);
        let lookup = |k: &str| map.get(k).map(|s| (*s).to_string());
        let exports = GcsStorageConfig::exports_from_lookup(lookup).unwrap();
        assert_eq!(exports.bucket, "proj-exports");
        let documents = GcsStorageConfig::from_lookup(lookup).unwrap();
        assert_eq!(documents.bucket, "proj-documents");
    }

    #[test]
    fn surreal_archive_lane_prefers_the_firm_retention_bucket() {
        use std::collections::HashMap;
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_SURREAL_ARCHIVES_BUCKET", "neon-law-archives"),
            ("NAVIGATOR_STORAGE_BUCKET", "worktree-exports"),
        ]);
        let cfg = GcsStorageConfig::surreal_archives_from_lookup(|key| {
            map.get(key).map(|value| (*value).to_string())
        })
        .unwrap();
        assert_eq!(cfg.bucket, "neon-law-archives");
    }

    #[test]
    fn surreal_archive_lane_requires_its_dedicated_bucket() {
        use std::collections::HashMap;
        let map: HashMap<&str, &str> = HashMap::from([
            ("NAVIGATOR_DOCUMENTS_BUCKET", "proj-documents"),
            ("NAVIGATOR_STORAGE_BUCKET", "worktree-exports"),
        ]);
        let error = GcsStorageConfig::surreal_archives_from_lookup(|key| {
            map.get(key).map(|value| (*value).to_string())
        })
        .unwrap_err();
        assert!(matches!(
            error,
            StorageError::MissingEnv("NAVIGATOR_SURREAL_ARCHIVES_BUCKET")
        ));
    }

    /// A minimal but fully decodable `storage#object` resource — every
    /// field the crate's typed `Object` requires on deserialize
    /// (`selfLink`, `mediaLink`, `name`, `id`, `bucket`, and the
    /// string-encoded int64s `generation`/`metageneration`).
    fn object_json(key: &str, cache_control: Option<&str>) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "kind": "storage#object",
            "selfLink": format!("http://mock/storage/v1/b/navigator/o/{key}"),
            "mediaLink": format!("http://mock/download/storage/v1/b/navigator/o/{key}?alt=media"),
            "name": key,
            "id": format!("navigator/{key}/1"),
            "bucket": "navigator",
            "generation": "1",
            "metageneration": "1",
        });
        if let Some(cc) = cache_control {
            obj["cacheControl"] = cc.into();
        }
        obj
    }

    /// A mock GCS endpoint whose metadata PATCH answers 2xx with a body
    /// the typed decode rejects (the a test endpoint shape), and whose object
    /// GET reports `cache_control` — the two halves of `put_cached`'s
    /// tolerate-then-verify branch.
    async fn mock_gcs_with_undecodable_patch(
        cache_control_on_read: Option<&str>,
    ) -> wiremock::MockServer {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/storage/v1/b/navigator/o"))
            .respond_with(ResponseTemplate::new(200).set_body_json(object_json("blank.pdf", None)))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/navigator/o/blank.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "name": "blank.pdf" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/navigator/o/blank.pdf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(object_json("blank.pdf", cache_control_on_read)),
            )
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn put_cached_tolerates_undecodable_patch_when_read_back_confirms() {
        let server = mock_gcs_with_undecodable_patch(Some("public, max-age=60")).await;
        let storage = GcsStorage::new_from_config(GcsStorageConfig {
            bucket: "navigator".into(),
            endpoint: Some(server.uri()),
        })
        .await
        .unwrap();
        storage
            .put_cached(
                "blank.pdf",
                b"%PDF",
                "application/pdf",
                "public, max-age=60",
            )
            .await
            .expect("2xx PATCH with undecodable body + confirming read-back is success");
    }

    #[tokio::test]
    async fn put_cached_fails_when_undecodable_patch_did_not_apply() {
        // Same undecodable 2xx PATCH, but the read-back shows the
        // directive never landed — `put_cached` must NOT claim success.
        let server = mock_gcs_with_undecodable_patch(None).await;
        let storage = GcsStorage::new_from_config(GcsStorageConfig {
            bucket: "navigator".into(),
            endpoint: Some(server.uri()),
        })
        .await
        .unwrap();
        let err = storage
            .put_cached(
                "blank.pdf",
                b"%PDF",
                "application/pdf",
                "public, max-age=60",
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&err, StorageError::Gcs { message, .. } if message.contains("read-back")),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn signed_url_is_unsupported_against_an_emulator_endpoint() {
        // The KIND dev loop runs the GCS backend against a test endpoint,
        // which has no signing identity. `signed_url` must report
        // `Unsupported` (so `web` streams the bytes) instead of a signer
        // error the caller treats as a 500.
        let storage = GcsStorage::new_from_config(GcsStorageConfig {
            bucket: "navigator".into(),
            endpoint: Some("http://localhost:30443".into()),
        })
        .await
        .unwrap();
        let err = storage
            .signed_url("notations/x/document.pdf", Duration::from_mins(1))
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn exports_lane_requires_storage_bucket() {
        // With only DOCUMENTS set, the exports lane has no bucket to fall
        // back to — it must error rather than silently borrow documents.
        let err = GcsStorageConfig::exports_from_lookup(|k| {
            (k == "NAVIGATOR_DOCUMENTS_BUCKET").then(|| "proj-documents".to_string())
        })
        .unwrap_err();
        assert!(matches!(err, StorageError::MissingEnv(_)), "got {err:?}");
    }
}
