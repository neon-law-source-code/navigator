//! The Owner-only "create a firm" form — `/app/owner/firms/new`.
//!
//! Owner opens a second (or subsequent) Firm and appoints its first Admin DRI
//! in the same submission — the same atomic guarantee `store::firms::create`
//! already gives (ENG-499: there is no setup state a Firm passes through
//! before it has an Admin DRI). Entity and Admin DRI are each chosen from an
//! existing row or minted inline without leaving the page, the same
//! post/redirect/get pattern `webapp::project_new` established: each inline
//! create posts to its own route and redirects back here naming the new row,
//! which this loader preselects. Status is `active` at creation and is not a
//! field here — [`crate::firm_edit`] is where it is later changed.
//!
//! # Authorization
//!
//! Owner only. [`crate::admin_listing::require_owner`] commits a real `403`
//! for anyone else — Rego's `owner_only_path` rule already denies every
//! non-Owner session at the route, so this is defense in depth, not the
//! primary gate.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard, Heading, PersonChoice};
use crate::people::ViewerRole;
use crate::project_edit::{entity_options, EntityOption, ENTITY_HELP};

/// Everything the create page reads off the query string: the main form's
/// `?error=` flash and echoed field, plus each inline disclosure's own error
/// flash and echoed fields, plus the "just created" ids a redirect names.
#[derive(Deserialize, Serialize, Clone, Default, PartialEq, Eq)]
pub struct FirmNewQuery {
    // The main "create firm" form.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub admin_dri_person_id: Option<String>,
    /// The entity the inline create just made — preselected in the picker.
    #[serde(default)]
    pub entity: Option<String>,
    /// The admin the inline create just made — preselected in the picker.
    #[serde(default)]
    pub admin: Option<String>,
    // The "New entity" disclosure.
    #[serde(default)]
    pub entity_error: Option<String>,
    #[serde(default)]
    pub entity_name: Option<String>,
    #[serde(default)]
    pub entity_type_id: Option<String>,
    #[serde(default)]
    pub jurisdiction_id: Option<String>,
    // The "New admin" disclosure.
    #[serde(default)]
    pub admin_error: Option<String>,
    #[serde(default)]
    pub admin_name: Option<String>,
    #[serde(default)]
    pub admin_email: Option<String>,
}

/// One `<option>` with an id value and a display label.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct IdChoice {
    pub id: String,
    pub label: String,
}

/// The rendered "create firm" page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmNewView {
    /// `false` for a non-Owner caller — the page renders not-found under a
    /// committed `403` (`require_owner` already committed the status).
    pub found: bool,
    pub entities: Vec<EntityOption>,
    /// Existing `admin`-role persons — the only tier eligible to become a
    /// Firm's Admin DRI (`store::firms::FirmError::IneligibleAdminDriTier`).
    pub admins: Vec<PersonChoice>,
    pub entity_types: Vec<IdChoice>,
    pub jurisdictions: Vec<IdChoice>,
    pub csrf_token: String,
    pub query: FirmNewQuery,
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
}

/// Load the create-firm form: the entity and admin pickers, plus the
/// entity-type and jurisdiction pickers the inline "New entity" form needs.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_firm_new_form() -> Result<FirmNewView, ServerFnError> {
    let role = crate::admin_listing::require_owner().await?;

    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let query =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Query<FirmNewQuery>, _>()
            .await
            .map(|axum::extract::Query(q)| q)
            .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let entities = store::entities::all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|e| EntityOption {
            id: e.id.to_string(),
            name: e.name,
        })
        .collect();
    // The admin roster is filtered in Rust: `persons` is in the other
    // engine and the directory read has no role predicate.
    let admins = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .filter(|p| p.role == store::persons::Role::Admin)
        .map(|p| PersonChoice::new(p.id.to_string(), p.name, p.email))
        .collect();
    let entity_types = store::entity_types::list(&surreal, &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|t| IdChoice {
            id: t.id.to_string(),
            label: t.name,
        })
        .collect();
    let jurisdictions = store::jurisdictions::list_all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|j| IdChoice {
            label: format!("{} ({})", j.name, j.code),
            id: j.id.to_string(),
        })
        .collect();

    Ok(FirmNewView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: true,
        entities,
        admins,
        entity_types,
        jurisdictions,
        csrf_token,
        query,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
    })
}

/// Prefer a non-empty echoed value, else the empty string.
fn echoed(value: Option<&String>) -> String {
    value.cloned().unwrap_or_default()
}

/// Build a picker's options from `choices`, leading with `blank`.
fn id_options(blank: &str, choices: &[IdChoice]) -> Vec<Choice> {
    let mut options = vec![Choice::new("", blank)];
    options.extend(
        choices
            .iter()
            .map(|c| Choice::new(c.id.clone(), c.label.clone())),
    );
    options
}

/// The main "create firm" form. Status is not offered here: a new Firm
/// always opens `active` (`store::firms::create`), and its edit page is
/// where that later changes.
fn firm_new_form(view: &FirmNewView) -> Element {
    let q = &view.query;
    // A freshly created record wins over an echoed selection: the operator
    // just made it for this Firm.
    let selected_entity = q.entity.clone().or_else(|| q.entity_id.clone());
    let selected_admin = q.admin.clone().or_else(|| q.admin_dri_person_id.clone());
    let fields = vec![
        Field::text("Name", "name", echoed(q.name.as_ref())).required(),
        Field::select(
            "Entity",
            "entity_id",
            entity_options(&view.entities),
            selected_entity,
        )
        .required()
        .help(ENTITY_HELP),
        Field::person_picker(
            "Admin DRI",
            "admin_dri_person_id",
            "— pick the Admin DRI —",
            view.admins.clone(),
            selected_admin,
        )
        .required()
        .help(
            "The Firm's first Admin — who administers its settings, membership, and brands. \
             Must already carry the admin role. Create the admin person first if they aren't listed.",
        ),
    ];

    rsx! {
        if let Some(error) = q.error.as_ref() {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        FormCard {
            title: "Create firm".to_string(),
            action: "/app/owner/firms".to_string(),
            submit_label: "Create".to_string(),
            heading: Heading::H2,
            csrf_token: Some(view.csrf_token.clone()),
            fields,
        }
    }
}

/// The inline "New entity" disclosure — a native `POST` that creates the
/// entity and redirects back with it preselected.
fn new_entity_form(view: &FirmNewView) -> Element {
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
                action: "/app/owner/firms/new/entity".to_string(),
                submit_label: "Create entity".to_string(),
                heading: Heading::H2,
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
        }
    }
}

/// The inline "New admin" disclosure. The role is pinned to `admin` by the
/// handler — this form only ever mints a person eligible to become the
/// Firm's Admin DRI.
fn new_admin_form(view: &FirmNewView) -> Element {
    let q = &view.query;
    let fields = vec![
        Field::text("Name", "admin_name", echoed(q.admin_name.as_ref())).required(),
        Field::email("Email", "admin_email", echoed(q.admin_email.as_ref())).required(),
    ];
    rsx! {
        details { class: "inline-create", open: q.admin_error.is_some(),
            summary { class: "inline-create__summary", "New admin" }
            if let Some(error) = q.admin_error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            FormCard {
                title: "Add admin".to_string(),
                action: "/app/owner/firms/new/admin".to_string(),
                submit_label: "Create admin".to_string(),
                heading: Heading::H2,
                csrf_token: Some(view.csrf_token.clone()),
                fields,
            }
        }
    }
}

/// The loaded create page: the main form, then the two inline creates.
fn new_body(view: &FirmNewView) -> Element {
    rsx! {
        document::Title { "{view.firm_name} | Owner | Create firm" }
        header { class: "page-header",
            h1 { "Create firm" }
            p { a { href: "/app/owner", "← Back to firms" } }
        }
        {firm_new_form(view)}
        p { class: "project-form-cancel",
            a { class: "nav-btn nav-btn--secondary", href: "/app/owner", "Cancel" }
        }
        section { class: "inline-create-group", "aria-label": "Create a missing record",
            p { class: "muted",
                "Missing the entity or the admin? Create either here without leaving the form."
            }
            {new_entity_form(view)}
            {new_admin_form(view)}
        }
    }
}

/// `/app/owner/firms/new` — Owner opens a Firm.
#[component]
pub fn OwnerFirmNew() -> Element {
    let resource = use_server_future(get_firm_new_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "firm-new", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "firm-new", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "firm-new", class: "nav-theme",
            if view.found {
                {new_body(&view)}
            } else {
                document::Title { "{view.firm_name} | Not found" }
                h1 { "Not found" }
                p { "No create-firm form is available at this address." }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{new_body, FirmNewQuery, FirmNewView, IdChoice};
    use crate::components::PersonChoice;
    use crate::people::ViewerRole;
    use crate::project_edit::EntityOption;

    const ENTITY_ID: &str = "00000000-0000-0000-0000-000000000001";
    const ADMIN_ID: &str = "00000000-0000-0000-0000-000000000002";
    const TYPE_ID: &str = "00000000-0000-0000-0000-000000000003";
    const JUR_ID: &str = "00000000-0000-0000-0000-000000000004";

    fn view(query: FirmNewQuery) -> FirmNewView {
        FirmNewView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            found: true,
            entities: vec![EntityOption {
                id: ENTITY_ID.to_string(),
                name: "Acme".to_string(),
            }],
            admins: vec![PersonChoice::new(ADMIN_ID, "Ada Admin", "ada@example.com")],
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
            role: ViewerRole::Owner,
            logo: None,
        }
    }

    fn render(view: &FirmNewView) -> String {
        dioxus_ssr::render_element(new_body(view))
    }

    #[test]
    fn the_create_form_targets_the_firm_collection_with_every_required_field() {
        let html = render(&view(FirmNewQuery::default()));
        assert!(html.contains(r#"action="/app/owner/firms""#), "{html}");
        assert!(
            html.contains(r#"name="_csrf" value="CSRF-TOKEN""#),
            "{html}"
        );
        assert!(html.contains(r#"name="name""#), "{html}");
        assert!(html.contains(r#"name="entity_id""#), "{html}");
        assert!(html.contains(r#"name="admin_dri_person_id""#), "{html}");
        // No status control at create — a new Firm always opens active.
        assert!(!html.contains(r#"name="status""#), "{html}");
    }

    #[test]
    fn both_inline_creates_are_native_posts() {
        let html = render(&view(FirmNewQuery::default()));
        assert!(
            html.contains(r#"action="/app/owner/firms/new/entity""#),
            "{html}"
        );
        assert!(
            html.contains(r#"action="/app/owner/firms/new/admin""#),
            "{html}"
        );
        assert!(html.contains(r#"name="entity_name""#), "{html}");
        assert!(html.contains(r#"name="admin_email""#), "{html}");
        assert!(
            !html.contains("<details class=\"inline-create\" open"),
            "{html}"
        );
    }

    #[test]
    fn a_just_created_entity_and_admin_come_back_preselected() {
        let html = render(&view(FirmNewQuery {
            entity: Some(ENTITY_ID.to_string()),
            admin: Some(ADMIN_ID.to_string()),
            ..FirmNewQuery::default()
        }));
        assert!(
            html.contains(&format!(r#"<option value="{ENTITY_ID}" selected"#)),
            "{html}"
        );
        assert!(
            html.contains(&format!(r#"<option value="{ADMIN_ID}" selected"#)),
            "{html}"
        );
    }

    /// A refused create names the rule (e.g. an ineligible Admin DRI tier)
    /// and echoes the typed fields, so nothing is retyped.
    #[test]
    fn a_refused_create_names_the_rule_and_echoes_the_typed_fields() {
        let html = render(&view(FirmNewQuery {
            error: Some("The Admin DRI must be a person whose role is admin.".to_string()),
            name: Some("Acme Legal".to_string()),
            entity_id: Some(ENTITY_ID.to_string()),
            admin_dri_person_id: Some(ADMIN_ID.to_string()),
            ..FirmNewQuery::default()
        }));
        assert!(
            html.contains("The Admin DRI must be a person whose role is admin."),
            "{html}"
        );
        assert!(html.contains(r#"value="Acme Legal""#), "{html}");
        assert!(
            html.contains(&format!(r#"<option value="{ENTITY_ID}" selected"#)),
            "{html}"
        );
    }

    #[test]
    fn a_refused_inline_entity_create_reopens_its_disclosure_over_the_typed_values() {
        let html = render(&view(FirmNewQuery {
            entity_error: Some("Pick a jurisdiction.".to_string()),
            entity_name: Some("Acme Holdings".to_string()),
            entity_type_id: Some(TYPE_ID.to_string()),
            ..FirmNewQuery::default()
        }));
        assert!(html.contains(">Pick a jurisdiction.<"), "{html}");
        assert!(html.contains(r#"value="Acme Holdings""#), "{html}");
        assert!(
            html.contains("<details class=\"inline-create\" open"),
            "{html}"
        );
    }

    #[test]
    fn every_form_on_the_page_meets_the_layer_one_a11y_invariants() {
        let html = render(&view(FirmNewQuery {
            error: Some("Pick an entity.".to_string()),
            entity_error: Some("Name is required.".to_string()),
            admin_error: Some("That email is already in use.".to_string()),
            ..FirmNewQuery::default()
        }));
        crate::components::assert_forms_accessible(&html, "firm_new");
    }

    #[test]
    fn keeps_the_admin_form_e2e_hook() {
        let html = render(&view(FirmNewQuery::default()));
        assert!(html.contains("admin-form"), "{html}");
    }
}
