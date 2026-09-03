//! Admin visitor-analytics page as a Dioxus component (#956 Phase 4).
//!
//! The successor to the `views::pages::admin::analytics`. Read-only:
//! aggregate public-website visits by day, month, country, route, and source.
//! There is no form and no `POST` — the counts are written by the public
//! `count_public_visit` layer, never from this page.
//!
//! Admin-only. `require_admin` commits a real `403` for a non-admin caller,
//! matching the status the handler returned.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Column, DataTable, SortState};
use crate::people::ViewerRole;

/// The page's own path.
pub const ANALYTICS_PATH: &str = "/app/admin/analytics";

/// Visits in one time bucket — a day (`2026-07-09`) or a month (`2026-07`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct PeriodTotal {
    pub bucket: String,
    pub visits: i64,
}

/// Visits grouped by one dimension's label — a country, route, or source.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct DimensionTotal {
    pub label: String,
    pub visits: i64,
}

/// The rendered analytics page: the aggregate totals and the viewer's tier.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct AnalyticsView {
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    pub total_visits: i64,
    pub daily: Vec<PeriodTotal>,
    pub monthly: Vec<PeriodTotal>,
    pub countries: Vec<DimensionTotal>,
    pub routes: Vec<DimensionTotal>,
    pub sources: Vec<DimensionTotal>,
    pub role: ViewerRole,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Load the analytics page: refuse any non-admin caller, then read the
/// aggregate visitor summary.
#[server]
pub async fn get_analytics() -> Result<AnalyticsView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;

    let db = consume_context::<store::surreal::SurrealDb>();
    let summary = match store::visitor_analytics::summary(&db).await {
        Ok(summary) => summary,
        Err(e) => {
            // Commit a real 500 (the status the handler returned) so a
            // failed aggregate surfaces as an error, not a successful page
            // whose body happens to say the load failed.
            tracing::error!(error = %e, "analytics: visitor summary failed");
            dioxus_fullstack_core::FullstackContext::commit_http_status(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                None,
            );
            return Err(ServerFnError::new(e.to_string()));
        }
    };

    let periods = |rows: Vec<store::visitor_analytics::PeriodTotal>| {
        rows.into_iter()
            .map(|row| PeriodTotal {
                bucket: row.bucket,
                visits: row.visits,
            })
            .collect()
    };
    let dimensions = |rows: Vec<store::visitor_analytics::DimensionTotal>| {
        rows.into_iter()
            .map(|row| DimensionTotal {
                label: row.label,
                visits: row.visits,
            })
            .collect()
    };

    Ok(AnalyticsView {
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        total_visits: summary.total_visits,
        daily: periods(summary.daily),
        monthly: periods(summary.monthly),
        countries: dimensions(summary.countries),
        routes: dimensions(summary.routes),
        sources: dimensions(summary.sources),
        role,
    })
}

/// The admin visitor-analytics page.
#[component]
pub fn AdminAnalytics() -> Element {
    let resource = use_server_future(get_analytics)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "analytics", p { "Failed to load visitor analytics." } }
            }
        }
        None => {
            return rsx! {
                main { id: "analytics", p { "Loading…" } }
            }
        }
    };

    analytics_body(&view)
}

/// One time-bucket table (daily or monthly).
#[component]
fn PeriodTable(rows: Vec<PeriodTotal>, heading: String) -> Element {
    let columns = vec![
        Column::fixed("bucket", heading.clone()),
        Column::fixed("visits", "Visits"),
    ];
    rsx! {
        DataTable {
            columns,
            sort: SortState::default(),
            base_path: ANALYTICS_PATH.to_string(),
            if rows.is_empty() {
                tr {
                    td { colspan: "2", class: "nav-muted", "No rows." }
                }
            } else {
                for row in rows.iter() {
                    tr {
                        td { "{row.bucket}" }
                        td { "{row.visits}" }
                    }
                }
            }
        }
    }
}

/// One dimension table (country, route, or source). The label renders as
/// `<code>` because a route is a path pattern.
#[component]
fn DimensionTable(rows: Vec<DimensionTotal>, heading: String) -> Element {
    let columns = vec![
        Column::fixed("label", heading.clone()),
        Column::fixed("visits", "Visits"),
    ];
    rsx! {
        DataTable {
            columns,
            sort: SortState::default(),
            base_path: ANALYTICS_PATH.to_string(),
            if rows.is_empty() {
                tr {
                    td { colspan: "2", class: "nav-muted", "No rows." }
                }
            } else {
                for row in rows.iter() {
                    tr {
                        td { code { "{row.label}" } }
                        td { "{row.visits}" }
                    }
                }
            }
        }
    }
}

/// The loaded page. Split from the component so the tests render a fixed view
/// without standing up the server function.
fn analytics_body(view: &AnalyticsView) -> Element {
    let view = view.clone();
    let total = view.total_visits;

    rsx! {
        document::Title { "{view.firm_name} | Admin | Visitor analytics" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        nav { class: "lawyer-nav",
            a { class: "nav-link", href: "/app/projects", "Portal" }
            if view.role.is_lawyer_tier() {
                a { class: "nav-link", href: "/app/lawyer", "Lawyer" }
            }
            if view.role.is_admin_tier() {
                a { class: "nav-link", href: "/app/admin", "Admin" }
            }
            a { class: "nav-link", href: "/auth/logout", "Sign out" }
        }
        main { id: "analytics", class: "nav-theme",
            header { class: "page-header",
                h1 { "Visitor analytics" }
                p { class: "nav-muted",
                    "Aggregate public website visits by day, month, country, route, and source."
                }
            }
            p { class: "nav-card", "Total visits: {total}" }
            if view.total_visits == 0 {
                p { class: "nav-muted", role: "status",
                    "No visitor analytics have been recorded yet."
                }
            } else {
                section {
                    h2 { "Daily visits" }
                    PeriodTable { rows: view.daily.clone(), heading: "Day".to_string() }
                }
                section {
                    h2 { "Monthly visits" }
                    PeriodTable { rows: view.monthly.clone(), heading: "Month".to_string() }
                }
                section {
                    h2 { "Countries" }
                    DimensionTable { rows: view.countries.clone(), heading: "Country".to_string() }
                }
                section {
                    h2 { "Routes" }
                    DimensionTable { rows: view.routes.clone(), heading: "Route".to_string() }
                }
                section {
                    h2 { "Sources" }
                    DimensionTable { rows: view.sources.clone(), heading: "Source".to_string() }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn period(bucket: &str, visits: i64) -> PeriodTotal {
        PeriodTotal {
            bucket: bucket.to_string(),
            visits,
        }
    }

    fn dimension(label: &str, visits: i64) -> DimensionTotal {
        DimensionTotal {
            label: label.to_string(),
            visits,
        }
    }

    fn view() -> AnalyticsView {
        AnalyticsView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            total_visits: 4,
            daily: vec![period("2026-07-09", 4)],
            monthly: vec![period("2026-07", 4)],
            countries: vec![dimension("US", 3)],
            routes: vec![dimension("/blog/{slug}", 4)],
            sources: vec![dimension("linkedin", 2)],
            role: ViewerRole::Admin,
        }
    }

    #[test]
    fn analytics_renders_empty_state() {
        let html = dioxus_ssr::render_element(analytics_body(&AnalyticsView {
            role: ViewerRole::Admin,
            ..AnalyticsView::default()
        }));

        assert!(html.contains("Visitor analytics"), "{html}");
        assert!(html.contains("Total visits: "), "{html}");
        assert!(
            html.contains("No visitor analytics have been recorded yet."),
            "{html}"
        );
    }

    #[test]
    fn analytics_renders_aggregate_tables() {
        let html = dioxus_ssr::render_element(analytics_body(&view()));

        assert!(html.contains("2026-07-09"), "{html}");
        assert!(html.contains("2026-07"), "{html}");
        assert!(html.contains("US"), "{html}");
        // Dioxus escapes `{` and `}` in text nodes as-is, but the route pattern
        // must survive verbatim so an operator recognizes the matched route.
        assert!(html.contains("/blog/{slug}"), "{html}");
        assert!(html.contains("linkedin"), "{html}");
        // Every dimension heading renders.
        for heading in ["Day", "Month", "Country", "Route", "Source"] {
            assert!(html.contains(heading), "missing {heading} in {html}");
        }
    }

    #[test]
    fn an_empty_dimension_says_so_rather_than_rendering_a_bare_table() {
        let html = dioxus_ssr::render_element(analytics_body(&AnalyticsView {
            countries: vec![],
            ..view()
        }));
        assert!(html.contains("No rows."), "{html}");
    }
}
