//! The client portal projects list (`/app/projects`) as a Dioxus component
//! (#641 Phase 3, portal cluster) — the read-only "Your Projects" dashboard.
//!
//! The successor to the `portal::project_list_response`. It scopes the
//! signed-in client's matters through `store::access::visible_projects_as_client`
//! (the visibility layer relocated to `store` in #733 so a `#[server]` function
//! can reach it), aggregates the open/closed KPI summary, and renders the
//! dashboard cards. Read-only — the client clicks a card to the project detail
//! page; there are no forms.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The signed-in viewer's linked `persons.id`, injected into the request by the
/// portal `inject_person_id` layer — a wasm-safe newtype the `#[server]` function
/// extracts to scope the visible matters. `webapp` cannot see the portal
/// `SessionData` where it lives, so it is injected the same way [`ViewerRole`] /
/// [`crate::csrf::CsrfToken`] are. `None` when the session has no linked person.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PersonId(pub Option<String>);

/// One project card, in a wasm-safe shape (plain strings — no `store`/`SeaORM`
/// types cross to the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ClientProjectRow {
    pub id: String,
    pub code: String,
    pub name: String,
    pub status: String,
}

/// The dashboard KPI summary: open vs closed matters.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ClientProjectsSummary {
    pub open_projects: usize,
    pub closed_projects: usize,
}

/// The rendered client projects dashboard.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ClientProjectsView {
    pub summary: ClientProjectsSummary,
    pub rows: Vec<ClientProjectRow>,
    pub role: ViewerRole,
    /// Who the viewer is acting as, when an admin is impersonating a client.
    /// `None` for an ordinary session, which renders no banner.
    #[serde(default)]
    pub impersonation: Option<crate::components::ImpersonationView>,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    /// The deploy's own firm, for the document title. Resolved from
    /// request-scoped branding so a white-label portal names its operator.
    #[serde(default)]
    pub firm_name: String,
}

/// Fetch the signed-in client's projects dashboard: scope the matters through
/// `store::access::visible_projects_as_client` and aggregate the KPI summary —
/// the same command boundary `project_list_response` used, now server-side of
/// a Dioxus render.
#[server]
pub async fn list_client_projects() -> Result<ClientProjectsView, ServerFnError> {
    let PersonId(person_id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PersonId>, _>()
            .await
            .map(|axum::Extension(id)| id)
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
    let person_id = person_id.and_then(|raw| raw.parse::<uuid::Uuid>().ok());

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let projects = store::access::visible_projects_as_client(&surreal, person_id)
        .await
        .map_err(|e| ServerFnError::new(e.clone()))?;

    let summary = ClientProjectsSummary {
        open_projects: projects
            .iter()
            .filter(|p| p.status.eq_ignore_ascii_case("open"))
            .count(),
        closed_projects: projects
            .iter()
            .filter(|p| p.status.eq_ignore_ascii_case("closed"))
            .count(),
    };

    let rows = projects
        .into_iter()
        .map(|p| ClientProjectRow {
            id: p.id.to_string(),
            code: p.code,
            name: p.name,
            status: p.status,
        })
        .collect();

    Ok(ClientProjectsView {
        summary,
        rows,
        role,
        impersonation,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
    })
}

/// The client portal projects dashboard. Server-side rendered with the matters
/// already in the markup (via [`use_server_future`]), readable before hydration.
#[component]
pub fn ClientProjects() -> Element {
    let resource = use_server_future(list_client_projects)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "portal-projects", p { "Failed to load your projects." } }
            }
        }
        None => {
            return rsx! {
                main { id: "portal-projects", p { "Loading…" } }
            }
        }
    };

    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Portal" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::ImpersonationBanner { view: view.impersonation.clone() }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "portal-projects", class: "nav-theme",
            h1 { "Your Projects" }
            div { class: "portal-kpis",
                {kpi_card("Open", view.summary.open_projects, "Currently active")}
                {kpi_card("Closed", view.summary.closed_projects, "Completed")}
            }
            if is_empty {
                p { class: "portal-empty", "You have no projects yet." }
            }
            div { class: "portal-projects",
                for row in view.rows.iter().cloned() {
                    a { class: "portal-project-card", key: "{row.id}", href: "/app/projects/{row.code}",
                        div { class: "portal-project-card__name", "{row.name}" }
                        div { class: "portal-project-card__status", "Status: {row.status}" }
                    }
                }
            }
        }
    }
}

/// One KPI tile on the dashboard.
fn kpi_card(label: &str, value: usize, hint: &str) -> Element {
    rsx! {
        div { class: "portal-kpi",
            div { class: "portal-kpi__value", "{value}" }
            div { class: "portal-kpi__label", "{label}" }
            div { class: "portal-kpi__hint", "{hint}" }
        }
    }
}
