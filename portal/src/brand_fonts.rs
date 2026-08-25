//! Firm brand fonts — the `/app/team` firm-tier download.
//!
//! `GET /app/team/fonts/gorp-serif.zip` streams the licensed GORP Serif
//! desktop family (the `.otf` faces) as one ZIP. The bytes live only in
//! the **private** documents bucket, uploaded by `navigator assets fonts
//! upload-desktop`; this handler pulls them through
//! [`cloud::StorageService`] and streams them straight to the caller.
//!
//! Storing the ZIP in the private lane — not the public assets bucket the
//! WOFF2 web faces use — is load-bearing: it means the only path to the
//! bytes is this route, so a predictable public object URL can never
//! bypass authorization.
//!
//! Authorization is embedded Rego policy's: the object sits under the team
//! home's own `/app/team` prefix, whose rules admit all four firm tiers —
//! Owner, Admin, Lawyer, and Clerk — and deny a client. A brand asset is not
//! lawyer work, so it needs neither the `/lawyer` prefix nor the exact-path
//! Clerk exception that prefix used to force. `require_auth` rejects anonymous
//! callers first, so no handler-layer role check is needed here. A missing
//! object is a loud `502`, never a fallback.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

/// The documents-bucket key the desktop family is published to, kept in one
/// place so the upload seam (`navigator assets fonts upload-desktop`) and
/// this download handler cannot drift apart.
pub const GORP_OTF_ZIP_KEY: &str = "fonts/gorp-serif/gorp-serif-otf.zip";

/// `GET /app/team/fonts/gorp-serif.zip` — download the GORP Serif desktop
/// family as one ZIP attachment.
///
/// `State<Arc<dyn StorageService>>` resolves to the *documents* (private)
/// lane via [`crate::admin::AdminState`]'s `FromRef` — the same seam
/// per-matter blobs use — so the bytes never touch the public assets bucket.
pub async fn download_get(State(storage): State<Arc<dyn cloud::StorageService>>) -> Response {
    let zip = match storage.get(GORP_OTF_ZIP_KEY).await {
        Ok(zip) => zip,
        Err(cloud::StorageError::NotFound(_)) => {
            tracing::error!(
                object_path = GORP_OTF_ZIP_KEY,
                "brand_fonts: GORP Serif ZIP missing from the documents bucket — run `navigator assets fonts upload-desktop`"
            );
            return (StatusCode::BAD_GATEWAY, "font bundle unavailable").into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "brand_fonts: documents storage read failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
        }
    };
    (
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"gorp-serif.zip\"".to_string(),
            ),
        ],
        zip.bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{download_get, GORP_OTF_ZIP_KEY};
    use axum::extract::State;
    use axum::http::StatusCode;
    use std::sync::Arc;

    async fn fs_storage(tag: &str) -> Arc<dyn cloud::StorageService> {
        Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join(format!(
                "navigator-brand-fonts-{tag}-{}",
                uuid::Uuid::new_v4()
            )))
            .await
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn downloads_the_staged_zip_as_an_attachment() {
        let storage = fs_storage("staged").await;
        storage
            .put(GORP_OTF_ZIP_KEY, b"PK\x03\x04 fake-zip", "application/zip")
            .await
            .unwrap();
        let resp = download_get(State(storage)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let headers = resp.headers();
        assert_eq!(headers["content-type"], "application/zip");
        assert!(headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("gorp-serif.zip"));
    }

    #[tokio::test]
    async fn a_missing_bucket_object_is_a_loud_502_not_a_fallback() {
        let storage = fs_storage("empty").await;
        let resp = download_get(State(storage)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
