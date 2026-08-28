//! The lawyer matter-detail workbench (`/app/projects/{code}`) as a Dioxus
//! component (#641 Phase 3, projects cluster) — the firm-side single-matter view.
//!
//! The successor to the `admin::projects_detail_lawyer` render. Lawyer reach
//! it only through the lawyer lens (`store::access::can_see_project_as_lawyer` —
//! admin keeps the all-project bypass); a lawyer not on the matter, and a
//! non-lawyer caller, both get `404` (the matter does not exist for them). The
//! page gathers, server-side of the render: the header (name / code / status /
//! entity / the two DRIs), the missing-onboarding notice, the estate section, the
//! forge repository link, the calendar, the participation ledger (with admin
//! add/edit/remove), the documents table + uploader, and the close-matter
//! control. The write forms render markup that posts to the existing native
//! `/app/projects/{code}/...` handlers; only the rendering moves.
//!
//! The calendar is [`crate::project_calendar`] scoped to this matter — the same
//! surface the lawyer workbench carries across every matter, and empty for the
//! same reason (#350).
//!
//! Two seams `webapp` cannot cross itself, injected by the portal router the same
//! wasm-safe way as [`ViewerRole`] / [`crate::csrf::CsrfToken`]:
//! [`ProjectRepositoryPointer`] (this matter's one deployment-configured
//! repository URL) and [`LawyerEstate`] (the estate/`workflows`-coupled notation
//! view).

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Field, FormCard, Heading};
use crate::csrf::CsrfToken;
use crate::people::ViewerRole;

/// This matter's one source repository, as a resolved browser URL.
///
/// A Project has exactly one repository, named for its Project code, holding
/// that Project's notation templates and its client portal. The portal router
/// builds this from the active deployment's configured organization and forge
/// host plus `projects.code`. `None` leaves the pointer out of the lawyer-only
/// section: the coordinate is *derived*, so a deployment with no configured
/// forge coordinate has no repository to point at, and that is a legitimate
/// outcome rather than a degraded one.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProjectRepositoryPointer(pub Option<String>);

/// One generated estate draft (Northstar), in a wasm-safe shape.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EstateDraft {
    pub title: String,
    pub kind: String,
    pub status: String,
}

/// The transcript-driven estate notation for this matter, when it is one. The
/// detection (`crate::estate::transcript_driven_notation`) is `workflows`-coupled
/// and lives in `portal`, so the portal router computes this and injects it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EstateData {
    pub notation_id: String,
    pub state: String,
    pub drafts: Vec<EstateDraft>,
}

/// The injected estate view: `Some` only for a transcript-driven estate matter.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LawyerEstate(pub Option<EstateData>);

/// One document row (filename + id for the download link).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct LawyerDocRow {
    pub id: String,
    pub filename: String,
}

/// One participation-ledger row: who is assigned, their system tier, the
/// participation derived from it, and the row id for the edit/remove actions.
/// Both columns render because the tier is what *explains* the participation —
/// and a legacy row where the two disagree is worth seeing.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ParticipationRow {
    pub id: String,
    pub person_name: String,
    pub person_email: String,
    pub person_role: String,
    pub participation: String,
    /// The accountability markers this row carries. Each side is a set, so any
    /// number of rows may hold one — the lawyer markers name everyone who
    /// answers for the matter, and any of them may close it.
    pub is_lawyer_dri: bool,
    pub is_client_dri: bool,
}

/// The rendered lawyer workbench — every field wasm-safe (plain scalars; no
/// `store`/`SeaORM`/`cloud`/`repos` type crosses to the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LawyerDetailView {
    pub id: String,
    pub code: String,
    pub name: String,
    pub status: String,
    pub entity_id: Option<String>,
    pub entity_name: Option<String>,
    /// Everyone accountable on each side, by name. Empty on the lawyer side is
    /// the unassigned matter the oversight lens exists to flag.
    pub lawyer_dris: Vec<String>,
    pub client_dris: Vec<String>,
    /// `true` when the reader is one of this matter's lawyer DRIs, which is what
    /// entitles them to govern the lawyer side from this page. The server
    /// resolves it; the render never infers accountability from the tier.
    pub viewer_is_lawyer_dri: bool,
    /// The matter's six collaboration resources, already filtered to what this
    /// reader may see. The two Slack columns and the Notion pages are no longer
    /// carried loose on this view: a firm-only URL must not be serialised into
    /// a page whose reader may not see it, and one filtered list is the only
    /// place that decision is made.
    pub resources: crate::project_resources::ProjectResourcesView,
    pub xero_invoice_url: Option<String>,
    pub repository_url: Option<String>,
    pub estate: Option<EstateData>,
    pub participations: Vec<ParticipationRow>,
    pub documents: Vec<LawyerDocRow>,
    /// The upload form's Kind select, as `(value, label)` pairs — every
    /// `rules::kind::Kind::valid_for(Lane::Asset)` value, computed
    /// server-side so the wasm client never needs the `rules` crate.
    pub asset_kind_choices: Vec<(String, String)>,
    pub csrf_token: String,
    /// The matter calendar's active sort, read from `?sort=`/`?dir=` and
    /// normalised to the advertised columns.
    pub calendar_sort: String,
    pub calendar_dir: String,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
}

/// Resolve a side's DRI names from the designated ids, alphabetical.
///
/// A designated person who is no longer on file drops out rather than blanking
/// the whole side: the other names are still the answer to "who is accountable".
#[cfg(feature = "server")]
async fn dri_names(
    surreal: &store::surreal::SurrealDb,
    person_ids: &[uuid::Uuid],
) -> Result<Vec<String>, ServerFnError> {
    if person_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = store::persons::find_by_ids(surreal, person_ids)
        .await
        .map_err(server_error)?
        .into_iter()
        .map(|person| person.name)
        .collect();
    names.sort();
    Ok(names)
}

/// The document-upload Kind select's options, as `(value, label)` — every
/// `Kind` valid in the asset lane, in [`rules::kind::Kind::ALL`] order. Computed
/// here rather than duplicated as literal strings, so a new asset-lane kind
/// reaches the upload form for free.
#[cfg(feature = "server")]
fn asset_kind_choices() -> Vec<(String, String)> {
    rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
        .map(|k| (k.as_str().to_string(), k.describe().to_string()))
        .collect()
}

/// Fetch the lawyer-lens workbench for one matter. Refuses a non-lawyer caller and
/// a lawyer not on the matter with `404`; a query failure is a `500`.
#[server]
#[cfg_attr(feature = "server", allow(clippy::too_many_lines))]
pub async fn get_lawyer_project_detail() -> Result<LawyerDetailView, ServerFnError> {
    let axum::extract::Path(code) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::extract::Path<String>, _>()
            .await?;
    // The calendar's sort. Lenient like the workbench's — an unrecognised
    // column falls back to the leftmost rather than refusing the matter, which
    // is why this route carries no bad-sort pre-handler.
    let calendar_query = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<crate::project_calendar::CalendarQuery>,
        _,
    >()
    .await
    .map_or_else(
        |_| crate::project_calendar::CalendarQuery::default(),
        |axum::extract::Query(q)| q,
    );
    let calendar_sort = crate::project_calendar::sort_field(
        calendar_query.sort.as_deref(),
        crate::project_calendar::MATTER_COLUMNS,
    );
    let calendar_dir = crate::project_calendar::sort_dir(calendar_query.dir.as_deref());
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    // The navbar renders on the 404 body too, so the mark is resolved before the
    // first early return rather than only on the happy path.
    let logo = crate::app_chrome::app_logo_from_context().await;
    // A non-lawyer caller gets the handler's 404 — the workbench is hidden,
    // not merely refused.
    if !role.is_lawyer_tier() {
        return Ok(not_found(uuid::Uuid::nil(), role, logo, String::new()));
    }
    let csrf_token =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<CsrfToken>, _>()
            .await
            .map(|axum::Extension(token)| token.0)
            .unwrap_or_default();
    let ProjectRepositoryPointer(repository_url) =
        dioxus_fullstack_core::FullstackContext::extract::<
            axum::Extension<ProjectRepositoryPointer>,
            _,
        >()
        .await
        .map(|axum::Extension(pointer)| pointer)
        .unwrap_or_default();
    let LawyerEstate(estate) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<LawyerEstate>, _>()
            .await
            .map(|axum::Extension(estate)| estate)
            .unwrap_or_default();
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

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let Some(project) = store::projects::find_by_code(&surreal, &code)
        .await
        .map_err(server_error)?
    else {
        return Ok(not_found(uuid::Uuid::nil(), role, logo, String::new()));
    };
    let id = project.id;

    // Anyone not on the matter gets a 404, not a peek — Owner and Admin
    // included. This goes through `store::access`, which is the layer that
    // requires the participation row of every tier; the raw
    // `store::projects` predicate still carries the privileged short-circuit
    // its own remaining callers depend on, and reaching for it here is what
    // silently handed the workbench to an unassigned Owner.
    let viewer = store::access::matter_viewer(&surreal, person_id, store_role, id)
        .await
        .map_err(server_error)?;
    let visible = viewer.is_some();
    if !visible {
        return Ok(not_found(id, role, logo, csrf_token));
    }
    let entity = store::entities::find_by_id(&surreal, project.entity_id)
        .await
        .map_err(server_error)?;
    let entity_id = entity.as_ref().map(|entity| entity.id.to_string());
    let entity_name = entity.map(|entity| entity.name);

    // The matter's Xero invoice, if any (at most one — the mirror is unique
    // on `project_id`, so it is already grouped per matter). Absent until an
    // invoice raised in Xero is mirrored here.
    let xero_invoice_url = store::xero_invoices::for_projects(&surreal, &[id])
        .await
        .map_err(server_error)?
        .into_iter()
        .next()
        .map(|invoice| {
            format!(
                "https://go.xero.com/AccountsReceivable/View.aspx?InvoiceID={}",
                invoice.xero_invoice_id
            )
        });

    // The two DRIs — a read failure is a 500, never a silently-blank DRI on an
    // accountability surface.
    let participation_rows = store::projects::participations_for_project(&surreal, id)
        .await
        .map_err(server_error)?;
    let lawyer_dri_ids: Vec<uuid::Uuid> = participation_rows
        .iter()
        .filter(|row| row.is_lawyer_dri)
        .map(|row| row.person_id)
        .collect();
    let client_dri_ids: Vec<uuid::Uuid> = participation_rows
        .iter()
        .filter(|row| row.is_client_dri)
        .map(|row| row.person_id)
        .collect();
    // Whether *this* reader is accountable, resolved from the ledger rather
    // than from their tier: a lawyer who is not a DRI on this matter does not
    // govern its lawyer side.
    let viewer_is_lawyer_dri = person_id.is_some_and(|me| lawyer_dri_ids.contains(&me));
    let lawyer_dris = dri_names(&surreal, &lawyer_dri_ids).await?;
    let client_dris = dri_names(&surreal, &client_dri_ids).await?;

    let documents = store::assets::for_project(&surreal, id)
        .await
        .map_err(server_error)?
        .into_iter()
        .map(|d| LawyerDocRow {
            id: d.id.to_string(),
            filename: d.filename.unwrap_or_default(),
        })
        .collect();

    // The participation ledger: the rows, plus the linked people in one batched
    // query so the system tier is visible without conflating it with
    // participation.
    let people = store::persons::find_by_ids(
        &surreal,
        &participation_rows
            .iter()
            .map(|r| r.person_id)
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(server_error)?;
    let people_by_id: std::collections::HashMap<_, _> = people.iter().map(|p| (p.id, p)).collect();
    let participations = participation_rows
        .iter()
        .filter_map(|row| {
            people_by_id.get(&row.person_id).map(|p| ParticipationRow {
                id: row.id.to_string(),
                person_name: p.name.clone(),
                person_email: p.email.clone(),
                person_role: p.role.as_str().to_string(),
                participation: row.participation.clone(),
                is_lawyer_dri: row.is_lawyer_dri,
                is_client_dri: row.is_client_dri,
            })
        })
        .collect();

    let code_for_resources = project.code.clone();
    Ok(LawyerDetailView {
        id: project.id.to_string(),
        code: project.code,
        name: project.name,
        status: project.status,
        entity_id,
        entity_name,
        lawyer_dris,
        client_dris,
        viewer_is_lawyer_dri,
        resources: crate::project_resources::ProjectResourcesView {
            resources: crate::project_resources::visible_resources(
                &crate::project_resources::ProjectResourceLinks {
                    private_slack_channel_url: project.internal_slack_channel_url,
                    private_notion_page_url: project.private_notion_page_url,
                    drive_folder_id: project.drive_folder_id,
                    shared_slack_channel_url: project.external_slack_channel_url,
                    shared_notion_page_url: project.shared_notion_page_url,
                },
                &code_for_resources,
                role,
            ),
            can_configure: role.is_lawyer_tier(),
            project_code: code_for_resources.clone(),
        },
        xero_invoice_url,
        repository_url,
        estate,
        participations,
        documents,
        asset_kind_choices: asset_kind_choices(),
        csrf_token,
        calendar_sort,
        calendar_dir,
        role,
        logo,
    })
}

/// Commit a `500` and wrap a query error — an unavailable workbench is a server
/// error, not a `200` with an error body.
#[cfg(feature = "server")]
fn server_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

/// Commit the `404` the handler returned for a matter the caller cannot see
/// and return an empty (nameless) view.
#[cfg(feature = "server")]
fn not_found(
    id: uuid::Uuid,
    role: ViewerRole,
    logo: Option<crate::components::AppLogo>,
    csrf_token: String,
) -> LawyerDetailView {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::NOT_FOUND,
        None,
    );
    LawyerDetailView {
        id: id.to_string(),
        role,
        logo,
        csrf_token,
        ..LawyerDetailView::default()
    }
}

/// The lawyer matter-detail workbench, server-side rendered.
#[component]
pub fn LawyerProjectDetail() -> Element {
    let resource = use_server_future(get_lawyer_project_detail)?;

    let view = match &*resource.read() {
        Some(Ok(view)) if !view.name.is_empty() => view.clone(),
        Some(Ok(_)) => {
            return rsx! {
                main { id: "lawyer-project", p { "That matter was not found." } }
            }
        }
        Some(Err(_)) => {
            return rsx! {
                main { id: "lawyer-project", p { "Failed to load this matter." } }
            }
        }
        None => {
            return rsx! {
                main { id: "lawyer-project", p { "Loading…" } }
            }
        }
    };

    let is_admin = view.role.is_admin_tier();
    let csrf = view.csrf_token.clone();
    // Precompute the header's optional fields — a dash for the absent ones —
    // rather than interpolate method chains inside an rsx format string.
    let dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".to_string());
    let entity_disp = dash(&view.entity_name);
    // Each side reads as one line whatever its size, so a matter with two
    // accountable lawyers says so in the place a reader already looks.
    let names = |v: &[String]| {
        if v.is_empty() {
            "—".to_string()
        } else {
            v.join(", ")
        }
    };
    let lawyer_dri_disp = names(&view.lawyer_dris);
    let client_dri_disp = names(&view.client_dris);
    // Who may govern each side, mirroring `store::participation::authorize` so
    // the page renders exactly the controls the command would honour.
    let may_govern_lawyer_side = is_admin || view.viewer_is_lawyer_dri;
    let may_govern_client_side = view.role.is_lawyer_tier();

    rsx! {
        document::Title { "{view.name} — Project" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "lawyer-project", class: "nav-theme lawyer-detail",
            header { class: "page-header",
                h1 { "{view.name}" }
                p { class: "nav-muted",
                    "Code: " code { "{view.code}" }
                    " · Status: {view.status}"
                    " · Entity: {entity_disp}"
                    if let Some(entity_id) = view.entity_id.as_ref() {
                        " · "
                        a { class: "nav-link", href: "/lawyer/entities/{entity_id}/edit", "Edit entity" }
                    }
                    " · Lawyer DRI: {lawyer_dri_disp}"
                    " · Client DRI: {client_dri_disp}"
                    " · "
                    a { class: "nav-link", href: "/app/projects/{view.code}/edit", "Edit project" }
                }
                form {
                    class: "lawyer-detail__inline-form",
                    method: "post",
                    action: "/app/projects/{view.code}/view-as-client",
                    input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                    button { class: "nav-btn nav-btn--secondary", r#type: "submit", "View as Client" }
                }
            }

            if let Some(estate) = view.estate.as_ref() {
                EstateSection { project_code: view.code.clone(), estate: estate.clone(), csrf_token: csrf.clone() }
            }

            crate::project_resources::ProjectResourcesPanel { view: view.resources.clone() }

            if view.xero_invoice_url.is_some() || view.repository_url.is_some() {
                section { class: "lawyer-detail__section project-integrations",
                    h2 { "Integrations" }
                    p { class: "lawyer-detail__integration-links",
                        if let Some(url) = view.xero_invoice_url.as_ref() {
                            a {
                                class: "nav-btn nav-btn--secondary",
                                href: "{url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                "Xero"
                            }
                        }
                        if let Some(url) = view.repository_url.as_ref() {
                            a {
                                class: "nav-btn nav-btn--secondary",
                                href: "{url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                "Source repository"
                            }
                        }
                    }
                }
            }

            // This matter's slice of the workbench calendar. Empty for the same
            // reason that one is (#350): the page must not pass its documents,
            // participations, or notations off as scheduled events.
            crate::project_calendar::ProjectCalendar {
                section_class: "lawyer-detail__section project-calendar".to_string(),
                heading: "Calendar".to_string(),
                empty_message: "No calendar events scheduled for this matter.".to_string(),
                columns: crate::project_calendar::MATTER_COLUMNS.to_vec(),
                path: format!("/app/projects/{}", view.code),
                query_prefix: String::new(),
                sort: view.calendar_sort.clone(),
                dir: view.calendar_dir.clone(),
            }

            section { class: "lawyer-detail__section project-participations",
                div { class: "lawyer-detail__section-head",
                    h2 { "Matter people" }
                    if is_admin {
                        a { class: "nav-btn nav-btn--primary", href: "/app/projects/{view.code}/people/new", "Add person" }
                    }
                }
                p { class: "nav-muted", "Participation records who is assigned to this matter and follows each person's system tier. Adding or removing someone here does not change that tier." }
                if view.participations.is_empty() {
                    p { class: "projects-empty", "No people are assigned to this matter yet." }
                } else {
                    div { class: "nav-table-wrap",
                        table { class: "nav-table",
                            thead {
                                tr {
                                    th { scope: "col", "Person" }
                                    th { scope: "col", "System tier" }
                                    th { scope: "col", "Participation" }
                                    th { scope: "col", "Accountability" }
                                    if is_admin {
                                        th { scope: "col", class: "nav-table__end", "" }
                                    }
                                }
                            }
                            tbody {
                                for row in view.participations.iter() {
                                    tr {
                                        td {
                                            strong { "{row.person_name}" }
                                            " "
                                            span { class: "nav-muted", "<" "{row.person_email}" ">" }
                                        }
                                        td { span { class: "status-chip", "{row.person_role}" } }
                                        td { code { "{row.participation}" } }
                                        // The accountability marker rides the
                                        // participation row, so the ledger is
                                        // where it belongs on the page — and so
                                        // is the control that moves it. Each
                                        // side is a set, so the control adds and
                                        // removes rather than reassigning.
                                        td { class: "matter-dri-cell",
                                            if row.is_lawyer_dri {
                                                span { class: "status-chip", "Lawyer DRI" }
                                            } else if row.is_client_dri {
                                                span { class: "status-chip", "Client DRI" }
                                            } else {
                                                span { class: "nav-muted", "—" }
                                            }
                                            {
                                                // A firm-side row answers to the
                                                // lawyer rule, a client-side row
                                                // to the client rule; the tier on
                                                // the row is what decides which.
                                                let firm_side = !row.participation.eq("client");
                                                let may = if firm_side { may_govern_lawyer_side } else { may_govern_client_side };
                                                let holds = row.is_lawyer_dri || row.is_client_dri;
                                                rsx! {
                                                    if may {
                                                        " "
                                                        form {
                                                            class: "lawyer-detail__inline-form",
                                                            method: "post",
                                                            action: if holds {
                                                                format!("/app/projects/{}/people/{}/dri/remove", view.code, row.id)
                                                            } else {
                                                                format!("/app/projects/{}/people/{}/dri", view.code, row.id)
                                                            },
                                                            input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                                                            button {
                                                                class: "nav-btn nav-btn--secondary matter-dri-toggle",
                                                                r#type: "submit",
                                                                if holds { "Remove DRI" } else { "Make DRI" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if is_admin {
                                            td { class: "nav-table__end",
                                                a {
                                                    class: "nav-btn nav-btn--secondary",
                                                    href: "/app/projects/{view.code}/people/{row.id}/edit",
                                                    "Edit"
                                                }
                                                " "
                                                form {
                                                    class: "lawyer-detail__inline-form",
                                                    method: "post",
                                                    action: "/app/projects/{view.code}/people/{row.id}/delete",
                                                    input { r#type: "hidden", name: "_csrf", value: "{csrf}" }
                                                    button { class: "nav-btn nav-btn--danger", r#type: "submit", "Remove" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            section { class: "lawyer-detail__section project-documents",
                h2 { "Documents" }
                if view.documents.is_empty() {
                    p { class: "projects-empty", "No documents yet." }
                } else {
                    div { class: "nav-table-wrap",
                        table { class: "nav-table",
                            thead { tr { th { scope: "col", "Filename" } th { scope: "col", "Download" } } }
                            tbody {
                                for doc in view.documents.iter() {
                                    tr {
                                        td { a { class: "nav-link", href: "/app/projects/{view.code}/documents/{doc.id}", "{doc.filename}" } }
                                        td { a { class: "nav-link", href: "/app/projects/{view.code}/documents/{doc.id}/download", "Download" } }
                                    }
                                }
                            }
                        }
                    }
                }
                // Real-time upload progress: `upload-progress.js` finds the
                // form via this field's id and replays the native submit as
                // an XHR so it can render `upload.loaded` / `upload.total`
                // as they arrive — a plain `Field::file(...)` id defaults to
                // its `name` ("file"), which the Estate transcript uploader
                // below also uses, so this needs its own id to stay unique.
                document::Script { src: "/public/js/upload-progress.js", defer: true }
                FormCard {
                    title: "Upload documents".to_string(),
                    action: "/app/projects/{view.code}/documents/upload",
                    submit_label: "Upload".to_string(),
                    heading: Heading::H2,
                    multipart: true,
                    csrf_token: Some(csrf.clone()),
                    fields: vec![
                        Field::file("Files", "file").id("document-upload-file").required().multiple()
                            .help("Select one file or several — each is filed as its own document."),
                        Field::select(
                            "Kind",
                            "kind",
                            view.asset_kind_choices
                                .iter()
                                .map(|(value, label)| crate::components::Choice::new(value.clone(), label.clone()))
                                .collect(),
                            Some("unclassified".to_string()),
                        )
                        .help("Defaults to unclassified. Applies to every file in this batch."),
                        Field::text("Description", "description", "")
                            .placeholder("Letter from Acme Bank dated 2026-05-23")
                            .help("Optional. Applies to every file in this batch."),
                        Field::select("Visibility", "visibility",
                            vec![
                                crate::components::Choice::new("internal", "Internal (lawyer only)"),
                                crate::components::Choice::new("client", "Client-visible"),
                            ],
                            Some("internal".to_string()))
                            .help("Applies to every file in this batch. Internal work product — memos, drafts — stays internal."),
                    ],
                }
            }

            // Closing a matter is bespoke: it is asked for by email and opened
            // by the lawyer DRI, never fired from a button here. Every firm
            // participant reads where to ask — the accountability marker
            // decides who acts on the request, not who may raise it.
            if view.status == "open" {
                section { class: "lawyer-detail__section project-close",
                    p { class: "nav-muted",
                        "To close this matter, email the lawyer DRI"
                        if !view.lawyer_dris.is_empty() {
                            " ({lawyer_dri_disp})"
                        }
                        " or "
                        a { class: "nav-link", href: "mailto:support@neonlaw.com", "support@neonlaw.com" }
                        ". Closing is bespoke: a lawyer DRI opens the offboarding-letter walk, and signing the offboarding letter marks the matter complete."
                    }
                    // A matter always has at least one lawyer DRI — they are who
                    // close it. An empty set is therefore a gap to fill, not a
                    // neutral blank, so the workbench says so and links to the
                    // form that fixes it.
                    if view.lawyer_dris.is_empty() && is_admin {
                        p { class: "nav-form-error", role: "alert",
                            "This matter has no lawyer DRI. "
                            a { class: "nav-link", href: "/app/projects/{view.code}/people/new", "Designate the accountable lawyer" }
                            "."
                        }
                    }
                }
            }
        }
    }
}

/// The Northstar estate section: the workflow state and the stage-appropriate
/// control — the transcript uploader at `BEGIN`, the generated drafts and a
/// release control at `lawyer_review`, a waiting note at `client_review`.
#[component]
fn EstateSection(project_code: String, estate: EstateData, csrf_token: String) -> Element {
    rsx! {
        section { class: "lawyer-detail__section project-estate",
            h2 { "Estate plan — Northstar" }
            p { "Workflow state: " strong { class: "estate-state", "{estate.state}" } }
            if estate.state == "BEGIN" {
                p { class: "nav-muted", "The sitting is recorded offline and transcribed. File it here in whichever form you have — you can do this from a phone. Paste the transcript text, upload a transcript file, or paste a link to the recording." }
                FormCard {
                    title: "File the sitting transcript".to_string(),
                    action: "/app/projects/{project_code}/notations/{estate.notation_id}/transcript",
                    submit_label: "File transcript".to_string(),
                    heading: Heading::H2,
                    multipart: true,
                    csrf_token: Some(csrf_token.clone()),
                    fields: vec![
                        Field::textarea("Paste the transcript", "transcript_text", "", 8).help("Paste the transcribed sitting here."),
                        Field::file("…or upload a transcript file", "file"),
                        Field::text("…or paste a link to the recording", "link", "").placeholder("https://…").help("A link to the recording or transcript."),
                    ],
                }
            } else {
                if estate.drafts.is_empty() {
                    p { class: "nav-muted", "The transcript has been filed. The drafts are being prepared from the sitting." }
                } else {
                    h3 { "Generated drafts" }
                    div { class: "portal-agreements",
                        for draft in estate.drafts.iter() {
                            div { class: "portal-agreement",
                                span {
                                    "{draft.title}"
                                    span { class: "status-chip", " {draft.kind}" }
                                }
                                span { class: "status-chip", "{draft.status}" }
                            }
                        }
                    }
                    if estate.state == "lawyer_review" {
                        p { class: "nav-muted", "Releasing advances the matter to client review and makes each draft readable on the client's review surface. Nothing reaches the client until you do this." }
                        FormCard {
                            title: "Approve & release drafts to the client".to_string(),
                            action: "/lawyer/notations/{estate.notation_id}/release-drafts",
                            submit_label: "Release drafts to client".to_string(),
                            heading: Heading::H2,
                            csrf_token: Some(csrf_token.clone()),
                            fields: vec![],
                        }
                    } else if estate.state == "client_review" {
                        p { class: "nav-muted", "Released to the client. Waiting for the client to read each draft and approve the plan." }
                    }
                }
            }
        }
    }
}
