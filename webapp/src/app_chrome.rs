//! The `/app` chrome: which destinations an authenticated viewer sees in the
//! navbar, and the deploy's brand mark, resolved server-side.
//!
//! The application counterpart to [`crate::public_chrome`], and the same split:
//! the presentational component ([`crate::components::AppNavbar`]) is a leaf in
//! the theme, while the role gate and the brand resolution live here, beside the
//! pages that mount it.
//!
//! Every `/app` page's `#[server]` loader already reads the injected
//! [`ViewerRole`]. It calls [`app_logo_from_context`] for the mark and carries
//! the result on its view struct, so the row is rendered from the same values
//! server-side and after hydration.

use dioxus::prelude::*;

use crate::components::{AppLogo, AppNavLink};
use crate::people::ViewerRole;

/// The matter list — one path for every tier, so it is the one destination every
/// authenticated viewer gets.
pub const APP_PROJECTS_HREF: &str = "/app/projects";

/// The firm team home — the post-login landing for every firm tier, and where
/// the `navigator` CLI is downloaded. Not a client's, so it is gated to the firm
/// tier both here and in the route's Rego rule.
pub const APP_TEAM_HREF: &str = "/app/team";

/// The firm workbench. Lawyer tier and up; the handler gates it too.
pub const APP_LAWYER_HREF: &str = "/app/lawyer";

/// The administrative landing. Admin tier and up.
pub const APP_ADMIN_HREF: &str = "/app/admin";

/// Where Sign out posts the viewer. Clears the app session; the provider's SSO
/// end-session endpoint is a separate hop.
pub const APP_SIGN_OUT_HREF: &str = "/auth/logout";

/// The `/app` destinations a viewer of `role` may see, in render order.
///
/// Links, not access decisions: the route middleware and each handler's own gate
/// stay authoritative. This decides only what the navbar advertises, so a client
/// is not shown a door that answers 403.
///
/// The row is deliberately short: Projects, the firm's Team home, and Sign out.
/// The Workbench and Admin doors are not here — they are cards on the Team home,
/// which every firm tier lands on at sign-in, so the tier-gated surfaces are one
/// click from the row rather than two more items in it.
///
/// Pure, so the role→destinations mapping is unit-tested directly rather than
/// through nine rendered pages.
#[must_use]
pub fn app_destinations(role: ViewerRole) -> Vec<AppNavLink> {
    let mut destinations = vec![AppNavLink::new("Projects", APP_PROJECTS_HREF)];
    if role.is_firm_tier() {
        destinations.push(AppNavLink::new("Team", APP_TEAM_HREF));
    }
    destinations.push(AppNavLink::new("Sign out", APP_SIGN_OUT_HREF));
    destinations
}

/// Request-extension carrier for the resolved brand identity: the navbar's mark
/// and the name `/app` copy addresses the client under.
///
/// `portal` injects this on the request task, where the brand `task_local` is
/// live, and a page's `#[server]` loader reads it back — the same seam
/// [`crate::components::Impersonating`] and [`crate::csrf::CsrfToken`] use. A
/// distinct type rather than a bare `Option<AppLogo>` so no other injector
/// can collide with it.
///
/// The name travels beside the mark rather than being read off it: a deploy that
/// configures no logo still has a firm name, and `/app` prose must not fall back
/// to this firm's when it does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppBrandMark {
    pub logo: Option<AppLogo>,
    /// The firm's site name for this request. Empty only on a `Default` value,
    /// which [`firm_name_from_context`] treats as unresolved.
    pub firm_name: String,
}

/// The running deploy's firm brand mark. Server-only: `views::brand` does not
/// compile to wasm.
///
/// `None` when the mounted brand configures no mark, so a deploy without one
/// renders the destinations alone rather than a broken image.
#[cfg(feature = "server")]
#[must_use]
pub fn firm_app_logo() -> Option<AppLogo> {
    let brand = &views::brand::FIRM_BRAND;
    if brand.logo_href.is_empty() {
        return None;
    }
    Some(AppLogo {
        src: brand.logo_href.to_string(),
        href: brand.home_href.to_string(),
        brand_name: brand.site_name.to_string(),
    })
}

/// The running deploy's brand identity, resolved where the brand `task_local`
/// is live. `portal`'s pre-layer calls this; a server function must not.
#[cfg(feature = "server")]
#[must_use]
pub fn firm_brand_mark() -> AppBrandMark {
    AppBrandMark {
        logo: firm_app_logo(),
        firm_name: views::brand::FIRM_BRAND.site_name.to_string(),
    }
}

/// Resolve the navbar's brand mark for the current request.
///
/// Prefers the mark the portal pre-layer (`inject_app_brand_mark`) resolved on
/// the request task: a Dioxus server function runs on a task that does not
/// inherit the brand `task_local`, so resolving here under a mounted
/// white-label bundle would publish the DEFAULT brand's mark. Falls back to
/// building it, which is what the middleware-free test paths get.
#[cfg(feature = "server")]
pub async fn app_logo_from_context() -> Option<AppLogo> {
    if let Ok(axum::Extension(mark)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<AppBrandMark>, _>().await
    {
        return mark.logo;
    }
    firm_app_logo()
}

/// Resolve the firm name `/app` copy names for the current request — the name a
/// client reads in the portal document title.
///
/// Same seam and same reason as [`app_logo_from_context`]: the pre-layer
/// resolved it while the brand `task_local` was live, so a white-label deploy's
/// portal addresses its own clients under its own name.
/// The firm name as a server function, for a page whose loader returns
/// something other than a view struct (the matter surface resolves a viewer
/// kind, not a view) and so has nowhere to carry it. Every other page threads
/// the name on its own view rather than paying for a second round trip.
#[server]
pub async fn firm_name() -> Result<String, ServerFnError> {
    Ok(firm_name_from_context().await)
}

#[cfg(feature = "server")]
pub async fn firm_name_from_context() -> String {
    if let Ok(axum::Extension(mark)) =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<AppBrandMark>, _>().await
    {
        if !mark.firm_name.is_empty() {
            return mark.firm_name;
        }
    }
    views::brand::FIRM_BRAND.site_name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::app_navbar::{AppNavbar, AppNavbarProps};

    fn labels(role: ViewerRole) -> Vec<String> {
        app_destinations(role)
            .into_iter()
            .map(|link| link.label)
            .collect()
    }

    /// A client sees the one destination every tier has, and the way out — never
    /// the firm-only Team home. A clerk is a firm tier, so it does get Team.
    /// This is the boundary the row still draws; the tier splits above it moved
    /// to the Team home's cards.
    #[test]
    fn a_client_is_offered_no_firm_workspace() {
        assert_eq!(labels(ViewerRole::Client), ["Projects", "Sign out"]);
        assert_eq!(labels(ViewerRole::Clerk), ["Projects", "Team", "Sign out"]);
    }

    /// Every firm tier is offered the same three: the row does not grow with
    /// authority, because the tier-gated doors are the Team home's cards now.
    #[test]
    fn every_firm_tier_is_offered_the_same_row() {
        for role in [
            ViewerRole::Clerk,
            ViewerRole::Lawyer,
            ViewerRole::Admin,
            ViewerRole::Owner,
        ] {
            assert_eq!(
                labels(role),
                ["Projects", "Team", "Sign out"],
                "rank {}",
                role.authority_rank()
            );
        }
    }

    /// The workbench and admin doors are not navbar items at any tier. They are
    /// cards on `/app/team`, which every firm tier lands on at sign-in — so the
    /// row must not carry them even for an Owner.
    #[test]
    fn the_row_carries_neither_workbench_nor_admin() {
        let hrefs: Vec<String> = app_destinations(ViewerRole::Owner)
            .into_iter()
            .map(|link| link.href)
            .collect();
        assert_eq!(hrefs, ["/app/projects", "/app/team", "/auth/logout"]);
    }

    /// The mapping reaches the rendered row: a firm viewer's navbar carries the
    /// Team home and neither tier-gated door; a client's carries neither the
    /// Team home nor them.
    #[test]
    fn the_rendered_navbar_gates_the_firm_destinations() {
        fn render(role: ViewerRole) -> String {
            let mut dom = VirtualDom::new_with_props(
                AppNavbar,
                AppNavbarProps {
                    destinations: app_destinations(role),
                    logo: None,
                },
            );
            dom.rebuild_in_place();
            dioxus_ssr::render(&dom)
        }

        for role in [ViewerRole::Lawyer, ViewerRole::Admin, ViewerRole::Owner] {
            let out = render(role);
            assert!(out.contains(r#"href="/app/team""#), "{out}");
            assert!(!out.contains(r#"href="/app/lawyer""#), "{out}");
            assert!(!out.contains(r#"href="/app/admin""#), "{out}");
        }

        let client = render(ViewerRole::Client);
        assert!(!client.contains(r#"href="/app/team""#), "{client}");
        assert!(client.contains(r#"href="/app/projects""#), "{client}");
    }

    /// Nothing under `/app/admin` is advertised to a tier that cannot enter it,
    /// including the surfaces that hang beneath the desk rather than beside it.
    ///
    /// The assertion above matches `href="/app/admin"` with its closing quote,
    /// so a deeper path like the matter directory at `/app/admin/projects`
    /// slips past it. This one matches the prefix, which is the property that
    /// actually holds: admission to every one of them is the same Owner/Admin
    /// route bypass.
    #[test]
    fn no_admin_prefix_reaches_a_tier_below_admin() {
        for role in [ViewerRole::Lawyer, ViewerRole::Clerk, ViewerRole::Client] {
            let mut dom = VirtualDom::new_with_props(
                AppNavbar,
                AppNavbarProps {
                    destinations: app_destinations(role),
                    logo: None,
                },
            );
            dom.rebuild_in_place();
            let out = dioxus_ssr::render(&dom);
            assert!(
                !out.contains(r#"href="/app/admin"#),
                "rank {} sees an admin destination: {out}",
                role.authority_rank()
            );
        }
    }
}
