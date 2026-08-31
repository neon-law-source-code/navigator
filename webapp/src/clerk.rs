//! The supervised Clerk rendering of the matter surface (`/app/projects` and
//! `/app/projects/{code}`) as Dioxus
//! components (#956 Phase 4) — the supervised, read-only Project lens.
//!
//! The successor to the `views::pages::clerk` renders. The surface is
//! deliberately small: a Clerk sees a matter only when they hold a firm-side
//! participation row **and** the matter names a currently licensed lawyer as its
//! lawyer DRI, and then sees only its name, status, and supervisor. Legal advice,
//! drafting, document contents, approvals, filing, Git, MCP, A2A, and
//! conversations are absent by construction, not merely hidden.
//!
//! Reads go through `store::access::supervised_projects` — the same shared
//! `store` call the handler used, which resolves visibility and the
//! supervisor disclosure as one answer so the list and the detail page can never
//! disagree about who supervises a matter. There is no `/api` read cluster for
//! the Clerk lens yet (#866 has not covered it); when one lands, both loaders
//! move onto it rather than growing a bespoke JSON endpoint (#690).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::csrf::CsrfToken;
use crate::people::ViewerRole;
use crate::portal_project_list::PersonId;

/// One supervised matter, in a wasm-safe shape (plain strings — no
/// `store`/`SeaORM` types cross to the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ClerkProjectRow {
    pub id: String,
    /// The Project code, which keys both the matter page and client portal.
    pub code: String,
    pub name: String,
    pub status: String,
    /// The display name of the licensed lawyer accountable for the matter. Never
    /// empty: a matter whose supervisor could not be resolved is not visible.
    pub lawyer_dri: String,
}

/// The rendered Clerk projects list.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ClerkProjectsView {
    /// The supervised matters. Empty both when the Clerk has no assignments yet
    /// and when the surface is hidden — [`ClerkProjectsView::found`] tells them
    /// apart.
    pub rows: Vec<ClerkProjectRow>,
    /// The surface exists for this viewer (they hold the `clerk` role). `false`
    /// renders the not-found body under a committed `404`, the same answer the
    /// handler gave a non-Clerk.
    pub found: bool,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The rendered Clerk project detail page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ClerkProjectDetailView {
    /// The matter, or `None` when it is not visible to this Clerk (an unknown
    /// id, another Clerk's matter, or a non-Clerk caller) — rendered as
    /// not-found under a committed `404`.
    pub project: Option<ClerkProjectRow>,
    /// The matter's six collaboration resources, read-only. A Clerk is a firm
    /// tier, so all six are visible; `can_configure` is `false`, because a
    /// supervised non-lawyer reads the matter's working surfaces without
    /// holding authority to change them.
    #[serde(default)]
    pub resources: crate::project_resources::ProjectResourcesView,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
    /// The session CSRF token for the client-preview form. The control changes
    /// only the current browser lens; it does not change a matter.
    #[serde(default)]
    pub csrf_token: String,
}

/// Read the injected viewer tier, defaulting to the least-privileged tier when
/// the request did not carry it (a direct hit on the generated `#[server]`
/// endpoint need not run behind the route's auth + embedded Rego policy gate).
#[cfg(feature = "server")]
async fn injected_role() -> ViewerRole {
    dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default()
}

/// Read the injected `persons.id` of the signed-in viewer. `None` when the
/// session carries no linked person — fail-closed, so the loader sees nothing.
#[cfg(feature = "server")]
async fn injected_person_id() -> Option<uuid::Uuid> {
    dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PersonId>, _>()
        .await
        .ok()
        .and_then(|axum::Extension(id)| id.0)
        .and_then(|raw| raw.parse::<uuid::Uuid>().ok())
}

/// Commit a `404` and wrap a query error as a `500`, mirroring the
/// handler's `not_found` / `internal_error` responses.
#[cfg(feature = "server")]
fn commit_not_found() {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
}

#[cfg(feature = "server")]
fn server_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

/// Load the supervised matters for the signed-in Clerk.
///
/// The exact-role check is repeated here rather than left to the route layers:
/// Admin's general embedded Rego policy bypass must not turn `/clerk` into another admin
/// workbench, and a direct hit on the generated endpoint carries no gate at all.
/// A non-Clerk caller gets the handler's `404` — the surface is hidden from
/// them, not merely refused.
#[server]
pub async fn get_clerk_projects() -> Result<ClerkProjectsView, ServerFnError> {
    let role = injected_role().await;
    let firm_name = crate::app_chrome::firm_name_from_context().await;
    let Some(person_id) = injected_person_id()
        .await
        .filter(|_| role == ViewerRole::Clerk)
    else {
        commit_not_found();
        return Ok(ClerkProjectsView {
            role,
            firm_name,
            ..ClerkProjectsView::default()
        });
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    // The lens resolves each matter's supervising lawyer as part of deciding
    // visibility, so there is no second lookup and no way to render a matter
    // whose supervisor could not be named.
    let supervised = store::access::supervised_projects(&surreal, person_id)
        .await
        .map_err(server_error)?;

    Ok(ClerkProjectsView {
        firm_name,
        rows: supervised
            .into_iter()
            .map(|(project, supervisor)| ClerkProjectRow {
                id: project.id.to_string(),
                code: project.code,
                name: project.name,
                status: project.status,
                lawyer_dri: supervisor.name,
            })
            .collect(),
        found: true,
        role,
    })
}

/// Load one supervised matter for the signed-in Clerk. Visibility and the
/// supervisor disclosure are the same answer as the list's, so the detail page
/// cannot disagree with the list about who supervises a matter. Anything the
/// Clerk may not see — a non-Clerk caller, an unknown id, another Clerk's matter
/// — is the same `404`.
#[server]
pub async fn get_clerk_project() -> Result<ClerkProjectDetailView, ServerFnError> {
    let axum::extract::Path(code) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    let role = injected_role().await;
    let firm_name = crate::app_chrome::firm_name_from_context().await;
    let csrf_token =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<CsrfToken>, _>()
            .await
            .map(|axum::Extension(token)| token.0)
            .unwrap_or_default();
    let not_found = |role| {
        commit_not_found();
        Ok(ClerkProjectDetailView {
            firm_name: firm_name.clone(),
            project: None,
            resources: crate::project_resources::ProjectResourcesView::default(),
            role,
            csrf_token: csrf_token.clone(),
        })
    };
    let Some(person_id) = injected_person_id()
        .await
        .filter(|_| role == ViewerRole::Clerk)
    else {
        return not_found(role);
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let supervised = store::access::supervised_projects(&surreal, person_id)
        .await
        .map_err(server_error)?;
    let Some((project, supervisor)) = supervised
        .into_iter()
        .find(|(project, _supervisor)| project.code == code)
    else {
        return not_found(role);
    };

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
        // Read-only: a Clerk holds no authority to change a matter.
        can_configure: false,
        project_code: project.code.clone(),
    };
    Ok(ClerkProjectDetailView {
        firm_name,
        project: Some(ClerkProjectRow {
            id: project.id.to_string(),
            code: project.code,
            name: project.name,
            status: project.status,
            lawyer_dri: supervisor.name,
        }),
        resources,
        role,
        csrf_token,
    })
}

/// The Clerk nav chrome. One destination now that the supervised surface is a
/// rendering of `/app/projects` rather than its own namespace.
/// A Clerk is not Lawyer, so no `/app/lawyer` or `/admin` link is offered —
/// the `PageLayout` drew the same three links for this tier.
fn clerk_nav() -> Element {
    rsx! {
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
    }
}

/// The page body shown when the Clerk surface does not exist for this viewer.
fn clerk_not_found(heading: &str, message: &str) -> Element {
    rsx! {
        h1 { "{heading}" }
        p { class: "nav-muted", "{message}" }
    }
}

/// The Clerk matter list. Server-side rendered with the supervised
/// matters already in the markup — readable before hydration, each card a real
/// anchor to the matter's detail page.
#[component]
pub fn ClerkProjects() -> Element {
    let resource = use_server_future(get_clerk_projects)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "clerk-projects", class: "nav-theme", p { "Failed to load your projects." } }
            }
        }
        None => {
            return rsx! {
                main { id: "clerk-projects", class: "nav-theme", p { "Loading…" } }
            }
        }
    };

    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Clerk projects" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        {clerk_nav()}
        main { id: "clerk-projects", class: "nav-theme",
            if view.found {
                h1 { "Clerk projects" }
                if is_empty {
                    p { class: "nav-muted",
                        "No supervised projects are assigned to you. A project appears here only after an administrator records your firm-side participation and a licensed lawyer is its disclosed lawyer DRI."
                    }
                } else {
                    p { class: "nav-muted",
                        "This is a read-only coordination view. Your supervising lawyer is named on every project; legal advice, drafts, documents, approvals, Git, and conversations are not available here."
                    }
                    div { class: "portal-projects",
                        for row in view.rows.iter().cloned() {
                            a { class: "portal-project-card", key: "{row.id}", href: "/app/projects/{row.code}",
                                div { class: "portal-project-card__name", "{row.name}" }
                                div { class: "portal-project-card__status", "Status: {row.status}" }
                                div { class: "clerk-supervisor",
                                    "Supervising lawyer: "
                                    strong { "{row.lawyer_dri}" }
                                }
                            }
                        }
                    }
                }
            } else {
                {clerk_not_found("Not found", "This page is not available.")}
            }
        }
    }
}

/// The Clerk matter page — the matter's name,
/// status, and supervising lawyer, plus the limited-access disclosure. The one
/// form opens the matter's existing client rendering, and the server rechecks
/// the Clerk's supervised access before switching the session.
#[component]
pub fn ClerkProjectDetail() -> Element {
    let resource = use_server_future(get_clerk_project)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "clerk-project-detail", class: "nav-theme", p { "Failed to load the project." } }
            }
        }
        None => {
            return rsx! {
                main { id: "clerk-project-detail", class: "nav-theme", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Title { "{view.firm_name} | Clerk project" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        {clerk_nav()}
        main { id: "clerk-project-detail", class: "nav-theme",
            match view.project.as_ref() {
                Some(project) => rsx! {
                    p { class: "portal-detail__back", a { href: "/app/projects", "← Projects" } }
                    h1 { "{project.name}" }
                    p { class: "nav-muted", "Read-only coordination view for a supervised non-lawyer Clerk." }
                    dl { class: "clerk-facts",
                        dt { "Matter status" }
                        dd { "{project.status}" }
                        dt { "Supervising lawyer" }
                        dd { "{project.lawyer_dri}" }
                    }
                    form {
                        class: "lawyer-detail__inline-form",
                        method: "post",
                        action: "/app/projects/{project.code}/view-as-client",
                        input { r#type: "hidden", name: "_csrf", value: "{view.csrf_token}" }
                        button { class: "nav-btn nav-btn--secondary", r#type: "submit", "View as Client" }
                    }
                    crate::project_resources::ProjectResourcesPanel { view: view.resources.clone() }
                    div { class: "nav-form-notice", role: "note",
                        strong { "Limited access. " }
                        "The Project's client portal is available above. Legal advice, drafting, approval, filing, Git, MCP, A2A, and conversations are not available on this surface."
                    }
                },
                None => rsx! {
                    {clerk_not_found("Not found", "This project is not available to you.")}
                    p { a { href: "/app/projects", "← Projects" } }
                },
            }
        }
    }
}
