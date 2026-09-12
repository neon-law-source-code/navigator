//! The `/app/admin/brands` house-of-brands home — every registered `brand` row
//! (ENG-586), system-wide and Firm-scoped alike.
//!
//! Owner and Admin. A lawyer who works under a brand still sees it on every
//! page they render, just not this registry view. Lawyer and Clerk are
//! answered 403 at the route, so this page never renders for them.
//!
//! Gated like the other `/app/admin` desks: `require_auth` then
//! `require_policy` at the router, `require_admin` in the loader — so an
//! anonymous request is a redirect to sign-in, and an authenticated
//! non-admin-tier caller is a `403` rather than a rendered page.
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The `<meta description>` for the brands home.
const DESCRIPTION: &str = "Every brand registered on this Navigator deployment.";

/// One `brand` row, as rendered on this page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct BrandCard {
    pub key: String,
    pub name: String,
    /// "System-wide" for a `firm_id: None` row, else the owning Firm's name.
    pub owner_label: String,
    /// The stored `#rrggbb` hex, when set — rendered as a swatch.
    pub primary_color: Option<String>,
    /// The typeface catalog id, or the uploaded font's family name.
    pub font_label: String,
    pub has_logo: bool,
    pub edit_href: String,
}

/// Everything the brands home renders: every registered brand, the viewer's
/// tier, and the mounted brand's mark for the app chrome.
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
    #[serde(default)]
    pub cards: Vec<BrandCard>,
}

#[cfg(feature = "server")]
fn font_label(brand: &store::brands::Brand) -> String {
    if brand.typeface.as_deref() == Some("uploaded") {
        brand
            .font_family
            .clone()
            .unwrap_or_else(|| "Uploaded font (no family set)".to_string())
    } else {
        brand
            .typeface
            .clone()
            .unwrap_or_else(|| "Compiled default".to_string())
    }
}

/// Resolve the Admin-tier viewer and every registered brand, system-wide and
/// Firm-scoped alike.
///
/// A hidden link is not an authorization boundary, so this handler-level gate
/// refuses Lawyer and Clerk, matching the `/app/admin` route bypass. The store
/// still refuses a non-DRI Admin on a Firm-scoped write.
#[server]
pub async fn brands_home_view() -> Result<BrandsHomeView, ServerFnError> {
    let role = crate::admin_listing::require_admin().await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();

    let mut brands = store::brands::system_wide(&surreal)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?;
    brands.extend(
        store::brands::all_firm_scoped(&surreal)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?,
    );

    let mut cards = Vec::with_capacity(brands.len());
    for brand in brands {
        let owner_label = match brand.firm_id {
            None => "System-wide".to_string(),
            Some(firm_id) => store::firms::find_by_id(&surreal, firm_id)
                .await
                .map_err(|error| ServerFnError::new(error.to_string()))?
                .map_or_else(|| "Unknown firm".to_string(), |firm| firm.name),
        };
        cards.push(BrandCard {
            key: brand.key.clone(),
            name: brand.name.clone(),
            owner_label,
            primary_color: brand.primary_color.clone(),
            font_label: font_label(&brand),
            has_logo: brand.logo_object_key.is_some(),
            edit_href: format!("{}/{}/edit", crate::app_chrome::APP_BRANDS_HREF, brand.key),
        });
    }

    Ok(BrandsHomeView {
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        cards,
    })
}

/// The route entry for `/app/admin/brands`.
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

    let cards = view.cards.iter().map(|c| {
        let swatch_style = c
            .primary_color
            .as_deref()
            .map(|hex| format!("background-color: {hex};"));
        rsx! {
            article {
                key: "{c.key}",
                id: "brand-card-{c.key}",
                class: "brands-home__card",
                h2 { class: "brands-home__card-title", "{c.name}" }
                p { class: "brands-home__card-owner", "{c.owner_label}" }
                if let Some(style) = swatch_style {
                    span { class: "brands-home__swatch", style: "{style}" }
                }
                p { class: "brands-home__card-family", "{c.font_label}" }
                p { class: "brands-home__card-logo",
                    if c.has_logo { "Logo uploaded" } else { "No logo uploaded" }
                }
                a {
                    class: "brands-home__card-link",
                    href: "{c.edit_href}",
                    "Edit"
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
                    "Every brand registered on this deployment, system-wide and Firm-scoped."
                }
                p { a { class: "nav-btn nav-btn--primary", href: APP_BRAND_NEW_HREF, "New brand" } }
            }
            div { class: "brands-home__cards", "aria-label": "Registered brands",
                if view.cards.is_empty() {
                    p { class: "page-subtitle", "No brands are registered on this deployment." }
                } else {
                    {cards}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{brands_home_body, BrandCard, BrandsHomeView};
    use crate::people::ViewerRole;

    fn view(cards: Vec<BrandCard>) -> BrandsHomeView {
        BrandsHomeView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Owner,
            logo: None,
            cards,
        }
    }

    fn render(cards: Vec<BrandCard>) -> String {
        dioxus_ssr::render_element(brands_home_body(&view(cards)))
    }

    #[test]
    fn lists_a_brand_row_with_its_owner_font_and_logo_status() {
        let html = render(vec![BrandCard {
            key: "neon".to_string(),
            name: "Neon Law".to_string(),
            owner_label: "System-wide".to_string(),
            primary_color: Some("#007c91".to_string()),
            font_label: "gorp-serif".to_string(),
            has_logo: false,
            edit_href: "/app/admin/brands/neon/edit".to_string(),
        }]);
        assert!(html.contains(r#"id="brand-card-neon""#), "{html}");
        assert!(html.contains("System-wide"), "{html}");
        assert!(html.contains("gorp-serif"), "{html}");
        assert!(html.contains("No logo uploaded"), "{html}");
        assert!(html.contains("background-color: #007c91"), "{html}");
        assert!(
            html.contains(r#"href="/app/admin/brands/neon/edit""#),
            "{html}"
        );
    }

    #[test]
    fn a_firm_scoped_brand_names_its_owning_firm() {
        let html = render(vec![BrandCard {
            key: "acme-brand".to_string(),
            name: "Acme Brand".to_string(),
            owner_label: "Acme Practice".to_string(),
            primary_color: None,
            font_label: "Custom Sans".to_string(),
            has_logo: true,
            edit_href: "/app/admin/brands/acme-brand/edit".to_string(),
        }]);
        assert!(html.contains("Acme Practice"), "{html}");
        assert!(html.contains("Logo uploaded"), "{html}");
    }

    #[test]
    fn empty_inventory_still_renders_the_heading_and_new_brand_link() {
        let html = render(Vec::new());
        assert!(html.contains("Brands"), "{html}");
        assert!(
            html.contains("No brands are registered on this deployment."),
            "{html}"
        );
        assert!(html.contains(r#"href="/app/admin/brands/new""#), "{html}");
    }
}
