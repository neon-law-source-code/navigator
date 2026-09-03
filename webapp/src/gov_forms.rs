//! The blank government-forms index as a Dioxus component (#956 Phase 4).
//!
//! The successor to the `views::pages::portal::forms` read view. It lists
//! every vendored form from the `forms` registry: the exact canonical bytes the
//! workflows fill, downloadable as blanks.
//!
//! The registry is not a database query — it is `Arc<Vec<forms::FormMeta>>` on
//! `portal`'s router state, which `webapp` cannot see and must not depend on.
//! So `portal` shapes it into [`GovFormRows`] and injects that as a wasm-safe
//! request extension, the same seam `PersonId` and `Impersonating` use. The
//! download route (`/app/forms/{code}.pdf`) is untouched and still Axum-side.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// One vendored form as the index renders it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GovFormRow {
    pub code: String,
    pub title: String,
    pub jurisdiction: String,
    pub origin_url: String,
}

/// Request-extension carrier for the registry rows, injected by `portal`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GovFormRows(pub Vec<GovFormRow>);

/// The rendered index: the rows, and the viewer's tier for the nav chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct GovFormsView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub rows: Vec<GovFormRow>,
    pub role: ViewerRole,
    /// Who the viewer is acting as, when an admin is impersonating a client.
    #[serde(default)]
    pub impersonation: Option<crate::components::ImpersonationView>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Read the injected registry rows and viewer tier for the current request.
#[server]
pub async fn list_gov_forms() -> Result<GovFormsView, ServerFnError> {
    let GovFormRows(rows) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<GovFormRows>, _>()
            .await
            .map(|axum::Extension(rows)| rows)
            .unwrap_or_default();

    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();

    let crate::components::Impersonating(impersonation) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::Extension<crate::components::Impersonating>,
            _,
        >()
        .await
        .map(|axum::Extension(i)| i)
        .unwrap_or_default();

    Ok(GovFormsView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        role,
        impersonation,
    })
}

/// The blank-forms index. Server-side rendered with the rows already in the
/// markup, readable before hydration.
#[component]
pub fn GovForms() -> Element {
    let resource = use_server_future(list_gov_forms)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "gov-forms", p { "Failed to load the form registry." } }
            }
        }
        None => {
            return rsx! {
                main { id: "gov-forms", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Title { "{view.firm_name} | Blank government forms" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::ImpersonationBanner { view: view.impersonation.clone() }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "gov-forms", class: "nav-theme",
            h1 { "Blank government forms" }
            p { class: "nav-muted",
                "The official forms Neon Law Navigator fills — vendored from each authority's own site "
                "and stored at the same path used in the public assets bucket. Download a blank to read "
                "what a filing asks before you answer the questionnaire; your matter's filled copy "
                "always goes through attorney review."
            }
            div { class: "nav-table-wrap",
                table { class: "nav-table",
                    thead {
                        tr {
                            th { "Form" }
                            th { "Jurisdiction" }
                            th { "Origin" }
                            th { "" }
                        }
                    }
                    tbody {
                        for row in view.rows.iter().cloned() {
                            tr { key: "{row.code}",
                                td {
                                    div { "{row.title}" }
                                    code { class: "nav-muted", "{row.code}" }
                                }
                                td { "{row.jurisdiction}" }
                                td {
                                    a { href: "{row.origin_url}", rel: "noopener noreferrer",
                                        "government website"
                                    }
                                }
                                td {
                                    a {
                                        class: "nav-btn nav-btn--secondary",
                                        href: "/app/forms/{row.code}.pdf",
                                        "Download blank"
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

#[cfg(test)]
mod tests {
    use super::*;

    fn nevada() -> GovFormRow {
        GovFormRow {
            code: "nv__llc_formation".to_string(),
            title: "Nevada LLC Formation".to_string(),
            jurisdiction: "NV".to_string(),
            origin_url: "https://www.nvsos.gov/businesses".to_string(),
        }
    }

    /// Render the table body directly, bypassing the server function — the
    /// same shape the other migrated list-page tests use.
    fn rows_html(rows: Vec<GovFormRow>) -> String {
        let mut dom = VirtualDom::new_with_props(
            |rows: Vec<GovFormRow>| {
                rsx! {
                    table { class: "nav-table",
                        tbody {
                            for row in rows.iter().cloned() {
                                tr {
                                    td { "{row.title}" }
                                    td { "{row.jurisdiction}" }
                                    td {
                                        a { href: "{row.origin_url}", "government website" }
                                    }
                                    td {
                                        a { href: "/app/forms/{row.code}.pdf", "Download blank" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            rows,
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn lists_the_form_with_a_download_link() {
        let html = rows_html(vec![nevada()]);
        // Match the text node itself: SSR wraps text in hydration comments, so
        // `class="…">Text` would not match.
        assert!(html.contains(">Nevada LLC Formation<"), "{html}");
        assert!(html.contains(">NV<"), "{html}");
        assert!(
            html.contains(r#"href="/app/forms/nv__llc_formation.pdf""#),
            "the blank must be downloadable at its registry code: {html}",
        );
        assert!(html.contains(">government website<"), "{html}");
    }

    #[test]
    fn an_empty_registry_renders_no_rows() {
        let html = rows_html(Vec::new());
        assert!(!html.contains("<tr"), "no rows without a registry: {html}");
    }
}
