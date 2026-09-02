//! Client testimonial cards, as a Dioxus component (issue #641, Phase 2).
//!
//! The successor to the `views::components::testimonial`. A two-column
//! grid of quote cards for the firm's service pages: an optional product label,
//! the quote, and an attribution row with either a profile image or a
//! generated-initials avatar. Styled by the Dioxus Components theme.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use super::avatar::initials;

/// One testimonial card.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestimonialCard {
    pub quote: String,
    pub attribution: String,
    pub detail: Option<String>,
    pub profile_image_url: Option<String>,
    pub product_label: Option<String>,
}

/// A titled section wrapping a responsive grid of [`TestimonialCard`]s. Renders
/// nothing when `cards` is empty.
#[component]
pub fn TestimonialSection(heading: String, lead: String, cards: Vec<TestimonialCard>) -> Element {
    if cards.is_empty() {
        return rsx! {};
    }
    rsx! {
        section { class: "testimonial-section",
            div { class: "testimonial-section__head",
                h2 { "{heading}" }
                p { class: "nav-text-muted", "{lead}" }
            }
            div { class: "testimonial-grid",
                for card in cards.iter() {
                    article { class: "nav-card testimonial-card",
                        div { class: "nav-card__body testimonial-card__body",
                            if let Some(label) = &card.product_label {
                                p { class: "testimonial-card__label", "{label}" }
                            }
                            blockquote { class: "testimonial-card__quote",
                                p { "\u{201C}{card.quote}\u{201D}" }
                            }
                            div { class: "testimonial-card__by",
                                if let Some(url) = &card.profile_image_url {
                                    img {
                                        class: "testimonial-card__avatar",
                                        src: "{url}",
                                        alt: "{card.attribution} profile image",
                                        width: "56",
                                        height: "56",
                                    }
                                } else {
                                    div {
                                        class: "testimonial-card__avatar testimonial-card__avatar--initials",
                                        "aria-hidden": "true",
                                        "{initials(&card.attribution)}"
                                    }
                                }
                                div {
                                    p { class: "testimonial-card__name", "{card.attribution}" }
                                    if let Some(detail) = &card.detail {
                                        p { class: "nav-text-muted testimonial-card__detail", "{detail}" }
                                    }
                                }
                            }
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

    #[test]
    fn renders_quote_and_initials_avatar_when_no_image() {
        fn app() -> Element {
            rsx! {
                TestimonialSection {
                    heading: "What clients say".to_string(),
                    lead: "Real outcomes.".to_string(),
                    cards: vec![TestimonialCard {
                        quote: "They opened my matter in a day.".to_string(),
                        attribution: "Aries Ram".to_string(),
                        detail: Some("LLC formation".to_string()),
                        profile_image_url: None,
                        product_label: Some("Namesake".to_string()),
                    }],
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("testimonial-card"), "{html}");
        assert!(html.contains("They opened my matter in a day."), "{html}");
        assert!(html.contains("initials"), "{html}");
        assert!(html.contains(">AR<") || html.contains("AR"), "{html}");
    }

    #[test]
    fn empty_cards_render_nothing() {
        fn app() -> Element {
            rsx! {
                TestimonialSection { heading: "x".to_string(), lead: "y".to_string(), cards: vec![] }
            }
        }
        assert!(!ssr(app).contains("testimonial-section"));
    }
}
