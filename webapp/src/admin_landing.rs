//! The `/admin` console landing, as a Dioxus component (#956 Phase 4).
//!
//! The successor to the `views::pages::admin::landing`. The admin root is
//! a small hub, not a data table: it links to the admin-only surfaces so each
//! has a stable home. The people directory itself lives one click away at
//! `/app/admin/people`.
//!
//! The page has no per-request content beyond the viewer's tier, so the tiles
//! are a compile-time table rather than a loaded view model.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::Card;
use crate::people::ViewerRole;

/// One tile on the admin landing: a titled card with a short blurb and a
/// primary link into the surface.
struct AdminLink {
    title: &'static str,
    blurb: &'static str,
    href: &'static str,
    cta: &'static str,
}

/// The admin-only surfaces the hub links to.
const ADMIN_LINKS: &[AdminLink] = &[
    AdminLink {
        title: "People",
        blurb: "Administer every person: roles, records, impersonation, and removal. \
                The bootstrap Owner record's email and role are pinned.",
        href: "/app/admin/people",
        cta: "Manage people",
    },
    AdminLink {
        title: "Visitor analytics",
        blurb: "Traffic to the public site — visits by day and month, top routes, \
                countries, and referrers.",
        href: "/app/admin/analytics",
        cta: "View analytics",
    },
    AdminLink {
        title: "Matters",
        blurb: "Every matter the firm carries — its code, name, status, and the lawyer \
                accountable for it.",
        href: crate::matter_directory::MATTER_DIRECTORY_PATH,
        cta: "Browse matters",
    },
];

/// Everything the hub renders: the viewer's tier and the deploy's brand mark,
/// both for the nav chrome. The tiles are compile-time constants, so nothing
/// else crosses the boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct AdminLandingView {
    pub role: ViewerRole,
    /// `None` when the mounted brand configures no mark.
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The deploy's firm name, for the document title. Resolved from the
    /// request-scoped branding rather than written into the copy, so a
    /// white-label deploy's tab reads its own name.
    #[serde(default)]
    pub firm_name: String,
}

/// Resolve the hub. Admin-only: `require_admin` commits a real `403` for a
/// non-admin caller, the status the `admin_gate` returned, so a direct hit
/// on the generated endpoint cannot render the admin hub.
#[server]
pub async fn admin_landing_view() -> Result<AdminLandingView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    Ok(AdminLandingView {
        firm_name: crate::app_chrome::firm_name_from_context().await,
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
    })
}

/// The `/admin` route entry.
#[component]
pub fn AdminLandingEntry() -> Element {
    let resource = use_server_future(admin_landing_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "admin", p { "Failed to load." } }
            }
        }
        None => {
            return rsx! {
                main { id: "admin", p { "Loading…" } }
            }
        }
    };
    admin_landing_body(&view)
}

/// The hub body. Prop-driven and free of any server future, so it
/// server-renders and unit-tests directly.
pub fn admin_landing_body(view: &AdminLandingView) -> Element {
    let role = view.role;
    rsx! {
        document::Title { "{view.firm_name} | Admin" }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "admin", class: "nav-theme",
            header { class: "page-header",
                h1 { "Admin" }
            }
            div { class: "admin-hub",
                for link in ADMIN_LINKS.iter() {
                    Card {
                        h2 { "{link.title}" }
                        p { class: "nav-muted", "{link.blurb}" }
                        a { class: "nav-btn nav-btn--primary", href: "{link.href}", "{link.cta}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(role: ViewerRole) -> String {
        dioxus_ssr::render_element(admin_landing_body(&AdminLandingView {
            firm_name: "Neon Law".to_string(),
            role,
            logo: None,
        }))
    }

    #[test]
    fn the_hub_links_to_people_analytics_and_matters() {
        let out = html(ViewerRole::Admin);
        assert!(
            out.contains(r#"href="/app/admin/people""#),
            "people tile: {out}"
        );
        assert!(
            out.contains(r#"href="/app/admin/analytics""#),
            "analytics tile: {out}"
        );
        assert!(
            out.contains(r#"href="/app/admin/projects""#),
            "matter directory tile: {out}"
        );
        assert!(out.contains("Manage people"), "people call to action");
        assert!(out.contains("View analytics"), "analytics call to action");
        assert!(out.contains("Browse matters"), "matters call to action");
    }

    #[test]
    fn the_hub_is_not_the_people_table() {
        let out = html(ViewerRole::Admin);
        assert!(
            !out.contains("<table"),
            "the hub must not embed the people table: {out}"
        );
    }

    #[test]
    fn each_surface_gets_its_own_card() {
        let out = html(ViewerRole::Admin);
        assert_eq!(
            out.matches(r#"class="nav-card""#).count(),
            ADMIN_LINKS.len(),
            "one themed card per admin surface: {out}"
        );
        // The tiles carry theme classes, not Bootstrap's grid/button classes —
        // the hub styled itself with `row`/`col-md-6`/`btn btn-primary`,
        // which the Dioxus pages do not load.
        assert!(out.contains(r#"class="admin-hub""#), "themed tile grid");
        assert!(!out.contains("btn-primary"), "no Bootstrap button classes");
        assert!(!out.contains("col-md-6"), "no Bootstrap grid classes");
    }

    /// The row is the shared three. The workbench and admin doors are cards on
    /// `/app/team`, not navbar items, so this page's nav must not grow them
    /// back — reaching them from here is one hop through Team.
    #[test]
    fn the_nav_offers_the_shared_firm_row_to_an_admin() {
        let out = html(ViewerRole::Admin);
        assert!(out.contains(r#"href="/app/projects""#), "matter surface");
        assert!(out.contains(r#"href="/app/team""#), "team home: {out}");
        assert!(out.contains(r#"href="/auth/logout""#), "sign out");
        assert!(
            !out.contains(r#"href="/app/lawyer""#),
            "the workbench is a Team-home card, not a navbar door: {out}"
        );
    }

    /// The mark is configured per deploy: rendered when the brand supplies one,
    /// absent when it does not.
    #[test]
    fn the_nav_renders_the_configured_brand_mark() {
        let with_mark = dioxus_ssr::render_element(admin_landing_body(&AdminLandingView {
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Admin,
            logo: Some(crate::components::AppLogo {
                src: "/public/brand/firm-logo.svg".to_string(),
                href: "/".to_string(),
                brand_name: "Example Law".to_string(),
            }),
        }));
        assert!(
            with_mark.contains(r#"src="/public/brand/firm-logo.svg""#),
            "{with_mark}"
        );
        assert!(!html(ViewerRole::Admin).contains("lawyer-nav__brand"));
    }
}
