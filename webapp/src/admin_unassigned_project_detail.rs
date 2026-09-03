//! The Owner/Admin "participation only" matter view (`/app/projects/{code}`)
//! — the sixth rendering `crate::matter_surface::ProjectDetail` dispatches
//! when `store::access::matter_viewer` answers `None` for an Owner/Admin
//! caller.
//!
//! `matter_viewer` carries no privileged short-circuit (ENG-81): it answers
//! `None` for this caller exactly as it would for anyone else with no
//! participation row. This page exists on top of that unchanged answer, so
//! that `None` does not have to mean `404` for the two tiers whose job
//! includes staffing a matter nobody has put them on yet — adding the first
//! participant to a brand-new matter, or reassigning one on an existing
//! matter, would otherwise require already being on it. It discloses the
//! matter's name/code/status/entity and the participation ledger — the same
//! [`crate::lawyer_project_detail::ParticipationTable`] the workbench renders
//! — and nothing else: no document, no notation, no resource link, no
//! calendar, no "Edit project" link (that form's own gate is
//! `store::access::can_see_project`, the same ENG-81 rule, so it would just
//! `404` here too). "Edit entity" stays, because an entity is firm-wide
//! reference data with no participation ledger of its own
//! (`crate::admin_listing::require_lawyer`'s gate, not the matter surface's).
//!
//! Reusing the workbench's admin-gated write routes
//! (`/app/projects/{code}/people/...`, `crate::project_participation`) is what
//! makes this safe: those routes already check `role.is_admin_tier()` alone,
//! never a participation row, so this page adds no new write capability —
//! only a place to reach the one that already existed.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::lawyer_project_detail::ParticipationRow;
use crate::people::ViewerRole;

/// The rendered participation-only view.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct AdminUnassignedView {
    /// `false` when the caller is not admin-tier or the matter does not
    /// exist — the page renders not-found under a committed `404`.
    pub found: bool,
    pub code: String,
    pub name: String,
    pub status: String,
    pub entity_id: Option<String>,
    pub entity_name: Option<String>,
    pub participations: Vec<ParticipationRow>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    /// The deploy's firm name, for the document title.
    #[serde(default)]
    pub firm_name: String,
}

/// Commit the `404` the handler returned for a matter the caller cannot see
/// (or a non-admin caller) and return an empty (nameless) view.
#[cfg(feature = "server")]
fn not_found(
    role: ViewerRole,
    logo: Option<crate::components::AppLogo>,
    firm_name: String,
) -> AdminUnassignedView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    AdminUnassignedView {
        role,
        logo,
        firm_name,
        ..AdminUnassignedView::default()
    }
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

/// Fetch the participation-only view for one matter. Refuses a non-admin
/// caller and an unknown matter with `404`, independent of whichever
/// dispatch decision routed the request here — a direct hit on this
/// generated endpoint owes the same answer.
#[server]
pub async fn get_admin_unassigned_project_detail() -> Result<AdminUnassignedView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    let logo = crate::app_chrome::app_logo_from_context().await;
    let tokens_href = crate::app_chrome::app_tokens_href_from_context().await;
    let firm_name = crate::app_chrome::firm_name_from_context().await;
    let person_id = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::portal_project_list::PersonId>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::Extension(pid)| pid.0);
    // Admin-tier only, hidden rather than refused — the same rule
    // `crate::project_participation` already applies to every write this page
    // links to. `person_id` too, mirroring the fail-closed rule
    // `store::access::matter_viewer` applies before it ever queries: a
    // session naming no linked person is not an identified admin.
    if !role.is_admin_tier() || person_id.is_none() {
        return Ok(not_found(role, logo, firm_name));
    }
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let Ok(axum::extract::Path(code)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>().await
    else {
        return Ok(not_found(role, logo, firm_name));
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(project) = store::projects::find_by_code(&surreal, &code)
        .await
        .map_err(server_error)?
    else {
        return Ok(not_found(role, logo, firm_name));
    };

    let entity = store::entities::find_by_id(&surreal, project.entity_id)
        .await
        .map_err(server_error)?;
    let entity_id = entity.as_ref().map(|entity| entity.id.to_string());
    let entity_name = entity.map(|entity| entity.name);

    let participations =
        crate::lawyer_project_detail::participation_rows_for(&surreal, project.id).await?;

    Ok(AdminUnassignedView {
        found: true,
        code: project.code,
        name: project.name,
        status: project.status,
        entity_id,
        entity_name,
        participations,
        csrf_token,
        role,
        logo,
        tokens_href,
        firm_name,
    })
}

/// The Owner/Admin participation-only matter view.
#[component]
pub fn AdminUnassignedProjectDetail() -> Element {
    let resource = use_server_future(get_admin_unassigned_project_detail)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if view.found => view.clone(),
        Some(Ok(_)) => {
            return rsx! {
                main { id: "admin-unassigned-project", p { "That matter was not found." } }
            }
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "admin-unassigned-project", p { "Failed to load this matter." } }
            }
        }
        None => {
            return rsx! {
                main { id: "admin-unassigned-project", p { "Loading…" } }
            }
        }
    };

    let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".to_string());
    let entity_disp = dash(&view.entity_name);

    rsx! {
        document::Title { "{view.name} — Project" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "admin-unassigned-project", class: "nav-theme lawyer-detail",
            header { class: "page-header",
                h1 { "{view.name}" }
                p { class: "nav-muted",
                    "Code: " code { "{view.code}" }
                    " · Status: {view.status}"
                    " · Entity: {entity_disp}"
                    if let Some(entity_id) = view.entity_id.as_ref() {
                        " · "
                        a { class: "nav-link", href: "/app/admin/entities/{entity_id}/edit", "Edit entity" }
                    }
                }
                p { class: "nav-muted",
                    "You are not assigned to this matter, so its documents and other content stay hidden here. You can still manage who is assigned."
                }
            }

            crate::lawyer_project_detail::ParticipationTable {
                code: view.code.clone(),
                csrf: view.csrf_token.clone(),
                participations: view.participations.clone(),
                is_admin: true,
                may_govern_lawyer_side: true,
                may_govern_client_side: true,
            }
        }
    }
}
