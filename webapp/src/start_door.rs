//! The service start door: disclosure first, then a small confirmation form.
//!
//! The portal resolves whether a service may start and injects the copy and
//! service name. This crate only renders that decision with the shared public
//! chrome.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The service-specific decision and copy resolved by the portal route.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct StartDoorContent {
    pub service_id: String,
    pub service_name: String,
    pub disclosure: String,
    pub refusal: String,
    pub start_label: String,
    pub can_start: bool,
}

/// The portal pre-layer's content injection.
#[derive(Clone, Default)]
pub struct InjectedStartDoor(pub StartDoorContent);

/// The CSRF token the portal resolved from the ordinary session cookie.
#[derive(Clone, Default)]
pub struct StartDoorCsrf(pub String);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct StartDoorView {
    pub chrome: PublicChrome,
    pub content: StartDoorContent,
    pub csrf_token: String,
}

/// Read the route's decision and the session CSRF token.
#[server]
pub async fn start_door_view() -> Result<StartDoorView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedStartDoor>, _>()
            .await
            .map(|axum::Extension(content)| content.0)
            .unwrap_or_default();
    let csrf_token =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<StartDoorCsrf>, _>()
            .await
            .map(|axum::Extension(token)| token.0)
            .unwrap_or_default();
    Ok(StartDoorView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
        csrf_token,
    })
}

/// The route entry used by the portal's Dioxus SSR router.
#[component]
pub fn StartDoorEntry() -> Element {
    let resource = use_server_future(start_door_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        StartDoorPage {
            chrome: view.chrome,
            content: view.content,
            csrf_token: view.csrf_token,
        }
    }
}

/// The pure service start page.
#[component]
pub fn StartDoorPage(
    chrome: PublicChrome,
    content: StartDoorContent,
    csrf_token: String,
) -> Element {
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
    let footer = rsx! { PublicFooter { chrome: chrome.clone() } };
    rsx! {
        document::Title { "{content.service_name} | {chrome.brand_name}" }
        PublicShell { header, footer,
            article { class: "start-door",
                h1 { "{content.service_name}" }
                p { class: "start-door__disclosure", "{content.disclosure}" }
                if content.can_start {
                    form { method: "post", action: "/start/{content.service_id}",
                        input { type: "hidden", name: "_csrf", value: "{csrf_token}" }
                        button { class: "nav-btn nav-btn--primary", r#type: "submit",
                            "{content.start_label}"
                        }
                    }
                } else {
                    p { class: "start-door__refusal", role: "alert", "{content.refusal}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Neon Law".into(),
            ..PublicChrome::default()
        }
    }

    fn render(content: StartDoorContent) -> String {
        let mut dom = VirtualDom::new_with_props(
            StartDoorPage,
            StartDoorPageProps {
                chrome: chrome(),
                content,
                csrf_token: "csrf".into(),
            },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn configured_service_shows_disclosure_and_start_form() {
        let html = render(StartDoorContent {
            service_id: "llc-file".into(),
            service_name: "Start a company".into(),
            disclosure: "Disclosure copy".into(),
            refusal: "Refusal copy".into(),
            start_label: "Start".into(),
            can_start: true,
        });
        assert!(html.contains("Disclosure copy"), "{html}");
        assert!(html.contains("action=\"/start/llc-file\""), "{html}");
        assert!(html.contains("name=\"_csrf\""), "{html}");
        assert!(html.contains(">Start<"), "{html}");
        assert!(!html.contains("Refusal copy"), "{html}");
    }

    #[test]
    fn unavailable_service_shows_refusal_without_a_form() {
        let html = render(StartDoorContent {
            service_id: "llc-file".into(),
            service_name: "Start a company".into(),
            disclosure: "Disclosure copy".into(),
            refusal: "Refusal copy".into(),
            start_label: "Start".into(),
            can_start: false,
        });
        assert!(html.contains("Refusal copy"), "{html}");
        assert!(!html.contains("<form"), "{html}");
    }
}
