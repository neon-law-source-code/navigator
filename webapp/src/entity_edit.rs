//! Lawyer "edit entity" form as a Dioxus component (#641 Phase 3, admin cluster)
//! — the first migrated CRUD **edit form** (a create form prefilled from an
//! existing record, keyed by its `{id}` path parameter).
//!
//! The successor to the `views::pages::admin::entities::edit_form` GET
//! render. It reads the `{id}`, loads the entity (a not-found state when the id
//! resolves to no row), and renders the shared [`crate::components::FormCard`]
//! prefilled with the entity's name and selected type / jurisdiction, posting to
//! `/app/admin/entities/{id}` — the existing update handler. That handler is
//! POST-only and follows post/redirect/get: it redirects to the list on success
//! and back to this page on a refusal, carrying the message and the rejected
//! values in the query, which the loader overlays onto the stored row.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard};
use crate::entity_new::FormChoice;
use crate::people::ViewerRole;

/// The prefilled fields of the entity being edited.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EntityFields {
    pub name: String,
    pub selected_type: String,
    pub selected_jurisdiction: String,
}

/// The rendered "edit entity" form: the entity id (for the form action), its
/// prefilled fields (`None` when the id resolves to no entity), the type and
/// jurisdiction options, the session CSRF token, and the viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct EntityEditView {
    pub id: String,
    pub fields: Option<EntityFields>,
    pub types: Vec<FormChoice>,
    pub jurisdictions: Vec<FormChoice>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The `?error=` flash rendered above the form — set when `POST
    /// /app/admin/entities/{id}` refuses the update and redirects back here. `None`
    /// on a plain visit.
    #[serde(default)]
    pub error: Option<String>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The "edit entity" form query: the `?error=` flash a refused update redirects
/// back with, plus the values that update submitted. A refusal is the caller's
/// to correct, so the rejected edit is what the form must show — reloading the
/// stored row instead would silently discard what they typed.
#[derive(Deserialize, Default)]
pub struct EntityEditQuery {
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entity_type_id: Option<String>,
    #[serde(default)]
    pub jurisdiction_id: Option<String>,
}

/// Load the "edit entity" form for the `{id}` in the request path: refuse
/// non-lawyer, read the injected CSRF token, load the entity (`fields: None` when
/// it resolves to no row), overlay any values a refused update redirected back
/// with, and list the type/jurisdiction options.
#[server]
pub async fn get_entity_edit_form() -> Result<EntityEditView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let axum::extract::Path(id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<uuid::Uuid>, _>()
            .await?;
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<EntityEditQuery>,
        _,
    >()
    .await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let entity = store::entities::find_by_id(&surreal, id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let fields = entity.map(|e| EntityFields {
        name: query.name.unwrap_or(e.name),
        selected_type: query
            .entity_type_id
            .unwrap_or_else(|| e.entity_type_id.to_string()),
        selected_jurisdiction: query
            .jurisdiction_id
            .unwrap_or_else(|| e.jurisdiction_id.to_string()),
    });

    // A valid UUID that resolves to no row is a missing resource: set the SSR
    // response status to the same 404 the retired edit handler returned,
    // so clients, caches, and monitoring see the not-found state as not-found
    // rather than a successful page. The component still renders the not-found
    // body under that status (the status is committed before the initial chunk).
    if fields.is_none() {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
    }

    // Both reference tables live in SurrealDB (ENG-20); ordered by name
    // so the pickers read alphabetically.
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

    Ok(EntityEditView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        id: id.to_string(),
        fields,
        types,
        jurisdictions,
        csrf_token,
        role,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// Prepend the "Choose…" placeholder the select carried, then the options.
fn options_with_placeholder(choices: &[FormChoice]) -> Vec<Choice> {
    let mut opts = vec![Choice::new("", "Choose…")];
    opts.extend(
        choices
            .iter()
            .map(|c| Choice::new(c.value.clone(), c.label.clone())),
    );
    opts
}

/// The lawyer "edit entity" form. Server-side rendered as a native `POST` to
/// `/app/admin/entities/{id}` carrying the CSRF token, prefilled with the entity's
/// values — or a not-found state when the id resolves to no entity.
#[component]
pub fn LawyerEntityEdit() -> Element {
    let resource = use_server_future(get_entity_edit_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "entity-edit", p { "Failed to load the entity." } }
            }
        }
        None => {
            return rsx! {
                main { id: "entity-edit", p { "Loading…" } }
            }
        }
    };

    entity_edit_body(&view)
}

/// The loaded form (or the not-found state). Split from the component so the
/// tests render a fixed view without standing up the server function.
fn entity_edit_body(view: &EntityEditView) -> Element {
    let view = view.clone();
    let error = view.error.clone();

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "entity-edit", class: "nav-theme",
            if let Some(error) = error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            match view.fields {
                Some(fields) => {
                    let type_opts = options_with_placeholder(&view.types);
                    let jur_opts = options_with_placeholder(&view.jurisdictions);
                    let form_fields = vec![
                        Field::text("Name", "name", fields.name).required(),
                        Field::select(
                            "Entity type",
                            "entity_type_id",
                            type_opts,
                            Some(fields.selected_type),
                        )
                        .required(),
                        Field::select(
                            "Jurisdiction",
                            "jurisdiction_id",
                            jur_opts,
                            Some(fields.selected_jurisdiction),
                        )
                        .required(),
                    ];
                    rsx! {
                        document::Title { "{view.firm_name} | Lawyer | Entities | Edit entity" }
                        FormCard {
                            title: "Edit entity".to_string(),
                            action: "/app/admin/entities/{view.id}",
                            submit_label: "Save changes".to_string(),
                            csrf_token: Some(view.csrf_token.clone()),
                            fields: form_fields,
                        }
                        p { a { href: "/app/admin/entities", "← Cancel" } }
                    }
                }
                None => rsx! {
                    document::Title { "{view.firm_name} | Lawyer | Entities | Not found" }
                    h1 { "Entity not found" }
                    p { "No entity exists with id " code { "{view.id}" } "." }
                    p { a { href: "/app/admin/entities", "← Back to entities" } }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::form::assert_forms_accessible;

    const ID: &str = "00000000-0000-0000-0000-000000000002";

    fn view(fields: Option<EntityFields>) -> EntityEditView {
        EntityEditView {
            firm_name: "Neon Law".to_string(),
            id: ID.to_string(),
            fields,
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
        }
    }

    #[test]
    fn the_edit_form_meets_the_page_accessibility_invariants() {
        // Carried over from the retired `views/tests/accessibility.rs`
        // `entity_forms_are_accessible`: no positive tabindex, every
        // `<label for>` and `aria-describedby` resolves, and the form is named.
        let html = dioxus_ssr::render_element(entity_edit_body(&view(Some(EntityFields {
            name: "Acme".to_string(),
            selected_type: "00000000-0000-0000-0000-000000000001".to_string(),
            selected_jurisdiction: "00000000-0000-0000-0000-000000000001".to_string(),
        }))));
        assert_forms_accessible(&html, "entity_edit::LawyerEntityEdit");
        assert!(
            html.contains(&format!("action=\"/app/admin/entities/{ID}\"")),
            "{html}"
        );
        assert!(html.contains("value=\"Acme\""), "{html}");
    }

    #[test]
    fn a_refused_update_shows_its_message_over_the_rejected_values() {
        // `POST /app/admin/entities/{id}` redirects a refused update back here with
        // its message and the submitted values in the query, which the loader
        // overlays onto the stored row. The form must therefore show what was
        // typed, not what is stored — otherwise the correction is retyped.
        let html = dioxus_ssr::render_element(entity_edit_body(&EntityEditView {
            fields: Some(EntityFields {
                name: "Neon Law".to_string(),
                selected_type: "00000000-0000-0000-0000-000000000001".to_string(),
                selected_jurisdiction: "00000000-0000-0000-0000-000000000001".to_string(),
            }),
            error: Some("That name is reserved for the firm.".to_string()),
            ..view(None)
        }));
        assert!(html.contains("nav-form-error"), "{html}");
        assert!(
            html.contains("That name is reserved for the firm."),
            "{html}"
        );
        assert!(html.contains("value=\"Neon Law\""), "{html}");
        assert!(
            html.contains(&format!("action=\"/app/admin/entities/{ID}\"")),
            "{html}"
        );
    }

    #[test]
    fn an_unresolvable_id_offers_no_form_to_submit() {
        let html = dioxus_ssr::render_element(entity_edit_body(&view(None)));
        assert!(html.contains("Entity not found"), "{html}");
        assert!(!html.contains("<form"), "{html}");
    }
}
