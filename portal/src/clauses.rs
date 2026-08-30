//! `/app/lawyer/notations/:id/clauses` — the admin clause editor.
//!
//! Lawyers add, edit, reorder, and remove the custom paragraphs spliced
//! into a single notation's assembled document (at the template body's
//! `{{custom_clauses}}` marker) before it is sent. Per-matter prose
//! without forking the shared template.
//!
//! Any clause is half of the review gate: a notation carrying custom
//! prose is routed back through `lawyer_review` before signature (see
//! `portal::retainer_walk`), so the bytes the attorney approves are the
//! bytes that get signed.

use axum::extract::{Extension, Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use uuid::Uuid;

use crate::admin::AdminState;
use crate::session::SessionData;

/// The bound template's title, for the page chrome.
async fn flow_label(state: &AdminState, notation_id: Uuid) -> Option<String> {
    let n = store::notations::find_by_id(&state.surreal, notation_id)
        .await
        .ok()
        .flatten()?;
    let t = store::templates::find_by_id(&state.surreal, n.template_id)
        .await
        .ok()
        .flatten()?;
    Some(t.title)
}

fn redirect_to_clauses(notation_id: Uuid) -> Response {
    Redirect::to(&format!("/app/lawyer/notations/{notation_id}/clauses")).into_response()
}

/// `GET /app/lawyer/notations/:id/clauses?format=json` — the thin JSON surface the
/// `navigator retainer clause list` CLI consumes (the same `format=json`
/// convention as the notation review route).
///
/// The HTML editor on the same path renders through Dioxus (#956 Phase 4). This
/// answers only the JSON query: the Dioxus route's pre-layer calls it when
/// `?format=json` is present and returns the result instead of rendering, so one
/// path keeps serving both. `None` means "not a JSON request — render the page".
pub(crate) async fn clauses_json(
    state: &AdminState,
    notation_id: Uuid,
    format: &str,
) -> Option<Response> {
    if format != "json" {
        return None;
    }
    if flow_label(state, notation_id).await.is_none() {
        return Some((StatusCode::NOT_FOUND, "notation not found").into_response());
    }
    let clauses = store::notation_clauses::for_notation(&state.surreal, notation_id)
        .await
        .unwrap_or_default();
    let json: Vec<_> = clauses
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id,
                "position": c.position,
                "body": c.body_markdown,
                "system_authored": c.authored_by_person_id.is_none(),
            })
        })
        .collect();
    Some(axum::Json(json).into_response())
}

/// POST body for adding / editing a clause.
#[derive(Debug, Deserialize)]
pub struct ClauseBody {
    pub body: String,
}

/// `POST /app/lawyer/notations/:id/clauses` — append one clause.
pub async fn clause_add(
    State(state): State<AdminState>,
    Path(notation_id): Path<Uuid>,
    session: Option<Extension<SessionData>>,
    Form(form): Form<ClauseBody>,
) -> Response {
    let body = form.body.trim();
    if body.is_empty() {
        return redirect_to_clauses(notation_id);
    }
    let author = session.as_deref().and_then(|s| s.person_id);
    if let Err(e) = store::notation_clauses::append(&state.surreal, notation_id, body, author).await
    {
        tracing::error!(error = %e, %notation_id, "clauses: append failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }
    redirect_to_clauses(notation_id)
}

/// `POST /app/lawyer/notations/:id/clauses/:cid/edit` — replace a
/// clause's body.
pub async fn clause_edit(
    State(state): State<AdminState>,
    Path((notation_id, clause_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<ClauseBody>,
) -> Response {
    let body = form.body.trim();
    if body.is_empty() {
        return redirect_to_clauses(notation_id);
    }
    if let Err(e) = store::notation_clauses::update_body(&state.surreal, clause_id, body).await {
        tracing::error!(error = %e, %clause_id, "clauses: update failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }
    redirect_to_clauses(notation_id)
}

/// `POST /app/lawyer/notations/:id/clauses/:cid/delete`.
pub async fn clause_delete(
    State(state): State<AdminState>,
    Path((notation_id, clause_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(e) = store::notation_clauses::delete(&state.surreal, clause_id).await {
        tracing::error!(error = %e, %clause_id, "clauses: delete failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }
    redirect_to_clauses(notation_id)
}

/// POST body for reordering a clause.
#[derive(Debug, Deserialize)]
pub struct MoveBody {
    pub direction: String,
}

/// `POST /app/lawyer/notations/:id/clauses/:cid/move` — swap a clause
/// with its neighbour (`direction=up|down`).
pub async fn clause_move(
    State(state): State<AdminState>,
    Path((notation_id, clause_id)): Path<(Uuid, Uuid)>,
    Form(form): Form<MoveBody>,
) -> Response {
    let up = form.direction == "up";
    if let Err(e) = store::notation_clauses::move_clause(&state.surreal, clause_id, up).await {
        tracing::error!(error = %e, %clause_id, "clauses: move failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "internal").into_response();
    }
    redirect_to_clauses(notation_id)
}
