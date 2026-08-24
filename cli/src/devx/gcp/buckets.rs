//! Provision the GCS buckets `web` writes to:
//!
//! - `<project>-assets` — marketing photography and licensed webfonts,
//!   kept private in GCS and served anonymously only through the site's
//!   `/assets/*` application route. Standard.
//! - `<project>-documents` — **private** client documents; holds the
//!   content-addressed `blobs/<sha>` objects `web` writes. Standard,
//!   no public binding. Kept separate from `-assets` so confidential
//!   client data is never co-mingled into the public bucket.
//! - `<project>-exports` — nightly Parquet/Iceberg archives. Standard.
//! - `<project>-logs` — long-lived audit / access logs. Nearline.
//!
//! Two more exist only when the deployment names them:
//!
//! - `neon-law-archives-<deployment>` — the long-term Iceberg archive of that
//!   deployment's Surreal store. Standard, no lifecycle rule: this is where
//!   long-term storage lives, and an age rule here would answer a retention
//!   question that belongs to a person.
//! - `<deployment>-telemetry` — the landing zone the `OTel` collector writes
//!   Parquet to, before the nightly lane promotes it into the archive.
//!   Standard, with a flat [`TELEMETRY_RETENTION_DAYS`]-day expiry.
//!
//! Every bucket here is private and single-region (location follows
//! `SetupConfig::region`, default `us-west4`), with uniform bucket level
//! access. Storage class is STANDARD except NEARLINE on logs, which are read
//! rarely and mostly written. Archives and telemetry additionally enforce
//! `publicAccessPrevention`; the four original kinds inherit the project's
//! setting, because the assets lane's public-read path depends on it. The
//! `-source` (git bundles) bucket is created out-of-band; see
//! `cloud/README.md`.
//!
//! ## Idempotency
//!
//! Re-running `setup` against a project that already has these
//! buckets must succeed. We POST `storage.buckets.insert`
//! unconditionally and treat HTTP **409 Conflict** as success —
//! that's the response GCS returns when a bucket with the same name
//! already exists in the same project. Anything else outside the
//! 2xx range bubbles up as an error.

use serde::Serialize;
use url::Url;

use super::client::{GcpClient, GcpService};
use super::error::{SetupError, SetupResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketKind {
    Assets,
    Documents,
    Exports,
    Logs,
    Applications,
    Archives,
    Telemetry,
}

impl BucketKind {
    #[must_use]
    pub const fn storage_class(self) -> &'static str {
        match self {
            // Archives is STANDARD, not NEARLINE: the nightly lane reads the
            // prior `v<N>.metadata.json` on every run to chain the snapshot
            // log, so a retrieval-priced class would bill a read per table per
            // night for data that is written just as often.
            // Applications is STANDARD: a Project portal's bundle is read on
            // every client visit, so a retrieval-priced class would bill each
            // asset fetch.
            Self::Assets
            | Self::Documents
            | Self::Exports
            | Self::Applications
            | Self::Archives
            | Self::Telemetry => "STANDARD",
            Self::Logs => "NEARLINE",
        }
    }

    /// How long objects live before the bucket's own lifecycle rule deletes
    /// them. `None` means the bucket carries no rule at all.
    ///
    /// Only the telemetry landing zone expires. Archives is where long-term
    /// storage lives, and retention there is a separate, deliberate decision —
    /// giving it an age rule here would answer that question by accident.
    #[must_use]
    pub const fn retention_days(self) -> Option<u32> {
        match self {
            Self::Telemetry => Some(TELEMETRY_RETENTION_DAYS),
            Self::Applications => Some(APPLICATIONS_RETENTION_DAYS),
            _ => None,
        }
    }

    /// `publicAccessPrevention` for this kind, or `None` to inherit the
    /// project's setting.
    #[must_use]
    pub const fn public_access_prevention(self) -> Option<&'static str> {
        match self {
            // Applications is enforced-private like archives and telemetry:
            // its bundles are streamed through Axum, never fetched by a
            // browser directly, so no anonymous grant is ever wanted here.
            Self::Applications | Self::Archives | Self::Telemetry => Some("enforced"),
            _ => None,
        }
    }

    /// Classify a bucket by its name. Returns `Assets` by default for
    /// unrecognized names — the caller is the source of truth on what it's
    /// creating. Every other kind is matched explicitly so none is silently
    /// treated as `Assets`.
    ///
    /// Archives is matched on a *prefix*, not a suffix: its buckets are named
    /// `neon-law-archives-<deployment>` rather than `<deployment>-archives`, so a
    /// suffix test would classify `neon-law-archives-prod` as `Assets`
    /// and create it with no lifecycle distinction from a public bucket.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        if name.ends_with(super::LOGS_BUCKET_SUFFIX) {
            Self::Logs
        } else if name.ends_with(super::DOCUMENTS_BUCKET_SUFFIX) {
            Self::Documents
        } else if name.ends_with(super::EXPORTS_BUCKET_SUFFIX) {
            Self::Exports
        } else if name.ends_with(super::APPLICATIONS_BUCKET_SUFFIX) {
            Self::Applications
        } else if name.ends_with(super::TELEMETRY_BUCKET_SUFFIX) {
            Self::Telemetry
        } else if name.starts_with(super::ARCHIVES_BUCKET_PREFIX) {
            Self::Archives
        } else {
            Self::Assets
        }
    }
}

/// Objects in a telemetry landing bucket are deleted at 90 days.
///
/// The landing zone is not storage: the nightly lane promotes each day's
/// Parquet into that deployment's archive bucket, and the archive is what
/// keeps it. Ninety days is the window in which a promotion that failed can
/// still be re-run from the original bytes.
pub const TELEMETRY_RETENTION_DAYS: u32 = 90;

/// Objects in a Project-applications bucket are deleted at ten years.
///
/// The bucket holds client-facing engagement records, so this expiry bounds the
/// accumulation of superseded builds — it is not a recycling window for live
/// ones. Ten years outlives any engagement a portal documents. A thirty-day
/// window did not: it made deletion of a *published* portal the normal outcome
/// of publishing once and then going quiet.
///
/// **A publish must still overwrite every live object unconditionally.** Each
/// publish rewrites `index.html` and every current hashed asset, refreshing
/// their `updateTime`, so the age rule can only ever reach an *orphaned* asset
/// from a superseded build — never a live one. An "optimization" that skips
/// unchanged objects — `gcloud storage rsync` is exactly that — lets a live
/// asset age out and be deleted from under a served portal, and the breakage
/// surfaces long after the change that caused it. Ten years makes that slow
/// rather than imminent; it does not make it safe. Publish with `gcloud storage
/// cp`, never `rsync`: see `.github/actions/application-publish/action.yml`.
pub const APPLICATIONS_RETENTION_DAYS: u32 = 3650;

#[derive(Serialize)]
struct CreateBucketBody<'a> {
    name: &'a str,
    location: &'a str,
    #[serde(rename = "storageClass")]
    storage_class: &'a str,
    #[serde(rename = "iamConfiguration")]
    iam_configuration: IamConfig,
}

#[derive(Serialize)]
struct IamConfig {
    #[serde(rename = "uniformBucketLevelAccess")]
    uniform_bucket_level_access: UniformAccess,
    /// Omitted for the four original kinds, which inherit the project's
    /// setting. Set to `enforced` for archives and telemetry: neither is ever
    /// served to a browser, and `ensure_public_read` — the one path that wants
    /// an anonymous grant — runs against website buckets, which this call does
    /// not create.
    #[serde(
        rename = "publicAccessPrevention",
        skip_serializing_if = "Option::is_none"
    )]
    public_access_prevention: Option<&'static str>,
}

#[derive(Serialize)]
struct UniformAccess {
    enabled: bool,
}

/// Outcome of a single `ensure_bucket` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureOutcome {
    /// Bucket did not exist; we created it.
    Created,
    /// Bucket already existed (HTTP 409 from `buckets.insert`).
    AlreadyExists,
}

/// Idempotently ensure a bucket exists in `project_id` at `location`.
pub async fn ensure_bucket(
    client: &GcpClient,
    project_id: &str,
    name: &str,
    location: &str,
) -> SetupResult<EnsureOutcome> {
    let kind = BucketKind::from_name(name);
    let body = CreateBucketBody {
        name,
        location,
        storage_class: kind.storage_class(),
        iam_configuration: IamConfig {
            uniform_bucket_level_access: UniformAccess { enabled: true },
            public_access_prevention: kind.public_access_prevention(),
        },
    };
    let body_json = serde_json::to_value(&body).map_err(|source| SetupError::Json {
        what: "create bucket request body",
        source,
    })?;
    let resp = client
        .post_json(
            GcpService::Storage,
            &format!("/storage/v1/b?project={project_id}"),
            &body_json,
        )
        .await?;
    let status = resp.status_u16();
    match status {
        200..=299 => Ok(EnsureOutcome::Created),
        409 => Ok(EnsureOutcome::AlreadyExists),
        other => Err(SetupError::BadStatus {
            operation: format!("create bucket {name}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Retain a canonical browser-origin CORS rule on the assets bucket.
///
/// The normal website path is same-origin `/assets/*`, so this is not an
/// anonymous-access mechanism. It keeps the bucket portable to an authorized
/// edge proxy that forwards browser CORS semantics later.
pub async fn ensure_assets_cors(
    client: &GcpClient,
    name: &str,
    public_base_url: &str,
) -> SetupResult<()> {
    let font_rule = assets_font_cors_rule(public_base_url)?;
    let response = client
        .get(GcpService::Storage, &format!("/storage/v1/b/{name}"))
        .await?;
    let status = response.status_u16();
    if !(200..=299).contains(&status) {
        return Err(SetupError::BadStatus {
            operation: format!("read CORS for assets bucket {name}"),
            status,
            body: response.into_text(),
        });
    }

    let metadata: serde_json::Value =
        serde_json::from_str(&response.into_text()).map_err(|source| SetupError::Json {
            what: "assets bucket metadata",
            source,
        })?;
    let mut cors = metadata
        .get("cors")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let original_rule_count = cors.len();
    let legacy_font_rule = legacy_assets_font_cors_rule();
    cors.retain(|rule| rule != &legacy_font_rule);
    if cors.contains(&font_rule) && cors.len() == original_rule_count {
        return Ok(());
    }
    if !cors.contains(&font_rule) {
        cors.push(font_rule);
    }

    let body = serde_json::json!({ "cors": cors });
    let resp = client
        .patch_json(GcpService::Storage, &format!("/storage/v1/b/{name}"), &body)
        .await?;
    match resp.status_u16() {
        200..=299 => Ok(()),
        other => Err(SetupError::BadStatus {
            operation: format!("set CORS for assets bucket {name}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Idempotently set the object-expiry lifecycle rule this bucket's kind calls
/// for, and remove any rule from a kind that calls for none.
///
/// One unconditional PATCH rather than read-then-compare: the desired rule set
/// is a pure function of the kind, so writing it is already idempotent, and a
/// read first would let a rule drift back in between the two calls.
///
/// **The rule is unscoped by prefix, deliberately.** A telemetry bucket holds
/// nothing but telemetry, so an age condition needs no `matchesPrefix` to stay
/// off anything else — which is exactly what a prefix-scoped rule in a shared
/// bucket has to get right, and silently fails to do when a prefix moves.
pub async fn ensure_lifecycle(client: &GcpClient, name: &str) -> SetupResult<()> {
    let rules = match BucketKind::from_name(name).retention_days() {
        Some(days) => serde_json::json!([{
            "action": { "type": "Delete" },
            "condition": { "age": days },
        }]),
        None => serde_json::json!([]),
    };
    let body = serde_json::json!({ "lifecycle": { "rule": rules } });
    let resp = client
        .patch_json(GcpService::Storage, &format!("/storage/v1/b/{name}"), &body)
        .await?;
    match resp.status_u16() {
        200..=299 => Ok(()),
        other => Err(SetupError::BadStatus {
            operation: format!("set lifecycle for bucket {name}"),
            status: other,
            body: resp.into_text(),
        }),
    }
}

/// Reject a `NAV_BASE_URL` that cannot become a CORS origin *before* the
/// pipeline mutates anything. [`ensure_assets_cors`] runs at step 4a, after
/// services, networking, SQL, and the bucket already exist, so parsing only
/// there would leave a malformed value to fail against a half-provisioned
/// project. `run` calls this first so the command refuses upfront instead.
pub fn validate_public_base_url(public_base_url: &str) -> SetupResult<()> {
    assets_font_cors_rule(public_base_url).map(|_| ())
}

fn assets_font_cors_rule(public_base_url: &str) -> SetupResult<serde_json::Value> {
    let url = Url::parse(public_base_url).map_err(|_| {
        SetupError::InvalidPublicBaseUrl(
            "must be an absolute HTTPS URL with no path, query, or fragment".into(),
        )
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(SetupError::InvalidPublicBaseUrl(
            "must be an absolute HTTPS URL with no path, query, or fragment".into(),
        ));
    }

    Ok(serde_json::json!({
        "origin": [url.origin().ascii_serialization()],
        "method": ["GET", "HEAD"],
        "responseHeader": ["Content-Type"],
        "maxAgeSeconds": 3600
    }))
}

fn legacy_assets_font_cors_rule() -> serde_json::Value {
    serde_json::json!({
        "origin": ["*"],
        "method": ["GET", "HEAD"],
        "responseHeader": ["Content-Type"],
        "maxAgeSeconds": 3600
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::super::client::{GcpClient, GcpService, StaticToken};
    use super::{
        assets_font_cors_rule, ensure_assets_cors, ensure_bucket, ensure_lifecycle,
        legacy_assets_font_cors_rule, BucketKind, CreateBucketBody, EnsureOutcome, IamConfig,
        UniformAccess, APPLICATIONS_RETENTION_DAYS, TELEMETRY_RETENTION_DAYS,
    };

    const PUBLIC_BASE_URL: &str = "https://www.neonlaw.com";

    fn client_pointed_at(server: &MockServer) -> GcpClient {
        GcpClient::new(Arc::new(StaticToken("t".into())))
            .with_base_url(GcpService::Storage, server.uri())
    }

    #[tokio::test]
    async fn creates_bucket_when_post_returns_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .and(query_param("project", "proj"))
            .and(body_partial_json(json!({
                "name": "proj-assets",
                "location": "us-west4",
                "storageClass": "STANDARD",
                "iamConfiguration": {
                    "uniformBucketLevelAccess": { "enabled": true }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "proj-assets"})))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        let outcome = ensure_bucket(&client, "proj", "proj-assets", "us-west4")
            .await
            .unwrap();
        assert_eq!(outcome, EnsureOutcome::Created);
    }

    #[test]
    fn archives_is_classified_by_prefix_not_suffix() {
        // `neon-law-archives-prod` ends in `-production`, so every
        // suffix test in `from_name` misses it. Without the prefix arm it
        // falls through to `Assets` — created with no public-access
        // prevention, and never given the archive lane's treatment.
        assert_eq!(
            BucketKind::from_name("neon-law-archives-prod"),
            BucketKind::Archives
        );
        assert_eq!(
            BucketKind::from_name("neon-law-archives-prod"),
            BucketKind::Archives
        );
        assert_eq!(
            BucketKind::from_name("neon-law-stg-telemetry"),
            BucketKind::Telemetry
        );
        // The four original kinds keep classifying exactly as before.
        assert_eq!(BucketKind::from_name("proj-assets"), BucketKind::Assets);
        assert_eq!(
            BucketKind::from_name("proj-documents"),
            BucketKind::Documents
        );
        assert_eq!(BucketKind::from_name("proj-exports"), BucketKind::Exports);
        assert_eq!(BucketKind::from_name("proj-logs"), BucketKind::Logs);
        // The applications bucket is classified on its own suffix so it gets
        // the 30-day expiry and enforced-private access rather than the
        // Assets default.
        assert_eq!(
            BucketKind::from_name("neon-law-stg-applications"),
            BucketKind::Applications
        );
    }

    #[test]
    fn telemetry_and_applications_expire_but_archives_never_does() {
        // The whole point of the archive/telemetry split: the landing zone is
        // disposable and the archive is not. An age rule on the archive would
        // silently answer a retention question nobody asked. The applications
        // bucket expires too, at ten years — long enough that the expiry only
        // ever reaches orphaned assets from superseded builds, and never the
        // live bundle of an engagement still on foot.
        assert_eq!(
            BucketKind::Telemetry.retention_days(),
            Some(TELEMETRY_RETENTION_DAYS)
        );
        assert_eq!(
            BucketKind::Applications.retention_days(),
            Some(APPLICATIONS_RETENTION_DAYS)
        );
        // Ten years, in days. A short window here deletes published client
        // portals; ENG-273 raised it from thirty days for exactly that reason.
        assert_eq!(APPLICATIONS_RETENTION_DAYS, 3650);
        // The applications bundle is streamed through Axum, never served to a
        // browser directly, so it is enforced-private like archives/telemetry.
        assert_eq!(
            BucketKind::Applications.public_access_prevention(),
            Some("enforced")
        );
        assert_eq!(BucketKind::Applications.storage_class(), "STANDARD");
        assert_eq!(BucketKind::Archives.retention_days(), None);
        for kind in [
            BucketKind::Assets,
            BucketKind::Documents,
            BucketKind::Exports,
            BucketKind::Logs,
        ] {
            assert_eq!(kind.retention_days(), None, "{kind:?} must not expire");
        }
    }

    #[tokio::test]
    async fn archive_and_telemetry_buckets_are_created_with_public_access_prevented() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .and(query_param("project", "neon-law"))
            .and(body_partial_json(json!({
                "name": "neon-law-archives-prod",
                "storageClass": "STANDARD",
                "iamConfiguration": {
                    "uniformBucketLevelAccess": { "enabled": true },
                    "publicAccessPrevention": "enforced"
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "b"})))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        let outcome = ensure_bucket(&client, "neon-law", "neon-law-archives-prod", "us-west4")
            .await
            .unwrap();
        assert_eq!(outcome, EnsureOutcome::Created);
    }

    #[tokio::test]
    async fn assets_bucket_create_still_omits_public_access_prevention() {
        // The four original kinds inherit the project's setting. Enforcing it
        // here would break `ensure_public_read`, which the marketing website
        // path depends on.
        let body = serde_json::to_value(&CreateBucketBody {
            name: "proj-assets",
            location: "us-west4",
            storage_class: BucketKind::Assets.storage_class(),
            iam_configuration: IamConfig {
                uniform_bucket_level_access: UniformAccess { enabled: true },
                public_access_prevention: BucketKind::Assets.public_access_prevention(),
            },
        })
        .unwrap();
        assert!(
            body.get("publicAccessPrevention").is_none(),
            "assets create body must not carry publicAccessPrevention: {body}"
        );
    }

    #[tokio::test]
    async fn telemetry_lifecycle_sets_a_flat_ninety_day_delete() {
        // Flat — no `matchesPrefix`. A dedicated bucket needs no prefix scoping,
        // which is precisely the thing that goes stale in a shared one.
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/neon-law-stg-telemetry"))
            .and(body_partial_json(json!({
                "lifecycle": {
                    "rule": [{
                        "action": { "type": "Delete" },
                        "condition": { "age": 90 }
                    }]
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "b"})))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        ensure_lifecycle(&client, "neon-law-stg-telemetry")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn archive_lifecycle_clears_any_rule_rather_than_setting_one() {
        // Idempotent in the direction that matters: if someone attaches an age
        // rule to an archive bucket by hand, the next setup takes it back off.
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/neon-law-archives-prod"))
            .and(body_partial_json(json!({ "lifecycle": { "rule": [] } })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "b"})))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        ensure_lifecycle(&client, "neon-law-archives-prod")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn treats_409_conflict_as_already_exists() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .respond_with(ResponseTemplate::new(409).set_body_json(json!({
                "error": { "code": 409, "message": "You already own this bucket." }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        let outcome = ensure_bucket(&client, "proj", "proj-assets", "us-west4")
            .await
            .unwrap();
        assert_eq!(outcome, EnsureOutcome::AlreadyExists);
    }

    #[tokio::test]
    async fn assets_bucket_cors_allows_cross_origin_font_fetches() {
        let server = MockServer::start().await;
        let existing_rule = json!({
            "origin": ["https://operator.example.test"],
            "method": ["GET"],
            "responseHeader": ["ETag"],
            "maxAgeSeconds": 600
        });
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "cors": [existing_rule] })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/proj-assets"))
            .and(body_partial_json(json!({
                "cors": [
                    existing_rule,
                    assets_font_cors_rule(PUBLIC_BASE_URL).unwrap()
                ]
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        ensure_assets_cors(&client_pointed_at(&server), "proj-assets", PUBLIC_BASE_URL)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn assets_bucket_cors_does_not_duplicate_the_font_rule() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "cors": [assets_font_cors_rule(PUBLIC_BASE_URL).unwrap()] }),
            ))
            .expect(1)
            .mount(&server)
            .await;

        ensure_assets_cors(&client_pointed_at(&server), "proj-assets", PUBLIC_BASE_URL)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn assets_bucket_cors_replaces_the_legacy_wildcard_rule() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "cors": [legacy_assets_font_cors_rule()] })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/proj-assets"))
            .and(body_partial_json(json!({
                "cors": [assets_font_cors_rule(PUBLIC_BASE_URL).unwrap()]
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        ensure_assets_cors(&client_pointed_at(&server), "proj-assets", PUBLIC_BASE_URL)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn assets_bucket_cors_surfaces_a_failed_metadata_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
            .expect(1)
            .mount(&server)
            .await;

        let err = ensure_assets_cors(&client_pointed_at(&server), "proj-assets", PUBLIC_BASE_URL)
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("read CORS for assets bucket proj-assets"),
            "{message}"
        );
        assert!(message.contains("403"), "{message}");
    }

    #[tokio::test]
    async fn assets_bucket_cors_surfaces_unparseable_metadata() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .expect(1)
            .mount(&server)
            .await;

        let err = ensure_assets_cors(&client_pointed_at(&server), "proj-assets", PUBLIC_BASE_URL)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("assets bucket metadata"), "{err}");
    }

    #[tokio::test]
    async fn assets_bucket_cors_surfaces_a_failed_patch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/storage/v1/b/proj-assets"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .expect(1)
            .mount(&server)
            .await;

        let err = ensure_assets_cors(&client_pointed_at(&server), "proj-assets", PUBLIC_BASE_URL)
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("set CORS for assets bucket proj-assets"),
            "{message}"
        );
        assert!(message.contains("500"), "{message}");
    }

    #[test]
    fn assets_bucket_cors_uses_the_canonical_https_origin() {
        let rule = assets_font_cors_rule("https://www.neonlaw.com/").unwrap();
        assert_eq!(rule["origin"], json!(["https://www.neonlaw.com"]));
    }

    #[test]
    fn assets_bucket_cors_rejects_a_non_origin_base_url() {
        let err = assets_font_cors_rule("https://www.neonlaw.com/services")
            .expect_err("a path must not become a CORS origin");
        assert!(err.to_string().contains("NAV_BASE_URL"), "{err}");
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let server = MockServer::start().await;
        // First POST: bucket doesn't exist → 200.
        // Second POST: bucket exists → 409.
        // Wiremock matchers are FIFO when stacked under
        // `.up_to_n_times`, which is exactly the cadence we want
        // to model a real second run.
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "proj-assets"})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        let first = ensure_bucket(&client, "proj", "proj-assets", "us-west4")
            .await
            .unwrap();
        let second = ensure_bucket(&client, "proj", "proj-assets", "us-west4")
            .await
            .unwrap();
        assert_eq!(first, EnsureOutcome::Created);
        assert_eq!(second, EnsureOutcome::AlreadyExists);
    }

    #[tokio::test]
    async fn logs_bucket_uses_nearline_storage_class() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .and(body_partial_json(json!({
                "name": "proj-logs",
                "storageClass": "NEARLINE"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "proj-logs"})))
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        ensure_bucket(&client, "proj", "proj-logs", "us-west4")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn documents_bucket_uses_standard_storage_class() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .and(body_partial_json(json!({
                "name": "proj-documents",
                "storageClass": "STANDARD",
                "iamConfiguration": {
                    "uniformBucketLevelAccess": { "enabled": true }
                }
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"name": "proj-documents"})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = client_pointed_at(&server);
        ensure_bucket(&client, "proj", "proj-documents", "us-west4")
            .await
            .unwrap();
    }

    #[test]
    fn from_name_classifies_each_suffix() {
        use super::BucketKind;
        assert_eq!(BucketKind::from_name("proj-assets"), BucketKind::Assets);
        assert_eq!(
            BucketKind::from_name("proj-documents"),
            BucketKind::Documents
        );
        assert_eq!(BucketKind::from_name("proj-exports"), BucketKind::Exports);
        assert_eq!(BucketKind::from_name("proj-logs"), BucketKind::Logs);
        // Unknown names default to Assets (STANDARD).
        assert_eq!(BucketKind::from_name("proj-whatever"), BucketKind::Assets);
    }

    #[tokio::test]
    async fn unexpected_status_is_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/storage/v1/b"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        let client = client_pointed_at(&server);
        let err = ensure_bucket(&client, "p", "x", "us-west4")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("500"), "got {err}");
    }
}
