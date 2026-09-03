//! One filed matter document's provenance page as a Dioxus component (#956
//! Phase 4) — `/app/projects/{project_code}/documents/{doc_id}` and its client twin
//! `/app/projects/{project_code}/documents/{doc_id}`.
//!
//! The successor to the `views::pages::admin::projects::document_detail`.
//! A read-only page: where the document came from, when it arrived, and what is
//! stored, plus the Download action (a one-hour signed link served by the
//! unchanged `…/download` handler).
//!
//! # Authorization
//!
//! One mount serves both sides, so the lens comes from the caller's tier: a
//! client cannot ask for the firm view by rewriting a path, because they would
//! have to change what they *are*. The loader runs
//! `store::access::can_see_project` and then the same cross-project and
//! visibility guards the handler applied: an asset belonging to another
//! matter is not found, and under the client lens an `internal` asset is not
//! found either (#782) — so a client cannot fetch firm work product on their own
//! matter by guessing a `doc_id`.
//!
//! A refusal renders the not-found body at `200`, exactly as the handler
//! did; a database failure commits a `500` rather than masking a read error as a
//! missing document.
//!
//! The reads run the same `store` calls the handler made. There is no
//! `/api` read cluster for a single filed document yet; when one lands (#866)
//! this loader moves onto it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The one matter surface every link on this page is built from. Only the
/// server-side loader builds hrefs, so the constant follows it.
#[cfg(any(feature = "server", test))]
const PROJECTS_BASE: &str = "/app/projects";

/// The document's provenance and storage facts, as the page renders them.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DocumentFacts {
    pub filename: String,
    pub kind: String,
    pub source: String,
    pub received_at: String,
    pub description: Option<String>,
    pub content_type: String,
    pub byte_size: i64,
    pub sha256_hex: String,
    /// The signed-URL redirect endpoint.
    pub download_href: String,
    /// The matter this document is filed on.
    pub back_href: String,
}

/// The rendered document page. `document` is `None` for every refusal — an
/// unauthorized caller, an unknown or cross-project asset, and (under the client
/// lens) an internal one — all of which render the same not-found body.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DocumentDetailView {
    pub document: Option<DocumentFacts>,
    /// A read failure. Rendered as an error page under a committed `500`, so a
    /// broken read is never reported as a missing document.
    pub failed: bool,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Load one filed document, applying the access, cross-project, and visibility
/// guards. The lens comes from the caller's tier: one path serves both sides,
/// so a client cannot reach the firm view by rewriting a URL — they would have
/// to change what they *are*.
#[cfg(feature = "server")]
#[allow(clippy::too_many_lines)]
async fn load() -> Result<DocumentDetailView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    // Both refusal bodies render the navbar, so the mark is resolved before the
    // first early return rather than only on the happy path.
    let logo = crate::app_chrome::app_logo_from_context().await;
    let tokens_href = crate::app_chrome::app_tokens_href_from_context().await;
    let firm_name = crate::app_chrome::firm_name_from_context().await;
    let missing = DocumentDetailView {
        firm_name: firm_name.clone(),
        document: None,
        failed: false,
        role,
        logo: logo.clone(),
        tokens_href: tokens_href.clone(),
    };
    let failed = || {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            None,
        );
        DocumentDetailView {
            firm_name: firm_name.clone(),
            document: None,
            failed: true,
            role,
            logo: logo.clone(),
            tokens_href: tokens_href.clone(),
        }
    };

    let Ok(axum::extract::Path((project_code, doc_id))) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::extract::Path<(String, uuid::Uuid)>,
            _,
        >()
        .await
    else {
        return Ok(missing);
    };
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
    // The matter arrives as its code; everything below keys on the row id. A
    // code naming no matter is the same "not found" a caller off the matter
    // gets, so neither can tell the other's case from the response — but a
    // store *failure* is not a miss and must stay a 500, or an outage would
    // read to every caller as "your matter is gone".
    let project_id = match store::projects::find_by_code(&surreal, &project_code).await {
        Ok(Some(project)) => project.id,
        Ok(None) => return Ok(missing),
        Err(_) => return Ok(failed()),
    };
    // A refusal and a failed query are different answers: collapsing them
    // reports a store outage as a missing document. The gate reads the
    // participation ledger, so an outage breaks it before the asset lookup.
    let visible =
        match store::access::can_see_project(&surreal, person_id, store_role, project_id).await {
            Ok(visible) => visible,
            Err(e) => {
                tracing::error!(error = %e, %project_id, %doc_id, "document access check failed");
                return Ok(failed());
            }
        };
    let store_lens = store::access::ProjectLens::for_role(store_role);
    if !visible {
        tracing::info!(%project_id, %doc_id, "project document detail denied by access policy");
        return Ok(missing);
    }

    let doc = match store::assets::find_by_id(&surreal, doc_id).await {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(error = %e, %project_id, %doc_id, "db error loading project document");
            return Ok(failed());
        }
    };
    // The cross-project guard, then the client lens's visibility guard: an asset
    // filed on another matter, and an `internal` one under the client lens, are
    // both simply not found.
    let Some(doc) = doc.filter(|doc| doc.project_id == Some(project_id)) else {
        return Ok(missing);
    };
    if store_lens == store::access::ProjectLens::Client
        && doc.visibility != store::documents::visibility::CLIENT
    {
        tracing::warn!(
            %project_id, %doc_id, visibility = %doc.visibility,
            "internal project document requested through the client lens (visibility guard)"
        );
        return Ok(missing);
    }

    let base = PROJECTS_BASE;
    Ok(DocumentDetailView {
        firm_name,
        document: Some(DocumentFacts {
            filename: doc.filename.unwrap_or_default(),
            kind: doc.kind.unwrap_or_default(),
            source: doc.source.unwrap_or_default(),
            received_at: doc.received_at.unwrap_or_default(),
            description: doc.description,
            content_type: doc.content_type,
            byte_size: doc.byte_size,
            sha256_hex: doc.sha256_hex,
            download_href: format!("{base}/{project_code}/documents/{doc_id}/download"),
            back_href: format!("{base}/{project_code}"),
        }),
        failed: false,
        role,
        logo,
        tokens_href,
    })
}

/// Load one filed matter document through the caller's own lens.
#[server]
pub async fn get_project_document() -> Result<DocumentDetailView, ServerFnError> {
    load().await
}

/// The document's provenance and storage facts.
fn document_body(doc: &DocumentFacts, firm_name: &str) -> Element {
    let description = doc.description.clone().unwrap_or_else(|| "—".to_string());
    rsx! {
        document::Title { "{firm_name} | Document | {doc.filename}" }
        header { class: "page-header",
            h1 { "{doc.filename}" }
            p { a { href: "{doc.back_href}", "← Back to project" } }
        }
        section { class: "document-actions",
            p {
                a { class: "nav-btn nav-btn--primary", href: "{doc.download_href}", "Download" }
                " "
                span { class: "muted", "(signed link valid for one hour)" }
            }
        }
        section { class: "document-provenance",
            h2 { "Provenance" }
            dl { class: "detail-dl",
                dt { "Source" }
                dd { "{doc.source}" }
                dt { "Received" }
                dd { "{doc.received_at}" }
                dt { "Description" }
                dd { "{description}" }
            }
        }
        section { class: "document-storage",
            h2 { "Storage" }
            dl { class: "detail-dl",
                dt { "Kind" }
                dd { "{doc.kind}" }
                dt { "Content type" }
                dd { "{doc.content_type}" }
                dt { "Bytes" }
                dd { "{doc.byte_size}" }
                dt { "SHA-256" }
                dd { class: "font-monospace", "{doc.sha256_hex}" }
            }
        }
    }
}

/// Render a resolved document resource — shared by both lenses.
fn render_document(resource: &Resource<Result<DocumentDetailView, ServerFnError>>) -> Element {
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "project-document", p { "Failed to load the document." } }
            }
        }
        None => {
            return rsx! {
                main { id: "project-document", p { "Loading…" } }
            }
        }
    };
    let role = view.role;

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "project-document", class: "nav-theme",
            if let Some(doc) = view.document.as_ref() {
                {document_body(doc, &view.firm_name)}
            } else if view.failed {
                document::Title { "{view.firm_name} | Something went wrong" }
                h1 { "Something went wrong" }
                p { "This document could not be read. The error has been logged." }
            } else {
                document::Title { "{view.firm_name} | Not found" }
                h1 { "Not found" }
                p { "No document is available at this address." }
            }
        }
    }
}

/// `/app/projects/{project_code}/documents/{doc_id}` — one page, lensed by the tier.
#[component]
pub fn ProjectDocument() -> Element {
    let resource = use_server_future(get_project_document)?;
    render_document(&resource)
}

#[cfg(test)]
mod tests {
    use super::{document_body, DocumentFacts};

    fn facts() -> DocumentFacts {
        DocumentFacts {
            filename: "engagement-letter.pdf".to_string(),
            kind: "retainer".to_string(),
            source: "upload".to_string(),
            received_at: "2026-05-26T12:00:01Z".to_string(),
            description: Some("Initial sync".to_string()),
            content_type: "application/pdf".to_string(),
            byte_size: 2_048,
            sha256_hex: "deadbeefcafe1234567890abcdef0000".to_string(),
            download_href: "/app/projects/X/documents/Y/download".to_string(),
            back_href: "/app/projects/X".to_string(),
        }
    }

    #[test]
    fn renders_provenance_storage_and_the_download_link() {
        let html = dioxus_ssr::render_element(document_body(&facts(), "Neon Law"));
        assert!(html.contains(">engagement-letter.pdf<"), "{html}");
        assert!(html.contains(">Provenance<"), "{html}");
        assert!(html.contains(">Storage<"), "{html}");
        assert!(html.contains(">upload<"), "{html}");
        assert!(html.contains(">Initial sync<"), "{html}");
        assert!(html.contains(">application/pdf<"), "{html}");
        assert!(html.contains(">2048<"), "{html}");
        assert!(
            html.contains(">deadbeefcafe1234567890abcdef0000<"),
            "{html}"
        );
        assert!(
            html.contains(r#"href="/app/projects/X/documents/Y/download""#),
            "{html}"
        );
        assert!(html.contains(r#"href="/app/projects/X""#), "{html}");
        // The consolidation dropped `source_revision_id`; it must not render.
        assert!(!html.contains("Source revision"), "{html}");
    }

    #[test]
    fn a_missing_description_renders_an_em_dash_not_an_empty_row() {
        let mut facts = facts();
        facts.description = None;
        let html = dioxus_ssr::render_element(document_body(&facts, "Neon Law"));
        assert!(html.contains(">—<"), "{html}");
    }

    #[test]
    fn every_link_is_built_from_the_one_matter_surface() {
        // There is no longer a per-lens base to get wrong: both sides link
        // into the same path and the tier decides what is behind it.
        assert_eq!(super::PROJECTS_BASE, "/app/projects");
    }
}
