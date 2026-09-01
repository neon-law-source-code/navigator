//! The lawyer-facing form for opening a Notation on an existing Project.
//!
//! The form posts natively to the existing project-scoped notation-create
//! handler. It only collects the handler's two fields; the questionnaire starts
//! after that command opens the Notation.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Field, FormCard, Heading};
use crate::people::ViewerRole;

/// The data needed to render the project-scoped notation-create form.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectNotationNewView {
    pub found: bool,
    pub project_code: String,
    pub project_name: String,
    pub csrf_token: String,
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    #[serde(default)]
    pub firm_name: String,
}

#[cfg(feature = "server")]
async fn hidden(role: ViewerRole) -> ProjectNotationNewView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    ProjectNotationNewView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        ..ProjectNotationNewView::default()
    }
}

#[server]
pub async fn get_project_notation_new_form() -> Result<ProjectNotationNewView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    if !role.is_lawyer_tier() {
        return Ok(hidden(role).await);
    }

    let Ok(axum::extract::Path(project_code)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>().await
    else {
        return Ok(hidden(role).await);
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(project) = store::projects::find_by_code(&surreal, &project_code)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(hidden(role).await);
    };

    let person_id = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::portal_project_list::PersonId>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::Extension(id)| id.0)
    .and_then(|raw| raw.parse::<uuid::Uuid>().ok());
    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };
    // Keep the form's read scope identical to the existing POST command: a
    // lawyer must participate in the matter, while Admin/Owner retain the
    // command layer's privileged bypass.
    if !store::access::can_see_project_as_lawyer(&surreal, person_id, store_role, project.id)
        .await
        .map_err(|e| ServerFnError::new(e.clone()))?
    {
        return Ok(hidden(role).await);
    }

    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();

    Ok(ProjectNotationNewView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: true,
        project_code: project.code,
        project_name: project.name,
        csrf_token,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
    })
}

fn notation_new_body(view: &ProjectNotationNewView) -> Element {
    let action = format!("/app/projects/{}/notations/new", view.project_code);
    let fields = vec![
        Field::text("Template code", "template_code", "")
            .required()
            .placeholder("retainer")
            .help("The template code to read from this matter's repository."),
        Field::email("Client email", "client_email", "")
            .required()
            .placeholder("client@example.com")
            .help("The client this Notation is bound to."),
    ];

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Notations | New" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "project-notation-new", class: "nav-theme",
            header { class: "page-header",
                h1 { "New notation" }
                p { class: "nav-muted",
                    "Open a notation on " strong { "{view.project_name}" }
                    " (" code { "{view.project_code}" } ")."
                }
            }
            FormCard {
                title: "Open notation".to_string(),
                action,
                submit_label: "Start questionnaire".to_string(),
                heading: Heading::H2,
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
            p { class: "project-form-cancel",
                a { class: "nav-btn nav-btn--secondary", href: "/app/projects/{view.project_code}", "Cancel" }
            }
        }
    }
}

#[component]
pub fn LawyerProjectNotationNew() -> Element {
    let resource = use_server_future(get_project_notation_new_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if view.found => view.clone(),
        Some(Ok(view)) => {
            return rsx! {
                document::Title { "{view.firm_name} | Lawyer | Not found" }
                crate::components::AppNavbar {
                    destinations: crate::app_chrome::app_destinations(view.role),
                    logo: view.logo.clone(),
                }
                main { id: "project-notation-new", class: "nav-theme",
                    h1 { "Not found" }
                    p { "No notation form is available at this address." }
                }
            };
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "project-notation-new", p { "Failed to load the form." } }
            };
        }
        None => {
            return rsx! {
                main { id: "project-notation-new", p { "Loading…" } }
            };
        }
    };

    notation_new_body(&view)
}

#[cfg(test)]
mod tests {
    use super::{notation_new_body, ProjectNotationNewView};
    use crate::components::form::assert_forms_accessible;
    use crate::people::ViewerRole;

    fn view() -> ProjectNotationNewView {
        ProjectNotationNewView {
            found: true,
            project_code: "sample-litigation".to_string(),
            project_name: "Cruller v. Prine".to_string(),
            csrf_token: "CSRF-TOKEN".to_string(),
            role: ViewerRole::Lawyer,
            ..ProjectNotationNewView::default()
        }
    }

    #[test]
    fn the_form_drives_the_existing_project_notation_post() {
        let html = dioxus_ssr::render_element(notation_new_body(&view()));

        assert!(
            html.contains(r#"action="/app/projects/sample-litigation/notations/new""#),
            "{html}"
        );
        assert!(html.contains(r#"method="post""#), "{html}");
        assert!(
            html.contains(r#"name="_csrf" value="CSRF-TOKEN""#),
            "{html}"
        );
        assert!(html.contains(r#"name="template_code""#), "{html}");
        assert!(html.contains(r#"name="client_email""#), "{html}");
        assert!(html.contains("New notation"), "{html}");
        assert!(html.contains("admin-form"), "{html}");
        assert_forms_accessible(&html, "project_notation::LawyerProjectNotationNew");
    }
}
