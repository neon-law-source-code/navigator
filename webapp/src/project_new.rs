//! The lawyer "add project" (matter-open) form as a Dioxus component (#956
//! Phase 4) — `/app/projects/new`.
//!
//! The successor to the `views::pages::admin::projects::new_form` and its
//! two HTMX inline-create modals. Three native forms on one page, no JavaScript:
//!
//! - the **matter-open** form, posting to the unchanged `POST /app/projects`;
//! - **New entity**, posting to `POST /app/projects/new/entity`; and
//! - **New client**, posting to `POST /app/projects/new/client`.
//!
//! The two inline creates were a Bootstrap modal swapped over HTMX, with an
//! out-of-band `<select>` swap writing the new record into the matter form's
//! picker. They are now post/redirect/get: each handler creates the record and
//! redirects back here with `?entity=` / `?client=` naming it, and this loader
//! preselects it in the matching picker. A refusal redirects back with
//! `?entity_error=` / `?client_error=` and the submitted values echoed, so the
//! disclosure re-opens showing the error over what was typed.
//!
//! The matter-open form's own refusals travel the same way: `POST
//! /app/projects` redirects back with `?error=` plus every submitted field, so
//! a refused open (a missing attestation, a blocking conflict, a code clash)
//! re-renders with nothing retyped. The conflict attestation is the one field
//! never re-checked — the opening attorney re-attests on the corrected submit.
//!
//! # Authorization
//!
//! Lawyer tier (at this firm `lawyer` is an attorney), hidden rather than refused.
//! The reads run the same `store` calls the handler made; there is no `/api`
//! read cluster for the entity/client pickers yet, and when one lands
//! (#866) this loader moves onto it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard, Heading, PersonChoice};
use crate::people::ViewerRole;
use crate::project_edit::{app_navbar, entity_options, DESCRIPTION_HELP, ENTITY_HELP};

/// Everything the create page reads off the query string: the three forms'
/// error flashes, the ids of a just-created entity/client, and the echoed
/// values a refused submit carries back so nothing is retyped.
#[derive(Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct ProjectNewQuery {
    // The matter-open form.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub client_dri_person_id: Option<String>,
    /// A non-authoritative name/email filter for the client directory.
    #[serde(default)]
    pub client_dri_person_id_search: Option<String>,
    #[serde(default)]
    pub scope_of_services: Option<String>,
    /// The entity the inline create just made — preselected in the picker.
    #[serde(default)]
    pub entity: Option<String>,
    /// The client the inline create just made — preselected in the DRI picker.
    #[serde(default)]
    pub client: Option<String>,
    // The "New entity" disclosure.
    #[serde(default)]
    pub entity_error: Option<String>,
    #[serde(default)]
    pub entity_name: Option<String>,
    #[serde(default)]
    pub entity_type_id: Option<String>,
    #[serde(default)]
    pub jurisdiction_id: Option<String>,
    // The "New client" disclosure.
    #[serde(default)]
    pub client_error: Option<String>,
    #[serde(default)]
    pub client_name: Option<String>,
    #[serde(default)]
    pub client_email: Option<String>,
}

/// One `<option>` with an id value and a display label — the shape every picker
/// on this page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct IdChoice {
    pub id: String,
    pub label: String,
}

/// The rendered "add project" page: the three forms' options and echoed values.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectNewView {
    /// `false` for a non-lawyer caller — the page renders not-found under a
    /// committed `404`, the way the handler hid the route.
    pub found: bool,
    pub entities: Vec<crate::project_edit::EntityOption>,
    /// Existing `client`-role persons: a matter's client-side DRI is a real,
    /// pre-existing client of record, never a firm attorney.
    pub clients: Vec<PersonChoice>,
    pub entity_types: Vec<IdChoice>,
    pub jurisdictions: Vec<IdChoice>,
    pub csrf_token: String,
    pub query: ProjectNewQuery,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Load the matter-open form: the entity and client pickers, plus the
/// entity-type and jurisdiction pickers the inline "New entity" form needs.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_project_new_form() -> Result<ProjectNewView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    if !role.is_lawyer_tier() {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(ProjectNewView {
            found: false,
            role,
            logo: crate::app_chrome::app_logo_from_context().await,
            ..ProjectNewView::default()
        });
    }
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let query = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ProjectNewQuery>,
        _,
    >()
    .await
    .map(|axum::extract::Query(q)| q)
    .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let entities = store::entities::all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|e| crate::project_edit::EntityOption {
            id: e.id.to_string(),
            name: e.name,
        })
        .collect();
    // The client roster is filtered in Rust: `persons` is in the other
    // engine and the directory read has no role predicate.
    let clients = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .filter(|p| p.role == store::persons::Role::Client)
        .map(|p| PersonChoice::new(p.id.to_string(), p.name, p.email))
        .collect();
    // The entity-type reference table lives in SurrealDB (ENG-20);
    // `list` keeps the name ordering this picker always had.
    let entity_types = store::entity_types::list(&surreal, &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|t| IdChoice {
            id: t.id.to_string(),
            label: t.name,
        })
        .collect();
    // The jurisdiction reference table lives in SurrealDB (ENG-20);
    // `list_all` keeps the name ordering this picker always had.
    let jurisdictions = store::jurisdictions::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|j| IdChoice {
            label: format!("{} ({})", j.name, j.code),
            id: j.id.to_string(),
        })
        .collect();

    Ok(ProjectNewView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: true,
        entities,
        clients,
        entity_types,
        jurisdictions,
        csrf_token,
        query,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
    })
}

/// Prefer a non-empty echoed value, else the empty string.
fn echoed(value: Option<&String>) -> String {
    value.cloned().unwrap_or_default()
}

/// Build a picker's options from `choices`, leading with `blank` (the "not
/// chosen" option every required picker on this page renders).
fn id_options(blank: &str, choices: &[IdChoice]) -> Vec<Choice> {
    let mut options = vec![Choice::new("", blank)];
    options.extend(
        choices
            .iter()
            .map(|c| Choice::new(c.id.clone(), c.label.clone())),
    );
    options
}

/// The matter-open form itself.
fn open_matter_form(view: &ProjectNewView) -> Element {
    let q = &view.query;
    // A freshly created record wins over an echoed selection: the operator just
    // made it for this matter.
    let selected_entity = q.entity.clone().or_else(|| q.entity_id.clone());
    let selected_client = q.client.clone().or_else(|| q.client_dri_person_id.clone());
    let fields = vec![
        Field::text("Name", "name", echoed(q.name.as_ref())).required(),
        Field::text("Project code", "code", echoed(q.code.as_ref()))
            .required()
            .placeholder("fractional-gc")
            .help(
                "Lowercase letters, digits, and single hyphens, starting and ending with a letter \
                 or digit — no uppercase, no underscores, no spaces. The matter's page, the client's portal, \
                 and the lawyer-only repository name all become that exact word, chosen once and never \
                 changed. No edit form changes it later.",
            ),
        Field::select(
            "Entity",
            "entity_id",
            entity_options(&view.entities),
            selected_entity,
        )
        .required()
        .help(ENTITY_HELP),
        Field::textarea(
            "Description",
            "description",
            echoed(q.description.as_ref()),
            3,
        )
        .help(DESCRIPTION_HELP),
        Field::person_picker(
            "Client",
            "client_dri_person_id",
            "— pick the client —",
            view.clients.clone(),
            selected_client,
        )
        .person_search(view.query.client_dri_person_id_search.clone())
        .required()
        .help(
            "The client this matter is for — its client-side Directly Responsible Individual. \
             Create the client person first if they aren't listed.",
        ),
        Field::textarea(
            "Scope of services",
            "scope_of_services",
            echoed(q.scope_of_services.as_ref()),
            3,
        )
        .help("Describes the work this retainer covers; rendered into the agreement."),
        // Never re-checked on a re-render: the opening attorney re-attests on
        // the corrected submission, so a refused open cannot leave the
        // attestation silently ticked.
        Field::checkbox(
            "I attest that I have checked for conflicts of interest and that none prevent opening \
             this matter",
            "attestation",
            "1",
            false,
        )
        .required(),
    ];

    rsx! {
        if let Some(error) = q.error.as_ref() {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        FormCard {
            title: "Add project".to_string(),
            action: "/app/projects".to_string(),
            submit_label: "Create".to_string(),
            heading: Heading::H2,
            csrf_token: Some(view.csrf_token.clone()),
            fields,
        }
    }
}

/// The inline "New entity" disclosure — a native `POST` that creates the entity
/// and redirects back with it preselected in the matter form's Entity picker.
fn new_entity_form(view: &ProjectNewView) -> Element {
    let q = &view.query;
    let fields = vec![
        Field::text("Name", "entity_name", echoed(q.entity_name.as_ref())).required(),
        Field::select(
            "Entity type",
            "entity_type_id",
            id_options("Choose…", &view.entity_types),
            q.entity_type_id.clone().filter(|v| !v.is_empty()),
        )
        .required(),
        Field::select(
            "Jurisdiction",
            "jurisdiction_id",
            id_options("Choose…", &view.jurisdictions),
            q.jurisdiction_id.clone().filter(|v| !v.is_empty()),
        )
        .required(),
    ];
    rsx! {
        details { class: "inline-create", open: q.entity_error.is_some(),
            summary { class: "inline-create__summary", "New entity" }
            if let Some(error) = q.entity_error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            FormCard {
                title: "Add entity".to_string(),
                action: "/app/projects/new/entity".to_string(),
                submit_label: "Create entity".to_string(),
                heading: Heading::H2,
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
        }
    }
}

/// The inline "New client" disclosure. The role is pinned to `client` by the
/// handler — this form only ever mints the matter's client-side DRI, so there is
/// no role picker to get wrong.
fn new_client_form(view: &ProjectNewView) -> Element {
    let q = &view.query;
    let fields = vec![
        Field::text("Name", "client_name", echoed(q.client_name.as_ref())).required(),
        Field::email("Email", "client_email", echoed(q.client_email.as_ref())).required(),
    ];
    rsx! {
        details { class: "inline-create", open: q.client_error.is_some(),
            summary { class: "inline-create__summary", "New client" }
            if let Some(error) = q.client_error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            FormCard {
                title: "Add client".to_string(),
                action: "/app/projects/new/client".to_string(),
                submit_label: "Create client".to_string(),
                heading: Heading::H2,
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
        }
    }
}

/// The loaded create page: the matter-open form, then the two inline creates.
fn new_body(view: &ProjectNewView) -> Element {
    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Projects | Add project" }
        // First-party, same-origin, so `script-src 'self'` allows it with no
        // nonce. Live-validates the `#code` field's shape as the lawyer types
        // — the code is immutable and never re-typed, so catching a shape
        // mistake before submit beats learning about it from a redirect.
        document::Script { src: "/public/js/project-code-live-validate.js", defer: true }
        header { class: "page-header",
            h1 { "Add project" }
            p { a { href: "/app/projects", "← Back to projects" } }
        }
        {open_matter_form(view)}
        p { class: "project-form-cancel",
            a { class: "nav-btn nav-btn--secondary", href: "/app/projects", "Cancel" }
        }
        section { class: "inline-create-group", "aria-label": "Create a missing record",
            p { class: "muted",
                "Missing the entity or the client? Create either here without leaving the form."
            }
            {new_entity_form(view)}
            {new_client_form(view)}
        }
    }
}

/// `/app/projects/new` — open a matter.
#[component]
pub fn LawyerProjectNew() -> Element {
    let resource = use_server_future(get_project_new_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "project-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "project-new", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        {app_navbar(view.role, view.logo.clone())}
        main { id: "project-new", class: "nav-theme",
            if view.found {
                {new_body(&view)}
            } else {
                document::Title { "{view.firm_name} | Lawyer | Not found" }
                h1 { "Not found" }
                p { "No matter-open form is available at this address." }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{new_body, IdChoice, ProjectNewQuery, ProjectNewView};
    use crate::components::PersonChoice;
    use crate::people::ViewerRole;
    use crate::project_edit::EntityOption;

    const ENTITY_ID: &str = "00000000-0000-0000-0000-000000000001";
    const CLIENT_ID: &str = "00000000-0000-0000-0000-000000000002";
    const TYPE_ID: &str = "00000000-0000-0000-0000-000000000003";
    const JUR_ID: &str = "00000000-0000-0000-0000-000000000004";

    fn view(query: ProjectNewQuery) -> ProjectNewView {
        ProjectNewView {
            firm_name: "Neon Law".to_string(),
            found: true,
            entities: vec![EntityOption {
                id: ENTITY_ID.to_string(),
                name: "Acme".to_string(),
            }],
            clients: vec![PersonChoice::new(
                CLIENT_ID,
                "Libra Client",
                "libra@example.com",
            )],
            entity_types: vec![IdChoice {
                id: TYPE_ID.to_string(),
                label: "LLC".to_string(),
            }],
            jurisdictions: vec![IdChoice {
                id: JUR_ID.to_string(),
                label: "Nevada (NV)".to_string(),
            }],
            csrf_token: "CSRF-TOKEN".to_string(),
            query,
            role: ViewerRole::Lawyer,
            logo: None,
        }
    }

    fn render(view: &ProjectNewView) -> String {
        dioxus_ssr::render_element(new_body(view))
    }

    #[test]
    fn the_open_form_targets_the_matter_route_with_every_required_field() {
        let html = render(&view(ProjectNewQuery::default()));
        assert!(html.contains(r#"action="/app/projects""#), "{html}");
        assert!(
            html.contains(r#"name="_csrf" value="CSRF-TOKEN""#),
            "{html}"
        );
        assert!(html.contains(r#"name="code""#), "{html}");
        assert!(html.contains(r#"name="entity_id""#), "{html}");
        assert!(html.contains(r#"name="client_dri_person_id""#), "{html}");
        assert!(html.contains(r#"name="scope_of_services""#), "{html}");
        // Every open is attested, and no status control is offered (a matter
        // always opens `open`; navigator#770).
        assert!(html.contains(r#"name="attestation""#), "{html}");
        assert!(!html.contains(r#"name="status""#), "{html}");
    }

    #[test]
    fn the_client_picker_displays_email_and_filters_by_name_or_email() {
        let html = render(&view(ProjectNewQuery::default()));
        assert!(
            html.contains(r#"name="client_dri_person_id_search""#),
            "{html}"
        );
        assert!(html.contains("Filter people"), "{html}");
        assert!(
            html.contains("Libra Client &#60;libra@example.com&#62;"),
            "{html}"
        );
    }

    /// The one field a lawyer cannot take back tells them what it is for.
    ///
    /// The code is required at matter-open, immutable, and no edit form
    /// changes it later — so the rule and the consequence both belong on the
    /// field itself: a lawyer who learns the shape from a rejected submission
    /// has already had to guess, and one who learns the code is immutable
    /// later has already given a client a link.
    #[test]
    fn the_code_field_states_its_shape_and_that_the_code_is_immutable() {
        let html = render(&view(ProjectNewQuery::default()));

        for shape in ["Lowercase letters", "single hyphens", "no underscores"] {
            assert!(html.contains(shape), "the code's shape is unstated: {html}");
        }
        assert!(
            html.contains(
                "matter&#39;s page, the client&#39;s portal, and the lawyer-only repository name",
            ),
            "the field does not explain what the code names: {html}"
        );
        assert!(
            html.contains("chosen once and never changed"),
            "the field does not say the resulting code is immutable: {html}"
        );
        assert!(!html.contains("generated suffix"), "{html}");
        assert!(!html.contains("a1b2c3d4"), "{html}");
    }

    #[test]
    fn both_inline_creates_are_native_posts_with_no_htmx_and_no_bootstrap_modal() {
        let html = render(&view(ProjectNewQuery::default()));
        assert!(
            html.contains(r#"action="/app/projects/new/entity""#),
            "{html}"
        );
        assert!(
            html.contains(r#"action="/app/projects/new/client""#),
            "{html}"
        );
        assert!(html.contains(r#"name="entity_name""#), "{html}");
        assert!(html.contains(r#"name="client_email""#), "{html}");
        // The HTMX + Bootstrap modal machinery is gone.
        assert!(!html.contains("hx-"), "{html}");
        assert!(!html.contains("data-bs-"), "{html}");
        assert!(!html.contains("inline-create-modal"), "{html}");
        // Closed by default — the disclosure replaces the modal's hidden state.
        assert!(
            !html.contains("<details class=\"inline-create\" open"),
            "{html}"
        );
    }

    #[test]
    fn a_just_created_entity_and_client_come_back_preselected() {
        // The out-of-band `<select>` swap becomes a redirect naming the new
        // record; the picker must render it selected either way.
        let html = render(&view(ProjectNewQuery {
            entity: Some(ENTITY_ID.to_string()),
            client: Some(CLIENT_ID.to_string()),
            ..ProjectNewQuery::default()
        }));
        assert!(
            html.contains(&format!(r#"<option value="{ENTITY_ID}" selected"#)),
            "{html}"
        );
        assert!(
            html.contains(&format!(r#"<option value="{CLIENT_ID}" selected"#)),
            "{html}"
        );
    }

    #[test]
    fn a_refused_open_echoes_every_field_but_never_the_attestation() {
        let html = render(&view(ProjectNewQuery {
            error: Some("Attest that you have checked for and cleared conflicts.".to_string()),
            name: Some("Unattested matter".to_string()),
            code: Some("matter-open-2".to_string()),
            entity_id: Some(ENTITY_ID.to_string()),
            description: Some("Scope narrative.".to_string()),
            client_dri_person_id: Some(CLIENT_ID.to_string()),
            scope_of_services: Some("Some work".to_string()),
            ..ProjectNewQuery::default()
        }));
        assert!(html.contains("Attest that you have checked"), "{html}");
        assert!(html.contains(r#"value="Unattested matter""#), "{html}");
        assert!(html.contains(r#"value="matter-open-2""#), "{html}");
        assert!(html.contains("Scope narrative."), "{html}");
        assert!(html.contains("Some work"), "{html}");
        assert!(
            html.contains(&format!(r#"<option value="{CLIENT_ID}" selected"#)),
            "{html}"
        );
        // The attorney re-attests on the corrected submission, so the checkbox
        // never comes back ticked. Match the tag itself — the label prose says
        // "I have checked for conflicts", which a whole-page search would hit.
        let attestation = html
            .split("<input")
            .find(|frag| frag.contains(r#"name="attestation""#))
            .and_then(|frag| frag.split_once('>'))
            .map(|(tag, _)| tag.to_string())
            .expect("the attestation checkbox renders");
        assert!(!attestation.contains("checked"), "{attestation}");
    }

    #[test]
    fn a_refused_inline_create_reopens_its_disclosure_over_the_typed_values() {
        let html = render(&view(ProjectNewQuery {
            entity_error: Some("Pick a jurisdiction.".to_string()),
            entity_name: Some("Acme Holdings".to_string()),
            entity_type_id: Some(TYPE_ID.to_string()),
            ..ProjectNewQuery::default()
        }));
        assert!(html.contains(">Pick a jurisdiction.<"), "{html}");
        assert!(html.contains(r#"value="Acme Holdings""#), "{html}");
        assert!(
            html.contains(&format!(r#"<option value="{TYPE_ID}" selected"#)),
            "{html}"
        );
        assert!(
            html.contains("<details class=\"inline-create\" open"),
            "{html}"
        );
    }

    /// Every engagement is bespoke: the matter-open form opens a matter
    /// directly, carrying neither a service code nor a price.
    #[test]
    fn the_open_matter_form_asks_for_no_service_and_shows_no_price() {
        let html = render(&view(ProjectNewQuery::default()));
        assert!(!html.contains(r#"name="product_code""#), "{html}");
        assert!(!html.contains("pick the service"), "{html}");
        assert!(!html.contains('$'), "{html}");
    }

    #[test]
    fn every_form_on_the_page_meets_the_layer_one_a11y_invariants() {
        // The create form was covered by `views/tests/accessibility.rs`;
        // this is that gate, applied to all three Dioxus forms at once.
        let html = render(&view(ProjectNewQuery {
            error: Some("Pick an entity.".to_string()),
            entity_error: Some("Name is required.".to_string()),
            client_error: Some("That email is already in use.".to_string()),
            ..ProjectNewQuery::default()
        }));
        crate::components::assert_forms_accessible(&html, "project_new");
    }

    #[test]
    fn keeps_the_admin_form_e2e_hook() {
        // `server/tests/accessibility_e2e.rs` scopes axe to the first
        // `form.admin-form` on `/app/projects/new`; dropping the class times
        // the nightly deploy gate out.
        let html = render(&view(ProjectNewQuery::default()));
        assert!(html.contains("admin-form"), "{html}");
    }
}
