//! Death & Divorce: a black-and-white page for endings, transitions, and what
//! comes after.

use super::HomeContent;
use crate::components::is_external_href;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Copy loaded from the Death & Divorce home catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DeathAndDivorceContent {
    pub eyebrow: String,
    pub statement_heading: String,
    pub statement_body: String,
    pub practices: Vec<[String; 2]>,
    pub video_label: String,
    pub video_body: String,
    pub process_label: String,
    pub process_heading: String,
    pub steps: Vec<[String; 2]>,
    pub closing_heading: String,
    pub closing_body: String,
}

#[component]
pub(super) fn DeathAndDivorceHome(
    content: HomeContent,
    death_and_divorce: DeathAndDivorceContent,
) -> Element {
    let contact_external = is_external_href(&content.contact_href);
    rsx! {
        div { class: "death-and-divorce-home",
            section { class: "death-and-divorce-hero", "aria-labelledby": "death-and-divorce-title",
                div { class: "death-and-divorce-hero__copy",
                    p { class: "death-and-divorce-eyebrow", "{death_and_divorce.eyebrow}" }
                    h1 { id: "death-and-divorce-title", "{content.heading}" }
                    p { class: "death-and-divorce-hero__lead", "{content.lead}" }
                    a {
                        class: "nav-btn nav-btn--primary death-and-divorce-cta",
                        href: "{content.contact_href}",
                        target: if contact_external { Some("_blank") } else { None },
                        rel: if contact_external { Some("noopener noreferrer") } else { None },
                        "{content.contact_label}"
                    }
                }
                div { class: "death-and-divorce-hero__mark", aria_hidden: "true",
                    img {
                        src: "/public/brand/death-and-divorce/mark.svg",
                        alt: "",
                        width: "160",
                        height: "160",
                    }
                }
            }
            section { class: "death-and-divorce-statement", "aria-labelledby": "death-and-divorce-statement-title",
                p { class: "death-and-divorce-eyebrow", "{death_and_divorce.eyebrow}" }
                h2 { id: "death-and-divorce-statement-title", "{death_and_divorce.statement_heading}" }
                p { "{death_and_divorce.statement_body}" }
            }
            section { class: "death-and-divorce-practices", "aria-labelledby": "death-and-divorce-practices-title",
                h2 { id: "death-and-divorce-practices-title", "The work" }
                div { class: "death-and-divorce-practices__grid",
                    for (index, practice) in death_and_divorce.practices.iter().enumerate() {
                        article { key: "{index}",
                            h3 { "{practice[0]}" }
                            p { "{practice[1]}" }
                        }
                    }
                }
            }
            section { class: "death-and-divorce-video", "aria-labelledby": "death-and-divorce-video-title",
                div { class: "death-and-divorce-video__copy",
                    p { class: "death-and-divorce-eyebrow", "{death_and_divorce.video_label}" }
                    h2 { id: "death-and-divorce-video-title", "A quiet introduction" }
                    p { "{death_and_divorce.video_body}" }
                }
                div { class: "death-and-divorce-video__frame", role: "img", aria_label: "{death_and_divorce.video_label}",
                    span { class: "death-and-divorce-video__play", aria_hidden: "true", "▶" }
                    span { "Video coming soon" }
                }
            }
            section { class: "death-and-divorce-process", "aria-labelledby": "death-and-divorce-process-title",
                p { class: "death-and-divorce-eyebrow", "{death_and_divorce.process_label}" }
                h2 { id: "death-and-divorce-process-title", "{death_and_divorce.process_heading}" }
                ol {
                    for step in &death_and_divorce.steps {
                        li {
                            h3 { "{step[0]}" }
                            p { "{step[1]}" }
                        }
                    }
                }
            }
            section { class: "death-and-divorce-closing", "aria-labelledby": "death-and-divorce-closing-title",
                div {
                    h2 { id: "death-and-divorce-closing-title", "{death_and_divorce.closing_heading}" }
                    p { "{death_and_divorce.closing_body}" }
                }
                a {
                    class: "nav-btn nav-btn--primary death-and-divorce-cta",
                    href: "{content.contact_href}",
                    target: if contact_external { Some("_blank") } else { None },
                    rel: if contact_external { Some("noopener noreferrer") } else { None },
                    "{content.contact_label}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_three_practice_areas_without_numbered_steps() {
        fn app() -> Element {
            rsx! {
                DeathAndDivorceHome {
                    content: HomeContent {
                        heading: "Death & Divorce".into(),
                        lead: "For endings, transitions, and the beyond.".into(),
                        contact_href: "mailto:contact@example.com".into(),
                        contact_label: "Start a conversation".into(),
                        ..HomeContent::default()
                    },
                    death_and_divorce: DeathAndDivorceContent {
                        practices: vec![
                            ["Divorce".into(), "A way through.".into()],
                            ["Estate Planning".into(), "A plan.".into()],
                            ["Probate".into(), "What comes after.".into()],
                        ],
                        ..DeathAndDivorceContent::default()
                    },
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("Estate Planning"), "{html}");
        assert!(html.contains("Probate"), "{html}");
        assert!(!html.contains("01"), "{html}");
    }
}
