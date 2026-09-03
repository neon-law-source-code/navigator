//! The matter conversation page (`/app/projects/{project_code}/conversation`) as a
//! Dioxus component (#641 Phase 3, projects cluster) — the
//! privileged client↔firm thread on one matter.
//!
//! The successor to the `portal::conversation` render + its HTMX
//! `render_fragment`. The thread is scoped by lens: the firm (lawyer lens) sees
//! every row including firm-internal notes; a client sees every row *except*
//! internal notes (`store::communications::for_project` vs
//! `for_project_client_visible`). A caller who cannot see the matter through the
//! caller's tier gets `404`.
//!
//! The composer is a plain `<textarea>`. The form is a native `POST` (no HTMX,
//! no JavaScript) to the `…/conversation/messages` handler,
//! which redirects back (PRG).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::csrf::CsrfToken;
use crate::people::ViewerRole;

/// One message in the thread, in a wasm-safe shape.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ConversationMessage {
    pub channel: String,
    pub direction: String,
    pub author: String,
    pub subject: Option<String>,
    pub body: String,
    pub occurred_at: String,
}

/// The rendered conversation — every field wasm-safe.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ConversationView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub project_id: String,
    pub project_name: String,
    pub messages: Vec<ConversationMessage>,
    pub is_lawyer: bool,
    pub csrf_token: String,
    /// The matter base path (`/app/projects/{project_code}`) — the back link and the
    /// composer action derive from it.
    pub base: String,
}

/// A short, human label for a message channel. Mirrors the `channel_label`.
fn channel_label(channel: &str) -> &'static str {
    match channel {
        "document_comment" => "Comment",
        "email_inbound" | "email_outbound" => "Email",
        "portal_message" => "Message",
        "sms_inbound" | "sms_outbound" => "Text",
        _ => "Note",
    }
}

/// The theme modifier keyed on direction — internal notes stand apart so lawyers
/// never mistake one for a client-visible message. Mirrors the
/// `direction_class` (Bootstrap borders → theme classes).
fn direction_modifier(direction: &str) -> &'static str {
    match direction {
        "inbound" => "conversation-msg--inbound",
        "outbound" => "conversation-msg--outbound",
        _ => "conversation-msg--internal",
    }
}

/// Fetch the matter thread for the current request's lens. Refuses a caller who
/// cannot see the matter through that lens with `404`; a query failure is `500`.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_conversation() -> Result<ConversationView, ServerFnError> {
    use std::collections::HashMap;

    let axum::extract::Path(project_code) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
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

    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };
    let surreal = consume_context::<store::surreal::SurrealDb>();
    // One path serves both sides, so the tier is what says which thread this
    // is — not the prefix the caller typed.
    let is_lawyer = role.is_lawyer_tier();
    // The matter arrives as its code; everything below keys on the row id, so
    // this one lookup is both the resolution and the existence check.
    let Some(project) = store::projects::find_by_code(&surreal, &project_code)
        .await
        .map_err(server_error)?
    else {
        return Ok(not_found(
            project_code,
            "/app/projects".to_string(),
            is_lawyer,
            csrf_token,
        ));
    };
    let project_id = project.id;
    let base = format!("/app/projects/{}", project.code);

    let visible = store::access::can_see_project(&surreal, person_id, store_role, project_id)
        .await
        .map_err(server_error)?;
    if !visible {
        return Ok(not_found(
            project_id.to_string(),
            base,
            is_lawyer,
            csrf_token,
        ));
    }
    // Lens-scoped rows: the firm sees every row, a client every row except
    // firm-internal notes.
    let rows = if is_lawyer {
        store::communications::for_project(&surreal, project_id).await
    } else {
        store::communications::for_project_client_visible(&surreal, project_id).await
    }
    .map_err(server_error)?;

    // Batch-resolve author display names (no N+1).
    let author_ids: Vec<_> = rows.iter().filter_map(|c| c.author_person_id).collect();
    let authors: HashMap<_, _> = if author_ids.is_empty() {
        HashMap::new()
    } else {
        store::persons::find_by_ids(&surreal, &author_ids)
            .await
            .map_err(server_error)?
            .into_iter()
            .map(|p| (p.id, p.name))
            .collect()
    };

    let messages = rows
        .into_iter()
        .map(|c| {
            let author = c
                .author_person_id
                .and_then(|id| authors.get(&id).cloned())
                .or_else(|| c.counterparty.clone())
                .unwrap_or_else(|| "Firm".to_string());
            ConversationMessage {
                channel: c.channel,
                direction: c.direction,
                author,
                subject: c.subject,
                body: c.body,
                occurred_at: c.occurred_at,
            }
        })
        .collect();

    Ok(ConversationView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        project_id: project_id.to_string(),
        project_name: project.name,
        messages,
        is_lawyer,
        csrf_token,
        base,
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

/// Commit the `404` the handler returned for a matter the caller cannot see
/// through this lens, and return an empty (nameless) view.
#[cfg(feature = "server")]
fn not_found(
    project_id: String,
    base: String,
    is_lawyer: bool,
    csrf_token: String,
) -> ConversationView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    ConversationView {
        project_id,
        is_lawyer,
        csrf_token,
        base,
        ..ConversationView::default()
    }
}

/// The matter conversation page, server-side rendered. The thread and the
/// composer are readable and usable with no JavaScript at all.
#[component]
pub fn Conversation() -> Element {
    let resource = use_server_future(get_conversation)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if !view.project_name.is_empty() => view.clone(),
        Some(Ok(_)) => {
            return rsx! {
                main { id: "conversation", p { "That conversation was not found." } }
            }
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "conversation", p { "Failed to load this conversation." } }
            }
        }
        None => {
            return rsx! {
                main { id: "conversation", p { "Loading…" } }
            }
        }
    };

    let post_action = format!("{}/conversation/messages", view.base);
    let is_empty = view.messages.is_empty();

    rsx! {
        document::Title { "Conversation" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }

        main { id: "conversation", class: "nav-theme portal-conversation",
            nav { class: "portal-detail__back",
                a { class: "nav-link", href: "{view.base}", "← Back to matter" }
            }
            h1 { "Conversation" }
            p { class: "nav-muted", "{view.project_name}" }

            div { id: "conversation-thread",
                if is_empty {
                    p { class: "nav-muted", "No messages yet." }
                } else {
                    div { class: "conversation-thread",
                        for m in view.messages.iter() {
                            div { class: "conversation-msg {direction_modifier(&m.direction)}",
                                div { class: "conversation-msg__head",
                                    span {
                                        strong { "{m.author}" }
                                        span { class: "status-chip", "{channel_label(&m.channel)}" }
                                        if m.direction == "internal" {
                                            span { class: "status-chip status-chip--due", "Internal" }
                                        }
                                    }
                                    span { class: "nav-muted", "{m.occurred_at}" }
                                }
                                if let Some(subject) = m.subject.as_ref() {
                                    div { class: "conversation-msg__subject", "{subject}" }
                                }
                                div { class: "conversation-msg__body conversation-msg__body--plain", "{m.body}" }
                            }
                        }
                    }
                }
            }

            section { class: "portal-detail__section",
                h2 { "Add a message" }
                form { class: "conversation-composer", method: "post", action: "{post_action}",
                    input { r#type: "hidden", name: "_csrf", value: "{view.csrf_token}" }
                    textarea {
                        class: "nav-input",
                        id: "conversation-body",
                        name: "body",
                        rows: "3",
                        placeholder: "Write a message…",
                        required: true,
                    }
                    if view.is_lawyer {
                        div { class: "conversation-internal",
                            input { r#type: "checkbox", name: "internal", value: "1", id: "internal-note" }
                            label { r#for: "internal-note", " Internal note (not visible to the client)" }
                        }
                    }
                    button { class: "nav-btn nav-btn--primary", r#type: "submit", "Send" }
                }
            }
        }
    }
}
