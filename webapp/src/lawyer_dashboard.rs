//! The lawyer workbench at `/lawyer` as a Dioxus component (#956 Phase 4) — the
//! project-first landing page every signed-in lawyer reaches first.
//!
//! The successor to the `views::pages::admin::dashboard`. It carries three
//! sections: the project KPI overview (a conic-gradient status pie plus a
//! paginated, status-filtered list of the caller's matters), the project
//! calendar placeholder, and the "Details" directory of every administrative
//! sub-page and JSON endpoint.
//!
//! **The project list is the caller's workload, not the firm's.** The loader
//! reads the injected `person_id` and role and goes through
//! [`store::access::visible_projects_as_lawyer`], so a lawyer sees the
//! matters they are on and an admin sees everything. Both the counts and the
//! list derive from that one collection, so the page can never disclose the
//! name of a matter the caller may not see.
//!
//! **The calendar is a deliberate placeholder.** It has rendered empty since
//! the dashboard shipped (#350), and its covering test asserts it stays that
//! way — the page must not synthesize events from projects before real event
//! storage exists. It is ported as-is rather than removed, because "no events
//! yet" is a product decision, not dead code. The calendar itself lives in
//! [`crate::project_calendar`], which the matter workbench renders scoped to
//! one matter.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// How many matters the KPI list shows per page — the page's pagination.
pub const PROJECTS_PER_PAGE: usize = 5;

/// CRUD admin pages — full list / new / edit / delete surfaces.
const CRUD_PAGES: &[(&str, &str)] = &[
    // People is absent on purpose: the one browser surface that creates or edits
    // a Person is the admin console's `/app/admin/people`, which this workbench's
    // audience does not all reach. A lawyer's Person commands go through
    // `POST /app/api/people`.
    ("/app/admin/entities", "Entities"),
    ("/app/projects", "Projects"),
];

/// Read-only listings — every remaining domain table.
///
/// `entity-types`, `templates`, and `questions` live here because they are
/// seeded by the workspace (`navigator db import`, `store/seeds/`) rather than
/// authored from the web UI.
const LISTING_PAGES: &[(&str, &str)] = &[
    ("/app/admin/entity-types", "Entity types"),
    ("/app/admin/templates", "Templates"),
    ("/app/admin/questions", "Questions"),
    ("/lawyer/notations", "Notations"),
    ("/lawyer/outline", "Outline stage"),
    ("/lawyer/answers", "Answers"),
    ("/app/admin/addresses", "Addresses"),
    ("/app/admin/mailrooms", "Mailrooms"),
    ("/app/admin/letters", "Letters"),
    ("/lawyer/assets", "Assets"),
    ("/lawyer/person-entity-roles", "Person ↔ entity roles"),
    ("/lawyer/person-project-roles", "Person ↔ project roles"),
    ("/app/admin/jurisdictions", "Jurisdictions"),
    ("/app/admin/git-repositories", "Git repositories"),
    ("/lawyer/disclosures", "Disclosures"),
    ("/lawyer/relationship-logs", "Relationship logs"),
];

const API_ENDPOINTS: &[(&str, &str)] = &[
    ("/app/api/openapi.json", "OpenAPI 3.1 spec"),
    ("/app/api/people", "JSON: /app/api/people"),
    ("/app/api/entities", "JSON: /app/api/entities"),
    ("/app/api/jurisdictions", "JSON: /app/api/jurisdictions"),
    ("/app/api/entity-types", "JSON: /app/api/entity-types"),
];

/// One matter in the KPI list — the stable public code and its name.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ProjectLink {
    pub id: String,
    pub code: String,
    pub name: String,
}

/// The whole rendered dashboard.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DashboardView {
    pub total_projects: u64,
    pub open_projects: u64,
    pub closed_projects: u64,
    /// The current page of the status-filtered list.
    pub rows: Vec<ProjectLink>,
    /// `open` or `closed` — which tab is active.
    pub status: String,
    pub page: u64,
    pub total_pages: u64,
    /// The calendar's active sort, carried into every link the page emits so
    /// paginating or re-tabbing the list never resets the calendar's sort.
    pub sort: String,
    pub dir: String,
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

/// The dashboard's query string. All four are lenient: an unrecognised value
/// falls back to the default rather than refusing the request, which is why
/// this page carries no `400`-on-bad-sort pre-handler.
#[derive(Deserialize, Default)]
pub struct DashboardQuery {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub page: Option<u64>,
}

/// Load the lawyer workbench for the caller: refuse non-lawyer, read the injected
/// `person_id`, and derive both the counts and the visible page of matters from
/// the one access-scoped collection.
#[server]
pub async fn get_lawyer_dashboard() -> Result<DashboardView, ServerFnError> {
    let role = crate::admin_listing::require_lawyer().await?;
    let axum::extract::Query(query) = dioxus_fullstack_core::FullstackContext::extract::<
        axum::extract::Query<DashboardQuery>,
        _,
    >()
    .await?;
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
    // A failed access query is not an empty workload. Commit a real 500 rather
    // than rendering an honest-looking zero-count dashboard over a database
    // that never answered — the handler drew the same line.
    let projects = store::access::visible_projects_as_lawyer(&surreal, person_id, store_role)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "dashboard: visible_projects_as_lawyer failed");
            dioxus_fullstack_core::FullstackContext::commit_http_status(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                None,
            );
            ServerFnError::new(e.clone())
        })?;

    let status = if query.status.as_deref() == Some("closed") {
        "closed"
    } else {
        "open"
    };
    let open_projects = projects.iter().filter(|p| p.status == "open").count();
    let closed_projects = projects.iter().filter(|p| p.status == "closed").count();

    // Both the counts and the list come from `projects`, which is already
    // filtered through the lawyer lens, so the page cannot name an unrelated
    // matter.
    let filtered: Vec<&store::projects::Project> =
        projects.iter().filter(|p| p.status == status).collect();
    let total_pages = filtered
        .len()
        .saturating_add(PROJECTS_PER_PAGE - 1)
        .checked_div(PROJECTS_PER_PAGE)
        .unwrap_or(0)
        .max(1);
    let page = usize::try_from(query.page.unwrap_or(1))
        .unwrap_or(usize::MAX)
        .max(1)
        .min(total_pages);
    let first = page.saturating_sub(1).saturating_mul(PROJECTS_PER_PAGE);
    let last = first.saturating_add(PROJECTS_PER_PAGE).min(filtered.len());
    let rows = filtered[first..last]
        .iter()
        .map(|p| ProjectLink {
            id: p.id.to_string(),
            code: p.code.clone(),
            name: p.name.clone(),
        })
        .collect();

    Ok(DashboardView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        total_projects: u64::try_from(open_projects.saturating_add(closed_projects))
            .unwrap_or(u64::MAX),
        open_projects: u64::try_from(open_projects).unwrap_or(u64::MAX),
        closed_projects: u64::try_from(closed_projects).unwrap_or(u64::MAX),
        rows,
        status: status.to_string(),
        page: u64::try_from(page).unwrap_or(u64::MAX),
        total_pages: u64::try_from(total_pages).unwrap_or(u64::MAX),
        sort: crate::project_calendar::sort_field(
            query.sort.as_deref(),
            crate::project_calendar::WORKBENCH_COLUMNS,
        ),
        dir: crate::project_calendar::sort_dir(query.dir.as_deref()),
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
    })
}

/// The lawyer workbench. Server-side rendered; every control is a real anchor,
/// so the page works without JavaScript.
#[component]
pub fn LawyerDashboard() -> Element {
    let resource = use_server_future(get_lawyer_dashboard)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "lawyer-dashboard", p { "Failed to load the workbench." } }
            }
        }
        None => {
            return rsx! {
                main { id: "lawyer-dashboard", p { "Loading…" } }
            }
        }
    };

    lawyer_dashboard_body(&view)
}

/// The loaded workbench. Split from the component so the tests render a fixed
/// view without standing up the server function.
fn lawyer_dashboard_body(view: &DashboardView) -> Element {
    let view = view.clone();
    let role = view.role;

    rsx! {
        document::Title { "{view.firm_name} | Lawyer Workbench" }
        document::Meta {
            name: "description",
            content: "Neon Law Navigator lawyer project overview.",
        }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "lawyer-dashboard", class: "nav-theme",
            header { class: "page-header", h1 { "Lawyer workbench" } }
            ProjectKpiOverview { view: view.clone() }
            // The workbench calendar spans every matter the caller can see, and
            // carries the adjacent list's status so re-sorting one control does
            // not reset the other.
            crate::project_calendar::ProjectCalendar {
                section_class: "lawyer-project-calendar".to_string(),
                heading: "Project calendar".to_string(),
                empty_message: "No project calendar events scheduled.".to_string(),
                columns: crate::project_calendar::WORKBENCH_COLUMNS.to_vec(),
                path: "/app/lawyer".to_string(),
                query_prefix: format!("status={}&", view.status),
                sort: view.sort.clone(),
                dir: view.dir.clone(),
            }
            DashboardDetails { role }
        }
    }
}

/// The status pie beside the caller's paginated matter list.
#[component]
fn ProjectKpiOverview(view: DashboardView) -> Element {
    // The open share drives the conic-gradient stop. Integer maths to four
    // decimals keeps the rendered value stable across platforms — a float
    // would make the emitted style (and its test) architecture-dependent.
    let open_share_units = view
        .open_projects
        .saturating_mul(1_000_000)
        .saturating_add(view.total_projects / 2)
        .checked_div(view.total_projects)
        .unwrap_or(0);
    let open_share = format!(
        "{}.{:04}%",
        open_share_units / 10_000,
        open_share_units % 10_000
    );
    let chart_label = format!(
        "{} total projects: {} open, {} closed",
        view.total_projects, view.open_projects, view.closed_projects
    );
    let pie_class = if view.total_projects == 0 {
        "project-status-pie project-status-pie-empty"
    } else {
        "project-status-pie"
    };
    let pie_style = format!("--project-open-share:{open_share}");

    rsx! {
        div { class: "project-kpi-overview",
            section { class: "project-kpi-chart-col",
                h2 { "Project KPIs" }
                div { class: "nav-card project-kpi-chart",
                    div { class: "nav-card__body project-kpi-chart__body",
                        div {
                            class: "{pie_class}",
                            role: "img",
                            aria_label: "{chart_label}",
                            style: "{pie_style}",
                        }
                        div { class: "project-kpi-figures",
                            div { class: "project-kpi-total", "{view.total_projects}" }
                            div { class: "nav-muted", "Total projects" }
                            ul { class: "project-status-legend",
                                li {
                                    span {
                                        class: "project-status-key project-status-key-open",
                                        aria_hidden: "true",
                                    }
                                    strong { "Open projects: " }
                                    "{view.open_projects}"
                                }
                                li {
                                    span {
                                        class: "project-status-key project-status-key-closed",
                                        aria_hidden: "true",
                                    }
                                    strong { "Closed projects: " }
                                    "{view.closed_projects}"
                                }
                            }
                        }
                    }
                }
            }
            section { class: "project-kpi-list-col",
                h2 { "Projects" }
                ProjectKpiList { view }
            }
        }
    }
}

/// The status-tabbed, paginated list of the caller's matters.
#[component]
fn ProjectKpiList(view: DashboardView) -> Element {
    let closed = view.status == "closed";
    let open_class = if closed {
        "nav-tab"
    } else {
        "nav-tab is-active"
    };
    let closed_class = if closed {
        "nav-tab is-active"
    } else {
        "nav-tab"
    };
    let empty_message = if closed {
        "No closed projects in your workload."
    } else {
        "No active projects in your workload."
    };
    let open_href = format!(
        "/app/lawyer?status=open&sort={}&dir={}",
        view.sort, view.dir
    );
    let closed_href = format!(
        "/app/lawyer?status=closed&sort={}&dir={}",
        view.sort, view.dir
    );
    let rows = view.rows.clone();

    rsx! {
        div { class: "nav-card project-kpi-list",
            div { class: "nav-card__body",
                nav { class: "nav-tabs", aria_label: "Project status",
                    a { class: "{open_class}", href: "{open_href}", "Active" }
                    a { class: "{closed_class}", href: "{closed_href}", "Closed" }
                }
                if rows.is_empty() {
                    p { class: "nav-muted project-kpi-empty", "{empty_message}" }
                } else {
                    ul { class: "project-kpi-rows",
                        for row in rows.iter() {
                            li { a { href: "/app/projects/{row.code}", "{row.name}" } }
                        }
                    }
                }
                ProjectListPagination { view }
            }
        }
    }
}

/// Previous / Page N of M / Next. Renders nothing on a single page.
#[component]
fn ProjectListPagination(view: DashboardView) -> Element {
    if view.total_pages <= 1 {
        return rsx! {};
    }
    let page = view.page.max(1).min(view.total_pages);
    let href = |target: u64| {
        format!(
            "/app/lawyer?status={}&sort={}&dir={}&page={target}",
            view.status, view.sort, view.dir
        )
    };
    let prev_href = href(page.saturating_sub(1));
    let next_href = href(page.saturating_add(1));
    let summary = format!("Page {page} of {}", view.total_pages);

    rsx! {
        nav { class: "nav-pagination", aria_label: "Project list pagination",
            if page > 1 {
                a { class: "nav-pagination__link", href: "{prev_href}", "Previous" }
            } else {
                span { class: "nav-pagination__link is-disabled", aria_disabled: "true", "Previous" }
            }
            span { class: "nav-pagination__status", aria_current: "page", "{summary}" }
            if page < view.total_pages {
                a { class: "nav-pagination__link", href: "{next_href}", "Next" }
            } else {
                span { class: "nav-pagination__link is-disabled", aria_disabled: "true", "Next" }
            }
        }
    }
}

/// The directory of every administrative sub-page and JSON endpoint.
#[component]
fn DashboardDetails(role: ViewerRole) -> Element {
    rsx! {
        section { class: "admin-details",
            h2 { "Details" }
            div { class: "admin-details__columns",
                div {
                    h3 { class: "admin-details__heading", "Manage" }
                    ul {
                        for (href , label) in CRUD_PAGES.iter() {
                            li { a { href: "{href}", "{label}" } }
                        }
                    }
                }
                div {
                    h3 { class: "admin-details__heading", "Listings" }
                    ul {
                        for (href , label) in LISTING_PAGES.iter() {
                            li { a { href: "{href}", "{label}" } }
                        }
                    }
                }
                div {
                    h3 { class: "admin-details__heading", "Operations" }
                    ul {
                        // Visitor analytics is admin-only; a lawyer must
                        // not be offered a link the route will refuse.
                        if role.is_admin_tier() {
                            li { a { href: "/app/admin/analytics", "Visitor analytics" } }
                        }
                        li { a { href: "/app/admin/schedules", "Cron schedules" } }
                    }
                    h3 { class: "admin-details__heading", "JSON API" }
                    ul {
                        for (href , label) in API_ENDPOINTS.iter() {
                            li { a { href: "{href}", "{label}" } }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> DashboardView {
        DashboardView {
            firm_name: "Neon Law".to_string(),
            total_projects: 3,
            open_projects: 2,
            closed_projects: 1,
            rows: vec![ProjectLink {
                id: "00000000-0000-0000-0000-000000000001".to_string(),
                code: "acme-contract-review".to_string(),
                name: "Acme contract review".to_string(),
            }],
            status: "open".to_string(),
            page: 1,
            total_pages: 1,
            sort: "date".to_string(),
            dir: "asc".to_string(),
            role: ViewerRole::Lawyer,
            logo: None,
        }
    }

    #[test]
    fn the_kpi_pie_reports_the_open_share_and_names_itself_for_a_screen_reader() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&view()));
        // The pie is decorative geometry, so the counts must also be readable:
        // the aria-label is the only thing a screen reader gets from it.
        assert!(
            html.contains("aria-label=\"3 total projects: 2 open, 1 closed\""),
            "{html}"
        );
        // 2/3 open, to the four decimals the integer maths produces.
        assert!(html.contains("--project-open-share:66.6667%"), "{html}");
        assert!(html.contains(">2<"), "{html}");
        assert!(html.contains(">1<"), "{html}");
    }

    #[test]
    fn an_empty_workload_renders_the_pie_in_its_empty_state() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&DashboardView {
            total_projects: 0,
            open_projects: 0,
            closed_projects: 0,
            rows: vec![],
            ..view()
        }));
        assert!(html.contains("project-status-pie-empty"), "{html}");
        // Dividing by a zero total must not panic or emit a bogus share.
        assert!(html.contains("--project-open-share:0.0000%"), "{html}");
        assert!(
            html.contains("No active projects in your workload."),
            "{html}"
        );
    }

    #[test]
    fn the_closed_tab_is_active_and_names_its_own_empty_state() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&DashboardView {
            status: "closed".to_string(),
            rows: vec![],
            ..view()
        }));
        assert!(
            html.contains("No closed projects in your workload."),
            "{html}"
        );
        assert!(html.contains("nav-tab is-active"), "{html}");
    }

    #[test]
    fn pagination_carries_the_calendar_sort_so_the_two_controls_do_not_reset_each_other() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&DashboardView {
            page: 2,
            total_pages: 3,
            sort: "project".to_string(),
            dir: "desc".to_string(),
            ..view()
        }));
        // Both neighbours reachable, each preserving status + sort + dir.
        assert!(
            html.contains("/app/lawyer?status=open&#38;sort=project&#38;dir=desc&#38;page=1"),
            "{html}"
        );
        assert!(
            html.contains("/app/lawyer?status=open&#38;sort=project&#38;dir=desc&#38;page=3"),
            "{html}"
        );
        assert!(html.contains("Page 2 of 3"), "{html}");
    }

    #[test]
    fn a_single_page_offers_no_pagination() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&view()));
        assert!(!html.contains("nav-pagination"), "{html}");
    }

    #[test]
    fn the_calendar_stays_empty_and_its_headers_toggle_direction() {
        // #350: the dashboard must not synthesize calendar events from projects
        // before real event storage exists. The seeded matter in `view()` is a
        // witness — it must not appear as an event.
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&DashboardView {
            sort: "project".to_string(),
            dir: "asc".to_string(),
            ..view()
        }));
        assert!(
            html.contains("No project calendar events scheduled."),
            "{html}"
        );
        // The active column shows its direction and offers the reverse.
        assert!(html.contains("Project (asc)"), "{html}");
        assert!(
            html.contains("/app/lawyer?status=open&#38;sort=project&#38;dir=desc"),
            "{html}"
        );
        // An inactive column offers ascending and carries no marker.
        assert!(
            html.contains("/app/lawyer?status=open&#38;sort=entity&#38;dir=asc"),
            "{html}"
        );
    }

    #[test]
    fn visitor_analytics_is_offered_to_an_admin_and_withheld_from_lawyer() {
        let lawyer = dioxus_ssr::render_element(lawyer_dashboard_body(&view()));
        assert!(!lawyer.contains("/app/admin/analytics"), "{lawyer}");
        // Every lawyer still gets the non-admin operations links.
        assert!(lawyer.contains("/app/admin/schedules"), "{lawyer}");

        let admin = dioxus_ssr::render_element(lawyer_dashboard_body(&DashboardView {
            role: ViewerRole::Admin,
            ..view()
        }));
        assert!(admin.contains("/app/admin/analytics"), "{admin}");
    }

    /// The navbar the workbench renders is the shared `/app` row: the workbench
    /// itself is offered to lawyer (the destination the hand-written nav dropped),
    /// the admin desk only to an admin, and the mark comes from the view.
    #[test]
    fn the_navbar_offers_the_role_appropriate_app_destinations() {
        let lawyer = dioxus_ssr::render_element(lawyer_dashboard_body(&view()));
        assert!(lawyer.contains(r#"href="/app/projects""#), "{lawyer}");
        assert!(lawyer.contains(r#"href="/app/team""#), "{lawyer}");
        assert!(lawyer.contains(r#"href="/auth/logout""#), "{lawyer}");
        assert!(!lawyer.contains(r#"href="/app/admin""#), "{lawyer}");
        assert!(!lawyer.contains("lawyer-nav__brand"), "no mark configured");

        // The row does not grow with authority: the workbench and admin doors
        // are cards on the Team home, so an Admin's navbar is a Lawyer's.
        let admin = dioxus_ssr::render_element(lawyer_dashboard_body(&DashboardView {
            role: ViewerRole::Admin,
            logo: Some(crate::components::AppLogo {
                src: "/public/brand/firm-logo.svg".to_string(),
                href: "/".to_string(),
                brand_name: "Example Law".to_string(),
            }),
            ..view()
        }));
        assert!(!admin.contains(r#"href="/app/admin""#), "{admin}");
        assert!(admin.contains(r#"href="/app/team""#), "{admin}");
        assert!(
            admin.contains(r#"src="/public/brand/firm-logo.svg""#),
            "{admin}"
        );
    }

    #[test]
    fn every_directory_link_is_rendered() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&view()));
        for (href, label) in CRUD_PAGES
            .iter()
            .chain(LISTING_PAGES.iter())
            .chain(API_ENDPOINTS.iter())
        {
            assert!(html.contains(&format!("href=\"{href}\"")), "{href}: {html}");
            assert!(html.contains(label), "{label}: {html}");
        }
    }

    /// The billing and cap-table listings are gone — the Firm bills through
    /// Xero and keeps cap tables in Carta — and their routes are unmounted. So
    /// is the people index: ENG-304 deleted the `/lawyer/people` mirror, and the
    /// one people surface now answers at `/app/admin/people`, which this workbench's
    /// lawyer-tier audience is refused at.
    ///
    /// A nav entry outliving its route is a link straight to a 404, and
    /// `every_directory_link_is_rendered` above cannot catch that: it walks
    /// the same tables this nav is built from, so re-adding an entry would
    /// satisfy it. This names the dead paths directly instead.
    #[test]
    fn the_removed_listings_are_not_advertised() {
        let html = dioxus_ssr::render_element(lawyer_dashboard_body(&view()));
        for href in [
            "/lawyer/entity-billing-profiles",
            "/lawyer/invoices",
            "/lawyer/invoice-line-items",
            "/cap-table",
            "/lawyer/people",
        ] {
            assert!(!html.contains(href), "{href} is still linked: {html}");
        }
        // A surviving listing anchors the assertion: the nav renders, so the
        // absences above are removals rather than an empty page.
        assert!(html.contains("/lawyer/disclosures"), "{html}");
    }
}
