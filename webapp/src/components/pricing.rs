//! Flat-fee pricing / offer cards, as a Dioxus component (issue #641, Phase 2).
//!
//! The successor to the `views::components::pricing`. Every card shares one
//! treatment — the cyan header band carrying the flat-fee label, an outcome-led
//! title, the headline price, a blurb, optional inclusion bullets, and a solid
//! CTA. There is intentionally no "most popular" badge (an unsubstantiable
//! popularity claim trips attorney-advertising rules). An off-site `http(s)` CTA
//! opens in a new tab with the OWASP `rel` pair; an on-site target stays a plain
//! link.

use dioxus::prelude::*;

use crate::components::{Icon, IconName};

/// One pricing / offer card. Owned strings so it crosses the server→client
/// boundary; the owning marketing content is mapped onto this per request.
#[derive(Clone, PartialEq)]
pub struct PricingCard {
    pub title: String,
    pub price: String,
    pub cadence: Option<String>,
    pub blurb: String,
    pub features: Vec<String>,
    pub cta_label: String,
    pub cta_href: String,
    /// Label for the cyan band; falls back to the price when `None`.
    pub featured_label: Option<String>,
}

/// A responsive row of pricing cards. `cols_lg` is the desktop column count
/// (`1` stacks one per row at every breakpoint); cards are equal height.
#[component]
pub fn PricingSection(cards: Vec<PricingCard>, cols_lg: u8) -> Element {
    let cols = cols_lg.clamp(1, 4);
    rsx! {
        div {
            class: "pricing-grid",
            style: "--pricing-cols: {cols};",
            for card in cards.iter() {
                {pricing_card(card)}
            }
        }
    }
}

/// One rendered card.
fn pricing_card(card: &PricingCard) -> Element {
    let band_label = card
        .featured_label
        .clone()
        .unwrap_or_else(|| card.price.clone());
    let is_external = card.cta_href.starts_with("http://") || card.cta_href.starts_with("https://");
    rsx! {
        div { class: "nav-card pricing-card",
            div { class: "nav-card__header pricing-card__band", "{band_label}" }
            div { class: "nav-card__body pricing-card__body",
                h3 { class: "pricing-card__title", "{card.title}" }
                p { class: "pricing-card__price",
                    span { class: "pricing-card__amount", "{card.price}" }
                    if let Some(cadence) = &card.cadence {
                        " "
                        span { class: "pricing-card__cadence nav-text-muted", "{cadence}" }
                    }
                }
                p { class: "nav-text-muted", "{card.blurb}" }
                if !card.features.is_empty() {
                    ul { class: "pricing-card__features",
                        for feature in card.features.iter() {
                            li {
                                span { class: "pricing-card__check",
                                    Icon { name: IconName::CheckLg }
                                }
                                "{feature}"
                            }
                        }
                    }
                }
                if is_external {
                    a {
                        class: "nav-btn nav-btn--primary pricing-card__cta",
                        href: "{card.cta_href}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "{card.cta_label}"
                        " "
                        Icon { name: IconName::BoxArrowUpRight }
                    }
                } else {
                    a {
                        class: "nav-btn nav-btn--primary pricing-card__cta",
                        href: "{card.cta_href}",
                        "{card.cta_label}"
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

    fn sample() -> PricingCard {
        PricingCard {
            title: "Living trust".to_string(),
            price: "$3,500".to_string(),
            cadence: None,
            blurb: "For planning your legacy.".to_string(),
            features: vec!["Attorney-drafted".to_string()],
            cta_label: "Get started".to_string(),
            cta_href: "https://cal.example/book".to_string(),
            featured_label: Some("$3,500, once".to_string()),
        }
    }

    #[test]
    fn renders_band_title_price_and_features() {
        fn app() -> Element {
            rsx! { PricingSection { cards: vec![sample()], cols_lg: 3 } }
        }
        let html = ssr(app);
        assert!(html.contains("pricing-card"), "{html}");
        assert!(html.contains("$3,500, once"), "band label: {html}");
        assert!(html.contains("Living trust"), "{html}");
        assert!(html.contains("Attorney-drafted"), "{html}");
        assert!(
            html.contains("nav-icon"),
            "check icon is inline SVG: {html}"
        );
    }

    #[test]
    fn external_cta_opens_new_tab_with_owasp_rel() {
        fn app() -> Element {
            rsx! { PricingSection { cards: vec![sample()], cols_lg: 3 } }
        }
        let html = ssr(app);
        assert!(html.contains(r#"target="_blank""#), "{html}");
        assert!(html.contains(r#"rel="noopener noreferrer""#), "{html}");
        // Off-site CTAs keep the "opens off-site" arrow cue (box-arrow-up-right).
        assert!(html.contains("M8.636 3.5"), "external arrow glyph: {html}");
    }

    #[test]
    fn on_site_cta_stays_a_plain_link() {
        fn app() -> Element {
            let mut card = sample();
            card.cta_href = "mailto:contact@neonlaw.com".to_string();
            rsx! { PricingSection { cards: vec![card], cols_lg: 1 } }
        }
        let html = ssr(app);
        assert!(
            html.contains(r#"href="mailto:contact@neonlaw.com""#),
            "{html}"
        );
        assert!(!html.contains(r#"target="_blank""#), "{html}");
        // On-site CTAs stay plain: no off-site arrow cue.
        assert!(!html.contains("M8.636 3.5"), "no external arrow: {html}");
    }
}
