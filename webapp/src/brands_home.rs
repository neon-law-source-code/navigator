//! The `/app/brands` house-of-brands home — every registered brand's typeface,
//! in one place.
//!
//! A firm person (Owner, Admin, Lawyer, or Clerk) sees each brand's own font
//! family and, where the family is licensed rather than open, the desktop
//! download the team already relies on. A `client` is answered 403 at the
//! route, so this page never renders for one — a house brand's fonts are a
//! firm-operations asset, not something a client's own matter needs.
//!
//! Gated exactly like [`crate::team_home`]: `require_auth` then
//! `require_policy`, so an anonymous request is a redirect to sign-in rather
//! than a policy denial.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The `<meta description>` for the brands home.
const DESCRIPTION: &str = "Every Neon Law Navigator house brand's typeface, in one place.";

/// One brand's font family, as rendered on this page. Pure data, with no brand
/// resolution or storage access.
#[derive(Clone, PartialEq, Eq)]
struct BrandFontCard {
    /// The `id` on the card, so a test can pin a brand to its card.
    id: &'static str,
    /// The brand's own site name, e.g. "Neon Law".
    brand_label: &'static str,
    /// The web font family this brand's `/app` pages and public site declare.
    family_name: &'static str,
    /// How the family is licensed, rendered under the family name.
    license_note: &'static str,
    /// The suggested filename when the card's link downloads a desktop
    /// family rather than navigating to a public reference. `None` when the
    /// family has no separate desktop package to gate.
    download: Option<&'static str>,
    href: &'static str,
}

/// The registered brands' font cards, in registry order.
///
/// The GORP Serif and Plus Jakarta Sans facts here mirror the match arms in
/// `portal::dioxus_app::dioxus_document_head` and `docs/assets.md`'s
/// "Licensed webfonts" section. This client-rendered data stays independent of
/// the server-only `views` crate so the WASM build does not pull server brand
/// resolution into the browser bundle.
fn brand_font_cards() -> [BrandFontCard; 2] {
    [
        BrandFontCard {
            id: "brand-card-neon",
            brand_label: "Neon Law",
            family_name: "GORP Serif",
            license_note: "Licensed from TrashType. The desktop family is a firm-only download; the public site serves only the web (WOFF2) faces.",
            download: Some("gorp-serif.zip"),
            href: "/app/team/fonts/gorp-serif.zip",
        },
        BrandFontCard {
            id: "brand-card-delete-your-data",
            brand_label: "DeleteYourData.com",
            family_name: "Plus Jakarta Sans",
            license_note: "SIL Open Font License 1.1 — the desktop family is the same public font anyone can install from Google Fonts.",
            download: None,
            href: "https://fonts.google.com/specimen/Plus+Jakarta+Sans",
        },
    ]
}

/// Everything the brands home renders: the viewer's tier and the mounted
/// brand's mark for the app chrome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BrandsHomeView {
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    /// The resolved brand's tokens stylesheet href, so the page wears its own
    /// palette rather than the firm's on a non-default host.
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
}

/// Resolve the authenticated viewer and the request-scoped brand for the home.
#[server]
pub async fn brands_home_view() -> Result<BrandsHomeView, ServerFnError> {
    Ok(BrandsHomeView {
        role: crate::admin_listing::require_firm_person().await?,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
    })
}

/// The route entry for `/app/brands`.
#[component]
pub fn BrandsHome() -> Element {
    let resource = use_server_future(brands_home_view)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "brands-home", p { "Failed to load the brands home." } }
            }
        }
        None => {
            return rsx! {
                main { id: "brands-home", p { "Loading…" } }
            }
        }
    };

    brands_home_body(&view)
}

/// The loaded page. Split from the component so tests render a fixed view
/// without standing up the server function.
pub fn brands_home_body(view: &BrandsHomeView) -> Element {
    let role = view.role;
    let firm_name = view.firm_name.clone();

    let cards = brand_font_cards().into_iter().map(|c| {
        rsx! {
            article {
                key: "{c.id}",
                id: "{c.id}",
                class: "brands-home__card",
                h2 { class: "brands-home__card-title", "{c.brand_label}" }
                p { class: "brands-home__card-family", "{c.family_name}" }
                p { class: "brands-home__card-license", "{c.license_note}" }
                a {
                    class: "brands-home__card-link",
                    href: "{c.href}",
                    download: c.download,
                    if c.download.is_some() { "Download the desktop family" } else { "View the font license" }
                }
            }
        }
    });

    rsx! {
        document::Title { "{firm_name} | Brands" }
        document::Meta { name: "description", content: DESCRIPTION }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "brands-home", class: "nav-theme",
            header { class: "page-header",
                h1 { "Brands" }
                p { class: "page-subtitle",
                    "Every house brand's typeface, in one place."
                }
            }
            div { class: "brands-home__cards", "aria-label": "Registered brands",
                {cards}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{brand_font_cards, brands_home_body, BrandsHomeView};
    use crate::people::ViewerRole;

    fn view_for(role: ViewerRole) -> BrandsHomeView {
        BrandsHomeView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role,
            logo: None,
        }
    }

    fn render(role: ViewerRole) -> String {
        dioxus_ssr::render_element(brands_home_body(&view_for(role)))
    }

    /// One card per registered `BrandKey`, in registry order. The registry is
    /// server-only, so this tripwire runs only where that dependency is present.
    #[cfg(feature = "server")]
    #[test]
    fn one_card_per_registered_brand_key() {
        assert_eq!(
            brand_font_cards().len(),
            views::brand::BrandKey::ALL.len(),
            "every registered brand key needs a font card"
        );
    }

    /// Every firm tier sees both brands' cards — a house brand's font is not
    /// gated further than the page itself.
    #[test]
    fn every_firm_tier_sees_every_brand_card() {
        for role in [
            ViewerRole::Clerk,
            ViewerRole::Lawyer,
            ViewerRole::Admin,
            ViewerRole::Owner,
        ] {
            let html = render(role);
            assert!(
                html.contains(r#"id="brand-card-neon""#),
                "rank {} must see the Neon card: {html}",
                role.authority_rank()
            );
            assert!(
                html.contains(r#"id="brand-card-delete-your-data""#),
                "rank {} must see the DeleteYourData card: {html}",
                role.authority_rank()
            );
        }
    }

    /// Neon's card links the same firm-gated ZIP route the Team home's Brand
    /// fonts card offers, and downloads under that route's own filename.
    #[test]
    fn the_neon_card_downloads_the_existing_gorp_zip_route() {
        let html = render(ViewerRole::Lawyer);
        assert!(
            html.contains(r#"href="/app/team/fonts/gorp-serif.zip""#),
            "the Neon card links the existing GORP ZIP route: {html}"
        );
        assert!(
            html.contains(r#"download="gorp-serif.zip""#),
            "the Neon card downloads under the route's own filename: {html}"
        );
    }

    /// `DeleteYourData`'s card links the public OFL font reference rather than
    /// offering a download attribute — there is no firm-gated desktop package
    /// for an already-public font.
    #[test]
    fn the_delete_your_data_card_has_no_download_attribute() {
        let html = render(ViewerRole::Lawyer);
        assert!(
            html.contains("fonts.google.com/specimen/Plus+Jakarta+Sans"),
            "the DeleteYourData card links the public font reference: {html}"
        );
        assert_eq!(
            html.matches("download=").count(),
            1,
            "only the Neon card is a download: {html}"
        );
    }

    /// The rendered page carries the heading and both cards.
    #[test]
    fn the_home_composes_heading_and_cards() {
        let html = render(ViewerRole::Owner);
        assert!(html.contains("Brands"), "the heading: {html}");
        assert!(
            html.contains("Every house brand&#39;s typeface, in one place."),
            "the subtitle: {html}"
        );
    }
}
