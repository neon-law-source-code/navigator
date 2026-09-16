//! The `/app/api/authorities` door (ENG-712).
//!
//! One operation: create (or find) the global Authority for a citation,
//! archiving the submitted bytes through the Asset service first and
//! recording the resulting asset id as `archived_asset_id`. Lawyer-tier
//! only — minting global legal reference data is firm-side authoring, the
//! same tier every other `/app/api` authoring door takes.
//!
//! Bytes travel base64-encoded in the JSON body, the same shape
//! `POST /app/api/projects/{id}/documents` uses, so the CLI (the only
//! caller today — see `cli::authorities`) needs no multipart client.
//!
//! `store::authorities::record` is already find-or-create on `citation`
//! (#890): a repeat of the same citation returns the existing global row
//! untouched, including its original `archived_asset_id`. This door adds
//! no new semantics there — it only reaches the seam, which is the whole
//! point of the issue: before this, nothing outside `store`'s own tests
//! called `record` at all.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use crate::api::{ApiError, ApiState, LawyerSession};

/// `POST /app/api/authorities` request body. `archive_base64` is the
/// archived artifact's bytes; `content_type` defaults to
/// `application/octet-stream` when absent or blank, the same default
/// `POST /app/api/projects/{id}/documents` uses.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateAuthorityRequest {
    class: String,
    citation: String,
    title: String,
    short_cite: Option<String>,
    publisher: Option<String>,
    issued_on: Option<String>,
    canonical_url: Option<String>,
    checked_on: Option<String>,
    archive_base64: String,
    content_type: Option<String>,
}

/// A `400 Bad Request` JSON body in the shared `{error, message}` shape —
/// its own copy of `api::bad_request`, which is private to that module.
fn bad_request(error: &'static str, message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": error, "message": message })),
    )
        .into_response()
}

/// A present, non-blank, trimmed value — `None` for an absent or
/// whitespace-only field. Every optional `NewAuthority` field goes through
/// this so a caller sending `""` for `short_cite` gets the same `None` a
/// caller who omitted the field entirely would.
fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

/// `POST /app/api/authorities` — create (or find) the global Authority for
/// `citation`, archiving `archive_base64` as a bare content asset first.
///
/// Lawyer-tier only; the [`LawyerSession`] extractor rejects an anonymous
/// (401) or non-lawyer (403) caller before the body is even parsed.
/// `400 invalid_class` when `class` is outside
/// [`rules::citation::AuthorityClass`]; `400 archive_unreadable` when the
/// archive is missing, not valid base64, or decodes to zero bytes. The
/// archive is ingested through [`store::assets::ingest_content`] before
/// [`store::authorities::record`] runs, so an asset-persistence failure
/// records no Authority row.
pub(crate) async fn create_authority_door(
    State(state): State<ApiState>,
    _lawyer: LawyerSession,
    Json(input): Json<CreateAuthorityRequest>,
) -> Result<Response, ApiError> {
    let Some(class) = rules::citation::AuthorityClass::parse(input.class.trim()) else {
        return Ok(bad_request(
            "invalid_class",
            &format!(
                "class must be one of: {}",
                rules::citation::AuthorityClass::ALL
                    .iter()
                    .map(|class| class.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    };

    let bytes = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(input.archive_base64.as_bytes())
            .ok()
            .filter(|bytes| !bytes.is_empty())
    };
    let Some(bytes) = bytes else {
        return Ok(bad_request(
            "archive_unreadable",
            "archive_base64 is missing, not valid base64, or decodes to zero bytes.",
        ));
    };

    let content_type = input
        .content_type
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("application/octet-stream");
    let asset_id =
        match store::assets::ingest_content(&state.surreal, &state.storage, &bytes, content_type)
            .await
        {
            Ok(id) => id,
            Err(error) => {
                tracing::error!(error = %error, "api: authority archive ingest failed");
                return Err(ApiError::Db(
                    "the archived artifact could not be persisted".to_string(),
                ));
            }
        };

    let new = store::authorities::NewAuthority {
        class,
        citation: input.citation.trim(),
        short_cite: trimmed(input.short_cite.as_deref()),
        title: input.title.trim(),
        publisher: trimmed(input.publisher.as_deref()),
        issued_on: trimmed(input.issued_on.as_deref()),
        canonical_url: trimmed(input.canonical_url.as_deref()),
        checked_on: trimmed(input.checked_on.as_deref()),
        archived_asset_id: Some(asset_id),
    };
    match store::authorities::record(&state.surreal, &new).await {
        Ok(authority) => Ok(Json(authority).into_response()),
        Err(error) => {
            tracing::error!(error = %error, "api: authority record failed");
            Err(ApiError::Db(
                "the authority could not be recorded".to_string(),
            ))
        }
    }
}
