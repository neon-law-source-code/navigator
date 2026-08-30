//! Admin governed-expunge mutation (git-repos surfaces Task 1).
//!
//! `POST /app/lawyer/documents/:doc_id/expunge` drives [`crate::expunge::expunge`]
//! for the chosen document, then redirects (post/redirect/get) back to the
//! Dioxus surface at the same path: `?record=` carries the audit-row id the
//! result state renders, `?error=` a rejected submit. The confirmation screen
//! itself is `webapp::expunge_document` (#956 Phase 4).
//!
//! # Authorization
//!
//! The surface is **admin-only**. Although the expunge primitive itself
//! re-checks that the authorizer is an admin (the gate lives in the
//! primitive, not the caller), this handler also 404s any non-admin
//! session *before* the dangerous act runs, so the route's existence
//! isn't disclosed to lawyer or clients. In production the admin
//! sub-router's embedded Rego policy layer already blocks unauthenticated traffic; this is
//! the role-tier check on top.
//!
//! The chosen `documents` row resolves to the repo path (its filename)
//! and the object-storage key (the joined `blobs` row's `storage_key`);
//! the authorizer is the acting admin's `persons` id from the session.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use serde::Deserialize;
use store::expunge_records;
use uuid::Uuid;

use crate::admin::AdminState;
use crate::session::SessionData;

/// True only for an `admin` session. Lawyer and clients are treated as
/// if the route did not exist.
fn is_admin(session: Option<&SessionData>) -> bool {
    session.is_some_and(|s| s.role.is_admin_tier())
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
}

/// Map a posted category string to one of the canonical
/// `expunge_records::CATEGORY_*` constants, or `None` if unrecognized.
fn canonical_category(raw: &str) -> Option<&'static str> {
    match raw.trim() {
        expunge_records::CATEGORY_PRIVILEGE => Some(expunge_records::CATEGORY_PRIVILEGE),
        expunge_records::CATEGORY_SEALING => Some(expunge_records::CATEGORY_SEALING),
        expunge_records::CATEGORY_CLIENT_REQUEST => Some(expunge_records::CATEGORY_CLIENT_REQUEST),
        _ => None,
    }
}

/// Resolve a document `assets` row, whose `filename` is the repo path to
/// expunge and whose `storage_key` is the object-storage key to delete.
/// `None` if the row is missing.
async fn load_doc(state: &AdminState, doc_id: Uuid) -> Option<store::assets::Asset> {
    store::assets::find_by_id(&state.surreal, doc_id)
        .await
        .ok()
        .flatten()
}

/// Redirect back to the Dioxus confirmation screen carrying an `?error=` flash.
fn back_with_error(doc_id: Uuid, message: &str) -> Response {
    Redirect::to(&format!(
        "/app/lawyer/documents/{doc_id}/expunge?error={}",
        crate::admin::encode_query_value(message)
    ))
    .into_response()
}

#[derive(Deserialize)]
pub struct ExpungeForm {
    category: String,
    #[serde(default)]
    note: String,
}

/// `POST /app/lawyer/documents/:doc_id/expunge`.
pub async fn run(
    State(state): State<AdminState>,
    Path(doc_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
    Form(input): Form<ExpungeForm>,
) -> Response {
    if !is_admin(session.as_deref()) {
        return not_found();
    }
    let Some(doc) = load_doc(&state, doc_id).await else {
        return not_found();
    };
    let (Some(project_id), Some(filename)) = (doc.project_id, doc.filename.as_deref()) else {
        return not_found();
    };

    // Validate the category at the edge so a bad value re-shows the form with
    // its flash instead of bubbling a primitive error up as a 500.
    let Some(category) = canonical_category(&input.category) else {
        return back_with_error(doc_id, "Choose one of the listed expunge categories.");
    };

    // The authorizer must be a known person — the audit row records who.
    let Some(authorized_by) = session.as_deref().and_then(|s| s.person_id) else {
        return (
            StatusCode::FORBIDDEN,
            "No linked person on the session; cannot attribute the expunge.",
        )
            .into_response();
    };

    let note = input.note.trim();
    let note = (!note.is_empty()).then_some(note);

    match crate::expunge::expunge(
        &state.surreal,
        &state.storage,
        crate::expunge::ExpungeRequest {
            project_id,
            path: filename,
            category,
            authorized_by,
            storage_keys: crate::expunge::storage_keys_for_asset(&doc),
            note,
        },
    )
    .await
    {
        // Post/redirect/get: the audit-row id travels in the query, and the
        // Dioxus surface renders the result state from that row. A refresh
        // re-reads the audit record instead of re-posting the expunge.
        Ok(record_id) => Redirect::to(&format!(
            "/app/lawyer/documents/{doc_id}/expunge?record={record_id}"
        ))
        .into_response(),
        // The primitive's own admin gate — should be unreachable behind
        // `is_admin`, but map it honestly rather than as a 500.
        Err(crate::expunge::ExpungeError::NotAdmin) => not_found(),
        Err(e) => {
            tracing::error!(error = %e, %doc_id, %project_id,
                "governed expunge failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response()
        }
    }
}
