//! `POST /app/api/assets` — the authenticated public-asset publish door (ENG-909).
//!
//! Writes one small public-safe asset (a brand mark, a hero image, a web
//! font) straight to the deployment's public assets bucket
//! (`ApiState::assets_storage`), keyed exactly as the public `/assets/{key}`
//! route ([`crate::public_asset`]) serves it back — so a successful upload
//! is immediately reachable there with no further step.
//!
//! This is the OAuth-backed sibling of the ADC-backed
//! `navigator ops assets upload` batch command (`cli::assets::run_upload`):
//! that command targets real GCS directly with the operator's own bucket
//! credentials for bulk gallery publication, while this door lets a single
//! asset go out with nothing but a `navigator site login` bearer, so the
//! caller never needs a bucket name or a GCP credential. Both still land in
//! the same bucket; only the credential and the batch size differ.
//!
//! Owner/Admin only — the same tier `update_brand_presentation` uses for
//! every other site-branding write, since a public asset (a brand mark, a
//! hero image, a font family) is firm-side site configuration, not
//! matter-scoped lawyer work. The handler reads `ApiState::assets_storage`
//! exclusively — it never touches `ApiState::storage` (the private
//! documents bucket), so there is no path from this door into the
//! documents lane. A key must also fall below `brand/`, `img/`, or
//! `fonts/` — the three lanes `docs/assets.md` documents as bucket-served —
//! so this door cannot be used to litter the bucket outside them.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::api::{AdminSession, ApiError, ApiState};
use crate::public_asset_key_is_safe;

/// Largest object this door accepts. Matches
/// `store::documents::MAX_DOCUMENT_UPLOAD_BYTES` — generous headroom for a
/// hero image, a webfont family member, or a short brand video, while
/// keeping a runaway upload from parking an oversized object in the public
/// bucket.
pub const MAX_ASSET_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// Maximum REST request size needed to carry [`MAX_ASSET_UPLOAD_BYTES`] as
/// base64 JSON — the same `div_ceil` expansion
/// `store::documents::MAX_DOCUMENT_UPLOAD_REQUEST_BYTES` uses, so a
/// maximum-size asset does not spend its JSON allowance on the encoding.
pub const MAX_ASSET_UPLOAD_REQUEST_BYTES: usize =
    MAX_ASSET_UPLOAD_BYTES.div_ceil(3) * 4 + MAX_ASSET_UPLOAD_JSON_BYTES;

/// Room the request cap leaves for the upload's own JSON fields (`key`,
/// `content_type`, `sha256`).
const MAX_ASSET_UPLOAD_JSON_BYTES: usize = 1024;

/// The only top-level prefixes a public asset may publish under — the three
/// bucket-served lanes `docs/assets.md` documents. Rejecting everything
/// else keeps this door from becoming a general-purpose bucket-write API.
const ALLOWED_KEY_PREFIXES: &[&str] = &["brand/", "img/", "fonts/"];

/// `Cache-Control` stamped on the stored object, matching
/// `crate::STATIC_CACHE_CONTROL`'s value for `/public/` static assets — one
/// hour, the conservative default until asset paths are fingerprinted.
const ASSET_CACHE_CONTROL: &str = "public, max-age=3600";

/// `POST /app/api/assets` request body. Bytes travel base64-encoded, the
/// same shape every other `/app/api` upload door uses. `sha256` is the
/// caller's own digest of the decoded bytes, checked against what the
/// server actually decodes — catching any base64/JSON transit corruption
/// before a single byte reaches storage, which a post-write read-back alone
/// cannot: a corrupted decode would round-trip through storage unchanged.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadAssetRequest {
    /// The exact bucket key, e.g. `img/vesta-home/logo.svg` or
    /// `brand/rabbit.svg` — reachable afterwards at `/assets/{key}`.
    key: String,
    /// Base64-encoded file bytes.
    content_base64: String,
    /// Must equal the type [`expected_content_type`] derives from `key`'s
    /// extension — this door derives the type itself rather than trusting
    /// an arbitrary caller-chosen one, and the field exists so the request
    /// states what it believes it is sending.
    content_type: String,
    /// Lowercase hex SHA-256 of the decoded bytes.
    sha256: String,
}

/// The response `navigator site asset upload` renders per host.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct UploadAssetResponse {
    key: String,
    bytes: usize,
    content_type: String,
    sha256: String,
    /// `true` when an object already sat at `key` with these exact bytes
    /// and content type, so the write was skipped — the explicit answer to
    /// ENG-909's idempotency requirement, rather than leaving a caller to
    /// infer it from a same-looking response.
    unchanged: bool,
}

fn bad_request(error: &'static str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response()
}

/// A safe key ([`public_asset_key_is_safe`]'s traversal/control-character
/// checks) that also falls under one of [`ALLOWED_KEY_PREFIXES`].
fn asset_key_is_allowed(key: &str) -> bool {
    public_asset_key_is_safe(key)
        && ALLOWED_KEY_PREFIXES
            .iter()
            .any(|prefix| key.starts_with(prefix))
}

/// The content type this door accepts for `key`, derived from its
/// extension rather than trusted from the caller. `fonts/<family>/OFL.txt`
/// is the one non-image, non-font exception — the upstream license notice
/// `docs/assets.md` requires travel alongside each bucket-served font
/// family.
fn expected_content_type(key: &str) -> Option<&'static str> {
    let path = Path::new(key);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("avif") => Some("image/avif"),
        Some("webp") => Some("image/webp"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("svg") => Some("image/svg+xml"),
        Some("ico") => Some("image/x-icon"),
        Some("mp4") => Some("video/mp4"),
        Some("woff2") => Some("font/woff2"),
        Some("woff") => Some("font/woff"),
        Some("txt")
            if key.starts_with("fonts/")
                && path.file_name().and_then(|name| name.to_str()) == Some("OFL.txt") =>
        {
            Some("text/plain")
        }
        _ => None,
    }
}

/// `POST /app/api/assets` — publish one public-safe asset to the deployment's
/// public assets bucket. Owner/Admin only; the [`AdminSession`] extractor
/// rejects an anonymous (401) or non-admin (403) caller before the body is
/// even parsed.
///
/// `400 invalid_key` for a key outside `brand/`, `img/`, or `fonts/`, an
/// absolute path, or a `.`/`..` segment; `400 unsupported_content_type` for
/// an extension this door does not publish; `400 content_type_mismatch`
/// when the declared `content_type` does not match what the key's
/// extension requires; `400 content_unreadable` for missing, non-base64, or
/// empty bytes; `400 asset_too_large` over [`MAX_ASSET_UPLOAD_BYTES`]; `400
/// invalid_font` for a `.woff2` payload missing the `wOF2` signature; `400
/// invalid_license` for an `OFL.txt` that is not valid UTF-8 or exceeds
/// 64 KiB; `400 sha256_mismatch` when the declared digest does not match
/// the decoded bytes.
///
/// Writing the same key with the same bytes and content type again is a
/// no-op — `unchanged: true`, `200 OK` — rather than rewriting an identical
/// object; a new or changed key writes through `cloud::StorageService` and
/// answers `201 Created`. After any write (or confirming no write was
/// needed) the handler reads the object straight back from
/// `assets_storage` — the same storage the public `/assets/{key}` route
/// serves — so the returned `sha256` and `bytes` certify what a browser
/// will actually receive.
#[allow(clippy::too_many_lines)]
pub(crate) async fn upload_asset_door(
    State(state): State<ApiState>,
    _admin: AdminSession,
    Json(input): Json<UploadAssetRequest>,
) -> Result<Response, ApiError> {
    let key = input.key.trim().replace('\\', "/");
    if !asset_key_is_allowed(&key) {
        return Ok(bad_request(
            "invalid_key",
            "key must be a safe path below brand/, img/, or fonts/, with no `..` segment.",
        ));
    }
    let Some(expected_type) = expected_content_type(&key) else {
        return Ok(bad_request(
            "unsupported_content_type",
            "key has an extension this door does not publish.",
        ));
    };
    if input.content_type.trim() != expected_type {
        return Ok(bad_request(
            "content_type_mismatch",
            &format!("key `{key}` requires content type `{expected_type}`."),
        ));
    }

    let bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(input.content_base64.as_bytes())
            .ok()
            .filter(|bytes| !bytes.is_empty())
    };
    let Some(bytes) = bytes else {
        return Ok(bad_request(
            "content_unreadable",
            "content_base64 is missing, not valid base64, or decodes to zero bytes.",
        ));
    };
    if bytes.len() > MAX_ASSET_UPLOAD_BYTES {
        return Ok(bad_request(
            "asset_too_large",
            &format!(
                "the asset is {} bytes; the limit is {MAX_ASSET_UPLOAD_BYTES} bytes.",
                bytes.len()
            ),
        ));
    }
    if expected_type == "font/woff2" && !bytes.starts_with(b"wOF2") {
        return Ok(bad_request(
            "invalid_font",
            "a font/woff2 asset must begin with the wOF2 signature.",
        ));
    }
    if expected_type == "text/plain"
        && (bytes.len() > 64 * 1024 || std::str::from_utf8(&bytes).is_err())
    {
        return Ok(bad_request(
            "invalid_license",
            "OFL.txt must be valid UTF-8 text no larger than 64 KiB.",
        ));
    }

    let sha256 = store::assets::sha256_hex(&bytes);
    if input.sha256.trim().to_ascii_lowercase() != sha256 {
        return Ok(bad_request(
            "sha256_mismatch",
            "sha256 does not match the decoded asset bytes.",
        ));
    }

    let unchanged = match state.assets_storage.get(&key).await {
        Ok(existing) => existing.bytes == bytes && existing.content_type == expected_type,
        Err(cloud::StorageError::NotFound(_)) => false,
        Err(error) => {
            tracing::error!(error = %error, asset_key = %key, "api: public asset preflight read failed");
            return Err(ApiError::Db(
                "the asset store could not be read".to_string(),
            ));
        }
    };
    if !unchanged {
        state
            .assets_storage
            .put_cached(&key, &bytes, expected_type, ASSET_CACHE_CONTROL)
            .await
            .map_err(|error| {
                tracing::error!(error = %error, asset_key = %key, "api: public asset write failed");
                ApiError::Db("the asset could not be stored".to_string())
            })?;
    }

    // Read the object straight back from the same storage the public
    // `/assets/{key}` route serves, so the digest and byte count reported
    // to the caller certify what a browser will actually receive rather
    // than merely what was sent (ENG-909's read-back requirement).
    let confirmed = state.assets_storage.get(&key).await.map_err(|error| {
        tracing::error!(error = %error, asset_key = %key, "api: public asset read-back failed");
        ApiError::Db("the asset was written but could not be read back".to_string())
    })?;

    let status = if unchanged {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(UploadAssetResponse {
            key,
            bytes: confirmed.bytes.len(),
            content_type: confirmed.content_type,
            sha256: store::assets::sha256_hex(&confirmed.bytes),
            unchanged,
        }),
    )
        .into_response())
}
