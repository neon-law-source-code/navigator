//! Lawyer "add entity" form as a Dioxus component (#641 Phase 3, admin cluster)
//! — the first migrated **create form**.
//!
//! The successor to the `views::pages::admin::entities::new_form`. It reads
//! the entity-type and jurisdiction choices and the session CSRF token, and
//! renders the shared [`crate::components::FormCard`] as a native `POST` to
//! `/app/admin/entities` — the existing create handler. This establishes the
//! `FormCard` + CSRF create pattern the other CRUD forms reuse. The handler
//! follows post/redirect/get: a refused create redirects back here with its
//! message as `?error=`, which renders above the form.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard};
use crate::people::ViewerRole;

/// One `<select>` option: the submitted value (a row id) and its display label.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FormChoice {
    pub value: String,
    pub label: String,
}

/// The rendered "add entity" form: the entity-type and jurisdiction options, the
/// session CSRF token for the form's hidden field, and the viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct EntityNewView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub types: Vec<FormChoice>,
    pub jurisdictions: Vec<FormChoice>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The `?error=` flash rendered above the form — set when `POST
    /// /app/admin/entities` refuses the create and redirects back here. `None` on a
    /// plain visit.
    #[serde(default)]
    pub error: Option<String>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The "add entity" form query: the `?error=` flash a refused create redirects
/// back with.
#[derive(Deserialize, Default)]
pub struct EntityNewQuery {
    #[serde(default)]
    pub error: Option<String>,
}

/// Load the "add entity" form data: refuse non-lawyer, read the injected CSRF
/// token and the `?error=` flash, and list the entity types and jurisdictions as
/// `<select>` options (jurisdictions labelled `"{name} ({code})"`, as the
/// form did).
#[server]
pub async fn get_entity_new_form() -> Result<EntityNewView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<EntityNewQuery>,
        _,
    >()
    .await?;

    // Both reference tables live in SurrealDB (ENG-20); ordered by name
    // so the pickers read alphabetically.
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let types = store::entity_types::list(&surreal, &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|t| FormChoice {
            value: t.id.to_string(),
            label: t.name,
        })
        .collect();
    let jurisdictions = store::jurisdictions::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|j| FormChoice {
            value: j.id.to_string(),
            label: format!("{} ({})", j.name, j.code),
        })
        .collect();

    Ok(EntityNewView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        types,
        jurisdictions,
        csrf_token,
        role,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// The lawyer "add entity" form. Server-side rendered as a native `POST` form to
/// `/app/admin/entities` carrying the CSRF token, so it works without JavaScript.
#[component]
pub fn LawyerEntityNew() -> Element {
    let resource = use_server_future(get_entity_new_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "entity-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "entity-new", p { "Loading…" } }
            }
        }
    };

    entity_new_body(&view)
}

/// The loaded form. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn entity_new_body(view: &EntityNewView) -> Element {
    let type_choices: Vec<Choice> = view
        .types
        .iter()
        .map(|t| Choice::new(t.value.clone(), t.label.clone()))
        .collect();
    let jur_choices: Vec<Choice> = view
        .jurisdictions
        .iter()
        .map(|j| Choice::new(j.value.clone(), j.label.clone()))
        .collect();
    let fields = vec![
        Field::text("Name", "name", "").required(),
        Field::select("Entity type", "entity_type_id", type_choices, None).required(),
        Field::select("Jurisdiction", "jurisdiction_id", jur_choices, None).required(),
    ];

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Entities | Add entity" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "entity-new", class: "nav-theme",
            if let Some(error) = view.error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            FormCard {
                title: "Add entity".to_string(),
                action: "/app/admin/entities".to_string(),
                submit_label: "Create entity".to_string(),
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
            p { a { href: "/app/admin/entities", "← Cancel" } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    #[test]
    fn the_create_form_meets_the_page_accessibility_invariants() {
        // Carried over from the retired `views/tests/accessibility.rs`
        // `entity_forms_are_accessible`: no positive tabindex, every
        // `<label for>` and `aria-describedby` resolves, and the form is named.
        let html = dioxus_ssr::render_element(entity_new_body(&EntityNewView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            types: vec![FormChoice {
                value: "00000000-0000-0000-0000-000000000001".to_string(),
                label: "LLC".to_string(),
            }],
            jurisdictions: vec![FormChoice {
                value: "00000000-0000-0000-0000-000000000001".to_string(),
                label: "Nevada (NV)".to_string(),
            }],
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
            error: None,
        }));
        assert_forms_accessible(&html, "entity_new::LawyerEntityNew");
        assert!(html.contains("action=\"/app/admin/entities\""), "{html}");
        assert!(!html.contains("nav-form-error"), "{html}");
    }

    #[test]
    fn a_refused_create_surfaces_its_message_above_a_resubmittable_form() {
        // `POST /app/admin/entities` redirects a refused create back here carrying
        // its message as `?error=`. Without the flash the reload reads as a
        // no-op: the entity is simply absent and nothing says why.
        let html = dioxus_ssr::render_element(entity_new_body(&EntityNewView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            types: vec![],
            jurisdictions: vec![],
            csrf_token: "TOK".to_string(),
            role: ViewerRole::Lawyer,
            error: Some("Name is required.".to_string()),
        }));
        assert!(html.contains("nav-form-error"), "{html}");
        assert!(html.contains("Name is required."), "{html}");
        // The form is still there to correct and resubmit, CSRF token intact.
        assert!(html.contains("action=\"/app/admin/entities\""), "{html}");
        assert!(html.contains("value=\"TOK\""), "{html}");
    }
}
