//! `/app/projects/{project_code}/review/:doc_id` — the comment-only client
//! review surface.
//!
//! A client reads one attorney-reviewed draft (a will, a trust, a
//! directive) and leaves comments anchored to a text range. The surface
//! is read-only: a comment is the only thing the client writes. The page
//! is row-scoped to the matter exactly like the rest of `/app/projects/*` — a
//! non-participant gets `404`, never `403`. A draft is only reachable
//! once an attorney has advanced it past `draft` (the human-in-the-loop
//! gate the marketing copy promises): a `draft`-status row 404s here.
//!
//! Three routes:
//!
//! - `GET …/review/:doc_id` — the read-only document + comment sidebar.
//! - `POST …/review/:doc_id/comments` — create one anchored comment
//!   (form-encoded, CSRF-checked, comes from the viewer element).
//! - `GET …/review/:doc_id/comments` — the comment thread as JSON, so
//!   the viewer can refresh without a full reload.

use std::collections::HashMap;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use store::persons::Role;
use store::review_documents::STATUS_DRAFT;

use crate::session::SessionData;
use store::access::can_see_project_as_client;

/// Resolve `(project_id, doc_id)` to a client-visible review document,
/// or a `404` response. Enforces, in order: the document exists, it
/// belongs to a notation in *this* project, the caller may see the
/// project, and the draft has been advanced past `draft` so a client
/// never sees an un-reviewed document.
async fn visible_review_document(
    surreal: &store::surreal::SurrealDb,
    session: &SessionData,
    project_id: Uuid,
    doc_id: Uuid,
) -> Result<store::review_documents::ReviewDocument, Response> {
    let doc = store::review_documents::by_id(surreal, doc_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(not_found)?;

    let notation = store::notations::find_by_id(surreal, doc.notation_id)
        .await
        .ok()
        .flatten()
        .ok_or_else(not_found)?;
    if notation.project_id != project_id {
        return Err(not_found());
    }

    let visible = can_see_project_as_client(surreal, session.person_id, project_id)
        .await
        .unwrap_or(false);
    if !visible {
        return Err(not_found());
    }

    if doc.status == STATUS_DRAFT {
        return Err(not_found());
    }
    Ok(doc)
}

/// One comment, shaped for the viewer's JSON contract.
#[derive(Debug, Serialize)]
pub struct CommentJson {
    pub id: Uuid,
    pub anchor_start: i32,
    pub anchor_end: i32,
    pub quoted_text: String,
    pub body: String,
    pub resolved: bool,
    pub author: String,
    pub inserted_at: String,
}

/// Load a document's comments with author display names resolved in one
/// batched query (no N+1).
async fn load_comments(surreal: &store::surreal::SurrealDb, doc_id: Uuid) -> Vec<CommentJson> {
    let rows = store::document_comments::for_review_document(surreal, doc_id)
        .await
        .unwrap_or_default();
    let author_ids: Vec<Uuid> = rows.iter().map(|c| c.person_id).collect();
    let names: HashMap<Uuid, String> = if author_ids.is_empty() {
        HashMap::new()
    } else {
        store::persons::find_by_ids(surreal, &author_ids)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect()
    };
    rows.into_iter()
        .map(|c| CommentJson {
            author: names.get(&c.person_id).cloned().unwrap_or_default(),
            id: c.id,
            anchor_start: c.anchor_start,
            anchor_end: c.anchor_end,
            quoted_text: c.quoted_text,
            body: c.body,
            resolved: c.resolved,
            inserted_at: c.inserted_at.to_rfc3339(),
        })
        .collect()
}

/// `GET /app/projects/{project_code}/review/:doc_id/comments` — the thread as
/// JSON.
pub async fn list_comments(
    State(surreal): State<store::surreal::SurrealDb>,
    Path((project_code, doc_id)): Path<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    let Some(project_id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found();
    };
    let doc = match visible_review_document(&surreal, &session, project_id, doc_id).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    Json(load_comments(&surreal, doc.id).await).into_response()
}

/// Posted by the viewer when a reader anchors a comment to a selection.
#[derive(Debug, Deserialize)]
pub struct CommentForm {
    /// CSRF token — verified by the middleware before the handler runs;
    /// accepted here only so the form body parses.
    #[serde(rename = "_csrf", default)]
    pub csrf: String,
    pub anchor_start: i32,
    pub anchor_end: i32,
    pub quoted_text: String,
    pub body: String,
}

/// Typed failure of [`create_review_comment`], the command shared by the
/// browser form ([`create_comment`]) and the REST door
/// (`POST /app/api/review-documents/{doc_id}/comments`).
#[derive(Debug)]
pub enum ReviewCommentError {
    /// No such review document, it is still a draft, it is outside the
    /// caller's (client-lens) matter scope, the caller can't author (anonymous
    /// or a Clerk), or a nested-route project mismatch. All collapse to a bare
    /// 404 so the door never discloses a document the caller cannot see.
    NotFound,
    /// The body is blank or the anchor range is invalid.
    Invalid(&'static str),
    /// A database read or write failed.
    Db(String),
}

/// Create one anchored comment on a review document and fold it into the
/// matter's privileged conversation log. Shared by both doors so they resolve,
/// scope, and attribute a comment identically.
///
/// Access is **client-lens**: the caller must participate in the document's
/// matter through the client side ([`can_see_project_as_client`]) — the same
/// gate the read-only review surface enforces, so a firm-side-only lawyer sees
/// a 404 here exactly as on the portal. `direction` is derived from the
/// caller's role: a client's comment flows inbound, a lawyer/admin comment the
/// client reads flows outbound; a Clerk has no review-comment capability yet
/// and fails closed. `expected_project_id` is the nested browser route's
/// path project (checked against the document's real matter); the REST door,
/// keyed on the document alone, passes `None`.
#[allow(clippy::too_many_arguments)]
pub async fn create_review_comment(
    surreal: &store::surreal::SurrealDb,
    role: Role,
    person_id: Option<Uuid>,
    doc_id: Uuid,
    expected_project_id: Option<Uuid>,
    anchor_start: i32,
    anchor_end: i32,
    quoted_text: &str,
    body: &str,
) -> Result<store::document_comments::CreatedComment, ReviewCommentError> {
    let doc = store::review_documents::by_id(surreal, doc_id)
        .await
        .map_err(|e| ReviewCommentError::Db(e.to_string()))?
        .ok_or(ReviewCommentError::NotFound)?;
    let notation = store::notations::find_by_id(surreal, doc.notation_id)
        .await
        .map_err(|e| ReviewCommentError::Db(e.to_string()))?
        .ok_or(ReviewCommentError::NotFound)?;
    if let Some(expected) = expected_project_id {
        if notation.project_id != expected {
            return Err(ReviewCommentError::NotFound);
        }
    }
    let visible = can_see_project_as_client(surreal, person_id, notation.project_id)
        .await
        .unwrap_or(false);
    if !visible || doc.status == STATUS_DRAFT {
        return Err(ReviewCommentError::NotFound);
    }
    // A comment must be attributable to a person; an anonymous session can't
    // author one.
    let Some(person_id) = person_id else {
        return Err(ReviewCommentError::NotFound);
    };
    let body = body.trim();
    if body.is_empty() || anchor_end <= anchor_start {
        return Err(ReviewCommentError::Invalid(
            "empty comment or invalid range",
        ));
    }
    let direction = match role {
        Role::Client => store::communications::direction::INBOUND,
        // Clerk has no review-comment route until the separately supervised
        // Clerk capability exists; fail closed even under a test policy.
        Role::Clerk => return Err(ReviewCommentError::NotFound),
        Role::Owner | Role::Admin | Role::Lawyer => store::communications::direction::OUTBOUND,
    };
    store::document_comments::create_with_communication(
        surreal,
        &store::document_comments::NewLinkedComment {
            project_id: notation.project_id,
            review_document_id: doc.id,
            person_id,
            direction,
            anchor_start,
            anchor_end,
            quoted_text: quoted_text.trim(),
            body,
        },
    )
    .await
    .map_err(|e| ReviewCommentError::Db(e.to_string()))
}

/// `POST /app/projects/{project_code}/review/:doc_id/comments` — create one
/// anchored comment and return the refreshed thread as JSON.
pub async fn create_comment(
    State(surreal): State<store::surreal::SurrealDb>,
    Path((project_code, doc_id)): Path<(String, Uuid)>,
    session: Option<Extension<SessionData>>,
    axum::Form(form): axum::Form<CommentForm>,
) -> Response {
    let Some(Extension(session)) = session else {
        return not_found();
    };
    let Some(project_id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return not_found();
    };

    match create_review_comment(
        &surreal,
        session.role,
        session.person_id,
        doc_id,
        Some(project_id),
        form.anchor_start,
        form.anchor_end,
        &form.quoted_text,
        &form.body,
    )
    .await
    {
        Ok(_) => Json(load_comments(&surreal, doc_id).await).into_response(),
        Err(ReviewCommentError::NotFound) => not_found(),
        Err(ReviewCommentError::Invalid(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(ReviewCommentError::Db(e)) => {
            tracing::error!(error = %e, "review: create_comment failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response()
        }
    }
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        webapp::error_pages::not_found_signed_in(),
    )
        .into_response()
}
