//! The `/app/owner` listing — every practice and the house brands it wears.
//!
//! Owner only. Admin is denied here: an Admin is scoped to the firms they
//! belong to, and this page is the deployment-wide inventory of those firms.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::people::ViewerRole;

/// The `<meta description>` for the owner listing.
const DESCRIPTION: &str =
    "Every practice on this Navigator deployment, and the house brands each one wears.";

/// One practice on the listing, with the entity it is and the brand keys it wears.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FirmCard {
    pub id: String,
    pub name: String,
    pub status: String,
    pub entity_name: String,
    pub brand_keys: Vec<String>,
}

/// Everything the owner listing renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct OwnerHomeView {
    pub role: ViewerRole,
    #[serde(default)]
    pub logo: Option<crate::components::AppLogo>,
    #[serde(default)]
    pub tokens_href: String,
    #[serde(default)]
    pub firm_name: String,
    #[serde(default)]
    pub firms: Vec<FirmCard>,
}

/// Resolve the Owner viewer and every practice.
#[server]
pub async fn owner_home_view() -> Result<OwnerHomeView, ServerFnError> {
    let role = crate::admin_listing::require_owner().await?;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let firms = store::firms::all(&surreal)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?;
    let mut cards = Vec::new();
    for firm in firms {
        let entity_name = match firm.entity_id {
            Some(entity_id) => store::entities::find_by_id(&surreal, entity_id)
                .await
                .map_err(|error| ServerFnError::new(error.to_string()))?
                .map_or_else(|| "Unlinked entity".to_string(), |entity| entity.name),
            None => "Unlinked entity".to_string(),
        };
        let brand_keys = store::firms::brand_keys_for_firm(&surreal, firm.id)
            .await
            .map_err(|error| ServerFnError::new(error.to_string()))?;
        cards.push(FirmCard {
            id: firm.id.to_string(),
            name: firm.name,
            status: firm.status,
            entity_name,
            brand_keys,
        });
    }
    Ok(OwnerHomeView {
        role,
        logo: crate::app_chrome::app_logo_from_context().await,
        tokens_href: crate::app_chrome::app_tokens_href_from_context().await,
        firm_name: crate::app_chrome::firm_name_from_context().await,
        firms: cards,
    })
}

/// The route entry for `/app/owner`.
#[component]
pub fn OwnerHome() -> Element {
    let resource = use_server_future(owner_home_view)?;

    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        Some(Err(_)) => {
            return rsx! {
                main { id: "owner-home", p { "Failed to load the owner listing." } }
            }
        }
        None => {
            return rsx! {
                main { id: "owner-home", p { "Loading…" } }
            }
        }
    };

    owner_home_body(&view)
}

/// The loaded page. Split from the component so tests render a fixed view.
pub fn owner_home_body(view: &OwnerHomeView) -> Element {
    let role = view.role;
    let firm_name = view.firm_name.clone();
    let cards = view.firms.iter().map(|firm| {
        let brands = if firm.brand_keys.is_empty() {
            "No house brands attached.".to_string()
        } else {
            firm.brand_keys.join(", ")
        };
        rsx! {
            article {
                key: "{firm.id}",
                id: "firm-card-{firm.id}",
                class: "team-home__card",
                h2 { class: "team-home__card-title", "{firm.name}" }
                p { class: "team-home__card-desc",
                    "Entity: {firm.entity_name}. Status: {firm.status}."
                }
                p { class: "team-home__card-desc", "Brands: {brands}" }
            }
        }
    });

    rsx! {
        document::Title { "{firm_name} | Owner" }
        document::Meta { name: "description", content: DESCRIPTION }
        document::Stylesheet { href: crate::components::THEME_STYLESHEET_HREF }
        document::Stylesheet { href: "{view.tokens_href}" }
        crate::components::AppNavbar {
            destinations: crate::app_chrome::app_destinations(role),
            logo: view.logo.clone(),
        }
        main { id: "owner-home", class: "nav-theme",
            header { class: "page-header",
                h1 { "Firms" }
                p { class: "page-subtitle",
                    "Every practice on this deployment, and the house brands each one wears."
                }
            }
            div { class: "team-home__cards", "aria-label": "Firms",
                if view.firms.is_empty() {
                    p { class: "page-subtitle", "No practices are recorded on this deployment." }
                } else {
                    {cards}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{owner_home_body, FirmCard, OwnerHomeView};
    use crate::people::ViewerRole;

    fn render(firms: Vec<FirmCard>) -> String {
        dioxus_ssr::render_element(owner_home_body(&OwnerHomeView {
            tokens_href: String::new(),
            firm_name: "Neon Law".to_string(),
            role: ViewerRole::Owner,
            logo: None,
            firms,
        }))
    }

    #[test]
    fn lists_a_firm_with_its_entity_and_brands() {
        let html = render(vec![FirmCard {
            id: "firm-1".to_string(),
            name: "Shook Law PLLC".to_string(),
            status: "active".to_string(),
            entity_name: "Shook Law PLLC".to_string(),
            brand_keys: vec!["neon".to_string(), "delete-your-data".to_string()],
        }]);
        assert!(html.contains("Shook Law PLLC"), "{html}");
        assert!(html.contains("Entity: Shook Law PLLC"), "{html}");
        assert!(html.contains("neon, delete-your-data"), "{html}");
        assert!(html.contains(r#"id="owner-home""#), "{html}");
    }

    #[test]
    fn empty_inventory_still_renders_the_heading() {
        let html = render(Vec::new());
        assert!(html.contains("Every practice on this deployment"), "{html}");
        assert!(
            html.contains("No practices are recorded on this deployment."),
            "{html}"
        );
        assert!(!html.contains("firm-card-"), "{html}");
    }
}
