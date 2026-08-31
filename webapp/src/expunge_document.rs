//! Admin governed-expunge document surface as a Dioxus component (#956 Phase 4)
//! — `/app/lawyer/documents/{doc_id}/expunge`.
//!
//! The successor to the `views::pages::admin::expunge` pair. One route,
//! two states:
//!
//! - the **confirmation** form, naming the document and its storage key and
//!   demanding a category before the irreversible act; and
//! - the **result** state (`?record=<audit-row id>`), reached by the redirect
//!   the `POST` handler answers with, showing the audit-row id that survives the
//!   expunge.
//!
//! The mutation stays on its existing `POST /app/lawyer/documents/{doc_id}/expunge`
//! handler, reached through the shared [`FormCard`]'s native form carrying the
//! session CSRF token — no JavaScript. A rejected submit redirects back here
//! with `?error=`, surfaced above the form.
//!
//! # Authorization
//!
//! Admin-only, and hidden rather than refused: a lawyer or client caller gets the
//! same `404` the handler returned, so the route's existence is not
//! disclosed. That is the tier check on top of the `/app/lawyer/*` embedded Rego policy gate the route
//! already carries; the expunge primitive re-checks the authorizer itself.
//!
//! The reads run the same `store` calls the handler made. There is no
//! `/api` cluster for filed documents or expunge audit rows yet; when one lands
//! (#866) this loader moves onto it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard};
use crate::people::ViewerRole;

/// The confirmation screen's `?record=` (the completed expunge's audit-row id)
/// and `?error=` (a rejected submit) flashes.
#[derive(Deserialize, Default)]
pub struct ExpungeQuery {
    #[serde(default)]
    pub record: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// One expunge category, as the `<select>` renders it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct CategoryChoice {
    pub value: String,
    pub label: String,
}

/// The document about to be expunged.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ExpungeTarget {
    pub project_id: String,
    /// The repo path that will be removed from all history — the document's
    /// filename.
    pub filename: String,
    /// The object-storage key whose bytes will be deleted.
    pub storage_key: String,
    pub categories: Vec<CategoryChoice>,
}

/// The completed expunge, read back from its audit row.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ExpungeOutcome {
    pub record_id: String,
    pub project_id: String,
    pub filename: String,
    pub category: String,
}

/// Which of the route's states renders. `NotFound` is the fail-closed default:
/// a non-admin caller, an unknown document, and a non-document asset all land
/// there under a committed `404`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub enum ExpungeState {
    #[default]
    NotFound,
    Confirm(ExpungeTarget),
    Done(ExpungeOutcome),
}

/// The rendered expunge surface: which state to draw, the document id (for the
/// form action), any `?error=` flash, the session CSRF token, and the viewer's
/// tier for the nav chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ExpungeDocumentView {
    pub doc_id: String,
    pub state: ExpungeState,
    pub error: Option<String>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The expunge categories offered by the form, labelled for an attorney and
/// valued with the canonical `store` constants the `POST` handler re-validates
/// against — so the two can never drift.
#[cfg(feature = "server")]
fn category_choices() -> Vec<CategoryChoice> {
    use store::expunge_records::{CATEGORY_CLIENT_REQUEST, CATEGORY_PRIVILEGE, CATEGORY_SEALING};
    [
        (
            CATEGORY_PRIVILEGE,
            "Privilege clawback — privileged material committed in error",
        ),
        (CATEGORY_SEALING, "Court sealing order"),
        (CATEGORY_CLIENT_REQUEST, "Client lawful-deletion request"),
    ]
    .into_iter()
    .map(|(value, label)| CategoryChoice {
        value: value.to_string(),
        label: label.to_string(),
    })
    .collect()
}

/// Load the expunge surface for the `{doc_id}` in the request path.
///
/// Admin-only: any other tier gets a committed `404` and the not-found body,
/// never a hint that the route exists. With `?record=` the audit row is read
/// back and the result state renders (the document's bytes and history are gone
/// by then, so the outcome is rendered from the audit row alone); otherwise the
/// `assets` row is resolved into the confirmation form.
#[server]
pub async fn get_expunge_document() -> Result<ExpungeDocumentView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let query =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<ExpungeQuery>, _>()
            .await
            .map(|axum::extract::Query(q)| q)
            .unwrap_or_default();

    let firm_name = crate::app_chrome::firm_name_from_context().await;
    let hidden = |doc_id: String| {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        ExpungeDocumentView {
            firm_name: firm_name.clone(),
            doc_id,
            state: ExpungeState::NotFound,
            error: None,
            csrf_token: String::new(),
            role,
        }
    };

    // The tier check sits before the path parse so a non-admin never learns
    // whether the id was even well-formed.
    if !role.is_admin_tier() {
        return Ok(hidden(String::new()));
    }
    let Ok(axum::extract::Path(doc_id)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await
    else {
        return Ok(hidden(String::new()));
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();

    // The completed-expunge state, keyed by the audit row the redirect carries.
    if let Some(raw) = query.record.as_deref() {
        let Ok(record_id) = raw.parse::<uuid::Uuid>() else {
            return Ok(hidden(doc_id.to_string()));
        };
        let record = store::expunge_records::by_id(&surreal, record_id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        let Some(record) = record else {
            return Ok(hidden(doc_id.to_string()));
        };
        return Ok(ExpungeDocumentView {
            firm_name: firm_name.clone(),
            doc_id: doc_id.to_string(),
            state: ExpungeState::Done(ExpungeOutcome {
                record_id: record.id.to_string(),
                project_id: record.project_id.to_string(),
                filename: record.path,
                category: record.category,
            }),
            error: None,
            csrf_token,
            role,
        });
    }

    let asset = store::assets::find_by_id(&surreal, doc_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    // Only a project document asset (project-scoped, with a repo-path filename)
    // can be expunged — a bare content asset is not a document.
    let Some((asset, project_id, filename)) = asset.and_then(|a| {
        let project_id = a.project_id?;
        let filename = a.filename.clone()?;
        Some((a, project_id, filename))
    }) else {
        return Ok(hidden(doc_id.to_string()));
    };

    Ok(ExpungeDocumentView {
        firm_name,
        doc_id: doc_id.to_string(),
        state: ExpungeState::Confirm(ExpungeTarget {
            project_id: project_id.to_string(),
            filename,
            storage_key: asset.storage_key,
            categories: category_choices(),
        }),
        error: query.error,
        csrf_token,
        role,
    })
}

/// The role-appropriate lawyer nav chrome every migrated admin page carries.
fn expunge_nav(role: ViewerRole) -> Element {
    rsx! {
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
    }
}

/// The confirmation form: the irreversibility warning, the document and its
/// storage key, and the native `POST` that runs the expunge.
fn confirm_body(view: &ExpungeDocumentView, target: &ExpungeTarget) -> Element {
    let project_href = format!("/app/projects/{}", target.project_id);
    let action = format!("/app/lawyer/documents/{}/expunge", view.doc_id);
    let mut options = vec![Choice::new("", "Choose a category…")];
    options.extend(
        target
            .categories
            .iter()
            .map(|c| Choice::new(c.value.clone(), c.label.clone())),
    );
    let fields = vec![
        Field::select("Reason", "category", options, None).required(),
        Field::textarea("Note", "note", "", 2)
            .help("Optional — a docket reference, not document content."),
    ];
    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Documents | Expunge" }
        header { class: "page-header",
            h1 { "Expunge document" }
            p { class: "muted",
                "Project: "
                a { href: "{project_href}", "{target.project_id}" }
            }
        }
        div { class: "nav-alert expunge-warning", role: "alert",
            p { class: "nav-alert__title", "This rewrites the matter's history and cannot be undone." }
            p { class: "nav-alert__body",
                "The document is removed from every commit, its stored bytes are deleted, and \
                 existing clones of the repository become invalid. Only the audit record of the \
                 expunge — who, when, and why — is kept."
            }
        }
        if let Some(error) = view.error.as_ref() {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        dl { class: "expunge-detail",
            dt { "Document" }
            dd { class: "font-monospace", "{target.filename}" }
            dt { "Stored at" }
            dd { class: "font-monospace", "{target.storage_key}" }
        }
        FormCard {
            title: "Confirm the expunge".to_string(),
            action,
            submit_label: "Expunge permanently".to_string(),
            heading: crate::components::Heading::H2,
            csrf_token: Some(view.csrf_token.clone()),
            fields,
        }
        p { class: "expunge-cancel",
            a { class: "nav-btn nav-btn--secondary", href: "{project_href}", "Cancel" }
        }
    }
}

/// The completed-expunge state, rendered from the audit row.
fn done_body(outcome: &ExpungeOutcome, firm_name: &str) -> Element {
    let project_href = format!("/app/projects/{}", outcome.project_id);
    rsx! {
        document::Title { "{firm_name} | Lawyer | Documents | Expunged" }
        header { class: "page-header",
            h1 { "Document expunged" }
        }
        p {
            strong { class: "font-monospace", "{outcome.filename}" }
            " has been removed from the matter's history and storage."
        }
        dl { class: "expunge-detail",
            dt { "Audit record" }
            dd { class: "font-monospace", "{outcome.record_id}" }
            dt { "Category" }
            dd { "{outcome.category}" }
        }
        p { a { href: "{project_href}", "Back to the matter" } }
    }
}

/// The admin governed-expunge surface. Server-side rendered; the confirmation
/// form is a native `POST` carrying the session CSRF token, so it works without
/// JavaScript.
#[component]
pub fn AdminExpungeDocument() -> Element {
    let resource = use_server_future(get_expunge_document)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "expunge-document", p { "Failed to load the document." } }
            }
        }
        None => {
            return rsx! {
                main { id: "expunge-document", p { "Loading…" } }
            }
        }
    };
    let role = view.role;

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        {expunge_nav(role)}
        main { id: "expunge-document", class: "nav-theme",
            match &view.state {
                ExpungeState::Confirm(target) => confirm_body(&view, target),
                ExpungeState::Done(outcome) => done_body(outcome, &view.firm_name),
                ExpungeState::NotFound => rsx! {
                    document::Title { "{view.firm_name} | Lawyer | Not found" }
                    h1 { "Not found" }
                    p { "No expungeable document is available at this address." }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        confirm_body, done_body, CategoryChoice, ExpungeDocumentView, ExpungeOutcome, ExpungeState,
        ExpungeTarget,
    };
    use crate::people::ViewerRole;
    use dioxus::prelude::*;

    fn categories() -> Vec<CategoryChoice> {
        [
            ("privilege", "Privilege clawback"),
            ("sealing", "Court sealing order"),
            ("client_request", "Client lawful-deletion request"),
        ]
        .into_iter()
        .map(|(value, label)| CategoryChoice {
            value: value.to_string(),
            label: label.to_string(),
        })
        .collect()
    }

    fn confirm_view(error: Option<&str>) -> ExpungeDocumentView {
        ExpungeDocumentView {
            firm_name: "Neon Law".to_string(),
            doc_id: "00000000-0000-0000-0000-000000000007".to_string(),
            state: ExpungeState::Confirm(ExpungeTarget {
                project_id: "00000000-0000-0000-0000-000000000009".to_string(),
                filename: "privileged.pdf".to_string(),
                storage_key: "blobs/deadbeef".to_string(),
                categories: categories(),
            }),
            error: error.map(ToString::to_string),
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Admin,
        }
    }

    /// Render the page body for a fixed view, bypassing the server function.
    fn render_body(view: &ExpungeDocumentView) -> String {
        dioxus_ssr::render_element(match &view.state {
            ExpungeState::Confirm(target) => confirm_body(view, target),
            ExpungeState::Done(outcome) => done_body(outcome, &view.firm_name),
            ExpungeState::NotFound => rsx! { h1 { "Not found" } },
        })
    }

    #[test]
    fn confirm_names_the_doc_warns_and_lists_every_category() {
        let html = render_body(&confirm_view(None));
        assert!(html.contains(">privileged.pdf<"), "{html}");
        assert!(html.contains(">blobs/deadbeef<"), "{html}");
        assert!(html.contains("rewrites the matter&#39;s history"), "{html}");
        assert!(html.contains("existing clones"), "{html}");
        assert!(
            html.contains(
                r#"action="/app/lawyer/documents/00000000-0000-0000-0000-000000000007/expunge""#
            ),
            "{html}"
        );
        assert!(
            html.contains(r#"name="_csrf" value="TOK""#) || html.contains(r#"value="TOK""#),
            "{html}"
        );
        for value in ["privilege", "sealing", "client_request"] {
            assert!(
                html.contains(&format!(r#"<option value="{value}""#)),
                "{html}"
            );
        }
    }

    #[test]
    fn confirm_keeps_the_admin_form_e2e_hook() {
        // `web/tests/accessibility_e2e.rs` scopes axe to `form.admin-form`;
        // dropping the class times the nightly deploy gate out.
        let html = render_body(&confirm_view(None));
        assert!(html.contains("admin-form"), "{html}");
    }

    #[test]
    fn confirm_renders_the_error_flash_when_set() {
        let html = render_body(&confirm_view(Some("Unknown category.")));
        assert!(html.contains(">Unknown category.<"), "{html}");
        assert!(html.contains("nav-form-error"), "{html}");
    }

    #[test]
    fn done_shows_the_audit_row_id() {
        let view = ExpungeDocumentView {
            firm_name: "Neon Law".to_string(),
            doc_id: "00000000-0000-0000-0000-000000000007".to_string(),
            state: ExpungeState::Done(ExpungeOutcome {
                record_id: "00000000-0000-0000-0000-00000000002a".to_string(),
                project_id: "00000000-0000-0000-0000-000000000009".to_string(),
                filename: "privileged.pdf".to_string(),
                category: "sealing".to_string(),
            }),
            error: None,
            csrf_token: String::new(),
            role: ViewerRole::Admin,
        };
        let html = render_body(&view);
        assert!(
            html.contains(">00000000-0000-0000-0000-00000000002a<"),
            "{html}"
        );
        assert!(html.contains(">privileged.pdf<"), "{html}");
        assert!(html.contains(">sealing<"), "{html}");
        // Nothing on the completed page invites another write.
        assert!(!html.contains("admin-form"), "{html}");
    }
}
