//! The client portal matter-detail page (`/app/projects/{code}`) as a Dioxus
//! component (#641 Phase 3, projects cluster) — the single-matter client view.
//!
//! The successor to the `portal::projects::detail` render. Every caller
//! sees the matter through the client lens: a lawyer/admin user who also holds
//! client-side matters gets the same view a client does, and a caller without
//! client-side scope gets `404` (never `403` — the matter does not exist from
//! their perspective). The page gathers, server-side of the render:
//!
//! - the matter name and status;
//! - the invoice from the local Xero mirror (never Xero live);
//! - the matter's notations (retainer, etc.) with a download link per PDF that
//!   exists in the object store (`store::notations` keys, probed through the
//!   injected storage handle);
//! - the client-readable review drafts;
//! - the matter's documents.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;
use crate::portal_project_list::PersonId;

/// The matter's invoice, read from the local Xero mirror. The Xero invoice id is
/// deliberately not carried — only the client-facing amount and status.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct InvoiceView {
    /// Formatted total, e.g. `$3,333.00`.
    pub amount: String,
    /// Provider status mirror (`AUTHORISED`, `PAID`, …).
    pub status: String,
    /// `true` once reconcile has seen the invoice paid in full.
    pub paid: bool,
}

/// One of the matter's notations (e.g. the retainer), in plain words, with the
/// download links keyed off which of its three PDFs exist in storage.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct NotationRow {
    pub id: String,
    pub title: String,
    /// Client-friendly status, e.g. "Signed" / "Awaiting your signature".
    pub status: String,
    pub rendered_ready: bool,
    pub signed_ready: bool,
    pub certificate_ready: bool,
}

/// One attorney-advanced draft the client may read and comment on.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ReviewDocRow {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
}

/// The rendered matter-detail view — every field wasm-safe (plain scalars; no
/// `store`/`SeaORM`/`cloud` type crosses to the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectDetailView {
    pub id: String,
    /// The Project code, which keys both the matter page and client portal.
    pub code: String,
    pub name: String,
    pub status: String,
    pub invoice: Option<InvoiceView>,
    pub notations: Vec<NotationRow>,
    pub documents: Vec<String>,
    pub review_docs: Vec<ReviewDocRow>,
    /// The matter's collaboration resources, filtered to a client's audience:
    /// the shared Slack channel, the shared Notion page, and the portal. The
    /// firm's three private resources are never built for this view, so no
    /// firm-only URL reaches a client's markup or hydration payload.
    #[serde(default)]
    pub resources: crate::project_resources::ProjectResourcesView,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The original firm actor when this client lens is a matter preview. The
    /// page renders its exit banner from this server-injected state rather than
    /// inferring anything from the effective client session.
    #[serde(default)]
    pub impersonation: Option<crate::components::ImpersonationView>,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
}

/// Client-friendly status for a notation, derived from its workflow state and
/// which PDFs have materialized — never the raw docket state. Mirrors the
/// `notation_status_label`.
#[cfg(feature = "server")]
fn notation_status_label(state: &str, signed_ready: bool, rendered_ready: bool) -> &'static str {
    if signed_ready {
        "Signed"
    } else if state.starts_with("sent_for_signature") {
        "Awaiting your signature"
    } else if rendered_ready {
        "Ready for signature"
    } else {
        "In preparation"
    }
}

/// Format integer cents as a US dollar amount with thousands separators, e.g.
/// `333_300` → `"$3,333.00"`. Mirrors the `format_usd`; money never flows
/// through a float.
#[cfg(feature = "server")]
fn format_usd(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    let digits = (abs / 100).to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let grouped: String = grouped.chars().rev().collect();
    format!("${sign}{grouped}.{:02}", abs % 100)
}

/// Fetch one matter's client-lens detail for the current request. Refuses a
/// caller without client-side scope with a `404` (the matter does not exist for
/// them), then gathers the invoice, notations (with per-PDF storage probes),
/// review drafts, and documents.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_project_detail() -> Result<ProjectDetailView, ServerFnError> {
    use std::sync::Arc;

    let axum::extract::Path(code) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    let PersonId(person_id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PersonId>, _>()
            .await
            .map(|axum::Extension(id)| id)
            .unwrap_or_default();
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    // The navbar renders on the 404 body too, so the mark is resolved before the
    // first early return rather than only on the happy path.
    let logo = crate::app_chrome::app_logo_from_context().await;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let crate::components::Impersonating(impersonation) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::Extension<crate::components::Impersonating>,
            _,
        >()
        .await
        .map(|axum::Extension(impersonation)| impersonation)
        .unwrap_or_default();
    let person_id = person_id.and_then(|raw| raw.parse::<uuid::Uuid>().ok());

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let storage = consume_context::<Arc<dyn cloud::StorageService>>();
    let Some(project) = store::projects::find_by_code(&surreal, &code)
        .await
        .map_err(server_error)?
    else {
        return Ok(not_found(uuid::Uuid::nil(), role, logo, csrf_token));
    };
    let id = project.id;

    // Row-visibility runs before the row load, so an unauthorised caller never
    // even pulls the matter name into the response — the same 404 a missing id
    // would produce (never 403). A read error is a 500.
    let visible = store::projects::can_access_as_client_in_surreal(&surreal, person_id, id)
        .await
        .map_err(server_error)?;
    if !visible {
        return Ok(not_found(id, role, logo, csrf_token));
    }
    // Notations, each with which of its three PDFs exist in storage.
    let notation_rows = notation_rows(&surreal, storage.as_ref(), id).await?;

    // Client-readable drafts (only those an attorney has advanced past `draft`).
    let review_docs = store::review_documents::client_visible_for_project(&surreal, id)
        .await
        .map_err(server_error)?;
    let review_rows = review_docs
        .iter()
        .map(|d| ReviewDocRow {
            id: d.id.to_string(),
            title: d.title.clone(),
            kind: d.kind.clone(),
            status: d.status.clone(),
        })
        .collect();

    // Documents (read-only list of filenames) — gated to the assets a
    // lawyer has explicitly marked client-visible. Internal work product
    // (`review_memo`, `unclassified` lawyer/email uploads) never reaches
    // this list (#782).
    let documents = store::assets::for_project(&surreal, id)
        .await
        .map_err(server_error)?
        .into_iter()
        .filter(|d| d.visibility == store::documents::visibility::CLIENT)
        .map(|d| d.filename.unwrap_or_default())
        .collect();

    // Invoice from the local mirror; only amount + status reach the client.
    let invoice = store::xero_invoices::for_projects(&surreal, &[id])
        .await
        .map_err(server_error)?
        .into_iter()
        .next()
        .map(|r| InvoiceView {
            amount: format_usd(r.amount_cents),
            paid: r.amount_cents > 0 && r.amount_paid_cents >= r.amount_cents,
            status: r.status,
        });

    let resources = crate::project_resources::ProjectResourcesView {
        resources: crate::project_resources::visible_resources(
            &crate::project_resources::ProjectResourceLinks {
                private_slack_channel_url: project.internal_slack_channel_url.clone(),
                private_notion_page_url: project.private_notion_page_url.clone(),
                drive_folder_id: project.drive_folder_id.clone(),
                shared_slack_channel_url: project.external_slack_channel_url.clone(),
                shared_notion_page_url: project.shared_notion_page_url.clone(),
            },
            &project.code,
            role,
        ),
        // A client never configures a resource; the affordance is a lawyer's.
        can_configure: false,
        project_code: project.code.clone(),
    };
    Ok(ProjectDetailView {
        id: project.id.to_string(),
        code: project.code,
        name: project.name,
        status: project.status,
        invoice,
        notations: notation_rows,
        documents,
        review_docs: review_rows,
        resources,
        csrf_token,
        role,
        impersonation,
        logo,
    })
}

/// Build the per-notation rows for a matter: title, a client-friendly status,
/// and which of the three PDFs exist. `exists` is a metadata-only HEAD, so a
/// handful of probes per matter is cheap. Mirrors the `notation_rows`.
#[cfg(feature = "server")]
async fn notation_rows(
    surreal: &store::surreal::SurrealDb,
    storage: &dyn cloud::StorageService,
    project_id: uuid::Uuid,
) -> Result<Vec<NotationRow>, ServerFnError> {
    let notations = store::notations::list_by_project(surreal, project_id)
        .await
        .map_err(server_error)?;
    let mut rows = Vec::with_capacity(notations.len());
    for n in &notations {
        let title = store::templates::find_by_id(surreal, n.template_id)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "Agreement".to_string(), |t| t.title);
        let rendered_ready = storage
            .exists(&store::notations::document_pdf_storage_key(n.id))
            .await
            .unwrap_or(false);
        let signed_ready = storage
            .exists(&store::notations::signed_document_storage_key(n.id))
            .await
            .unwrap_or(false);
        let certificate_ready = storage
            .exists(&store::notations::certificate_of_completion_storage_key(
                n.id,
            ))
            .await
            .unwrap_or(false);
        rows.push(NotationRow {
            id: n.id.to_string(),
            title,
            status: notation_status_label(&n.state, signed_ready, rendered_ready).to_string(),
            rendered_ready,
            signed_ready,
            certificate_ready,
        });
    }
    Ok(rows)
}

/// Commit a `500` and wrap a query error, mirroring the sibling list pages: a
/// matter whose detail cannot be loaded is a server error, not a `200` with an
/// error body.
#[cfg(feature = "server")]
fn server_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

/// Commit the `404` the handler returned for a matter the caller cannot see
/// (or that does not exist) and return an empty view — the render shows the
/// not-found state under the committed status.
#[cfg(feature = "server")]
fn not_found(
    id: uuid::Uuid,
    role: ViewerRole,
    logo: Option<crate::components::AppLogo>,
    csrf_token: String,
) -> ProjectDetailView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    ProjectDetailView {
        id: id.to_string(),
        role,
        logo,
        csrf_token,
        ..ProjectDetailView::default()
    }
}

/// The client matter-detail page. Server-side rendered with the matter already
/// in the markup (via [`use_server_future`]), readable before hydration.
#[component]
pub fn ClientProjectDetail() -> Element {
    let resource = use_server_future(get_project_detail)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if !view.name.is_empty() => view.clone(),
        // A committed 404 returns an empty (nameless) view; render the same
        // "not found" state the handler served under that status.
        Some(Ok(_)) => {
            return rsx! {
                main { id: "portal-project", p { "That matter was not found." } }
            }
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "portal-project", p { "Failed to load this matter." } }
            }
        }
        None => {
            return rsx! {
                main { id: "portal-project", p { "Loading…" } }
            }
        }
    };

    let has_documents = !view.documents.is_empty();

    rsx! {
        document::Title { "{view.name}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::ImpersonationBanner { view: view.impersonation.clone() }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "portal-project", class: "nav-theme portal-detail",
            nav { class: "portal-detail__back",
                a { class: "nav-link", href: "/app/projects", "← Your Projects" }
            }
            h1 { "{view.name}" }
            p { span { class: "status-chip", "{view.status}" } }

            crate::project_resources::ProjectResourcesPanel { view: view.resources.clone() }

            p { class: "portal-detail__actions",
                if has_documents {
                    a {
                        class: "nav-btn nav-btn--secondary",
                        href: "/app/projects/{view.code}/documents.zip",
                        role: "button",
                        "Download all my documents"
                    }
                }
                a {
                    class: "nav-btn nav-btn--secondary",
                    href: "/app/projects/{view.code}/conversation",
                    "Conversation"
                }
            }

            if let Some(inv) = view.invoice.as_ref() {
                section { class: "portal-detail__section",
                    h2 { "Invoice" }
                    div { class: "portal-card portal-card--split",
                        div {
                            div { class: "portal-card__title", "{inv.amount}" }
                            div { class: "portal-card__meta", "Status: {inv.status}" }
                        }
                        if inv.paid {
                            span { class: "status-chip status-chip--paid", "Paid" }
                        } else {
                            span { class: "status-chip status-chip--due", "Due" }
                        }
                    }
                }
            }

            if !view.notations.is_empty() {
                section { class: "portal-detail__section",
                    h2 { "Your agreements" }
                    div { class: "portal-agreements",
                        for n in view.notations.iter() {
                            div { class: "portal-agreement", key: "{n.id}",
                                span {
                                    "{n.title}"
                                    span { class: "status-chip", " {n.status}" }
                                }
                                span { class: "portal-agreement__links",
                                    a {
                                        class: "nav-btn nav-btn--secondary",
                                        href: "{crate::notation_outline::notation_outline_href(&view.code, &n.id)}",
                                        "Outline"
                                    }
                                    if n.rendered_ready {
                                        a {
                                            class: "nav-btn nav-btn--secondary",
                                            href: "/app/notations/{n.id}/documents/retainer",
                                            "Agreement"
                                        }
                                    }
                                    if n.signed_ready {
                                        a {
                                            class: "nav-btn nav-btn--secondary",
                                            href: "/app/notations/{n.id}/documents/signed",
                                            "Signed copy"
                                        }
                                    }
                                    if n.certificate_ready {
                                        a {
                                            class: "nav-btn nav-btn--secondary",
                                            href: "/app/notations/{n.id}/documents/certificate",
                                            "Certificate"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if has_documents {
                section { class: "portal-detail__section",
                    h2 { "Your documents" }
                    div { class: "nav-table-wrap",
                        table { class: "nav-table",
                            thead {
                                tr { th { scope: "col", "Document" } }
                            }
                            tbody {
                                for filename in view.documents.iter() {
                                    tr { td { "{filename}" } }
                                }
                            }
                        }
                    }
                }
            }

            if view.review_docs.is_empty() {
                p { class: "nav-muted",
                    "Documents to review will appear here once your attorney has prepared them."
                }
            } else {
                section { class: "portal-detail__section",
                    h2 { "Documents to review" }
                    div { class: "nav-table-wrap",
                        table { class: "nav-table",
                            thead {
                                tr {
                                    th { scope: "col", "Document" }
                                    th { scope: "col", "Type" }
                                    th { scope: "col", "Status" }
                                    th { scope: "col", class: "nav-table__end", "Action" }
                                }
                            }
                            tbody {
                                for doc in view.review_docs.iter() {
                                    tr {
                                        td { "{doc.title}" }
                                        td { span { class: "status-chip", "{doc.kind}" } }
                                        td { span { class: "status-chip", "{doc.status}" } }
                                        td { class: "nav-table__end",
                                            a {
                                                class: "nav-btn nav-btn--secondary",
                                                href: "/app/projects/{view.code}/review/{doc.id}",
                                                "Review"
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
}
