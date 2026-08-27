//! Lawyer projects list as a Dioxus component (#641 Phase 3, projects cluster) —
//! the lawyer workbench matter directory.
//!
//! The successor to the `projects_index` render. A sortable table of the
//! matters visible through the lawyer lens (`store::access::visible_projects_as_lawyer`
//! — admin sees all, lawyer sees participated matters), each row carrying its
//! resolved entity name, its lifecycle indicator
//! (`store::projects::MatterLifecycle`: awaiting an engagement letter, engaged,
//! or closed) and, on a closed matter that still owes one, the outstanding
//! closing-letter badge (`store::projects::matter_flags`).
//! The resolved entity-name column and the badges are computed server-side, so
//! all four sort columns (`code` / `name` / `status` / `entity_name`) sort in
//! one in-memory composite comparator. The "Add project" control links to the
//! `/app/projects/new` create page, which remains an Axum form route.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;
use crate::portal_project_list::PersonId;

/// Where a matter sits on its lifecycle track, in a wasm-safe shape.
///
/// The rule is `store::projects::MatterLifecycle`, which the server-side loader
/// computes and converts into this. The client build cannot reach `store`, so
/// the variants are mirrored rather than re-derived — re-deriving the rule in
/// the view is exactly the drift that put `matter_flags` in the store in the
/// first place. `the_view_tone_mirrors_the_store_lifecycle` pins the two
/// together.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum MatterTone {
    /// Open, no engagement letter on file.
    Awaiting,
    /// Open, engagement letter on file.
    Engaged,
    /// Closed.
    Closed,
}

impl MatterTone {
    /// The indicator's class. Deliberately exhaustive: a fourth state fails to
    /// compile until it declares how it renders.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            Self::Awaiting => "matter-lifecycle matter-lifecycle--awaiting",
            Self::Engaged => "matter-lifecycle matter-lifecycle--engaged",
            Self::Closed => "matter-lifecycle matter-lifecycle--closed",
        }
    }

    /// The words on the indicator.
    ///
    /// Colour never travels alone: a red/green distinction carried by hue is
    /// invisible to the most common form of colour blindness, so each state
    /// says what it is in text beside the colour rather than instead of it.
    ///
    /// `Engaged` states the fact the store checked — a letter is **on file** —
    /// and never "live", "active", or "papered". Those are conclusions about
    /// the representation that a filed document does not establish, and this
    /// indicator asserts where the warning badge it replaces only rendered on
    /// absence. `store::projects::MatterLifecycle` carries the full reasoning.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Awaiting => "no engagement letter",
            Self::Engaged => "engagement letter on file",
            Self::Closed => "closed",
        }
    }

    /// The indicator's `title` — what was consulted, and what it does not claim.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Awaiting => {
                "No engagement letter is on file for this matter — no onboarding notation, and no uploaded document classified as one."
            }
            Self::Engaged => {
                "An engagement letter is on file for this matter, as an onboarding notation or an uploaded document classified as one. Filed, not verified as executed."
            }
            Self::Closed => "This matter is closed.",
        }
    }
}

#[cfg(feature = "server")]
impl From<store::projects::MatterLifecycle> for MatterTone {
    fn from(lifecycle: store::projects::MatterLifecycle) -> Self {
        match lifecycle {
            store::projects::MatterLifecycle::Awaiting => Self::Awaiting,
            store::projects::MatterLifecycle::Engaged => Self::Engaged,
            store::projects::MatterLifecycle::Closed => Self::Closed,
        }
    }
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
    /// The matter has no matter-opening engagement — surfaced as a warning badge.
    pub missing_retainer: bool,
    /// A `closed` matter with no closing letter — surfaced as a warning badge.
    /// Kept beside `lifecycle` rather than folded into it: a closed matter
    /// reads `Closed` whether or not it owes its letter, and collapsing the two
    /// would drop a real obligation off the row.
    pub missing_closing_letter: bool,
    /// Where this matter sits on its lifecycle track.
    #[serde(default = "awaiting_tone")]
    pub lifecycle: MatterTone,
}

/// `serde` default for [`ProjectRow::lifecycle`].
///
/// The warning state, not the clean one: a row deserialized without a lifecycle
/// is a row this build cannot place, and defaulting to `Engaged` would paint it
/// as papered on no evidence.
fn awaiting_tone() -> MatterTone {
    MatterTone::Awaiting
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
            let (missing_retainer, missing_closing_letter) = store::projects::matter_flags(
                has_engagement.contains(&m.id),
                &m.status,
                has_closing.contains(&m.id),
            );
            let lifecycle =
                store::projects::MatterLifecycle::of(&m.status, missing_retainer).into();
            ProjectRow {
                lifecycle,
                entity_name: by_entity(m.entity_id),
                id: m.id.to_string(),
                code: m.code,
                name: m.name,
                status: m.status,
                missing_retainer,
                missing_closing_letter,
            }
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
                                " "
                                // Every matter carries exactly one lifecycle
                                // indicator, including the clean one — the
                                // state a reader most needs to confirm is the
                                // common one, and a badge that only appears
                                // when something is wrong cannot confirm it.
                                span {
                                    class: "{row.lifecycle.class()}",
                                    title: "{row.lifecycle.description()}",
                                    "{row.lifecycle.label()}"
                                }
                                // Closed and still owing its offboarding letter
                                // is a second, independent fact: the matter is
                                // over, and something is still outstanding on
                                // it. It rides beside the red rather than
                                // inside it.
                                if row.missing_closing_letter {
                                    " "
                                    span { class: "matter-flag",
                                        title: "This closed matter has no closing letter.",
                                        "no closing letter"
                                    }
                                }
                            }
                            td { class: "project-status", "{row.status}" }
                            td { class: "project-entity", "{row.entity_name}" }
                        }
                    }
                }
            }
        }
    }
}
