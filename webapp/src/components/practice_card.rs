//! The linked practice card shared by the firm homepage and presentation slides.
//!
//! The card owns the semantic anchor and its decorative line mark. Page modules
//! decide how cards are arranged; `home.css` supplies the one visual treatment
//! so a card in a talk is the same component a visitor meets on `/`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Which decorative line mark opens a practice card.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum PracticeMark {
    /// The scales, for litigation.
    #[default]
    Scales,
    /// The handshake, for company-counsel work.
    Handshake,
    /// The gavel, for routine one-time matters.
    Gavel,
    /// Angle brackets around a circuit node, for technology leadership.
    Technology,
    /// A ship's wheel, for Neon Law Navigator.
    Helm,
}

impl PracticeMark {
    /// Stable identifier for the meaning carried by this mark.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scales => "scales",
            Self::Handshake => "handshake",
            Self::Gavel => "gavel",
            Self::Technology => "technology",
            Self::Helm => "helm",
        }
    }
}

/// One linked firm-practice card.
#[component]
pub(crate) fn PracticeCard(
    mark: PracticeMark,
    heading: String,
    body: String,
    href: String,
    heading_id: String,
) -> Element {
    rsx! {
        a {
            class: "neon-card home-practice",
            href: "{href}",
            "aria-labelledby": "{heading_id}",
            PracticeMarkGlyph {
                mark,
                class: "home-practice__mark".to_string(),
            }
            h3 { id: "{heading_id}", class: "home-practice__heading", "{heading}" }
            if !body.is_empty() {
                p { class: "home-practice__body", "{body}" }
            }
        }
    }
}

/// Draw one practice mark, stroked in `currentColor` and hidden from assistive
/// technology because the adjacent heading already names the card.
#[component]
pub(crate) fn PracticeMarkGlyph(mark: PracticeMark, #[props(default)] class: String) -> Element {
    rsx! {
        svg {
            class: "{class}",
            "data-practice-mark": mark.name(),
            "viewBox": "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "1.5",
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
            "aria-hidden": "true",
            "focusable": "false",
            match mark {
                PracticeMark::Scales => rsx! {
                    path { d: "m16 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z" }
                    path { d: "m2 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z" }
                    path { d: "M7 21h10" }
                    path { d: "M12 3v18" }
                    path { d: "M3 7h2c2 0 5-1 7-2 2 1 5 2 7 2h2" }
                },
                PracticeMark::Handshake => rsx! {
                    path { d: "m11 17 2 2a1 1 0 1 0 3-3" }
                    path { d: "m14 14 2.5 2.5a1 1 0 1 0 3-3l-3.88-3.88a3 3 0 0 0-4.24 0l-.88.88a1 1 0 1 1-3-3l2.81-2.81a5.79 5.79 0 0 1 7.06-.87l.47.28a2 2 0 0 0 1.42.25L21 4" }
                    path { d: "m21 3 1 11h-2" }
                    path { d: "M3 3 2 14l6.5 6.5a1 1 0 1 0 3-3" }
                    path { d: "M3 4h8" }
                },
                PracticeMark::Gavel => rsx! {
                    path { d: "m14.5 12.5-8 8a2.119 2.119 0 1 1-3-3l8-8" }
                    path { d: "m16 16 6-6" }
                    path { d: "m8 8 6-6" }
                    path { d: "m9 7 8 8" }
                    path { d: "m21 11-8-8" }
                },
                PracticeMark::Technology => rsx! {
                    path { d: "m8 7-5 5 5 5" }
                    path { d: "m16 7 5 5-5 5" }
                    circle { cx: "12", cy: "12", r: "2.25" }
                    path { d: "M12 3v6.75M12 14.25V21" }
                },
                PracticeMark::Helm => rsx! {
                    circle { cx: "12", cy: "12", r: "3.25" }
                    circle { cx: "12", cy: "12", r: "7" }
                    path { d: "M12 2v3M12 19v3M2 12h3M19 12h3" }
                    path { d: "m4.93 4.93 2.12 2.12m9.9 9.9 2.12 2.12m0-14.14-2.12 2.12m-9.9 9.9-2.12 2.12" }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_is_one_named_link_and_omits_an_empty_body() {
        fn app() -> Element {
            rsx! {
                PracticeCard {
                    mark: PracticeMark::Technology,
                    heading: "Personal Plan".to_string(),
                    body: String::new(),
                    href: "/personal-plan".to_string(),
                    heading_id: "practice-pp".to_string(),
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(
            html.contains(r#"class="neon-card home-practice""#),
            "{html}"
        );
        assert!(html.contains(r#"href="/personal-plan""#), "{html}");
        assert!(html.contains(r#"aria-labelledby="practice-pp""#), "{html}");
        assert!(html.contains("Personal Plan"), "{html}");
        assert!(
            html.contains(r#"data-practice-mark="technology""#),
            "{html}"
        );
        assert!(!html.contains("home-practice__body"), "{html}");
    }

    #[test]
    fn the_navigator_mark_is_a_ship_wheel() {
        fn app() -> Element {
            rsx! {
                PracticeMarkGlyph {
                    mark: PracticeMark::Helm,
                    class: "navigator-mark".to_string(),
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains(r#"data-practice-mark="helm""#), "{html}");
        assert!(html.contains("navigator-mark"), "{html}");
        assert_eq!(html.matches("<circle").count(), 2, "{html}");
        assert!(
            html.contains(r#"d="M12 2v3M12 19v3M2 12h3M19 12h3""#),
            "{html}"
        );
    }
}
