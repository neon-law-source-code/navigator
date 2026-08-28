//! Shared scaffolding for the generic read-only admin listings (#641 Phase 3,
//! admin cluster).
//!
//! The admin surface rendered a family of plain, non-sortable reference
//! tables through one `render_listing` helper: run an `Entity::find()`, project
//! each row to `Vec<String>`, and print a titled table. This module is the
//! Dioxus successor to that helper. Each migrated page is a thin pair — a
//! `#[server]` function that calls [`load`] with its query, headers, and row
//! projection, and a component that renders the result through
//! [`AdminListingScaffold`] — so the per-page code stays as small as the
//! `render_listing` call it replaces while the chrome, table, and states live
//! here once.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, Pagination, SortState};
use crate::people::ViewerRole;

/// What one listing discloses, and therefore which gate it owes its caller.
///
/// Every listing in [`crate::admin_listings`] is classified here exactly once.
/// The classification is the specification: `every_admin_listing_is_classified_exactly_once`
/// fails on a listing that is not in [`LAWYER_LISTINGS`], so a new page cannot
/// reach `listing_router!` without someone deciding what it discloses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disclosure {
    /// Firm reference data belonging to no matter — jurisdictions, addresses,
    /// the shared template catalog. [`require_lawyer`] is the whole gate.
    Reference,
    /// One matter's content. Scoped through [`require_lawyer_in_matters`] to
    /// the caller's participation ledger, failing closed on an unlinked row.
    MatterContent,
    /// An input to `store::conflicts::check_new_matter`. **Deliberately
    /// firm-wide for the whole lawyer tier**, including a lawyer on no
    /// matters: ABA Model Rule 1.10 imputes one lawyer's conflict to the whole
    /// firm, so a lawyer must be able to see a conflict arising out of a
    /// matter they are not on. Scoping one of these would silently narrow the
    /// conflict check to the checker's own caseload, which is the failure the
    /// rule exists to prevent. Do not "fix" these for consistency.
    ConflictGraph,
    /// Matter content the schema cannot scope: the row carries no link to a
    /// project, so there is no join to filter on. Raised to
    /// [`require_admin`] as the interim close. This is a holding position, not
    /// a design — see the follow-up that adds the missing link.
    AdminOnly,
}

/// Every lawyer-tier listing, its route, and the single class it belongs to.
///
/// Keyed by the `#[server]` function name in [`crate::admin_listings`], which
/// is what the classification guard greps for.
pub const LAWYER_LISTINGS: &[(&str, &str, Disclosure)] = &[
    // Firm reference data — no matter behind any row.
    (
        "list_jurisdictions",
        "/app/admin/jurisdictions",
        Disclosure::Reference,
    ),
    (
        "list_git_repositories",
        "/app/admin/git-repositories",
        Disclosure::Reference,
    ),
    (
        "list_addresses",
        "/app/admin/addresses",
        Disclosure::Reference,
    ),
    (
        "list_mailrooms",
        "/app/admin/mailrooms",
        Disclosure::Reference,
    ),
    (
        "list_templates",
        "/app/admin/templates",
        Disclosure::Reference,
    ),
    (
        "list_questions",
        "/app/admin/questions",
        Disclosure::Reference,
    ),
    // Who is on which matter is the ledger itself, not a matter's content.
    (
        "list_person_project_roles",
        "/lawyer/person-project-roles",
        Disclosure::Reference,
    ),
    // A notation names its matter but discloses only template/person/state —
    // no client answer, no document, no prose.
    ("list_notations", "/lawyer/notations", Disclosure::Reference),
    // Matter content, scoped to the caller's participation ledger.
    ("list_answers", "/lawyer/answers", Disclosure::MatterContent),
    ("list_assets", "/lawyer/assets", Disclosure::MatterContent),
    (
        "list_relationship_logs",
        "/lawyer/relationship-logs",
        Disclosure::MatterContent,
    ),
    // Conflict-graph inputs — firm-wide on purpose. See `Disclosure::ConflictGraph`.
    (
        "list_disclosures",
        "/lawyer/disclosures",
        Disclosure::ConflictGraph,
    ),
    (
        "list_person_entity_roles",
        "/lawyer/person-entity-roles",
        Disclosure::ConflictGraph,
    ),
    // No project link on `letter` or `sent_email` to scope by.
    ("list_letters", "/app/admin/letters", Disclosure::AdminOnly),
    (
        "list_email_log",
        "/app/admin/email-log",
        Disclosure::AdminOnly,
    ),
];

/// The `?page=` pagination state for a paginated listing (e.g. the email log),
/// carried across the server→client boundary so the client renders the same
/// pagination anchors. `current` is the 1-indexed active page; `total` is the
/// page count. Absent (`None`) on the unpaginated listings.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PageState {
    /// The 1-indexed active page.
    pub current: u32,
    /// The total number of pages (`max(1, ceil(rows / per_page))`).
    pub total: u32,
    /// The route path the `?page=` anchors target, e.g. `/app/admin/email-log`.
    pub base_path: String,
}

/// A rendered read-only admin listing, in a wasm-safe shape (plain strings — no
/// `store`/`SeaORM` types cross to the client build). The server function
/// projects each row to a `Vec<String>` of cell text, mirroring the
/// `render_listing` contract.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct AdminListingView {
    /// The document title (`<title>`) — the deploy's firm name and the page's
    /// own suffix, e.g. `Neon Law | Lawyer | Jurisdictions`. Assembled by
    /// [`view`] rather than written at the call site, so a white-label deploy's
    /// tab reads its own name. See [`crate::app_chrome::firm_name_from_context`].
    pub title: String,
    /// The page heading (`<h1>`), e.g. `Jurisdictions`.
    pub heading: String,
    /// An optional explanatory line under the heading (e.g. the email log's note
    /// about which mail is and isn't logged); `None` on listings without one.
    #[serde(default)]
    pub subtitle: Option<String>,
    /// The column headers, left to right.
    pub headers: Vec<String>,
    /// One `Vec<String>` of cell text per row, aligned to `headers`.
    pub rows: Vec<Vec<String>>,
    /// The viewer's tier, for the lawyer nav chrome.
    pub role: ViewerRole,
    /// The `?page=` pagination state, for the paginated listings; `None` on the
    /// unpaginated ones (which render no pager).
    #[serde(default)]
    pub pagination: Option<PageState>,
    /// The sort state, for the listings whose headers are clickable; `None` on
    /// the fixed-order ones. See [`sorted_view`].
    #[serde(default)]
    pub sort: Option<SortableState>,
}

/// What a sortable listing needs beyond its rows: which columns sort, the active
/// `?sort=` value, and the path the header anchors point back at.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SortableState {
    /// Sort keys aligned to [`AdminListingView::headers`] — an empty string
    /// leaves that column fixed. The keys are the ones the route's pre-handler
    /// advertises, so a header can never link to a `?sort=` that 400s.
    pub keys: Vec<String>,
    /// The active `?sort=` value, echoed back so the header shows direction.
    pub active: String,
    /// The listing's own path, which the sort anchors rebuild their query on.
    pub base_path: String,
}

/// Run a read-only admin listing on the server: read the injected viewer
/// role and `SurrealDb`, run `query`, and project each row to cell text.
///
/// A `store::<table>` module exposes plain functions, each with its own error
/// type, so this takes the read as a closure over the handle and lets the
/// caller name its own query. The lawyer gate runs *before* the closure, so a
/// non-lawyer caller never triggers the read.
///
/// The listings are Lawyer-tier only. The refusal here is defense in
/// depth over the route's `require_auth` + `require_policy` gate, so a direct
/// hit on the generated `#[server]` endpoint cannot disclose the rows.
///
/// # Errors
/// [`require_lawyer`]'s refusal for a non-lawyer caller, or whatever the
/// caller's read returns.
#[cfg(feature = "server")]
pub async fn load_surreal<T, Fut, Q, F>(
    query: Q,
    title_suffix: &str,
    heading: &str,
    headers: &[&str],
    project: F,
) -> Result<AdminListingView, ServerFnError>
where
    Q: FnOnce(store::surreal::SurrealDb) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<T>, ServerFnError>>,
    F: Fn(T) -> Vec<String>,
{
    // Gate before running the read, so a non-lawyer caller never triggers it.
    let role = require_lawyer().await?;

    let surreal = consume_context::<store::surreal::SurrealDb>();
    let rows = query(surreal).await?.into_iter().map(project).collect();

    Ok(view(role, title_suffix, heading, headers, rows).await)
}

/// A sortable listing whose rows a caller has already built.
///
/// The sort runs over the projected cell text rather than in the engine,
/// which is what these listings can do: they are unpaginated, so the rows in
/// hand are the whole table and ordering them here yields the same ordering
/// the engine would.
///
/// Pure assembly: the lawyer gate lives in [`require_lawyer`], which the
/// caller must run first.
#[cfg(feature = "server")]
pub async fn sorted_view(
    role: ViewerRole,
    title_suffix: &str,
    heading: &str,
    headers: &[&str],
    sort: &PortedSort<'_>,
    mut rows: Vec<Vec<String>>,
) -> AdminListingView {
    // Apply the requested keys in reverse so the first one named is the
    // primary sort — a stable sort leaves earlier passes intact underneath.
    for (key, descending) in parse_sort(sort.active).into_iter().rev() {
        let Some(column) = sort.keys.iter().position(|k| !k.is_empty() && *k == key) else {
            continue;
        };
        rows.sort_by(|a, b| {
            let ordering = a
                .get(column)
                .map(String::as_str)
                .cmp(&b.get(column).map(String::as_str));
            if descending {
                ordering.reverse()
            } else {
                ordering
            }
        });
    }

    let mut view = view(role, title_suffix, heading, headers, rows).await;
    view.sort = Some(SortableState {
        keys: sort.keys.iter().map(|k| (*k).to_string()).collect(),
        active: sort.active.to_string(),
        base_path: sort.base_path.to_string(),
    });
    view
}

/// How a sortable listing orders itself.
///
/// `keys` holds one sort key per column, aligned to the view's `headers`,
/// with an empty string for a column that stays fixed (the questions
/// listing's free-prose prompt, where sorting says nothing useful).
/// `active` is the already-validated `?sort=` value: the route's pre-handler
/// rejects an unadvertised key with a `400` before this runs.
#[cfg(feature = "server")]
pub struct PortedSort<'a> {
    pub keys: &'a [&'a str],
    pub active: &'a str,
    pub base_path: &'a str,
}

/// Parse a JSON:API `sort` value into `(key, descending)` pairs, in the order
/// given — the first field is primary. Server-only.
#[cfg(feature = "server")]
pub(crate) fn parse_sort(raw: &str) -> Vec<(String, bool)> {
    raw.split(',')
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "-")
        .map(|segment| match segment.strip_prefix('-') {
            Some(key) => (key.to_string(), true),
            None => (segment.to_string(), false),
        })
        .collect()
}

/// Read the injected viewer role and refuse any non-lawyer caller — the shared
/// lawyer gate for every admin listing. The page router injects the tier after
/// its `require_auth` + `require_policy` gate, but a direct hit on the generated
/// `#[server]` endpoint need not carry that gate and defaults the role to the
/// least-privileged `Client`, so this refuses it before any query runs. Returns
/// the lawyer/admin role for the caller to thread into the rendered view.
#[cfg(feature = "server")]
pub async fn require_lawyer() -> Result<ViewerRole, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();

    if role.is_lawyer_tier() {
        Ok(role)
    } else {
        Err(ServerFnError::new("lawyer access required"))
    }
}

/// Which matters a caller may read **content** from.
///
/// `store::access` states the matter surface's rule: "Every tier is scoped by
/// the participation ledger, Owner and Admin included (ENG-81) — there is no
/// privileged short-circuit here." A listing that reads matter *content* — a
/// document, a questionnaire answer, a matter's audit trail — owes a caller
/// the same answer that surface would, and this is the shape that rule takes
/// here. [`require_lawyer`] alone is not enough for such a listing: it admits
/// the whole lawyer tier, including a lawyer holding no participation row at
/// all.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatterScope {
    /// Read every matter's content. Owner and Admin, and nobody else.
    ///
    /// Not a bypass of the matter surface's rule — that surface scopes Owner
    /// and Admin too, and still does. This is the administrative *listing*
    /// surface, which is the thing [`ViewerRole::is_admin_tier`] exists to
    /// name.
    Unscoped,
    /// Read only the matters the caller's participation ledger names, as
    /// `store::access::visible_projects_as_lawyer` resolves them. Empty for a
    /// lawyer holding no firm-side row, who therefore reads nothing — the
    /// same zero they already see at `/app/projects`.
    Participating(std::collections::HashSet<uuid::Uuid>),
}

#[cfg(feature = "server")]
impl MatterScope {
    /// Whether a row linked to `project_id` belongs in this caller's read.
    ///
    /// A row carrying **no** project link is absent from a scoped read. There
    /// is no matter to check it against, so it fails closed: an unlinked row
    /// is precisely the row whose matter nothing can vouch for, and admitting
    /// it would make "unlinked" the way around the ledger.
    #[must_use]
    pub fn admits(&self, project_id: Option<uuid::Uuid>) -> bool {
        match self {
            Self::Unscoped => true,
            Self::Participating(visible) => project_id.is_some_and(|id| visible.contains(&id)),
        }
    }

    /// Drop every row this caller may not read, keyed by each row's project
    /// link. The single place a matter-content listing filters, so the
    /// fail-closed rule above is written once rather than per page.
    pub fn retain<T>(&self, rows: &mut Vec<T>, project_of: impl Fn(&T) -> Option<uuid::Uuid>) {
        if matches!(self, Self::Unscoped) {
            return;
        }
        rows.retain(|row| self.admits(project_of(row)));
    }
}

/// Read the injected `persons.id` of the signed-in viewer. `None` when the
/// request carried no linked person — a direct hit on the generated `#[server]`
/// endpoint need not run behind the route's injection layer, and a session may
/// have no linked person at all. Both fail closed at [`MatterScope::admits`].
#[cfg(feature = "server")]
async fn injected_person_id() -> Option<uuid::Uuid> {
    dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<crate::portal_project_list::PersonId>,
        _,
    >()
    .await
    .ok()
    .and_then(|axum::Extension(pid)| pid.0)
    .and_then(|raw| raw.parse::<uuid::Uuid>().ok())
}

/// The gate for a listing that reads **matter content**: the lawyer-tier check
/// [`require_lawyer`] runs, plus the [`MatterScope`] the caller reads through.
///
/// Every such listing calls this instead of [`require_lawyer`], and filters its
/// rows through the returned scope. Which listings those are — and why the two
/// firm-wide ones are firm-wide — is written down once in
/// [`MATTER_CONTENT_LISTINGS`] and [`CONFLICT_GRAPH_LISTINGS`].
///
/// # Errors
/// [`require_lawyer`]'s refusal for a non-lawyer caller, or a `500` if the
/// participation query fails. A failed access query is not an empty workload:
/// rendering an honest-looking empty listing over a database that never
/// answered would read as "you are on no matters", so this commits a real
/// `500` instead — the same line `webapp::lawyer_dashboard` draws.
#[cfg(feature = "server")]
pub async fn require_lawyer_in_matters(
    surreal: &store::surreal::SurrealDb,
) -> Result<(ViewerRole, MatterScope), ServerFnError> {
    let role = require_lawyer().await?;
    if role.is_admin_tier() {
        return Ok((role, MatterScope::Unscoped));
    }

    let person_id = injected_person_id().await;
    let store_role = match role {
        ViewerRole::Owner => store::persons::Role::Owner,
        ViewerRole::Admin => store::persons::Role::Admin,
        ViewerRole::Lawyer => store::persons::Role::Lawyer,
        ViewerRole::Clerk => store::persons::Role::Clerk,
        ViewerRole::Client => store::persons::Role::Client,
    };
    let visible = store::access::visible_projects_as_lawyer(surreal, person_id, store_role)
        .await
        .map_err(|error| {
            tracing::error!(%error, "matter-scoped listing: visible_projects_as_lawyer failed");
            dioxus_fullstack_core::FullstackContext::commit_http_status(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                None,
            );
            ServerFnError::new(error.clone())
        })?;

    Ok((
        role,
        MatterScope::Participating(visible.into_iter().map(|project| project.id).collect()),
    ))
}

/// Read the injected viewer role and refuse a client — the gate for surfaces
/// that ask "does this person work for the firm" rather than "may this person
/// do legal work".
///
/// Same defense-in-depth shape as [`require_lawyer`]: the page router injects
/// the tier after `require_auth` + `require_policy`, but a direct hit on the
/// generated `#[server]` endpoint need not carry that gate and defaults to the
/// least-privileged `Client`, so this refuses it before any work runs. Returns
/// the firm role for the caller to thread into the rendered view.
#[cfg(feature = "server")]
pub async fn require_firm_person() -> Result<ViewerRole, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();

    if role.is_firm_tier() {
        Ok(role)
    } else {
        Err(ServerFnError::new("firm access required"))
    }
}

/// Read the injected viewer role and refuse any caller outside Owner/Admin — the
/// shared gate for the `/admin/*` pages. In production embedded Rego policy already gates
/// `/admin/*` to Owner/Admin (default-deny + privileged bypass); this is the
/// same defense in depth [`require_lawyer`] applies, and the authority in tests
/// (where the test policy is permissive). Returns the administrative role for the caller.
#[cfg(feature = "server")]
pub async fn require_admin() -> Result<ViewerRole, ServerFnError> {
    let role = dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<ViewerRole>, _>()
        .await
        .map(|axum::Extension(role)| role)
        .unwrap_or_default();

    if role.is_admin_tier() {
        Ok(role)
    } else {
        // Commit a real 403 (the status the `admin_gate` returned) so a
        // non-admin sees Forbidden, not a successful page with an error body.
        // In production embedded Rego policy already denies non-admins on `/admin/*`; this is the
        // authority under the tests' permissive embedded Rego policy and defense in depth.
        dioxus_fullstack_core::FullstackContext::commit_http_status(
            axum::http::StatusCode::FORBIDDEN,
            None,
        );
        Err(ServerFnError::new("admin access required"))
    }
}

/// Assemble an [`AdminListingView`] from rows the caller has already built and
/// the role [`require_lawyer`] returned. The entry point for listings whose rows
/// come from a join or aggregation rather than a single-entity projection (e.g.
/// mailrooms resolves each row's address, letters its mailroom); [`load`] uses
/// it too. Pure assembly — the lawyer gate lives in [`require_lawyer`], which the
/// caller must run first.
///
/// `title_suffix` is what follows the firm name in the document title (e.g.
/// `Lawyer | Jurisdictions`) and `heading` is the `<h1>`; `headers` labels the
/// columns; `rows` is one `Vec<String>` of cell text per row.
///
/// Async because the firm name comes from the request extension the portal
/// pre-layer resolved, which is the only place it is correct: this runs on a
/// server-function task that does not inherit the brand `task_local`.
#[cfg(feature = "server")]
pub async fn view(
    role: ViewerRole,
    title_suffix: &str,
    heading: &str,
    headers: &[&str],
    rows: Vec<Vec<String>>,
) -> AdminListingView {
    let firm = crate::app_chrome::firm_name_from_context().await;
    AdminListingView {
        sort: None,
        title: format!("{firm} | {title_suffix}"),
        heading: heading.to_string(),
        subtitle: None,
        headers: headers.iter().map(|h| (*h).to_string()).collect(),
        rows,
        role,
        pagination: None,
    }
}

/// Render a loaded [`AdminListingView`]: the role-appropriate lawyer nav chrome,
/// the heading, and a fixed (non-sortable) [`DataTable`] of the projected rows.
/// The empty state mirrors the other lawyer directories.
#[component]
pub fn AdminListingScaffold(view: AdminListingView) -> Element {
    // A sortable listing names each column with the key the route advertises, so
    // the header anchor links to a `?sort=` that cannot 400; a fixed listing
    // keeps positional ids and renders plain headers.
    let sortable = view.sort.clone().unwrap_or_default();
    let columns: Vec<Column> = view
        .headers
        .iter()
        .enumerate()
        .map(|(index, label)| match sortable.keys.get(index) {
            Some(key) if !key.is_empty() => Column::sortable(key.clone(), label.clone()),
            _ => Column::fixed(format!("col-{index}"), label.clone()),
        })
        .collect();
    let column_count = columns.len();
    let sort_state = SortState::parse(Some(&sortable.active));
    let base_path = sortable.base_path.clone();
    let is_empty = view.rows.is_empty();

    rsx! {
        document::Title { "{view.title}" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        // The role-appropriate lawyer nav chrome, mirroring the other directories.
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Projects" }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "admin-listing", class: "nav-theme",
            h1 { "{view.heading}" }
            if let Some(subtitle) = view.subtitle.as_ref() {
                p { class: "admin-listing-subtitle", "{subtitle}" }
            }
            DataTable {
                columns,
                sort: sort_state,
                base_path,
                if is_empty {
                    tr {
                        td { class: "admin-listing-empty", colspan: "{column_count}", "No rows yet." }
                    }
                }
                for row in view.rows.iter() {
                    tr { class: "admin-listing-row",
                        for cell in row.iter() {
                            td { class: "admin-listing-cell", "{cell}" }
                        }
                    }
                }
            }
            // Paginated listings (the email log) carry `?page=` anchors; the
            // unpaginated ones render no pager. The component is a no-op for a
            // single page, so an empty or one-page table shows nothing.
            if let Some(page) = view.pagination.as_ref() {
                Pagination {
                    current: page.current,
                    total: page.total,
                    base_path: page.base_path.clone(),
                }
            }
        }
    }
}

/// Render an admin listing from the resolved `use_server_future` resource,
/// handling the loading and error states uniformly. Each migrated page's
/// component is a thin wrapper: resolve its `#[server]` function and hand the
/// resource here.
pub fn render_resource(resource: &Resource<Result<AdminListingView, ServerFnError>>) -> Element {
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "admin-listing", p { "Failed to load." } }
            }
        }
        None => {
            return rsx! {
                main { id: "admin-listing", p { "Loading…" } }
            }
        }
    };
    rsx! {
        AdminListingScaffold { view }
    }
}
