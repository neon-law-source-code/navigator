//! The comment-only client document-review page
//! (`/app/projects/{project_code}/review/{doc_id}`) as a Dioxus component (#641 Phase
//! 3).
//!
//! A client reads one attorney-reviewed draft (a will, a trust, a directive) and
//! leaves comments anchored to a text range; the surface is read-only, a comment
//! is the only thing the client writes. Row-scoped to the matter like the rest
//! of `/app/*`: a non-participant gets `404`, and a `draft`-status row also
//! `404`s (a draft is only reachable once an attorney has advanced it past
//! `draft` — the human-in-the-loop gate).
//!
//! The comment/selection/highlight behaviour is a first-party custom element,
//! `<document-review>`, upgraded by an external same-origin script
//! (`/public/js/document-review.js`, allowed by `script-src 'self'`, no nonce).
//! The sanitized draft body is injected as raw HTML (`dangerous_inner_html`), and
//! the comment thread is handed to the element as a JSON data attribute. The
//! layout CSS lives in the theme stylesheet (`.document-review-page`).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::csrf::CsrfToken;

/// The rendered review page — every field wasm-safe.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ReviewView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub project_id: String,
    pub project_code: String,
    pub doc_id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    /// The attorney-reviewed, server-sanitized draft body as HTML.
    pub body_html: String,
    /// The comment thread serialized as JSON for the viewer element.
    pub comments_json: String,
    pub csrf_token: String,
}

/// Fetch one review draft for the signed-in client. Refuses a non-participant,
/// an unknown/other-matter doc, and a `draft`-status row with `404`; a query
/// failure is `500`.
#[server]
pub async fn get_review() -> Result<ReviewView, ServerFnError> {
    let axum::extract::Path((project_code, doc_id)) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::extract::Path<(String, uuid::Uuid)>,
            _,
        >()
        .await?;
    let csrf_token =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<CsrfToken>, _>()
            .await
            .map(|axum::Extension(token)| token.0)
            .unwrap_or_default();
    let person_id = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::portal_project_list::PersonId>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::Extension(pid)| pid.0)
    .and_then(|raw| raw.parse::<uuid::Uuid>().ok());

    let surreal = consume_context::<store::surreal::SurrealDb>();
    // The draft's link names its matter by code. A code naming no matter is the
    // same refusal a non-participant gets, so neither reveals the other's case.
    let Some(project_id) = store::projects::id_for_code(&surreal, &project_code).await else {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(ReviewView {
            csrf_token: csrf_token.clone(),
            ..ReviewView::default()
        });
    };

    // Visibility: the client lens gates the matter, and a draft is only reachable
    // once an attorney advanced it past `draft`. Any failure of these checks is
    // the same 404 — the draft "doesn't exist" for the caller.
    let not_found = |project_id: uuid::Uuid, doc_id: uuid::Uuid| {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        Ok(ReviewView {
            project_id: project_id.to_string(),
            doc_id: doc_id.to_string(),
            csrf_token: csrf_token.clone(),
            ..ReviewView::default()
        })
    };

    let Some(doc) = store::review_documents::by_id(&surreal, doc_id)
        .await
        .map_err(server_error)?
    else {
        return not_found(project_id, doc_id);
    };
    let Some(notation) = store::notations::find_by_id(&surreal, doc.notation_id)
        .await
        .map_err(server_error)?
    else {
        return not_found(project_id, doc_id);
    };
    if notation.project_id != project_id
        || !store::access::can_see_project_as_client(&surreal, person_id, project_id)
            .await
            .map_err(server_error)?
        || doc.status == store::review_documents::STATUS_DRAFT
    {
        return not_found(project_id, doc_id);
    }

    let comments = store::document_comments::for_review_document(&surreal, doc.id)
        .await
        .map_err(server_error)?;
    let comments_json = serde_json::to_string(&comments).unwrap_or_else(|_| "[]".to_string());
    let Some(project) = store::projects::find_by_id(&surreal, project_id)
        .await
        .map_err(server_error)?
    else {
        return not_found(project_id, doc_id);
    };

    Ok(ReviewView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        project_id: project_id.to_string(),
        project_code: project.code,
        doc_id: doc.id.to_string(),
        title: doc.title,
        kind: doc.kind,
        status: doc.status,
        body_html: doc.body_html,
        comments_json,
        csrf_token,
    })
}

/// Commit a `500` and wrap a query error.
#[cfg(feature = "server")]
fn server_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

/// The comment-only client document-review page, server-side rendered. The
/// document is readable before hydration; the `document-review` custom element
/// upgrades it (text selection, comment sidebar, range highlights) once its
/// same-origin script loads.
#[component]
pub fn Review() -> Element {
    let resource = use_server_future(get_review)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if !view.title.is_empty() => view.clone(),
        Some(Ok(_)) => {
            return rsx! {
                main { id: "review", p { "That document was not found." } }
            }
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "review", p { "Failed to load this document." } }
            }
        }
        None => {
            return rsx! {
                main { id: "review", p { "Loading…" } }
            }
        }
    };

    let comments_url = format!(
        "/app/projects/{}/review/{}/comments",
        view.project_code, view.doc_id
    );

    rsx! {
        document::Title { "{view.title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        // The first-party custom-element script — same-origin, so `script-src
        // 'self'` allows it with no nonce; `defer` so it runs after the element
        // and its document body are in the DOM.
        document::Script { src: "/public/js/document-review.js", defer: true }

        main { id: "review", class: "nav-theme document-review-page",
            nav { class: "portal-detail__back",
                a { class: "nav-link", href: "/app/projects/{view.project_code}", "← Back to your matter" }
            }
            header {
                h1 { "{view.title}" }
                p {
                    span { class: "status-chip", "{view.kind}" }
                    " "
                    span { class: "status-chip", "{view.status}" }
                }
                p { class: "nav-muted",
                    "Read your document below. Select any text to leave a comment — you can't edit the document here, only comment. Nothing is final until you've had your say."
                }
            }
            document-review {
                "data-create-url": "{comments_url}",
                "data-comments": "{view.comments_json}",
                "data-csrf": "{view.csrf_token}",
                "data-doc-id": "{view.doc_id}",
                article { class: "nr-document portal-card",
                    dangerous_inner_html: "{view.body_html}",
                }
                aside { class: "nr-sidebar",
                    noscript {
                        p { class: "nav-muted", "Enable JavaScript to add comments. You can still read the document above." }
                    }
                }
            }
        }
    }
}
