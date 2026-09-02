//! The firm `/contact` page, migrated to Dioxus SSR (#641 / #730 PR6 —
//! content-backed, per #811).
//!
//! Content-backed like the service pages: the portal router pre-resolves the
//! page's [`ContactContent`] from the mounted brand's addresses and injects it
//! via `ServeConfig::context_providers`; [`contact_page_view`] reads it back.
//! Firm-owned, and it publishes the firm's channels only.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The resolved contact content — the portal router builds it from the brand
/// addresses and injects it; the wasm-safe carrier across the server-function
/// boundary.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ContactContent {
    /// The full document `<title>` ("Neon Law | Contact"), pre-formatted
    /// portal-side to match the head.
    pub head_title: String,
    pub meta_description: String,
    /// The page `<h1>` (`contact.title`).
    pub page_title: String,
    pub email_label: String,
    pub phone_label: String,
    pub firm_email: String,
    /// The firm's published voice line, rendered as written and dialled from
    /// the `tel:` link beside it.
    pub firm_phone: String,
}

/// The [`ContactContent`] injected into the render context by the portal router.
#[derive(Clone, Default)]
pub struct InjectedContact(pub ContactContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ContactPageView {
    pub chrome: PublicChrome,
    pub content: ContactContent,
}

/// Resolve the chrome from the process brand and the contact content from the
/// injected [`InjectedContact`] context.
#[server]
pub async fn contact_page_view() -> Result<ContactPageView, ServerFnError> {
    let content = consume_context::<InjectedContact>().0;
    Ok(ContactPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn ContactPageEntry() -> Element {
    let resource = use_server_future(contact_page_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        ContactPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure contact page: the firm's contact section inside the public shell.
/// Prop-driven, so it server-renders and unit-tests without a server future.
#[component]
pub fn ContactPage(chrome: PublicChrome, content: ContactContent) -> Element {
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
    let firm_mailto = format!("mailto:{}", content.firm_email);
    // `tel:` dials digits, not the human spacing the number is written with.
    let firm_tel = format!(
        "tel:{}",
        content
            .firm_phone
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '+')
            .collect::<String>()
    );
    rsx! {
        document::Title { "{content.head_title}" }
        document::Meta { name: "description", content: "{content.meta_description}" }
        // The Open Graph / Twitter share card the contact head emitted.
        SocialMeta {
            title: content.head_title.clone(),
            description: content.meta_description.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        PublicShell { header, footer,
            article { class: "contact-page",
                h1 { "{content.page_title}" }
                section {
                    dl {
                        dt { "{content.email_label}" }
                        dd {
                            a { href: "{firm_mailto}", "{content.firm_email}" }
                        }
                        dt { "{content.phone_label}" }
                        dd {
                            a { href: "{firm_tel}", "{content.firm_phone}" }
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

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn html() -> String {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            let content = ContactContent {
                head_title: "Neon Law | Contact".to_string(),
                meta_description: "Reach the firm.".to_string(),
                page_title: "Contact".to_string(),
                email_label: "Email".to_string(),
                phone_label: "Phone".to_string(),
                firm_email: "support@example.com".to_string(),
                firm_phone: "+1 555 010 0100".to_string(),
            };
            rsx! { ContactPage { chrome, content } }
        }
        ssr(app)
    }

    #[test]
    fn renders_the_firm_contact_section_with_a_mailto_link() {
        let out = html();
        assert!(out.contains("Contact"), "page title: {out}");
        assert!(
            out.contains(r#"href="mailto:support@example.com""#),
            "firm mailto"
        );
    }

    #[test]
    fn publishes_the_firm_as_the_only_contact_channel() {
        let out = html();
        // The page is the firm's call to action: one inbox, not a directory of
        // the brands the firm is adjacent to.
        assert_eq!(
            out.matches("mailto:").count(),
            1,
            "exactly one mailto channel: {out}"
        );
        assert!(
            !out.contains("the foundation"),
            "no Foundation contact section: {out}"
        );
    }

    #[test]
    fn publishes_no_self_serve_booking_link() {
        let out = html();
        // The page routes a prospective client through the firm's inbox, which
        // answers with a quote and a calendar link. A self-serve booking button
        // would sell a priced appointment before anyone has read the matter.
        assert!(!out.contains("Book a"), "no booking call to action: {out}");
        assert!(
            !out.contains("Appointment"),
            "no priced appointment offer: {out}"
        );
    }

    #[test]
    fn renders_the_firm_phone_as_a_dialable_tel_link() {
        let out = html();
        // Written with spaces for a reader; dialled as digits.
        assert!(out.contains("+1 555 010 0100"), "phone as written: {out}");
        assert!(
            out.contains(r#"href="tel:+15550100100""#),
            "tel href: {out}"
        );
        assert!(out.contains("Phone"), "phone label");
    }

    #[test]
    fn publishes_no_github_link() {
        let out = html();
        assert!(
            !out.contains("github"),
            "no GitHub channel on contact: {out}"
        );
    }

    #[test]
    fn wraps_the_page_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome");
        assert!(
            out.contains(r#"class="contact-page""#),
            "contact article class"
        );
    }
}
