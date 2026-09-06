//! The firm's `/team` page: one static statement, no roster.
//!
//! `/team` used to be a live, per-request roster (every confirmed, non-client
//! `Person`) with a generic `/team/{slug}` profile page per person. Avatars
//! moved to the private documents bucket (`store::persons::Person::
//! profile_image_url` now holds an admin-only route, not a public asset URL)
//! and had no reason to stay public once nothing kept them on this page, so
//! the roster and its per-person profile went with them. What is left is the
//! sentence the firm wants a visitor to read here.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The page's whole copy.
pub const STATEMENT: &str =
    "We believe that the whole is so much greater than the sum of its parts.";

/// Everything the `/team` page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TeamView {
    pub chrome: PublicChrome,
    pub head_title: String,
}

/// Resolve the chrome from the process brand. No store read: the page is
/// static copy, not a roster query.
#[cfg(feature = "server")]
async fn load_team() -> Result<TeamView, ServerFnError> {
    let chrome = crate::public_chrome::firm_public_chrome_from_context().await;
    let firm_name = chrome.brand_name.clone();
    Ok(TeamView {
        chrome,
        head_title: format!("{firm_name} | Team"),
    })
}

#[server]
pub async fn team_view() -> Result<TeamView, ServerFnError> {
    load_team().await
}

/// The `/team` route entry.
#[component]
pub fn TeamEntry() -> Element {
    let resource = use_server_future(team_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        TeamPage { chrome: view.chrome, head_title: view.head_title }
    }
}

/// The pure `/team` page. Prop-driven, so it server-renders and unit-tests
/// without a server future.
#[component]
pub fn TeamPage(chrome: PublicChrome, head_title: String) -> Element {
    let header = rsx! {
        SiteHeader {
            brand_name: chrome.brand_name.clone(),
            home_href: chrome.home_href.clone(),
            logo_href: chrome.logo_href.clone(),
            destinations: chrome
                .destinations
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
            utility: chrome
                .utility
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
        }
    };
    let footer = rsx! {
        PublicFooter { chrome: chrome.clone() }
    };
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: STATEMENT }
        SocialMeta {
            title: head_title.clone(),
            description: STATEMENT.to_string(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        PublicShell { header, footer,
            article { class: "team-statement",
                p { "{STATEMENT}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Neon Law".to_string(),
            home_href: "/".to_string(),
            logo_href: "/public/logo.svg".to_string(),
            social_image: "https://example.test/og.png".to_string(),
            ..PublicChrome::default()
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                TeamPage {
                    chrome: chrome(),
                    head_title: "Neon Law | Team".to_string(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn the_page_states_the_one_sentence() {
        let out = html();
        assert!(out.contains(STATEMENT), "{out}");
    }

    #[test]
    fn the_page_wraps_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "{out}");
        assert!(out.contains("site-footer__legal"), "{out}");
    }
}
