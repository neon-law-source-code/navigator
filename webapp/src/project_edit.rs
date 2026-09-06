//! The lawyer "edit project" form as a Dioxus component (#956 Phase 4) —
//! `/app/projects/{project_code}/edit`.
//!
//! The successor to the `views::pages::admin::projects::edit_form`. A
//! descriptive edit only: name, the entity the matter is opened against, and the
//! scope narrative. It renders no status control — a matter's lifecycle
//! transition (open/closed/archived) and its coupled retention date are their own
//! commands (navigator#770), so a picker here would silently discard the edit.
//! Neither does it re-choose the matter's parties: the client-side DRI and the
//! service are set at open.
//!
//! It is a native `POST` to the unchanged `POST /app/projects/{project_code}` handler
//! through the shared [`FormCard`], carrying the session CSRF token — no
//! JavaScript.
//!
//! # Authorization
//!
//! Lawyer tier, and hidden rather than refused (the handler's `404`). The
//! reads run the same `store` / `SeaORM` calls that handler made; there is no
//! `/api` read cluster for a single matter yet, and when one lands (#866) this
//! loader moves onto it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard, Heading};
use crate::people::ViewerRole;

/// The form's `?error=` flash, set by the update handler's
/// redirect-on-failure.
#[derive(Deserialize, Default)]
pub struct ProjectEditQuery {
    #[serde(default)]
    pub error: Option<String>,
}

/// One `<option>` in the Entity picker.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct EntityOption {
    pub id: String,
    pub name: String,
}

/// The rendered "edit project" form.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectEditView {
    /// `false` for a non-lawyer caller or an unknown matter — the page renders
    /// not-found under a committed `404`.
    pub found: bool,
    /// The matter's route segment — what the save posts to, and what the
    /// "Edit project" page itself is mounted at. Never the row's UUID: the
    /// router matches `POST /app/projects/{project_code}` by code, so an
    /// action built from the id would 404 the save.
    pub project_code: String,
    pub name: String,
    pub description: String,
    pub entity_id: Option<String>,
    pub entities: Vec<EntityOption>,
    pub internal_slack_channel_url: String,
    pub external_slack_channel_url: String,
    /// The firm-only Notion page. Blank when unset, and a blank submission
    /// clears it — the same terms as every other resource link.
    #[serde(default)]
    pub private_notion_page_url: String,
    /// The client-shared Notion page, on the same terms.
    #[serde(default)]
    pub shared_notion_page_url: String,
    /// The Project's source repository as a whole URL, on any forge. Blank when
    /// the matter records none, which is a legitimate state.
    pub repository_url: String,
    pub csrf_token: String,
    pub error: Option<String>,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Commit a `404` and render the not-found body — the fail-closed exit. Async
/// because the not-found body carries the navbar, mark included.
#[cfg(feature = "server")]
async fn hidden(role: ViewerRole) -> ProjectEditView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    ProjectEditView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: false,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        ..ProjectEditView::default()
    }
}

/// Load the "edit project" form for the `{id}` in the request path.
#[server]
pub async fn get_project_edit_form() -> Result<ProjectEditView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    if !role.is_lawyer_tier() {
        return Ok(hidden(role).await);
    }
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let error = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ProjectEditQuery>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::extract::Query(q)| q.error);
    let Ok(axum::extract::Path(project_code)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>().await
    else {
        return Ok(hidden(role).await);
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    // The matter arrives as its code; the gate and the form below key on the
    // row id. A code naming no matter is hidden, exactly as a matter the caller
    // is not on is hidden.
    let Some(project_id) = store::projects::id_for_code(&surreal, &project_code).await else {
        return Ok(hidden(role).await);
    };

    // The edit form is part of the matter surface, so it carries the matter's
    // gate: a firm-side participation row, of every tier. Without this an
    // unassigned Owner is denied `/app/projects/{project_code}` and then reads the
    // matter's name, status, and entity straight off its edit form.
    let person_id = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::portal_project_list::PersonId>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::Extension(pid)| pid.0)
    .and_then(|raw| raw.parse::<uuid::Uuid>().ok());
    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };
    if !store::access::can_see_project(&surreal, person_id, store_role, project_id)
        .await
        .map_err(|e| ServerFnError::new(e.clone()))?
    {
        return Ok(hidden(role).await);
    }

    let Some(project) = store::projects::find_by_id(&surreal, project_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(hidden(role).await);
    };
    let entities = store::entities::all(&surreal)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|e| EntityOption {
            id: e.id.to_string(),
            name: e.name,
        })
        .collect();

    Ok(ProjectEditView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: true,
        project_code: project.code.clone(),
        name: project.name,
        description: project.description.unwrap_or_default(),
        entity_id: Some(project.entity_id.to_string()),
        entities,
        internal_slack_channel_url: project.internal_slack_channel_url.unwrap_or_default(),
        external_slack_channel_url: project.external_slack_channel_url.unwrap_or_default(),
        private_notion_page_url: project.private_notion_page_url.unwrap_or_default(),
        shared_notion_page_url: project.shared_notion_page_url.unwrap_or_default(),
        repository_url: project.repository_url.unwrap_or_default(),
        csrf_token,
        error,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
    })
}

/// The `/app` navbar every matter form carries, from the viewer's tier and the
/// deploy's brand mark. Shared with the matter-open form, which renders the same
/// chrome.
pub(crate) fn app_navbar(role: ViewerRole, logo: Option<crate::components::AppLogo>) -> Element {
    rsx! {
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo,
        }
    }
}

/// The entity picker's options, with the blank "—" lead the form rendered.
/// Shared with the create form, which offers the same list.
pub(crate) fn entity_options(entities: &[EntityOption]) -> Vec<Choice> {
    let mut options = vec![Choice::new("", "—")];
    options.extend(
        entities
            .iter()
            .map(|e| Choice::new(e.id.clone(), e.name.clone())),
    );
    options
}

/// The help line under the entity picker — the same copy on both project forms.
pub(crate) const ENTITY_HELP: &str =
    "The legal organization this matter is opened against — a person's own Human entity for a \
     solo client. Create the entity first if it isn't listed.";

/// The help line under the scope-narrative textarea.
pub(crate) const DESCRIPTION_HELP: &str =
    "This matter's scope narrative (\"the project's story\").";

/// The help line under the internal Slack channel field.
pub(crate) const INTERNAL_SLACK_HELP: &str =
    "Private firm resource. Navigator never offers this Slack channel to the client.";

/// The help line under the external Slack channel field.
pub(crate) const EXTERNAL_SLACK_HELP: &str =
    "Navigator controls whether it offers this separate Slack channel to the client. Manage Slack permissions in Slack too.";

/// The help line under the private Notion page field.
///
/// Names the sharing boundary explicitly. Navigator stores the address and
/// nothing more: who may open the page is Notion's own permission, which
/// Navigator neither reads nor enforces — so a page left on a workspace default
/// is readable by everyone in that workspace no matter what this label says.
pub(crate) const PRIVATE_NOTION_HELP: &str =
    "Private firm resource. Navigator never offers this Notion page to the client. Manage Notion permissions in Notion too.";

/// The help line under the shared Notion page field.
pub(crate) const SHARED_NOTION_HELP: &str =
    "Navigator controls whether it offers this separate Notion page to the client. Manage Notion permissions in Notion too.";

/// The help line under the source repository field.
///
/// Says "any host" plainly, because the field replaced a coordinate that was
/// composed from one configured forge: a reader who remembers that behavior
/// would otherwise assume the organization is fixed.
pub(crate) const REPOSITORY_URL_HELP: &str =
    "Optional — the full URL of this matter's repository, \
     holding its notation templates and client portal. Any host: GitHub, GitLab, or a self-hosted \
     remote.";

#[derive(Props, Clone, PartialEq)]
struct ProjectResourceCardProps {
    service: String,
    private_field_name: String,
    private_label: String,
    private_url: String,
    private_help: String,
    shared_field_name: String,
    shared_label: String,
    shared_url: String,
    shared_placeholder: String,
    shared_help: String,
}

/// One private resource and its separately-addressed, optionally client-shared
/// counterpart. The private input is always present; the shared input exists
/// only while its explicit Navigator sharing toggle is on.
#[component]
fn ProjectResourceCard(props: ProjectResourceCardProps) -> Element {
    let initially_shared = !props.shared_url.trim().is_empty();
    let mut sharing = use_signal(|| initially_shared);
    let mut shared_value = use_signal(|| props.shared_url.clone());
    let mut clear_pending = use_signal(|| false);

    let toggle_id = format!("share-{}", props.service.to_lowercase());
    let toggle_name = format!("share_{}", props.service.to_lowercase());
    let toggle_help_id = format!("{toggle_id}-help");
    let shared_help_id = format!("{}-help", props.shared_field_name);
    let confirmation_id = format!("{toggle_id}-confirmation");
    let service = props.service.clone();
    let private_field = Field::text(
        props.private_label.clone(),
        props.private_field_name.clone(),
        props.private_url.clone(),
    )
    .placeholder(props.shared_placeholder.clone())
    .help(props.private_help.clone());
    let shared_field_name = props.shared_field_name.clone();
    let shared_placeholder = props.shared_placeholder.clone();
    let shared_help = props.shared_help.clone();
    let toggle_help =
        format!("When on, Navigator offers this separate {service} resource to the client.");

    rsx! {
        section {
            class: "nav-card project-resource-card",
            "data-resource-card": props.service.to_lowercase(),
            "aria-labelledby": "{toggle_id}-title",
            div { class: "nav-card__body",
                h2 { class: "project-resource-card__title", id: "{toggle_id}-title", "{service}" }
                {private_field.render()}
                div { class: "project-resource-card__sharing",
                    input {
                        class: "nav-checkbox",
                        r#type: "checkbox",
                        id: "{toggle_id}",
                        name: "{toggle_name}",
                        value: "1",
                        checked: sharing(),
                        "aria-controls": sharing().then(|| props.shared_field_name.clone()),
                        "aria-expanded": sharing().to_string(),
                        "aria-describedby": toggle_help_id.clone(),
                        onclick: move |_| {
                            if sharing() {
                                if shared_value().trim().is_empty() {
                                    sharing.set(false);
                                } else {
                                    clear_pending.set(true);
                                }
                            } else {
                                sharing.set(true);
                                clear_pending.set(false);
                            }
                        },
                    }
                    label { class: "nav-label", r#for: "{toggle_id}",
                        "Share a separate {service} resource with the client"
                    }
                    div { class: "nav-field__help", id: "{toggle_help_id}", "{toggle_help}" }
                }
                if *clear_pending.read() {
                    div {
                        class: "project-resource-card__confirmation",
                        role: "alertdialog",
                        "aria-describedby": confirmation_id.clone(),
                        "aria-label": "Confirm stopping client sharing",
                        p { id: "{confirmation_id}",
                            "Stop sharing this {service} resource with the client?"
                        }
                        button {
                            class: "nav-btn nav-btn--danger",
                            r#type: "button",
                            onclick: move |_| {
                                shared_value.set(String::new());
                                sharing.set(false);
                                clear_pending.set(false);
                            },
                            "Stop sharing"
                        }
                        button {
                            class: "nav-btn nav-btn--secondary",
                            r#type: "button",
                            onclick: move |_| clear_pending.set(false),
                            "Keep sharing"
                        }
                    }
                }
                if sharing() {
                    div { class: "project-resource-card__shared",
                        label { class: "nav-label", r#for: "{shared_field_name}",
                            "{props.shared_label}"
                        }
                        input {
                            class: "nav-input",
                            r#type: "text",
                            id: "{shared_field_name}",
                            name: "{shared_field_name}",
                            value: "{shared_value}",
                            placeholder: "{shared_placeholder}",
                            "aria-describedby": shared_help_id.clone(),
                            oninput: move |event| shared_value.set(event.value()),
                        }
                        div { class: "nav-field__help", id: "{shared_help_id}", "{shared_help}" }
                    }
                }
            }
        }
    }
}

/// The loaded edit form. Slack and Notion stay inside the one native save
/// form, but each resource has its own accessible card and sharing state.
fn edit_body(view: &ProjectEditView) -> Element {
    let action = format!("/app/projects/{}", view.project_code);
    let fields = vec![
        Field::text("Name", "name", view.name.clone()).required(),
        Field::select(
            "Entity",
            "entity_id",
            entity_options(&view.entities),
            view.entity_id.clone(),
        )
        .required()
        .help(ENTITY_HELP),
        Field::textarea("Description", "description", view.description.clone(), 3)
            .help(DESCRIPTION_HELP),
    ];
    let repository = Field::text(
        "Source repository",
        "repository_url",
        view.repository_url.clone(),
    )
    .placeholder("https://github.com/an-organization/a-project")
    .help(REPOSITORY_URL_HELP);
    let extra_fields = rsx! {
        div { class: "project-resource-cards",
            ProjectResourceCard {
                service: "Slack".to_string(),
                private_field_name: "internal_slack_channel_url".to_string(),
                private_label: "Private Slack channel".to_string(),
                private_url: view.internal_slack_channel_url.clone(),
                private_help: INTERNAL_SLACK_HELP.to_string(),
                shared_field_name: "external_slack_channel_url".to_string(),
                shared_label: "Shared Slack channel".to_string(),
                shared_url: view.external_slack_channel_url.clone(),
                shared_placeholder: "https://neonlaw.slack.com/archives/C0123456789".to_string(),
                shared_help: EXTERNAL_SLACK_HELP.to_string(),
            }
            ProjectResourceCard {
                service: "Notion".to_string(),
                private_field_name: "private_notion_page_url".to_string(),
                private_label: "Private Notion page".to_string(),
                private_url: view.private_notion_page_url.clone(),
                private_help: PRIVATE_NOTION_HELP.to_string(),
                shared_field_name: "shared_notion_page_url".to_string(),
                shared_label: "Shared Notion page".to_string(),
                shared_url: view.shared_notion_page_url.clone(),
                shared_placeholder: "https://www.notion.so/an-organization/A-matter-def456".to_string(),
                shared_help: SHARED_NOTION_HELP.to_string(),
            }
        }
        {repository.render()}
    };
    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Projects | Edit project" }
        header { class: "page-header",
            h1 { "Edit project" }
            p { a { href: "/app/projects", "← Back to projects" } }
        }
        if let Some(error) = view.error.as_ref() {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        FormCard {
            title: "Edit project".to_string(),
            action,
            submit_label: "Save".to_string(),
            heading: Heading::H2,
            csrf_token: Some(view.csrf_token.clone()),
            fields,
            extra_fields: Some(extra_fields),
        }
        p { class: "project-form-cancel",
            a { class: "nav-btn nav-btn--secondary", href: "/app/projects", "Cancel" }
        }
    }
}

/// `/app/projects/{project_code}/edit` — the descriptive matter edit.
#[component]
pub fn LawyerProjectEdit() -> Element {
    let resource = use_server_future(get_project_edit_form)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "project-edit", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "project-edit", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        {app_navbar(view.role, view.logo.clone())}
        main { id: "project-edit", class: "nav-theme",
            if view.found {
                {edit_body(&view)}
            } else {
                document::Title { "{view.firm_name} | Lawyer | Not found" }
                h1 { "Not found" }
                p { "No matter is available at this address." }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{edit_body, EntityOption, ProjectEditView};
    use crate::people::ViewerRole;

    fn view() -> ProjectEditView {
        ProjectEditView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            found: true,
            project_code: "acme-formation".to_string(),
            name: "Acme Formation".to_string(),
            description: "Form the LLC.".to_string(),
            entity_id: Some("00000000-0000-0000-0000-000000000001".to_string()),
            entities: vec![EntityOption {
                id: "00000000-0000-0000-0000-000000000001".to_string(),
                name: "Acme".to_string(),
            }],
            internal_slack_channel_url: "https://neonlaw.slack.com/archives/C0000000001"
                .to_string(),
            external_slack_channel_url: String::new(),
            private_notion_page_url: "https://www.notion.so/neonlaw/Private-abc123".to_string(),
            shared_notion_page_url: String::new(),
            // Deliberately on a self-hosted forge whose path resembles neither
            // the matter name nor a Project code: the field renders a stored
            // value and composes nothing.
            repository_url: "https://git.example.internal/a-group/unrelated-name.git".to_string(),
            csrf_token: "TOK".to_string(),
            error: None,
            role: ViewerRole::Lawyer,
            logo: None,
        }
    }

    fn render(view: &ProjectEditView) -> String {
        dioxus_ssr::render_element(edit_body(view))
    }

    #[test]
    fn posts_to_the_matter_route_by_code_not_id() {
        let html = render(&view());
        assert!(
            html.contains(r#"action="/app/projects/acme-formation""#),
            "the save must post to the matter's code, the segment the router \
             actually matches — an id here 404s the save: {html}"
        );
        assert!(html.contains(r#"name="_csrf" value="TOK""#), "{html}");
        assert!(
            html.contains(r#"<option value="00000000-0000-0000-0000-000000000001" selected"#),
            "{html}"
        );
    }

    /// Both Notion pages are editable here, prefilled from the stored value,
    /// and each names its audience in the label.
    ///
    /// This form is the only write path for the two columns, so a missing field
    /// would leave them settable only by a direct database write. The labels
    /// carry "Private"/"Shared" because that word is what tells a lawyer which
    /// box a client will be able to open.
    #[test]
    fn offers_both_notion_page_fields_with_their_audience_named() {
        let mut v = view();
        v.shared_notion_page_url = "https://www.notion.so/neonlaw/Shared-def456".to_string();
        let html = render(&v);
        assert!(html.contains(r#"name="private_notion_page_url""#), "{html}");
        assert!(html.contains(r#"name="shared_notion_page_url""#), "{html}");
        assert!(
            html.contains("https://www.notion.so/neonlaw/Private-abc123"),
            "the private page is prefilled: {html}"
        );
        assert!(html.contains("Private Notion page"), "{html}");
        assert!(html.contains("Shared Notion page"), "{html}");
    }

    /// The repository is editable here, prefilled from the stored value.
    ///
    /// This form is the write path for a Project's source, so a missing field
    /// would leave the column settable only by a direct database write.
    #[test]
    fn offers_the_source_repository_field_prefilled_from_the_stored_url() {
        let html = render(&view());
        assert!(html.contains(r#"name="repository_url""#), "{html}");
        assert!(
            html.contains("https://git.example.internal/a-group/unrelated-name.git"),
            "the stored URL must be prefilled verbatim: {html}"
        );
    }

    #[test]
    fn offers_no_status_control_and_no_matter_open_fields() {
        // Lifecycle transitions are their own commands (navigator#770), and the
        // client DRI / service / attestation belong to matter-open, not to a
        // descriptive edit.
        let html = render(&view());
        assert!(!html.contains(r#"name="status""#), "{html}");
        assert!(!html.contains(r#"name="client_dri_person_id""#), "{html}");
        assert!(!html.contains(r#"name="product_code""#), "{html}");
        assert!(!html.contains(r#"name="attestation""#), "{html}");
    }

    #[test]
    fn offers_both_slack_channel_fields_with_the_internal_one_prefilled() {
        let mut v = view();
        v.external_slack_channel_url = "https://neonlaw.slack.com/archives/C0000000002".to_string();
        let html = render(&v);
        assert!(
            html.contains(r#"name="internal_slack_channel_url""#)
                && html.contains("https://neonlaw.slack.com/archives/C0000000001"),
            "{html}"
        );
        assert!(
            html.contains(r#"name="external_slack_channel_url""#),
            "{html}"
        );
    }

    #[test]
    fn renders_slack_and_notion_as_separate_audience_cards() {
        let mut v = view();
        v.external_slack_channel_url = "https://neonlaw.slack.com/archives/C0000000002".to_string();
        v.shared_notion_page_url = "https://www.notion.so/neonlaw/Shared-def456".to_string();
        let html = render(&v);

        for card in ["slack", "notion"] {
            assert_eq!(
                html.matches(&format!(r#"data-resource-card="{card}""#))
                    .count(),
                1,
                "{card} must be its own card: {html}"
            );
        }
        assert!(html.contains("Private Slack channel"), "{html}");
        assert!(html.contains("Private Notion page"), "{html}");
        assert!(
            html.contains("Share a separate Slack resource with the client")
                && html.contains("Share a separate Notion resource with the client"),
            "each card needs an explicit sharing toggle: {html}"
        );
        assert!(
            html.contains(r#"name="internal_slack_channel_url""#),
            "{html}"
        );
        assert!(html.contains(r#"name="private_notion_page_url""#), "{html}");
        assert!(
            html.contains(r#"name="external_slack_channel_url""#),
            "{html}"
        );
        assert!(html.contains(r#"name="shared_notion_page_url""#), "{html}");
    }

    #[test]
    fn sharing_controls_follow_existing_shared_values_and_hide_unshared_fields() {
        let html = render(&view());
        assert!(
            !html.contains(r#"name="share_slack" checked"#),
            "an absent shared Slack value starts off: {html}"
        );
        assert!(
            !html.contains(r#"name="share_notion" checked"#),
            "an absent shared Notion value starts off: {html}"
        );
        assert!(
            !html.contains(r#"name="external_slack_channel_url""#)
                && !html.contains(r#"name="shared_notion_page_url""#),
            "an off toggle hides its shared input: {html}"
        );

        let mut shared = view();
        shared.external_slack_channel_url =
            "https://neonlaw.slack.com/archives/C0000000002".to_string();
        shared.shared_notion_page_url = "https://www.notion.so/neonlaw/Shared-def456".to_string();
        let html = render(&shared);
        assert!(
            html.contains(r#"name="share_slack" value="1" checked=true"#),
            "{html}"
        );
        assert!(
            html.contains(r#"name="share_notion" value="1" checked=true"#),
            "{html}"
        );
        assert!(
            html.contains("Manage Slack permissions in Slack too."),
            "{html}"
        );
        assert!(
            html.contains("Manage Notion permissions in Notion too."),
            "{html}"
        );
    }

    #[test]
    fn independent_card_values_survive_an_error_render() {
        let mut v = view();
        v.external_slack_channel_url = "https://neonlaw.slack.com/archives/C0000000002".to_string();
        v.shared_notion_page_url = "https://www.notion.so/neonlaw/Shared-def456".to_string();
        v.error = Some("The resource URL is invalid.".to_string());
        let html = render(&v);
        assert!(html.contains("The resource URL is invalid."), "{html}");
        assert!(
            html.contains("C0000000002"),
            "Slack's value was lost: {html}"
        );
        assert!(
            html.contains("Shared-def456"),
            "Notion's value was lost: {html}"
        );
        assert!(
            html.contains("Private-abc123"),
            "private value was lost: {html}"
        );
    }

    #[test]
    fn the_scope_narrative_survives_the_textarea_rcdata_trap() {
        // A `<textarea>` ignores `value=`; the body must be its inner content.
        let html = render(&view());
        assert!(html.contains("Form the LLC."), "{html}");
        assert!(html.contains("<textarea"), "{html}");
    }

    #[test]
    fn meets_the_layer_one_a11y_invariants() {
        let mut v = view();
        v.error = Some("Name is required.".to_string());
        crate::components::assert_forms_accessible(&render(&v), "project_edit");
    }

    #[test]
    fn the_error_flash_renders_and_no_htmx_ships() {
        let mut v = view();
        v.error = Some("Name is required.".to_string());
        let html = render(&v);
        assert!(html.contains(">Name is required.<"), "{html}");
        assert!(html.contains("admin-form"), "{html}");
        assert!(!html.contains("hx-"), "{html}");
    }
}
