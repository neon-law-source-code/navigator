//! `/{category}/{slug}/certificate/sent` — the neutral
//! confirmation a learner lands on after asking for their completion
//! certificate, migrated to Dioxus SSR (#956 Phase 4).
//!
//! Its predecessor was the certificate POST's own `200` response body, not
//! a page anyone could navigate to. That shape had a real cost: a reload
//! re-submitted the form and dispatched a second certificate. This is
//! post/redirect/get instead — the POST answers `303` here and this route
//! renders, so a refresh re-renders the confirmation rather than re-sending the
//! email.
//!
//! The copy is deliberately identical for every request. Completion is
//! client-trusted (localStorage, no telemetry) and the handler swallows a
//! dispatch failure, so this page must never reveal whether an address was
//! reached — it is a courtesy, not a receipt.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, CATALOG_STYLESHEET_HREF};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// Everything the confirmation renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CertificateSentContent {
    pub workshop_title: String,
    /// Back to the material hub.
    pub material_href: String,
}

/// The [`CertificateSentContent`] the portal pre-layer injects.
#[derive(Clone, Default)]
pub struct InjectedCertificateSent(pub CertificateSentContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CertificateSentView {
    pub chrome: PublicChrome,
    pub content: CertificateSentContent,
}

/// Resolve the shared chrome and which workshop was completed.
#[server]
pub async fn certificate_sent_view() -> Result<CertificateSentView, ServerFnError> {
    let content = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedCertificateSent>,
        _,
    >()
    .await
    .map(|axum::Extension(c)| c.0)
    .unwrap_or_default();
    Ok(CertificateSentView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn CertificateSentEntry() -> Element {
    let resource = use_server_future(certificate_sent_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        CertificateSentPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure confirmation.
#[component]
pub fn CertificateSentPage(chrome: PublicChrome, content: CertificateSentContent) -> Element {
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
        document::Title { "{chrome.brand_name} | Certificate on its way" }
        document::Meta {
            name: "description",
            content: "Your workshop completion certificate is on its way.",
        }
        document::Stylesheet { href: CATALOG_STYLESHEET_HREF }
        PublicShell { header, footer,
            article { class: "workshop-cert-sent",
                h1 { "Check your inbox" }
                p { class: "lede",
                    "Your certificate for "
                    em { "{content.workshop_title}" }
                    " is on its way from {chrome.firm_name}."
                }
                p {
                    "It can take a minute to arrive — and it's worth checking your spam folder."
                }
                a { class: "nav-btn nav-btn--secondary", href: "{content.material_href}",
                    "← Back to the workshop"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                CertificateSentPage {
                    chrome: PublicChrome::default(),
                    content: CertificateSentContent {
                        workshop_title: "Using Neon Law Navigator".into(),
                        material_href: "/workshops/use-the-navigator".into(),
                    },
                }
            }
        }
        ssr(app)
    }

    #[test]
    fn the_confirmation_names_the_workshop_and_offers_the_way_back() {
        let out = html();
        assert!(out.contains(">Check your inbox<"), "headline: {out}");
        assert!(
            out.contains(">Using Neon Law Navigator<"),
            "workshop title: {out}"
        );
        assert!(
            out.contains(r#"href="/workshops/use-the-navigator""#),
            "back to the hub: {out}"
        );
    }

    #[test]
    fn the_confirmation_reveals_nothing_about_the_recipient() {
        // Neutral by construction: nothing the submitter typed reaches this
        // page, so it cannot become a delivery receipt or an address oracle.
        let out = html();
        assert!(!out.contains('@'), "no address anywhere: {out}");
        assert!(!out.contains("<form"), "nothing to re-submit: {out}");
    }
}
