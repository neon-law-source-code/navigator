//! Lawyer projects list as a Dioxus component (#641 Phase 3, projects cluster) —
//! the lawyer workbench matter directory.
//!
//! The successor to the `projects_index` render. A sortable table of matters,
//! each row carrying its resolved entity name and the matter-lifecycle status
//! pill (`store::projects::matter_lifecycle`, which itself folds in
//! `store::projects::matter_flags`'s missing-onboarding signal — the pill is
//! the one place that flag surfaces now; see [`get_project_list`]).
//!
//! Owner and Admin read every matter here (`store::projects::all`), not only
//! their own: this is the administrative *listing* surface
//! (`webapp::admin_listing::MatterScope::Unscoped` names the same idea
//! elsewhere), not the matter surface. Every firm tier needs a firm-side
//! participation row before `/app/projects/{code}` renders matter content;
//! Owner/Admin route admission and administrative-listing reach do not bypass
//! that gate. `store::access::visible_projects_as_lawyer` is the scoped read
//! for the matter surface.
//! The resolved entity-name column and the pill are computed server-side, so
//! all five sort columns (`code` / `name` / `status` / `entity_name` /
//! `created_at`) sort in one in-memory composite comparator. The "Add project"
//! control links to the `/app/projects/new` create page, which remains an
//! Axum form route. A `Last commit` column is fixed (not sortable) and reads
//! live from GitHub via [`last_committed_at`] — best-effort, degrading to an
//! em dash rather than blocking the render.
//!
//! Two tabs, one list: `/app/projects` (Open, the default) and
//! `/app/projects/closed` filter the same query by [`ProjectListScope`] —
//! see `portal::dioxus_app::projects_router` / `projects_closed_router`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;
use crate::portal_project_list::PersonId;

/// Which lifecycle lens the projects list renders: the open matters (the
/// default, `/app/projects`) or the closed ones (`/app/projects/closed`).
/// Set once per route mount via `Extension` — see
/// `portal::dioxus_app::projects_router` / `projects_closed_router` — not
/// derived from the URL inside the loader, so a direct hit on the generated
/// `#[server]` endpoint still defaults to Open.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectListScope {
    #[default]
    Open,
    Closed,
}

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
    /// `store::projects::Project::inserted_at` (RFC 3339) — when the matter
    /// was opened.
    pub created_at: String,
    /// The committer date (RFC 3339) of the tip commit on the Project's
    /// GitHub-hosted repository's default branch, fetched live from
    /// `cloud::forge::ForgeService::head_commit_committed_at` on every
    /// render. `None` when the matter has no repository, its repository is
    /// not under this deployment's configured GitHub organization, or the
    /// live fetch failed — a missing value is not distinguished from an
    /// unreachable one, since neither is actionable from this list.
    pub last_committed_at: Option<String>,
    /// A `closed` matter with no offboarding letter on file — surfaced as a
    /// warning badge. (The matching "missing onboarding" signal already has a
    /// home: the `lifecycle_*` fields below fold it into the status pill
    /// instead of a second badge, since an open matter missing onboarding
    /// and an open matter needing onboarding were always the same fact.)
    pub missing_offboarding_letter: bool,
    /// `store::projects::MatterLifecycle::class()` for this row — the
    /// yellow/green/red indicator's CSS class. Computed server-side since
    /// `MatterLifecycle` is not a wasm-safe type.
    pub lifecycle_class: String,
    /// `store::projects::MatterLifecycle::label()` for this row — the text
    /// label that accompanies the colour, so the state never rests on colour
    /// alone.
    pub lifecycle_label: String,
    /// `store::projects::MatterLifecycle::title()` for this row — the hover
    /// and assistive text stating what the indicator did and did not verify.
    pub lifecycle_title: String,
}

/// The rendered lawyer projects list: the rows, the active `?sort=`, and the
/// viewer's tier (for the nav chrome).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectListView {
    pub rows: Vec<ProjectRow>,
    pub sort: String,
    pub role: ViewerRole,
    /// Which tab this render answers — Open (`/app/projects`) or Closed
    /// (`/app/projects/closed`) — so the page highlights the active tab and
    /// keeps sort links on the same tab.
    #[serde(default)]
    pub scope: ProjectListScope,
    /// The deploy's brand mark for the navbar. `None` when the mounted brand
    /// configures none.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
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
    last_committed_at: Option<String>,
) -> ProjectRow {
    let (missing_onboarding, missing_offboarding_letter) =
        store::projects::matter_flags(has_engagement, &m.status, has_closing);
    let lifecycle = store::projects::matter_lifecycle(
        &m.status,
        missing_onboarding,
        missing_offboarding_letter,
    );
    ProjectRow {
        entity_name,
        id: m.id.to_string(),
        code: m.code,
        name: m.name,
        created_at: m.inserted_at,
        last_committed_at,
        status: m.status,
        missing_offboarding_letter,
        lifecycle_class: lifecycle.class().to_string(),
        lifecycle_label: lifecycle.label().to_string(),
        lifecycle_title: lifecycle.title().to_string(),
    }
}

/// Read the injected [`ProjectListScope`] extension, defaulting to Open when
/// the request carried none (a direct hit on the generated `#[server]`
/// endpoint need not run behind either route mount's layer).
#[cfg(feature = "server")]
async fn injected_projects_scope() -> ProjectListScope {
    dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ProjectListScope>, _>()
        .await
        .map(|axum::Extension(scope)| scope)
        .unwrap_or_default()
}

/// One composite comparator so the first requested `?sort=` field is primary
/// and later fields only break ties (the JSON:API `SortSpec` precedence
/// contract).
#[cfg(feature = "server")]
fn sort_matters(
    matters: &mut [store::projects::Project],
    parsed: &[(String, bool)],
    by_entity: impl Fn(uuid::Uuid) -> String,
) {
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
                        "created_at" => a.inserted_at.cmp(&b.inserted_at),
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
}

/// Every matter's live [`last_committed_at`], fetched concurrently so N rows
/// cost one round trip's latency, not N. Missing forge configuration is a
/// skip, not an error — the same rule `project_surfaces::reconcile_from_env`
/// applies to the same forge.
#[cfg(feature = "server")]
async fn fetch_last_committed_ats(matters: &[store::projects::Project]) -> Vec<Option<String>> {
    let forge = cloud::forge::GitHubForge::from_env().ok();
    let workspace = cloud::workspace::WorkspaceConfig::from_env().ok();
    futures::future::join_all(matters.iter().map(|m| {
        let forge = forge.as_ref();
        let workspace = workspace.as_ref();
        async move { last_committed_at(forge, workspace, m).await }
    }))
    .await
}

/// The Project's live HEAD commit's committer date, best-effort: `None` when
/// the matter has no `repository_url`, when that URL is not this
/// deployment's own GitHub organization (an external or manually-entered
/// forge is not reachable through the deployment's configured token), or when
/// the live fetch itself fails. A per-row forge fault degrades the column,
/// not the whole list.
#[cfg(feature = "server")]
async fn last_committed_at(
    forge: Option<&cloud::forge::GitHubForge>,
    workspace: Option<&cloud::workspace::WorkspaceConfig>,
    project: &store::projects::Project,
) -> Option<String> {
    use cloud::forge::ForgeService;

    let forge = forge?;
    let workspace = workspace?;
    let repository_url = project.repository_url.as_deref()?;
    if repository_url != workspace.expected_repository_url(&project.code) {
        return None;
    }
    forge.head_commit_committed_at(&project.code).await.ok()?
}

/// Fetch the lawyer projects list for the current request: refuse non-lawyer,
/// resolve each matter's entity name and lifecycle badge, and sort in memory
/// (one composite comparator so the first requested `?sort=` field is
/// primary). The lifecycle lookup errors propagate rather than badging every
/// matter as missing its onboarding.
///
/// Owner and Admin read [`store::projects::all`] — every matter in the
/// deployment — the same way `reconcile_project_repositories_door` does for
/// its own administrative question: privileged reach is a place you navigate
/// to, not a silent widening of the matter surface. An ordinary Lawyer still
/// reads through `store::access::visible_projects_as_lawyer`, which grants no
/// such bypass.
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
    let scope = injected_projects_scope().await;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<ProjectListQuery>,
        _,
    >()
    .await?;
    let sort = query.sort.unwrap_or_default();
    let parsed = parse_sort(&sort);

    // The scoped lawyer-lens read below takes a store role; map the injected
    // wasm-safe tier back to it.
    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let mut matters = if role.is_admin_tier() {
        store::projects::all(&surreal).await.map_err(loader_error)?
    } else {
        store::access::visible_projects_as_lawyer(&surreal, person_id, store_role)
            .await
            .map_err(loader_error)?
    };

    // The tab is the filter: Open hides every terminal matter (the common
    // case — a firm tier working the docket does not want every inactive
    // matter of the deployment's history in the way), Closed shows only
    // them. "Terminal" is both lifecycle end states `store::projects`
    // documents (`open` → `closed` → `archived`, reachable via
    // `store::projects::Transition`), not just `closed` — an archived
    // matter is exactly as inactive as a closed one, so it belongs on the
    // same tab rather than being invisible from both.
    matters.retain(|m| {
        let is_terminal =
            m.status.eq_ignore_ascii_case("closed") || m.status.eq_ignore_ascii_case("archived");
        match scope {
            ProjectListScope::Open => !is_terminal,
            ProjectListScope::Closed => is_terminal,
        }
    });

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

    sort_matters(&mut matters, &parsed, by_entity);
    let last_committed_ats = fetch_last_committed_ats(&matters).await;

    let rows = matters
        .into_iter()
        .zip(last_committed_ats)
        .map(|(m, last_committed_at)| {
            let entity_name = by_entity(m.entity_id);
            let has_eng = has_engagement.contains(&m.id);
            let has_close = has_closing.contains(&m.id);
            project_row(entity_name, m, has_eng, has_close, last_committed_at)
        })
        .collect();

    Ok(ProjectListView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        rows,
        sort,
        role,
        scope,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
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
    let is_closed_tab = view.scope == ProjectListScope::Closed;
    let base_path = if is_closed_tab {
        "/app/projects/closed"
    } else {
        "/app/projects"
    };
    let open_tab_class = if is_closed_tab {
        "nav-tab"
    } else {
        "nav-tab is-active"
    };
    let closed_tab_class = if is_closed_tab {
        "nav-tab is-active"
    } else {
        "nav-tab"
    };
    let empty_message = if is_closed_tab {
        "No closed projects."
    } else {
        "No projects yet."
    };
    let columns = vec![
        Column::sortable("code", "Code"),
        Column::sortable("name", "Name"),
        Column::sortable("status", "Status"),
        Column::sortable("entity_name", "Entity"),
        Column::sortable("created_at", "Created"),
        Column::fixed("last_committed_at", "Last commit"),
    ];
    let error = view.error.clone();
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.firm_name} | Lawyer | Projects" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(view.role),
            logo: view.logo.clone(),
        }
        main { id: "projects", class: "nav-theme",
            header { class: "page-header",
                h1 { "Projects" }
                p { a { class: "nav-btn nav-btn--primary", href: "/app/projects/new", "Add project" } }
            }
            nav { class: "nav-tabs", aria_label: "Project status",
                a { class: "{open_tab_class}", href: "/app/projects", "Open" }
                a { class: "{closed_tab_class}", href: "/app/projects/closed", "Closed" }
            }
            if let Some(error) = error.as_ref() {
                p { class: "nav-form-error", role: "alert", "{error}" }
            }
            if is_empty {
                p { class: "projects-empty",
                    "{empty_message} "
                    if !is_closed_tab {
                        a { href: "/app/projects/new", "Add the first." }
                    }
                }
            } else {
                DataTable {
                    columns,
                    sort,
                    base_path: base_path.to_string(),
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
                                if row.missing_offboarding_letter {
                                    " "
                                    span { class: "matter-flag",
                                        title: "This closed project has no offboarding letter on file.",
                                        "no offboarding letter"
                                    }
                                }
                            }
                            td { class: "project-status",
                                span {
                                    class: "{row.lifecycle_class}",
                                    title: "{row.lifecycle_title}",
                                    "{row.lifecycle_label}"
                                }
                            }
                            td { class: "project-entity", "{row.entity_name}" }
                            td { class: "project-created-at", "{row.created_at}" }
                            td { class: "project-last-committed-at",
                                {row.last_committed_at.clone().unwrap_or_else(|| "—".to_string())}
                            }
                        }
                    }
                }
            }
        }
    }
}
