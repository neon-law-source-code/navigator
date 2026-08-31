//! Lawyer expunge-request queue as a Dioxus component (#641 Phase 3, admin
//! cluster) — the first migrated **row-action** page.
//!
//! The pending document-deletion requests, each with an admin-only "Authorize
//! deletion" action and a lawyer/admin "Deny" action. The read view moves to
//! Dioxus; the mutations stay on their existing `POST` handlers, reached through
//! native HTML forms (no JavaScript) that carry the session CSRF token — the
//! SSR-friendly form pattern the later CRUD pages reuse. The `#[server]` function
//! reads the injected viewer role and [`crate::csrf::CsrfToken`], lists the
//! pending requests, and resolves each row's matter, document, and requester.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// One pending expunge request, in a wasm-safe shape.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ExpungeRow {
    pub id: String,
    pub matter: String,
    pub filename: String,
    pub requester: String,
    pub requested_at: String,
}

/// The rendered queue: the pending rows, the session CSRF token for the action
/// forms, whether the viewer is an admin (only admins see "Authorize"), and the
/// viewer's tier for the nav chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ExpungeQueueView {
    pub rows: Vec<ExpungeRow>,
    pub csrf_token: String,
    pub is_admin: bool,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Fetch the pending expunge-request queue: refuse non-lawyer, read the injected
/// CSRF token, list the pending requests, and resolve each row's matter name,
/// document filename, and requester — the same per-row lookups the handler
/// did (the queue is small).
#[server]
pub async fn list_expunge_queue() -> Result<ExpungeQueueView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let pending = store::expunge_requests::list_pending(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let mut rows = Vec::with_capacity(pending.len());
    for req in &pending {
        let filename = store::assets::find_by_id(&surreal, req.asset_id)
            .await
            .ok()
            .flatten()
            .and_then(|a| a.filename)
            .unwrap_or_else(|| "(unknown document)".to_string());
        let matter = store::projects::find_by_id(&surreal, req.project_id)
            .await
            .ok()
            .flatten()
            .map_or_else(|| req.project_id.to_string(), |p| p.name);
        let requester = store::persons::find_by_id(&surreal, req.requested_by_person_id)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "(unknown)".to_string(), |p| p.name);
        rows.push(ExpungeRow {
            id: req.id.to_string(),
            matter,
            filename,
            requester,
            requested_at: req.inserted_at.to_rfc3339(),
        });
    }

    Ok(ExpungeQueueView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        csrf_token,
        is_admin: role.is_admin_tier(),
        role,
    })
}

/// The lawyer expunge-request queue. Server-side rendered with the pending rows
/// in the markup; each row's actions are native `POST` forms carrying the CSRF
/// token, so they work without JavaScript. Only admins see "Authorize deletion".
#[component]
pub fn LawyerExpungeQueue() -> Element {
    let resource = use_server_future(list_expunge_queue)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "expunge-queue", p { "Failed to load the deletion queue." } }
            }
        }
        None => {
            return rsx! {
                main { id: "expunge-queue", p { "Loading…" } }
            }
        }
    };

    let role = view.role;
    let is_admin = view.is_admin;
    let csrf = view.csrf_token.clone();
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Document deletion requests" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if role.is_lawyer_tier() {
                a { class: "nav-link", href: "/app/lawyer", "Lawyer" }
            }
            if role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "expunge-queue", class: "nav-theme",
            header { class: "page-header",
                h1 { "Document deletion requests" }
                p { class: "muted",
                    "Clients who have asked to delete a document. Authorizing runs an \
                     irreversible deletion that rewrites the matter's history."
                }
            }
            div { class: "nav-table-wrap",
                table { class: "nav-table",
                    thead {
                        tr {
                            th { "Matter" }
                            th { "Document" }
                            th { "Requested by" }
                            th { "Requested" }
                            th { "Action" }
                        }
                    }
                    tbody {
                        if is_empty {
                            tr {
                                td { class: "expunge-empty", colspan: "5",
                                    "No pending deletion requests."
                                }
                            }
                        }
                        for row in view.rows.iter() {
                            tr { class: "expunge-row",
                                td { class: "expunge-matter", "{row.matter}" }
                                td { class: "expunge-document",
                                    span { class: "font-monospace", "{row.filename}" }
                                }
                                td { class: "expunge-requester", "{row.requester}" }
                                td { class: "expunge-requested-at", "{row.requested_at}" }
                                td { class: "expunge-actions",
                                    if is_admin {
                                        form {
                                            class: "d-inline",
                                            method: "post",
                                            action: "/app/lawyer/expunge-requests/{row.id}/authorize",
                                            input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                                            button {
                                                class: "nav-btn nav-btn--danger",
                                                r#type: "submit",
                                                "Authorize deletion"
                                            }
                                        }
                                        " "
                                    }
                                    form {
                                        class: "d-inline",
                                        method: "post",
                                        action: "/app/lawyer/expunge-requests/{row.id}/deny",
                                        input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                                        button {
                                            class: "nav-btn nav-btn--secondary",
                                            r#type: "submit",
                                            "Deny"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
