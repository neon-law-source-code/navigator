//! Admin console "person show/edit" page as a Dioxus component (#641 Phase 3,
//! admin cluster) — the person edit form plus its per-record actions.
//!
//! The successor to the `admin_person_show` → `person_show_response` render
//! for the `/app/admin/people/{id}` surface. It loads the person by its `{id}` path
//! parameter (a not-found state when the id resolves to no row) and renders the
//! shared [`crate::components::FormCard`] prefilled with the person's name,
//! email, and role — a native `POST` to `/app/admin/people/{id}` (the update route
//! that wraps the person update command; the form `PATCH`ed the REST
//! `/app/api/people/{id}` over HTMX). Below the form is the actions panel: the
//! welcome-email action (a native `<details>` disclosure whose confirm button
//! `POST`s the send; the surface confirmed through HTMX's `hx-confirm`,
//! which this no-JS form cannot use under its strict CSP, and the disclosure
//! stays on the page so unsaved edits are not discarded), the Xero contact link,
//! and (for a client) the native impersonate form.
//!
//! The read-only legal-name parts and a locked role select disable rather than
//! submit. The bootstrap Owner record renders fully immutable —
//! every field disabled and no Save — because the command layer rejects every
//! write to it, so the form must not invite one.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Choice, Field, FormCard};
use crate::people::ViewerRole;

// Server-only: the `#[server]` body loads the person via `find`.

/// The configured `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL`, injected into the request
/// by the portal `admin_person_show_router` layer — a wasm-safe newtype the
/// `#[server]` function extracts to resolve the immutable bootstrap-Owner row.
/// `webapp` cannot see the portal `AppState` where the value lives (it depends
/// on `store`, not `portal`), so it is injected the same way [`crate::csrf::
/// CsrfToken`] and [`crate::people::ViewerRole`] are. `None` when unset.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BootstrapOwnerEmail(pub Option<String>);

/// The prefilled fields of the person being edited.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PersonFields {
    pub name: String,
    pub email: String,
    /// The role token (`owner` / `admin` / `lawyer` / `clerk` / `client`).
    pub role: String,
    pub given_name: String,
    pub family_name: String,
    pub middle_name: String,
}

/// A flash notice floated on arrival — the welcome-email send outcome (mapped
/// from `?notice=`) or a rejected update (`?error=`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PersonNotice {
    /// `true` renders the green success confirmation; `false` the red failure.
    pub success: bool,
    pub message: String,
}

/// The rendered admin person show/edit page, in a wasm-safe shape.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PersonShowView {
    /// The person id (for the form and action routes).
    pub id: String,
    /// The prefilled fields; `None` when the id resolves to no person (a 404).
    pub fields: Option<PersonFields>,
    /// The row is the bootstrap Owner: every field is disabled and
    /// no Save is offered.
    pub read_only: bool,
    /// The role select is locked (the bootstrap Owner, whose role is pinned).
    pub role_locked: bool,
    /// This person can be impersonated — only a `client` record.
    pub can_impersonate: bool,
    /// The Xero contact id when synced (an external link); `None` renders the
    /// "not synced yet" note.
    pub xero_contact_id: Option<String>,
    /// The flash notice, from `?notice=` / `?error=`; `None` on a plain visit.
    pub notice: Option<PersonNotice>,
    pub csrf_token: String,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The show view's query flags: the welcome-send `?notice=` and the update
/// `?error=` flash.
#[cfg(feature = "server")]
#[derive(Deserialize, Default)]
struct PersonShowQuery {
    #[serde(default)]
    notice: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The list path Cancel and "back to people" target.
pub const LIST_PATH: &str = "/app/admin/people";
/// The detail path base for the form action and the per-record action routes;
/// the `{id}` and any action suffix are appended.
pub const DETAIL_PATH: &str = "/app/admin/people";

/// Load the person show/edit page for the `{id}` in the request path: read the
/// injected CSRF token and the query flags, load the person (`fields: None` +
/// a committed 404 when the id resolves to no row), and resolve the immutable
/// bootstrap-Owner branch. The caller runs the admin auth gate and passes the
/// resolved `role`.
#[cfg(feature = "server")]
async fn load_person_show(role: ViewerRole) -> Result<PersonShowView, ServerFnError> {
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
    let query = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<PersonShowQuery>,
        _,
    >()
    .await
    .map(|axum::extract::Query(q)| q)
    .unwrap_or_default();

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let person = store::persons::find_by_id(&surreal, id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    let Some(p) = person else {
        // A valid UUID that resolves to no row is a missing resource: commit the
        // same 404 the show handler returned so clients, caches, and
        // monitoring see the not-found state as not-found.
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Ok(PersonShowView {
            id: id.to_string(),
            fields: None,
            csrf_token,
            role,
            ..PersonShowView::default()
        });
    };

    // The bootstrap Owner record is immutable: its role is pinned
    // and the command layer rejects every write, so the whole form locks. The
    // configured email is injected by the portal router (mirroring the CSRF
    // token) rather than read from the environment, so a test can set it on the
    // state without a process-global env var.
    let BootstrapOwnerEmail(bootstrap_owner_email) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<BootstrapOwnerEmail>, _>()
            .await
            .map(|axum::Extension(email)| email)
            .unwrap_or_default();
    let is_bootstrap_owner = bootstrap_owner_email
        .as_deref()
        .is_some_and(|configured| configured.eq_ignore_ascii_case(&p.email));

    // `?error=` (a rejected update) is a red flash; the welcome-send `?notice=`
    // maps to a green/red toast naming the recipient.
    let notice = match (query.error.as_deref(), query.notice.as_deref()) {
        (Some(error), _) if !error.is_empty() => Some(PersonNotice {
            success: false,
            message: error.to_string(),
        }),
        (_, Some("welcome_sent")) => Some(PersonNotice {
            success: true,
            message: format!("Welcome email sent to {}.", p.email),
        }),
        (_, Some("welcome_failed")) => Some(PersonNotice {
            success: false,
            message: format!(
                "Couldn't send the welcome email to {}. Check the email log.",
                p.email
            ),
        }),
        _ => None,
    };

    // Only a client can be impersonated.
    let can_impersonate = p.role == store::persons::Role::Client;
    // The role select locks for the pinned bootstrap Owner and for a target above
    // the caller's own authority — the command layer drops such a write, so the
    // form must not invite it. `require_admin` already guarantees the caller may
    // set roles at all.
    let target_rank = p.role.authority_rank();
    let role_locked = is_bootstrap_owner || target_rank > role.authority_rank();

    Ok(PersonShowView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        id: id.to_string(),
        fields: Some(PersonFields {
            name: p.name,
            role: p.role.as_str().to_string(),
            email: p.email,
            given_name: p.given_name.unwrap_or_default(),
            family_name: p.family_name.unwrap_or_default(),
            middle_name: p.middle_name.unwrap_or_default(),
        }),
        read_only: is_bootstrap_owner || target_rank > role.authority_rank(),
        role_locked,
        can_impersonate,
        xero_contact_id: p.xero_contact_id,
        notice,
        csrf_token,
        role,
    })
}

/// Load the person show/edit page (`/app/admin/people/{id}`): refuse non-admin,
/// then load through [`load_person_show`].
#[server]
pub async fn get_admin_person_show() -> Result<PersonShowView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    load_person_show(role).await
}

/// Build the edit form's fields from the prefilled values, applying the disabled
/// state for the read-only legal-name parts, a locked role select, and the fully
/// immutable bootstrap-Owner record.
fn edit_fields(
    fields: &PersonFields,
    read_only: bool,
    role_locked: bool,
    viewer_role: ViewerRole,
) -> Vec<Field> {
    let mut name = Field::text("Name", "name", fields.name.clone()).required();
    let mut email = Field::input("Email", "email", fields.email.clone(), "email").required();
    if read_only {
        // Name + email stay editable on an ordinary edit; a protected Owner record
        // locks them too so nothing on the page invites a rejected write.
        name = name.disabled();
        email = email.disabled();
    }

    let mut role_options = Vec::new();
    if viewer_role == ViewerRole::Owner {
        role_options.push(Choice::new("owner", "Owner"));
    }
    role_options.extend([
        Choice::new("admin", "Admin"),
        Choice::new("lawyer", "Lawyer (lawyer)"),
        Choice::new("clerk", "Clerk (non-lawyer)"),
        Choice::new("client", "Client"),
    ]);
    let selected = if fields.role.is_empty() {
        "client".to_string()
    } else {
        fields.role.clone()
    };
    let mut role = Field::select("Role", "role", role_options, Some(selected));
    role = if read_only {
        // The bootstrap Owner's role is pinned to `owner` by the command layer;
        // disable the select and say where it changes instead.
        role.disabled().help(
            "The bootstrap Owner role on this account cannot be changed from the UI. \
             Edit via NAVIGATOR_BOOTSTRAP_OWNER_EMAIL or a direct DB write.",
        )
    } else if role_locked {
        role.disabled()
            .help("Only an Owner or Admin can change an eligible person's role.")
    } else {
        role.help("The system-wide tier. A change takes effect the next time this person signs in.")
    };

    // The legal-name parts are filing-only details, read-only on the edit form.
    let mut given = Field::text("Given name", "given_name", fields.given_name.clone()).help(
        "Filing-only details: use Name for the everyday display name; enter these only \
         when a form needs separate legal-name parts.",
    );
    let mut family = Field::text("Family name", "family_name", fields.family_name.clone());
    let mut middle = Field::text("Middle name", "middle_name", fields.middle_name.clone());
    given = given.disabled();
    family = family.disabled();
    middle = middle.disabled();

    vec![name, email, role, given, family, middle]
}

/// The person show/edit page (`/app/admin/people/{id}`).
#[component]
pub fn AdminPersonShow() -> Element {
    let resource = use_server_future(get_admin_person_show)?;
    render_person_show(&resource)
}

/// The per-record actions panel below the edit form: the welcome-email
/// confirmation disclosure, the Xero contact link, and — for a client — the
/// impersonate form. `welcome_recipient` is the person's email, named in the
/// confirmation prompt.
fn person_actions(view: &PersonShowView, welcome_recipient: &str) -> Element {
    let welcome_action = format!("{DETAIL_PATH}/{}/welcome", view.id);
    let impersonate_action = format!("{DETAIL_PATH}/{}/impersonate", view.id);
    let csrf_token = view.csrf_token.clone();
    let xero_contact_id = view.xero_contact_id.clone();
    rsx! {
        section { id: "person-actions", class: "person-actions",
            h2 { "Actions" }
            div { class: "action-row",
                // Sending the welcome email is an external side effect, so it
                // takes a confirmation step. The surface used HTMX's
                // `hx-confirm`; this no-JS form (under a CSP that blocks inline
                // handlers) confirms with a native `<details>` disclosure
                // instead. Clicking "Send welcome email" only reveals the confirm
                // button, so an accidental click cannot fire the send. It also
                // stays on the page (no navigation), so unsaved edits in the form
                // above are not discarded while confirming.
                details { class: "welcome-confirm",
                    summary { class: "nav-btn nav-btn--secondary", "Send welcome email" }
                    form { method: "post", action: welcome_action,
                        "aria-label": "Confirm sending the welcome email",
                        input { r#type: "hidden", name: "_csrf", value: "{csrf_token}" }
                        p { "Send welcome email to {welcome_recipient}?" }
                        button { class: "nav-btn nav-btn--secondary", r#type: "submit", "Confirm and send welcome email" }
                    }
                }
                match xero_contact_id {
                    Some(contact_id) => rsx! {
                        a {
                            class: "nav-link",
                            href: "https://go.xero.com/Contacts/View/{contact_id}",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "View in Xero"
                        }
                    },
                    None => rsx! {
                        span { class: "nav-muted",
                            "Not synced to Xero yet — this client has no Xero contact on file."
                        }
                    },
                }
                if view.can_impersonate {
                    form { method: "post", action: impersonate_action,
                        "aria-label": "Impersonate client",
                        input { r#type: "hidden", name: "_csrf", value: "{csrf_token}" }
                        button { class: "nav-btn nav-btn--secondary", r#type: "submit", "Impersonate client" }
                    }
                }
            }
        }
    }
}

/// Render the resolved person show/edit page: the prefilled edit form posting to
/// the native `POST /app/admin/people/{id}` update route, then the per-record
/// actions (welcome email, Xero link, and — for a client — impersonate).
fn render_person_show(resource: &Resource<Result<PersonShowView, ServerFnError>>) -> Element {
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "person-show", p { "Failed to load the person." } }
            }
        }
        None => {
            return rsx! {
                main { id: "person-show", p { "Loading…" } }
            }
        }
    };

    let page_title = format!("{} | Admin | People | Edit person", view.firm_name);
    let not_found_title = format!("{} | Admin | People | Not found", view.firm_name);

    rsx! {
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "person-show", class: "nav-theme",
            match &view.fields {
                Some(fields) => {
                    let action = format!("{DETAIL_PATH}/{}", view.id);
                    let welcome_recipient = fields.email.clone();
                    let form_fields =
                        edit_fields(fields, view.read_only, view.role_locked, view.role);
                    rsx! {
                        document::Title { "{page_title}" }
                        if let Some(notice) = view.notice.as_ref() {
                            div {
                                class: if notice.success { "nav-flash nav-flash--success" } else { "nav-flash nav-flash--danger" },
                                role: if notice.success { "status" } else { "alert" },
                                "{notice.message}"
                            }
                        }
                        if view.read_only {
                            div { class: "nav-form-notice", role: "note",
                                "This is the bootstrap Owner record. It is immutable "
                                "from the UI — change it via "
                                code { "NAVIGATOR_BOOTSTRAP_OWNER_EMAIL" }
                                " or a direct database write."
                            }
                        }
                        FormCard {
                            title: "Edit person".to_string(),
                            action,
                            submit_label: "Save".to_string(),
                            csrf_token: Some(view.csrf_token.clone()),
                            read_only: view.read_only,
                            fields: form_fields,
                        }
                        p { a { href: "{LIST_PATH}", "← Cancel" } }
                        {person_actions(&view, &welcome_recipient)}
                    }
                }
                None => rsx! {
                    document::Title { "{not_found_title}" }
                    h1 { "Person not found" }
                    p { "No person exists with id " code { "{view.id}" } "." }
                    p { a { href: "{LIST_PATH}", "← Back to people" } }
                },
            }
        }
    }
}
