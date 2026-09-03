//! The `/app/team` team home — the post-login landing for every firm tier.
//!
//! A firm person (Owner, Admin, Lawyer, or Clerk) lands here after signing in;
//! a `client` is answered 403 at the route, so this page never renders for one.
//! The home is a hub, not a dashboard: it greets the person and offers a
//! role-filtered set of destination cards into the surfaces their tier may
//! reach.
//!
//! The lens is the caller's tier, resolved once by [`crate::app_chrome`] and read
//! back here. The cards are gated exactly like the navbar's
//! [`crate::app_chrome::app_destinations`]: a card is never shown for a door the
//! viewer's tier is answered 403 at, so the home advertises only what it can
//! actually open.
//!
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::app_chrome::{APP_ADMIN_HREF, APP_LAWYER_HREF, APP_PROJECTS_HREF};
use crate::people::ViewerRole;

/// The `<meta description>` for the team home.
const DESCRIPTION: &str = "Your Neon Law Navigator team home.";

/// The licensed GORP Serif desktop family, streamed as one ZIP from the private
/// documents bucket. The `.otf` faces are never committed — like the site
/// photography, the bytes live only in the bucket and this route is the one way
/// to them.
///
/// It sits under this page's own prefix, so embedded Rego's `/app/team` rules
/// admit it for exactly the four firm tiers that reach the page and deny a
/// client. That makes the card's audience the page's audience, and it needs no
/// tier gate here.
const BRAND_FONTS_HREF: &str = "/app/team/fonts/gorp-serif.zip";

/// Everything the team home renders: the viewer's tier and the mounted brand's
/// mark for the app chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamHomeView {
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears
    /// its own palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
}

/// Resolve the authenticated viewer and the request-scoped brand for the home.
#[server]
pub async fn team_home_view() -> Result<TeamHomeView, ServerFnError> {
    Ok(TeamHomeView {
        role: crate::admin_listing::require_firm_person().await?,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
    })
}

/// The route entry for `/app/team`.
#[component]
pub fn TeamHome() -> Element {
    let resource = use_server_future(team_home_view)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "team-home", p { "Failed to load the team home." } }
            }
        }
        None => {
            return rsx! {
                main { id: "team-home", p { "Loading…" } }
            }
        }
    };

    team_home_body(&view)
}

/// One destination on the team home: a titled, described link into a firm
/// surface.
#[derive(Clone, PartialEq, Eq)]
struct Destination {
    /// The `id` on the card, so a test can pin a tier to its cards.
    id: &'static str,
    title: &'static str,
    description: &'static str,
    href: &'static str,
    /// The suggested filename when the card downloads a file rather than
    /// navigating. `None` for a destination card.
    download: Option<&'static str>,
}

/// The destinations a viewer of `role` sees on the home, in render order.
///
/// Only surfaces that need a signed-in firm person are cards here. The docs and
/// the workshops are public — a card for either would advertise, from behind a
/// login, a page anyone can already open — so they are not on this page.
///
/// Matters and the brand fonts need no tier gate: their own rules already admit
/// the whole firm-tier audience this page is scoped to. The Lawyer workbench
/// (from Lawyer up) and Admin (from Admin up) are gated here because the navbar
/// no longer carries those doors — this page is now the way to them. Pure, so
/// the mapping is unit-tested directly.
fn destinations_for(role: ViewerRole) -> Vec<Destination> {
    let mut cards = vec![Destination {
        id: "team-card-projects",
        title: "Matters",
        description: "Every matter you can see, in one list.",
        href: APP_PROJECTS_HREF,
        download: None,
    }];
    if role.is_lawyer_tier() {
        cards.push(Destination {
            id: "team-card-lawyer",
            title: "Lawyer",
            description: "The firm workbench: your matters' status at a glance, the \
                          calendar, and the people, entities, and notations you manage.",
            href: APP_LAWYER_HREF,
            download: None,
        });
    }
    if role.is_admin_tier() {
        cards.push(Destination {
            id: "team-card-admin",
            title: "Admin",
            description: "Firm administration and the full matter directory.",
            href: APP_ADMIN_HREF,
            download: None,
        });
    }
    cards.push(Destination {
        id: "team-card-fonts",
        title: "Brand fonts",
        description: "The licensed GORP Serif desktop family, as one ZIP of .otf faces.",
        href: BRAND_FONTS_HREF,
        // The suggested filename matches the route's own
        // `Content-Disposition: attachment` header.
        download: Some("gorp-serif.zip"),
    });
    cards
}

/// The loaded page. Split from the component so tests render a fixed view
/// without standing up the server function.
pub fn team_home_body(view: &TeamHomeView) -> Element {
    let role = view.role;
    let firm_name = view.firm_name.clone();

    let cards = destinations_for(role).into_iter().map(|d| {
        rsx! {
            a {
                key: "{d.id}",
                id: "{d.id}",
                class: "team-home__card",
                href: "{d.href}",
                download: d.download,
                h2 { class: "team-home__card-title", "{d.title}" }
                p { class: "team-home__card-desc", "{d.description}" }
            }
        }
    });

    rsx! {
        document::Title { "{firm_name} | Team" }
        document::Meta { name: "description", content: DESCRIPTION }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "team-home", class: "nav-theme",
            header { class: "page-header",
                h1 { "Team home" }
                p { class: "page-subtitle",
                    "Your team together with Neon Law Navigator."
                }
            }
            nav { class: "team-home__cards", "aria-label": "Team destinations",
                {cards}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TeamHomeView;
    use super::{destinations_for, team_home_body};
    use crate::people::ViewerRole;

    fn view_for(role: ViewerRole) -> TeamHomeView {
        TeamHomeView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role,
            logo: None,
        }
    }

    fn render(role: ViewerRole) -> String {
        dioxus_ssr::render_element(team_home_body(&view_for(role)))
    }

    fn card_ids(role: ViewerRole) -> Vec<&'static str> {
        destinations_for(role).into_iter().map(|d| d.id).collect()
    }

    /// Every firm tier gets the Matters and Brand fonts cards; a clerk gets
    /// exactly those two and never the firm-workbench or admin doors.
    #[test]
    fn a_clerk_sees_only_the_shared_firm_destinations() {
        assert_eq!(
            card_ids(ViewerRole::Clerk),
            ["team-card-projects", "team-card-fonts"]
        );
    }

    /// A lawyer gains the workbench card, but not admin.
    #[test]
    fn a_lawyer_gains_the_workbench_not_admin() {
        assert_eq!(
            card_ids(ViewerRole::Lawyer),
            ["team-card-projects", "team-card-lawyer", "team-card-fonts"]
        );
    }

    /// The admin tiers gain the admin card too.
    #[test]
    fn the_admin_tiers_gain_the_admin_card() {
        for role in [ViewerRole::Admin, ViewerRole::Owner] {
            assert_eq!(
                card_ids(role),
                [
                    "team-card-projects",
                    "team-card-lawyer",
                    "team-card-admin",
                    "team-card-fonts"
                ],
                "rank {}",
                role.authority_rank()
            );
        }
    }

    /// The rendered page carries the greeting and the role-filtered cards, with
    /// no CLI distribution surface.
    #[test]
    fn the_home_composes_greeting_cards_without_downloads() {
        let lawyer = render(ViewerRole::Lawyer);
        assert!(lawyer.contains("Team home"), "the greeting: {lawyer}");
        assert!(
            lawyer.contains("Your team together with Neon Law Navigator."),
            "the requested subtitle: {lawyer}"
        );
        assert!(
            lawyer.contains("team-card-lawyer"),
            "workbench card: {lawyer}"
        );
        assert!(
            !lawyer.contains("team-card-admin"),
            "a lawyer must not see the admin card: {lawyer}"
        );
        // The public surfaces are not advertised from behind a login.
        assert!(
            !lawyer.contains("/app/docs"),
            "the docs are public and must not be a card: {lawyer}"
        );
        assert!(
            !lawyer.contains("/workshops"),
            "the workshops are public and must not be a card: {lawyer}"
        );
        assert!(
            !lawyer.contains("Navigator CLI"),
            "the CLI download section must be absent: {lawyer}"
        );
        assert!(
            !lawyer.contains("/app/team/download/"),
            "the team home must not publish a CLI download link: {lawyer}"
        );
    }

    /// The brand-font ZIP is a card here rather than a list item on the
    /// workbench or the Clerk's matter list. Every firm tier sees it, because
    /// the route's own rule admits all four — so a Clerk, the narrowest tier
    /// this page renders for, must carry it too.
    #[test]
    fn every_firm_tier_gets_the_brand_font_card() {
        for role in [
            ViewerRole::Clerk,
            ViewerRole::Lawyer,
            ViewerRole::Admin,
            ViewerRole::Owner,
        ] {
            let html = render(role);
            assert!(
                html.contains(r#"id="team-card-fonts""#),
                "rank {} must see the brand-font card: {html}",
                role.authority_rank()
            );
            assert!(
                html.contains(r#"href="/app/team/fonts/gorp-serif.zip""#),
                "the card links the ZIP route: {html}"
            );
        }
    }

    /// The card downloads rather than navigates, and names the file the route's
    /// own `Content-Disposition` header names. Only that card carries the
    /// attribute — a destination card must stay a destination.
    #[test]
    fn only_the_font_card_is_a_download() {
        let html = render(ViewerRole::Lawyer);
        assert!(
            html.contains(r#"download="gorp-serif.zip""#),
            "the font card downloads under the route's own filename: {html}"
        );
        assert_eq!(
            html.matches("download=").count(),
            1,
            "no destination card may be a download: {html}"
        );
    }

    /// The navbar advertises the Team home. The workbench and admin doors are
    /// this page's cards, not navbar items, so `/app/lawyer` appears on a
    /// lawyer's page exactly once — as the Lawyer card.
    #[test]
    fn the_page_navbar_carries_the_team_home_and_the_cards_carry_the_rest() {
        let lawyer = render(ViewerRole::Lawyer);
        assert!(
            lawyer.contains(r#"href="/app/team""#),
            "the navbar advertises the Team home: {lawyer}"
        );
        assert_eq!(
            lawyer.matches(r#"href="/app/lawyer""#).count(),
            1,
            "the workbench is the card only, never also a navbar door: {lawyer}"
        );
        let owner = render(ViewerRole::Owner);
        assert_eq!(
            owner.matches(r#"href="/app/admin""#).count(),
            1,
            "admin is the card only, never also a navbar door: {owner}"
        );
    }

    /// The workbench card is gated to the lawyer tier — Lawyer, Admin, and
    /// Owner see it; a Clerk never does.
    #[test]
    fn the_workbench_card_is_the_lawyer_tier_only() {
        for role in [ViewerRole::Lawyer, ViewerRole::Admin, ViewerRole::Owner] {
            assert!(
                render(role).contains(r#"href="/app/lawyer""#),
                "rank {} reaches the workbench",
                role.authority_rank()
            );
        }
        let clerk = render(ViewerRole::Clerk);
        assert!(
            !clerk.contains("/app/lawyer"),
            "a clerk is never offered the workbench: {clerk}"
        );
    }
}
