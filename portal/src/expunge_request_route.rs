//! Client-initiated document deletion — request, then attorney
//! authorization (git-repos surfaces Task 2).
//!
//! The governed-expunge primitive is admin-only; a client can only
//! *ask*. This module wires both halves:
//!
//! - **Client (request-only).** `POST
//!   /app/projects/{project_code}/documents/:doc_id/request-deletion` records a
//!   `pending` [`store::expunge_requests`] row. Nothing is deleted. The
//!   client UI honestly shows "deletion requested" until an attorney
//!   acts — never "deleted" before the bytes are actually gone.
//! - **Lawyer/admin (authorize → execute).** `GET
//!   /app/lawyer/expunge-requests` is the review queue; `POST
//!   .../:id/authorize` runs the admin-gated [`crate::expunge::expunge`]
//!   (category `client_request`) and links the audit row; `POST
//!   .../:id/deny` resolves it without deleting.
//!
//! No client-facing byte here ever mentions a repository — the surface
//! is about *documents*, not git.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use store::expunge_records;
use uuid::Uuid;

use crate::admin::AdminState;
use crate::session::SessionData;
use store::access::can_see_project_as_client;

fn is_admin(session: Option<&SessionData>) -> bool {
    session.is_some_and(|s| s.role.is_admin_tier())
}

fn is_lawyer_tier(session: Option<&SessionData>) -> bool {
    session.is_some_and(|s| s.role.is_lawyer_tier())
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        webapp::error_pages::server_error(),
    )
        .into_response()
}

/// Typed failure of [`request_document_deletion`], the command shared by the
/// browser form ([`client_request`]) and the REST door
/// (`POST /app/api/documents/{doc_id}/deletion-requests`).
#[derive(Debug)]
pub enum RequestDeletionError {
    /// No such document, it belongs to no matter, it is outside the caller's
    /// (client-lens) scope, or a nested-route project mismatch. All collapse to
    /// a bare 404 so the door never discloses a document the caller cannot see.
    NotFound,
    /// The session carries no Person, so the request can't be attributed.
    NoRequester,
    /// The matter-scope access check failed.
    Db(String),
    /// Reading or writing the deletion request itself failed. Separate
    /// from [`Self::Db`] because `expunge_requests` moved to SurrealDB
    /// with ENG-160 and carries its own error type.
    Request(store::expunge_requests::ExpungeRequestError),
    /// Reading the document asset failed. Separate from [`Self::Db`]
    /// because `assets` moved to SurrealDB with ENG-121 and carries its
    /// own error type.
    Asset(store::assets::AssetError),
}

/// Whether a pending request already existed, so callers can report 200 vs 201.
pub struct DeletionRequestOutcome {
    pub request_id: Uuid,
    pub already_pending: bool,
}

/// Ensure a `pending` expunge request exists for `doc_id`, on behalf of
/// `person_id`. Shared by both doors so they scope and attribute a request
/// identically. Access is **client-lens** (`can_see_project_as_client`): the
/// caller must participate in the document's matter through the client side.
/// Idempotent: a second ask while one is pending is a no-op that returns the
/// existing request. `expected_project_id` is the nested browser route's path
/// project (checked against the document's real matter); the REST door, keyed
/// on the document alone, passes `None`.
pub async fn request_document_deletion(
    surreal: &store::surreal::SurrealDb,
    person_id: Option<Uuid>,
    doc_id: Uuid,
    expected_project_id: Option<Uuid>,
) -> Result<DeletionRequestOutcome, RequestDeletionError> {
    let doc = store::assets::find_by_id(surreal, doc_id)
        .await
        .map_err(RequestDeletionError::Asset)?
        .ok_or(RequestDeletionError::NotFound)?;
    let Some(project_id) = doc.project_id else {
        return Err(RequestDeletionError::NotFound);
    };
    if let Some(expected) = expected_project_id {
        if project_id != expected {
            return Err(RequestDeletionError::NotFound);
        }
    }
    match can_see_project_as_client(surreal, person_id, project_id).await {
        Ok(true) => {}
        Ok(false) => return Err(RequestDeletionError::NotFound),
        Err(e) => return Err(RequestDeletionError::Db(e)),
    }
    let Some(requester) = person_id else {
        return Err(RequestDeletionError::NoRequester);
    };

    // Idempotent: don't stack duplicate pending requests for one document.
    if let Some(existing) = store::expunge_requests::pending_for_document(surreal, doc_id)
        .await
        .map_err(RequestDeletionError::Request)?
    {
        return Ok(DeletionRequestOutcome {
            request_id: existing.id,
            already_pending: true,
        });
    }
    let request_id = store::expunge_requests::create(
        surreal,
        &store::expunge_requests::NewExpungeRequest {
            project_id,
            asset_id: doc_id,
            requested_by_person_id: requester,
            note: None,
        },
    )
    .await
    .map_err(RequestDeletionError::Request)?;
    Ok(DeletionRequestOutcome {
        request_id,
        already_pending: false,
    })
}

/// `POST /app/projects/{project_code}/documents/:doc_id/request-deletion` — a
/// client (or any matter participant) asks for a document to be deleted.
/// Row-scoped like the rest of `/app`; creates one `pending` request
/// (idempotent — a second ask while one is pending is a no-op).
pub async fn client_request(
    State(state): State<AdminState>,
    Path((project_code, doc_id)): Path<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return not_found();
    };
    let person_id = session.as_deref().and_then(|s| s.person_id);
    match request_document_deletion(&state.surreal, person_id, doc_id, Some(project_id)).await {
        Ok(_) => {
            Redirect::to(&crate::dioxus_app::project_show_path(&state.surreal, project_id).await)
                .into_response()
        }
        Err(RequestDeletionError::NotFound) => not_found(),
        Err(RequestDeletionError::NoRequester) => {
            (StatusCode::FORBIDDEN, "No linked person on the session.").into_response()
        }
        Err(RequestDeletionError::Db(e)) => {
            tracing::error!(error = %e, %doc_id, "request-deletion: failed");
            internal_error()
        }
        Err(RequestDeletionError::Asset(e)) => {
            tracing::error!(error = %e, %doc_id, "request-deletion: failed");
            internal_error()
        }
        Err(RequestDeletionError::Request(e)) => {
            tracing::error!(error = %e, %doc_id, "request-deletion: failed");
            internal_error()
        }
    }
}
/// `POST /app/lawyer/expunge-requests/:id/authorize` — **admin only**:
/// run the governed expunge for the requested document, then mark the
/// request authorized and link the audit row.
/// Why resolving a client expunge request failed. Shared by the lawyer queue
/// forms and the `/app/api/expunge-requests/{id}/{authorize,deny}` doors.
#[derive(Debug)]
pub enum ExpungeRequestActionError {
    /// No such request, or its document asset is gone.
    NotFound,
    /// The request is no longer pending (already authorized or denied).
    AlreadyResolved,
    /// A store write or the expunge itself failed.
    Db(String),
}

impl std::fmt::Display for ExpungeRequestActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "expunge request not found"),
            Self::AlreadyResolved => write!(f, "expunge request already resolved"),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl std::error::Error for ExpungeRequestActionError {}

/// Authorize a pending client expunge request: run the governed expunge on the
/// requested document and mark the request authorized, linked to the audit
/// record. The one command behind both the lawyer queue's authorize form and the
/// REST door. The caller (`authorizer`) is the admin resolving it; tier
/// authorization is the adapter's job.
pub async fn authorize_expunge_request(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    request_id: Uuid,
    authorizer: Uuid,
) -> Result<(), ExpungeRequestActionError> {
    let req = store::expunge_requests::by_id(surreal, request_id)
        .await
        .map_err(|e| ExpungeRequestActionError::Db(e.to_string()))?
        .ok_or(ExpungeRequestActionError::NotFound)?;
    if req.status != store::expunge_requests::STATUS_PENDING {
        return Err(ExpungeRequestActionError::AlreadyResolved);
    }
    // Resolve the document asset → repo path (filename) + storage keys.
    let doc = store::assets::find_by_id(surreal, req.asset_id)
        .await
        .map_err(|e| ExpungeRequestActionError::Db(e.to_string()))?
        .ok_or(ExpungeRequestActionError::NotFound)?;
    let filename = doc
        .filename
        .as_deref()
        .ok_or(ExpungeRequestActionError::NotFound)?;
    let record_id = crate::expunge::expunge(
        surreal,
        storage,
        crate::expunge::ExpungeRequest {
            project_id: req.project_id,
            path: filename,
            category: expunge_records::CATEGORY_CLIENT_REQUEST,
            authorized_by: authorizer,
            storage_keys: crate::expunge::storage_keys_for_asset(&doc),
            note: None,
        },
    )
    .await
    .map_err(|e| ExpungeRequestActionError::Db(e.to_string()))?;
    store::expunge_requests::authorize(surreal, request_id, authorizer, record_id)
        .await
        .map_err(|e| ExpungeRequestActionError::Db(e.to_string()))?;
    Ok(())
}

/// Deny a pending client expunge request without deleting anything. The one
/// command behind both the lawyer queue's deny form and the REST door.
pub async fn deny_expunge_request(
    surreal: &store::surreal::SurrealDb,
    request_id: Uuid,
    resolver: Uuid,
) -> Result<(), ExpungeRequestActionError> {
    match store::expunge_requests::deny(surreal, request_id, resolver).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(ExpungeRequestActionError::NotFound),
        Err(e) => Err(ExpungeRequestActionError::Db(e.to_string())),
    }
}

pub async fn admin_authorize(
    State(state): State<AdminState>,
    Path(request_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    if !is_admin(session.as_deref()) {
        return not_found();
    }
    let Some(authorizer) = session.as_deref().and_then(|s| s.person_id) else {
        return (StatusCode::FORBIDDEN, "No linked person on the session.").into_response();
    };
    match authorize_expunge_request(&state.surreal, &state.storage, request_id, authorizer).await {
        // Already resolved — back to the queue rather than re-running.
        Ok(()) | Err(ExpungeRequestActionError::AlreadyResolved) => {
            Redirect::to("/app/lawyer/expunge-requests").into_response()
        }
        Err(ExpungeRequestActionError::NotFound) => not_found(),
        Err(e) => {
            tracing::error!(error = %e, %request_id, "authorize expunge request failed");
            internal_error()
        }
    }
}

/// `POST /app/lawyer/expunge-requests/:id/deny` — lawyer or admin
/// resolve a request without deleting anything.
pub async fn admin_deny(
    State(state): State<AdminState>,
    Path(request_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
) -> Response {
    if !is_lawyer_tier(session.as_deref()) {
        return not_found();
    }
    let Some(resolver) = session.as_deref().and_then(|s| s.person_id) else {
        return (StatusCode::FORBIDDEN, "No linked person on the session.").into_response();
    };
    match deny_expunge_request(&state.surreal, request_id, resolver).await {
        Ok(()) => Redirect::to("/app/lawyer/expunge-requests").into_response(),
        Err(ExpungeRequestActionError::NotFound) => not_found(),
        Err(e) => {
            tracing::error!(error = %e, %request_id, "deny expunge request failed");
            internal_error()
        }
    }
}
