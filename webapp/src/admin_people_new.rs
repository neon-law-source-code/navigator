//! The "add person" create form as a Dioxus component (#641 Phase 3, admin
//! cluster) — the admin console's `/app/admin/people/new`, Owner/Admin-only.
//!
//! The successor to the `admin_people_new` GET render. It reads the injected
//! CSRF token and any `?error=` flash, and renders the shared
//! [`crate::components::FormCard`] as a native `POST /app/admin/people` — a route
//! that wraps the person create command (the form posted to the REST
//! `/app/api/people` over HTMX; the Dioxus form uses a plain form, no
//! JavaScript). On a rejected create the handler redirects back here with
//! `?error=`, surfaced above the form.
//!
//! This is the only browser surface that creates a Person. The command itself
//! stays lawyer-tier at `POST /app/api/people`, so a Lawyer creates through the
//! API rather than through a second copy of this form.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard};
use crate::people::ViewerRole;

/// The create form's `?error=` flash (set by the create handler's
/// redirect-on-failure).
#[derive(Deserialize, Default)]
pub struct PeopleNewQuery {
    #[serde(default)]
    pub error: Option<String>,
}

/// The route the native create form posts to, and the list path Cancel returns
/// to. One surface, so one path.
pub const CREATE_PATH: &str = "/app/admin/people";

/// The rendered "add person" form: the session CSRF token, an optional error
/// flash, and the viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PeopleNewView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub csrf_token: String,
    pub error: Option<String>,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Read the injected CSRF token and the `?error=` flash — the shared prelude for
/// both surfaces' server functions.
#[cfg(feature = "server")]
async fn people_new_context() -> (String, Option<String>) {
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let error = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<PeopleNewQuery>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::extract::Query(q)| q.error);
    (csrf_token, error)
}

/// Load the "add person" form (`/app/admin/people/new`): refuse non-admin-tier
/// callers. The role select is unlocked, because `require_admin` admits only
/// Owner and Admin and both may set a role.
#[server]
pub async fn get_admin_people_new_form() -> Result<PeopleNewView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let (csrf_token, error) = people_new_context().await;
    Ok(PeopleNewView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        csrf_token,
        error,
        role,
    })
}

/// The admin console "add person" form.
#[component]
pub fn AdminPeopleNew() -> Element {
    let resource = use_server_future(get_admin_people_new_form)?;
    render_people_new(&resource)
}

/// Render the resolved "add person" form: a native `POST /app/admin/people`
/// carrying the CSRF token, with the name / email / role controls.
fn render_people_new(resource: &Resource<Result<PeopleNewView, ServerFnError>>) -> Element {
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "people-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "people-new", p { "Loading…" } }
            }
        }
    };

    let role = view.role;
    let title = format!("{} | Admin | People | Add person", view.firm_name);
    let mut role_options = Vec::new();
    if role == ViewerRole::Owner {
        role_options.push(Choice::new("owner", "Owner"));
    }
    role_options.extend([
        Choice::new("admin", "Admin"),
        Choice::new("lawyer", "Lawyer (lawyer)"),
        Choice::new("clerk", "Clerk (non-lawyer)"),
        Choice::new("client", "Client"),
    ]);
    // No locked branch: `require_admin` admits only Owner and Admin, and both may
    // set a role.
    let role_field = Field::select("Role", "role", role_options, Some("client".to_string()));
    let fields = vec![
        Field::text("Name", "name", "").required(),
        Field::input("Email", "email", "", "email").required(),
        role_field,
        Field::text("Notion user ID", "notion_user_id", "").help(
            "Optional stable Notion workspace user ID. This identifies the person in Notion; it is not a credential or permission.",
        ),
    ];

    rsx! {
        document::Title { "{title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "people-new", class: "nav-theme",
            if let Some(error) = view.error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            FormCard {
                title: "Add person".to_string(),
                action: "{CREATE_PATH}",
                submit_label: "Create person".to_string(),
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
            p { a { href: "{CREATE_PATH}", "← Cancel" } }
        }
    }
}
