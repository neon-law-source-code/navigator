//! Lawyer projects list as a Dioxus component (#641 Phase 3, projects cluster) —
//! the lawyer workbench matter directory.
//!
//! The successor to the `projects_index` render. A sortable table of the
//! matters visible through the lawyer lens (`store::access::visible_projects_as_lawyer`
//! — admin sees all, lawyer sees participated matters), each row carrying its
//! resolved entity name and the two matter-lifecycle warning badges
//! (`store::projects::matter_flags`: missing retainer, missing closing letter).
//! The resolved entity-name column and the badges are computed server-side, so
//! all four sort columns (`code` / `name` / `status` / `entity_name`) sort in
//! one in-memory composite comparator. The "Add project" control links to the
//! `/app/projects/new` create page, which remains an Axum form route.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;
use crate::portal_project_list::PersonId;

/// One matter row, in a wasm-safe shape (plain fields — no `store`/`SeaORM`
/// types cross to the client build).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ProjectRow {
    pub id: String,
    pub code: String,
    pub name: String,
    pub status: String,
    /// The resolved entity (matter owner) name; `?` when the FK does not resolve.
    pub entity_name: String,
    /// The matter has no matter-opening engagement — surfaced as a warning badge.
    pub missing_retainer: bool,
    /// A `closed` matter with no closing letter — surfaced as a warning badge.
    pub missing_closing_letter: bool,
    /// `store::projects::MatterLifecycle::class()` for this row — the
    /// yellow/green/red indicator's CSS class. Computed server-side since
    /// `MatterLifecycle` is not a wasm-safe type.
    pub lifecycle_class: String,
    /// `store::projects::MatterLifecycle::label()` for this row — the text
    /// label that accompanies the colour, so the state never rests on colour
    /// alone.
    pub lifecycle_label: String,
}

/// The rendered lawyer projects list: the rows, the active `?sort=`, and the
/// viewer's tier (for the nav chrome).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectListView {
    pub rows: Vec<ProjectRow>,
    pub sort: String,
    pub role: ViewerRole,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The `?error=` flash surfaced above the table — set when a matter delete
    /// or a participation removal is refused (dependents still reference the
    /// matter, or the lawyer-DRI lockout) and the handler redirects back here.
    /// `None` on a plain visit.
    #[serde(default)]
    pub error: Option<String>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// The projects list `?sort=` query, plus the `?error=` flash a refused delete
/// redirects back with.
#[derive(Deserialize, Default)]
pub struct ProjectListQuery {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Turn a loader query failure into a `ServerFnError`, committing a real `500`
/// first so an unavailable matter directory is reported as a server error
/// rather than a `200` with an error body — the explicit server error the
/// retired `projects_index` handler returned. `use_server_future` still
/// renders the error branch, now under the committed status (the status commits
/// before the initial chunk, exactly as the `person_show`/`entity_edit` 404s do).
#[cfg(feature = "server")]
fn loader_error(e: impl std::fmt::Display) -> ServerFnError {
    dioxus_fullstack_core::FullstackContext::commit_http_status(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        None,
    );
    ServerFnError::new(e.to_string())
}

/// Parse a JSON:API `sort` value into `(key, descending)` pairs. Server-only.
#[cfg(feature = "server")]
fn parse_sort(raw: &str) -> Vec<(String, bool)> {
    raw.split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "-")
        .map(|segment| match segment.strip_prefix('-') {
            Some(key) => (key.to_string(), true),
            None => (segment.to_string(), false),
        })
        .collect()
}

/// Build one rendered row from a matter and its lifecycle facts: the two
/// diligence flags ([`store::projects::matter_flags`]) plus the yellow/green/red
/// indicator ([`store::projects::matter_lifecycle`]) they feed.
#[cfg(feature = "server")]
fn project_row(
    entity_name: String,
    m: store::projects::Project,
    has_engagement: bool,
    has_closing: bool,
) -> ProjectRow {
    let (missing_retainer, missing_closing_letter) =
        store::projects::matter_flags(has_engagement, &m.status, has_closing);
    let lifecycle =
        store::projects::matter_lifecycle(&m.status, missing_retainer, missing_closing_letter);
    ProjectRow {
        entity_name,
        id: m.id.to_string(),
        code: m.code,
        name: m.name,
        status: m.status,
        missing_retainer,
        missing_closing_letter,
        lifecycle_class: lifecycle.class().to_string(),
        lifecycle_label: lifecycle.label().to_string(),
    }
}

/// Fetch the lawyer projects list for the current request: refuse non-lawyer,
/// scope the matters through the lawyer lens, resolve each matter's entity name
/// and lifecycle badges, and sort in memory (one composite comparator so the
/// first requested `?sort=` field is primary). The lifecycle lookup errors
/// propagate rather than badging every matter as missing its retainer.
#[server]
pub async fn get_project_list() -> Result<ProjectListView, ServerFnError> {
    // A non-lawyer caller (client / clerk) gets the `projects_index` handler's
    // 404, not the generated `#[server]` endpoint's default 200 error state — the
    // lawyer workbench is hidden from them, not merely refused. `require_lawyer`
    // returns `Err` without a status, so gate here and commit the 404 explicitly
    // (exactly as `loader_error` commits its 500).
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();
    if !role.is_lawyer_tier() {
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::NOT_FOUND,
            None,
        );
        return Err(ServerFnError::new("not found"));
    }
    let PersonId(person_id) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<PersonId>, _>()
            .await
            .map(|axum::Extension(id)| id)
            .unwrap_or_default();
    let person_id = person_id.and_then(|raw| raw.parse::<uuid::Uuid>().ok());
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ProjectListQuery>,
        _,
    >()
    .await?;
    let sort = query.sort.unwrap_or_default();
    let parsed = parse_sort(&sort);

    // The lawyer lens takes a store role for the admin bypass; map the injected
    // wasm-safe tier back to it.
    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let mut matters = store::access::visible_projects_as_lawyer(&surreal, person_id, store_role)
        .await
        .map_err(loader_error)?;

    let entities = store::entities::all(&surreal).await.map_err(loader_error)?;
    let by_entity = |id: uuid::Uuid| {
        entities
            .iter()
            .find(|e| e.id == id)
            .map_or("?", |e| e.name.as_str())
            .to_string()
    };

    // Lifecycle badges: two batched queries. A failed lookup propagates rather
    // than collapsing to "no engagement" and badging every matter falsely.
    let (has_engagement, has_closing) = store::projects::matter_lifecycle_sets(&surreal, &matters)
        .await
        .map_err(loader_error)?;

    // One composite comparator so the first requested field is primary and later
    // fields only break ties (the JSON:API `SortSpec` precedence contract).
    matters.sort_by(|a, b| {
        parsed
            .iter()
            .fold(std::cmp::Ordering::Equal, |acc, (key, descending)| {
                acc.then_with(|| {
                    let ordering = match key.as_str() {
                        "code" => a.code.cmp(&b.code),
                        "name" => a.name.cmp(&b.name),
                        "status" => a.status.cmp(&b.status),
                        "entity_name" => by_entity(a.entity_id).cmp(&by_entity(b.entity_id)),
                        _ => std::cmp::Ordering::Equal,
                    };
                    if *descending {
                        ordering.reverse()
                    } else {
                        ordering
                    }
                })
            })
    });

    let rows = matters
        .into_iter()
        .map(|m| {
            let entity_name = by_entity(m.entity_id);
            let has_eng = has_engagement.contains(&m.id);
            let has_close = has_closing.contains(&m.id);
            project_row(entity_name, m, has_eng, has_close)
        })
        .collect();

    Ok(ProjectListView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        sort,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        error: query.error.filter(|message| !message.is_empty()),
    })
}

/// The lawyer projects list. Server-side rendered with the sorted rows already in
/// the markup; the sort headers are real anchors, each row links to the matter
/// detail, and the lifecycle warnings render as badges.
#[component]
pub fn LawyerProjects() -> Element {
    let resource = use_server_future(get_project_list)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "projects", p { "Failed to load projects." } }
            }
        }
        None => {
            return rsx! {
                main { id: "projects", p { "Loading…" } }
            }
        }
    };

    let sort = SortState::parse(Some(&view.sort));
    let columns = vec![
        Column::sortable("code", "Code"),
        Column::sortable("name", "Name"),
        Column::sortable("status", "Status"),
        Column::sortable("entity_name", "Entity"),
    ];
    let error = view.error.clone();
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Projects" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "projects", class: "nav-theme",
            header { class: "page-header",
                h1 { "Projects" }
                p { a { class: "nav-btn nav-btn--primary", href: "/app/projects/new", "Add project" } }
            }
            if let Some(error) = error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            if is_empty {
                p { class: "projects-empty",
                    "No matters yet. "
                    a { href: "/app/projects/new", "Add the first." }
                }
            } else {
                DataTable {
                    columns,
                    sort,
                    base_path: "/app/projects".to_string(),
                    for row in view.rows.iter() {
                        tr { class: "project-row",
                            td { class: "project-code",
                                a {
                                    class: "nav-link",
                                    href: "/app/projects/{row.code}",
                                    "data-action": "view",
                                    "aria-label": "View details for {row.name}",
                                    "{row.code}"
                                }
                            }
                            td { class: "project-name",
                                "{row.name}"
                                if row.missing_retainer {
                                    " "
                                    span { class: "matter-flag",
                                        title: "This matter has no onboarding notation — it was never opened on a retainer.",
                                        "no retainer"
                                    }
                                }
                                if row.missing_closing_letter {
                                    " "
                                    span { class: "matter-flag",
                                        title: "This closed matter has no offboarding letter.",
                                        "no offboarding letter"
                                    }
                                }
                            }
                            td { class: "project-status",
                                span {
                                    class: "{row.lifecycle_class}",
                                    title: "Lifecycle: {row.lifecycle_label}",
                                    "{row.lifecycle_label}"
                                }
                            }
                            td { class: "project-entity", "{row.entity_name}" }
                        }
                    }
                }
            }
        }
    }
}
