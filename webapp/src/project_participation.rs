//! The matter participation add/edit form as a Dioxus component (#956 Phase 4)
//! — `/app/projects/{project_code}/people/new` and
//! `/app/projects/{project_code}/people/{role_id}/edit`.
//!
//! The successor to the `views::pages::admin::projects::participation_form`.
//! One shared render, two mounts: the add form posts to
//! `POST /app/projects/{project_code}/people`, the edit form to
//! `POST /app/projects/{project_code}/people/{role_id}/edit`. Both are native `POST`s
//! through the shared [`FormCard`] carrying the session CSRF token — no
//! JavaScript. A rejected submit redirects back here with `?error=`, surfaced
//! above the form (post/redirect/get), the way every other migrated admin form
//! reports a refusal.
//!
//! # Authorization
//!
//! Admin-only, and hidden rather than refused: lawyer and clients get the same
//! `404` the handler returned. That is deliberate — the participation
//! ledger is the project-scope ACL, so granting it stays with the administrative
//! owner rather than with ordinary lawyer. Impersonation cannot reach it either:
//! an impersonating session carries the `client` tier.
//!
//! Project and participation reads come from the `SurrealDB` projects cluster,
//! and people live in that same cluster, so the matter lookup and the
//! participation writes read one store.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard, Heading, PersonChoice};
use crate::people::ViewerRole;

/// The form's `?error=` flash, set by the create/update handler's
/// redirect-on-failure.
#[derive(Deserialize, Default)]
pub struct ParticipationQuery {
    #[serde(default)]
    pub error: Option<String>,
    /// The person the refused submit had chosen, echoed back so the picker keeps
    /// it selected across the redirect.
    #[serde(default)]
    pub person_id: Option<String>,
    /// The accountability choice a refused submit had made (`none`/`lawyer`/
    /// `client`), echoed back so the radio keeps it.
    #[serde(default)]
    pub dri: Option<String>,
}

/// The rendered participation form.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ParticipationView {
    /// `false` when the caller is not an admin, the matter is unknown, or the
    /// participation row does not belong to it — the page renders not-found
    /// under a committed `404`.
    pub found: bool,
    pub project_id: String,
    /// The matter's code — what every link on this form is built from. The id
    /// stays because the ledger rows key on it; only URLs changed.
    pub project_code: String,
    pub project_name: String,
    /// `Some` on the edit mount, `None` on the add mount.
    pub role_id: Option<String>,
    pub people: Vec<PersonChoice>,
    pub person_id: Option<String>,
    /// The accountability radio's value: `none`, `lawyer`, or `client`.
    pub dri: String,
    /// Everyone who currently carries each marker, named so an operator sees who
    /// is already accountable before adding another. Empty on the lawyer side
    /// means the matter is missing the marker it must always have.
    pub lawyer_dris: Vec<String>,
    pub client_dris: Vec<String>,
    pub csrf_token: String,
    pub error: Option<String>,
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

/// Commit a `404` and render the not-found body — the shared fail-closed exit.
/// Async because the not-found body carries the navbar, mark included.
#[cfg(feature = "server")]
async fn hidden(role: ViewerRole) -> ParticipationView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    ParticipationView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: false,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        ..ParticipationView::default()
    }
}

/// Who carries each accountability marker on this matter, by name.
///
/// Named rather than flagged: moving accountability is a decision about a
/// person, so the form says which one it would move it from.
#[cfg(feature = "server")]
async fn dri_holders(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
    people: &[PersonChoice],
) -> Result<(Vec<String>, Vec<String>), ServerFnError> {
    let rows = store::projects::participations_for_project(surreal, project_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let names_of = |flagged: fn(&store::projects::PersonProjectRole) -> bool| {
        let mut names: Vec<String> = rows
            .iter()
            .filter(|row| flagged(row))
            .filter_map(|row| {
                let id = row.person_id.to_string();
                people.iter().find(|p| p.id == id).map(|p| p.name.clone())
            })
            .collect();
        names.sort();
        names
    };
    Ok((
        names_of(|row| row.is_lawyer_dri),
        names_of(|row| row.is_client_dri),
    ))
}

/// The radio's seed: a refused submit's own choice, else the row's markers, else
/// "not a DRI" on a fresh add.
#[cfg(feature = "server")]
fn seeded_dri(
    submitted: Option<String>,
    existing: Option<&store::projects::PersonProjectRole>,
) -> String {
    submitted
        .filter(|raw| matches!(raw.as_str(), "none" | "lawyer" | "client"))
        .or_else(|| {
            existing.map(|row| {
                if row.is_lawyer_dri {
                    "lawyer".to_string()
                } else if row.is_client_dri {
                    "client".to_string()
                } else {
                    "none".to_string()
                }
            })
        })
        .unwrap_or_else(|| "none".to_string())
}

/// Load the matter, the assignable people, and the session context shared by
/// both mounts. `role_id` is the edit mount's participation row; `None` adds.
#[cfg(feature = "server")]
async fn load(
    project_code: &str,
    role_id: Option<uuid::Uuid>,
) -> Result<ParticipationView, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    // The participation ledger is the project ACL: admin only, and hidden from
    // everyone else rather than refused.
    if !role.is_admin_tier() {
        return Ok(hidden(role).await);
    }
    let csrf_token = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::csrf::CsrfToken>,
        _,
    >()
    .await
    .map(|axum::Extension(token)| token.0)
    .unwrap_or_default();
    let query = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ParticipationQuery>,
        _,
    >()
    .await
    .map(|axum::extract::Query(q)| q)
    .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(project) = store::projects::find_by_code(&surreal, project_code)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(hidden(role).await);
    };
    let project_id = project.id;

    // On the edit mount, the row must exist *and* belong to this matter — a
    // cross-matter role id is a not-found, never an edit of someone else's row.
    let existing = match role_id {
        Some(id) => {
            let row = store::projects::participation_by_id(&surreal, id)
                .await
                .map_err(|e| ServerFnError::new(e.to_string()))?;
            match row {
                Some(row) if row.project_id == project_id => Some(row),
                _ => return Ok(hidden(role).await),
            }
        }
        None => None,
    };

    let people: Vec<PersonChoice> = store::persons::list_directory(&surreal, "", "", &[])
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|p| PersonChoice::new(p.id.to_string(), p.name, p.email).with_detail(p.role.as_str()))
        .collect();

    // A refused submit echoes its person back through the redirect query, so the
    // operator does not re-pick it; otherwise the stored row (edit) or a blank
    // (add) seeds the control.
    let person_id = query
        .person_id
        .filter(|raw| !raw.is_empty())
        .or_else(|| existing.as_ref().map(|row| row.person_id.to_string()));

    let (lawyer_dris, client_dris) = dri_holders(&surreal, project_id, &people).await?;
    let dri = seeded_dri(query.dri.clone(), existing.as_ref());

    Ok(ParticipationView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        found: true,
        project_id: project_id.to_string(),
        project_code: project.code.clone(),
        project_name: project.name,
        role_id: role_id.map(|id| id.to_string()),
        people,
        person_id,
        dri,
        lawyer_dris,
        client_dris,
        csrf_token,
        error: query.error,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
    })
}

/// Load the **add** form (`/app/projects/{project_code}/people/new`).
#[server]
pub async fn get_participation_new() -> Result<ParticipationView, ServerFnError> {
    let Ok(axum::extract::Path(project_code)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>().await
    else {
        return Ok(hidden(ViewerRole::default()).await);
    };
    load(&project_code, None).await
}

/// Load the **edit** form
/// (`/app/projects/{project_code}/people/{role_id}/edit`).
#[server]
pub async fn get_participation_edit() -> Result<ParticipationView, ServerFnError> {
    let Ok(axum::extract::Path((project_code, role_id))) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::extract::Path<(String, uuid::Uuid)>,
            _,
        >()
        .await
    else {
        return Ok(hidden(ViewerRole::default()).await);
    };
    load(&project_code, Some(role_id)).await
}

/// The `/app` navbar this form carries, from the viewer's tier and the deploy's
/// brand mark.
fn participation_nav(view: &ParticipationView) -> Element {
    rsx! {
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
    }
}

/// The label for one side's radio choice, naming who already holds it.
fn dri_choice(label: &str, holders: &[String]) -> String {
    if holders.is_empty() {
        format!("{label} — nobody yet")
    } else {
        format!("{label} — currently {}", holders.join(", "))
    }
}

/// The accountability control: a radio over the two markers plus "not a DRI".
///
/// Nothing is locked. Each side is a set, so designating this person adds them
/// beside whoever is already there and takes nothing from anyone — there is no
/// displacement for the control to guard against. Whether this operator may add
/// them at all is `store::participation::authorize`'s call, at the write door.
fn dri_field(view: &ParticipationView) -> Field {
    Field::radio(
        "Accountability",
        "dri",
        vec![
            Choice::new("none", "Not a DRI"),
            Choice::new(
                "lawyer",
                dri_choice("Lawyer DRI (accountable lawyer)", &view.lawyer_dris),
            ),
            Choice::new(
                "client",
                dri_choice("Client DRI (client contact)", &view.client_dris),
            ),
        ],
        Some(view.dri.clone()),
    )
    .help(
        "A matter can have more than one DRI on each side, so this adds this person to that side \
         rather than replacing anyone. Lawyer DRIs are the accountable lawyers and close the \
         matter; client DRIs are the client-side contacts. Only a firm-side lawyer can hold the \
         lawyer marker.",
    )
}

/// The loaded form: the heading, the back link, the error flash, and the native
/// `POST` that adds or updates one participation row.
fn participation_body(view: &ParticipationView) -> Element {
    let editing = view.role_id.is_some();
    let title = if editing {
        "Edit matter person"
    } else {
        "Add matter person"
    };
    let submit = if editing { "Save" } else { "Add" };
    let project_href = format!("/app/projects/{}", view.project_code);
    let action = match view.role_id.as_ref() {
        Some(role_id) => format!("/app/projects/{}/people/{role_id}/edit", view.project_code),
        None => format!("/app/projects/{}/people", view.project_code),
    };
    // Person and accountability are the only controls: the matter-side
    // participation follows from the tier already on the person's account, so
    // there is nothing left to type.
    let mut fields = vec![Field::person_picker(
        "Person",
        "person_id",
        "Choose a person",
        view.people.clone(),
        view.person_id.clone(),
    )
    .help("Their participation on this matter follows the system tier shown beside each name.")
    .required()];
    fields.push(dri_field(view));

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Projects | {title}" }
        header { class: "page-header",
            h1 { "{title}" }
            p { a { href: "{project_href}", "← Back to project" } }
        }
        p { class: "participation-intro",
            "This changes only who reaches "
            strong { "{view.project_name}" }
            ". Their participation on the matter follows the system tier already on their \
             account, and that tier remains unchanged."
        }
        if let Some(error) = view.error.as_ref() {
            p { class: "nav-form-error", role: "alert", "{error}" }
        }
        FormCard {
            title: title.to_string(),
            action: action.clone(),
            submit_label: submit.to_string(),
            heading: Heading::H2,
            csrf_token: Some(view.csrf_token.clone()),
            fields,
        }
        p { class: "participation-cancel",
            a { class: "nav-btn nav-btn--secondary", href: "{project_href}", "Cancel" }
        }
    }
}

/// Render a resolved participation resource — shared by both mounts.
fn render_participation(resource: &Resource<Result<ParticipationView, ServerFnError>>) -> Element {
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "project-participation", p { "Failed to load the form." } }
            }
        }
        None => {
            return rsx! {
                main { id: "project-participation", p { "Loading…" } }
            }
        }
    };

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        {participation_nav(&view)}
        main { id: "project-participation", class: "nav-theme",
            if view.found {
                {participation_body(&view)}
            } else {
                document::Title { "{view.firm_name} | Lawyer | Not found" }
                h1 { "Not found" }
                p { "No matter participation is available at this address." }
            }
        }
    }
}

/// `/app/projects/{project_code}/people/new` — add one participation row.
#[component]
pub fn LawyerParticipationNew() -> Element {
    let resource = use_server_future(get_participation_new)?;
    render_participation(&resource)
}

/// `/app/projects/{project_code}/people/{role_id}/edit` — edit one participation row.
#[component]
pub fn LawyerParticipationEdit() -> Element {
    let resource = use_server_future(get_participation_edit)?;
    render_participation(&resource)
}

#[cfg(test)]
mod tests {
    use super::{participation_body, ParticipationView};
    use crate::{components::PersonChoice, people::ViewerRole};

    fn person() -> PersonChoice {
        PersonChoice::new(
            "00000000-0000-0000-0000-000000000001",
            "Libra Client",
            "libra@example.com",
        )
        .with_detail("client")
    }

    /// The firm-side lawyer the lawyer marker can actually live on.
    fn lawyer() -> PersonChoice {
        PersonChoice::new(
            "00000000-0000-0000-0000-000000000002",
            "Avery Attorney",
            "avery@neonlaw.com",
        )
        .with_detail("lawyer")
    }

    fn view(role_id: Option<&str>, error: Option<&str>) -> ParticipationView {
        ParticipationView {
            firm_name: "Neon Law".to_string(),
            found: true,
            project_id: "00000000-0000-0000-0000-0000000000aa".to_string(),
            project_code: "acme-formation".to_string(),
            project_name: "Acme Formation".to_string(),
            role_id: role_id.map(ToString::to_string),
            people: vec![person(), lawyer()],
            person_id: None,
            dri: "none".to_string(),
            lawyer_dris: Vec::new(),
            client_dris: Vec::new(),
            csrf_token: "TOK".to_string(),
            error: error.map(ToString::to_string),
            role: ViewerRole::Admin,
            logo: None,
        }
    }

    fn render(view: &ParticipationView) -> String {
        dioxus_ssr::render_element(participation_body(view))
    }

    #[test]
    fn add_posts_to_the_collection_route_and_lists_people() {
        let html = render(&view(None, None));
        assert!(
            html.contains(r#"action="/app/projects/acme-formation/people""#),
            "{html}"
        );
        assert!(html.contains(r#"name="_csrf" value="TOK""#), "{html}");
        assert!(html.contains(r#"name="person_id""#), "{html}");
        assert!(
            html.contains("Libra Client &#60;libra@example.com&#62;"),
            "{html}"
        );
        // The matter is named so an admin cannot confuse the ledger they edit.
        assert!(html.contains(">Acme Formation<"), "{html}");
    }

    /// Participation is derived from `persons.role` at the write seam, so the
    /// form must not offer it as an input at all — no control, and none of the
    /// datalist apparatus the free-text version carried.
    #[test]
    fn the_form_does_not_ask_for_a_participation() {
        for role_id in [None, Some("00000000-0000-0000-0000-0000000000bb")] {
            let html = render(&view(role_id, None));
            assert!(!html.contains(r#"name="participation""#), "{html}");
            assert!(!html.contains("<datalist"), "{html}");
            assert!(!html.contains(">Participation<"), "{html}");
        }
    }

    /// The person picker is what carries the derivation: the tier printed beside
    /// each name *is* the participation the row will take, and the copy says so
    /// rather than leaving an admin to infer it.
    #[test]
    fn the_person_picker_names_the_tier_that_becomes_the_participation() {
        let html = render(&view(None, None));
        assert!(
            html.contains("Libra Client &#60;libra@example.com&#62; — client"),
            "{html}"
        );
        assert!(
            html.contains("follows the system tier shown beside each name"),
            "{html}"
        );
        assert!(html.contains("that tier remains unchanged"), "{html}");
    }

    #[test]
    fn edit_posts_to_the_row_route_and_preselects_the_person() {
        let mut v = view(Some("00000000-0000-0000-0000-0000000000bb"), None);
        v.person_id = Some(person().id);
        let html = render(&v);
        assert!(
            html.contains(
                r#"action="/app/projects/acme-formation/people/00000000-0000-0000-0000-0000000000bb/edit""#
            ),
            "{html}"
        );
        assert!(
            html.contains(r#"<option value="00000000-0000-0000-0000-000000000001" selected"#),
            "{html}"
        );
    }

    #[test]
    fn the_error_flash_renders_above_the_form() {
        let html = render(&view(None, Some("That person is already assigned.")));
        assert!(
            html.contains(">That person is already assigned.<"),
            "{html}"
        );
        assert!(html.contains("nav-form-error"), "{html}");
    }

    /// Accountability is a radio, not free text: three mutually exclusive
    /// choices, and the copy names what each marker means.
    #[test]
    fn the_form_offers_the_two_markers_as_a_radio() {
        let html = render(&view(None, None));
        assert!(html.contains(r#"type="radio""#), "{html}");
        for value in ["none", "lawyer", "client"] {
            assert!(
                html.contains(&format!(r#"name="dri" value="{value}""#)),
                "the {value} choice is missing: {html}"
            );
        }
        assert!(html.contains("Lawyer DRI (accountable lawyer)"), "{html}");
        assert!(html.contains("Client DRI (client contact)"), "{html}");
        // Nobody holds either marker in this fixture, so nothing is locked.
        assert!(!html.contains("nav-radio--locked"), "{html}");
        assert!(!html.contains("disabled"), "{html}");
    }

    /// A side someone already holds is **not** locked: designation is additive,
    /// so adding a second lawyer DRI takes nothing from the first. The choice
    /// still names who is already accountable, because that is what the operator
    /// needs to know before adding to the set.
    #[test]
    fn a_held_side_stays_selectable_and_names_its_current_holders() {
        let mut v = view(None, None);
        v.lawyer_dris = vec!["Avery Attorney".to_string()];
        v.person_id = Some(person().id);
        let html = render(&v);

        assert!(
            !html.contains("nav-radio--locked"),
            "nothing is locked once a side is a set: {html}"
        );
        assert!(
            html.contains("Lawyer DRI (accountable lawyer) — currently Avery Attorney"),
            "{html}"
        );
        assert!(
            html.contains("Client DRI (client contact) — nobody yet"),
            "{html}"
        );
        assert!(
            !html.contains("Reassign the lawyer DRI"),
            "reassignment is retired: {html}"
        );
    }

    /// A side with several holders names them all, so an operator adding a third
    /// sees the two already there.
    #[test]
    fn a_side_with_several_holders_names_each_of_them() {
        let mut v = view(None, None);
        v.lawyer_dris = vec!["Avery Attorney".to_string(), "Nick Shook".to_string()];
        v.person_id = Some(lawyer().id);
        let html = render(&v);
        assert!(
            html.contains("currently Avery Attorney, Nick Shook"),
            "{html}"
        );
    }

    #[test]
    fn meets_the_layer_one_a11y_invariants() {
        let html = render(&view(None, Some("That person was not found.")));
        crate::components::assert_forms_accessible(&html, "project_participation");
    }

    #[test]
    fn keeps_the_admin_form_e2e_hook_and_ships_no_htmx() {
        // `server/tests/accessibility_e2e.rs` scopes axe to `form.admin-form`,
        // and `browser_e2e.rs` submits `form.admin-form` on this very page.
        let html = render(&view(None, None));
        assert!(html.contains("admin-form"), "{html}");
        assert!(!html.contains("hx-"), "{html}");
    }
}
