//! Daybridge Divorce Law: clear fees, three service commitments, one short path forward.

use super::HomeContent;
use crate::components::is_external_href;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Copy loaded from the Daybridge home catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DaybridgeContent {
    pub eyebrow: String,
    pub fee_label: String,
    pub fee_price: String,
    pub fee_unit: String,
    pub fee_body: String,
    pub fee_note: String,
    pub response_heading: String,
    pub response_body: String,
    pub motions_heading: String,
    pub motions_body: String,
    pub costs_heading: String,
    pub costs_body: String,
    pub process_label: String,
    pub process_heading: String,
    pub steps: Vec<[String; 2]>,
    pub closing_heading: String,
    pub closing_body: String,
}

#[component]
pub(super) fn DaybridgeHome(content: HomeContent, daybridge: DaybridgeContent) -> Element {
    let contact_external = is_external_href(&content.contact_href);
    rsx! {
        div { class: "daybridge-home",
            section { class: "daybridge-hero", "aria-labelledby": "daybridge-title",
                div { class: "daybridge-hero__copy",
                    p { class: "daybridge-eyebrow", "{daybridge.eyebrow}" }
                    h1 { id: "daybridge-title", "{content.heading}" }
                    p { class: "daybridge-lead", "{content.lead}" }
                    a {
                        class: "nav-btn nav-btn--primary daybridge-cta",
                        href: "{content.contact_href}",
                        target: if contact_external { Some("_blank") } else { None },
                        rel: if contact_external { Some("noopener noreferrer") } else { None },
                        "{content.contact_label}"
                    }
                }
                aside { class: "daybridge-fee", "aria-label": "{daybridge.fee_label}",
                    img {
                        class: "daybridge-fee__mark",
                        src: "/public/brand/daybridge/logo.svg",
                        alt: "",
                        width: "64",
                        height: "64",
                    }
                    p { class: "daybridge-eyebrow", "{daybridge.fee_label}" }
                    p { class: "daybridge-price",
                        "{daybridge.fee_price}"
                        span { "{daybridge.fee_unit}" }
                    }
                    p { class: "daybridge-fee__body", "{daybridge.fee_body}" }
                    p { class: "daybridge-note", "{daybridge.fee_note}" }
                }
            }
            section { class: "daybridge-promises", "aria-label": "How Daybridge works",
                article {
                    h2 { "{daybridge.response_heading}" }
                    p { "{daybridge.response_body}" }
                }
                article {
                    h2 { "{daybridge.motions_heading}" }
                    p { "{daybridge.motions_body}" }
                }
                article {
                    h2 { "{daybridge.costs_heading}" }
                    p { "{daybridge.costs_body}" }
                }
            }
            section { class: "daybridge-process", "aria-labelledby": "daybridge-process-title",
                p { class: "daybridge-eyebrow", "{daybridge.process_label}" }
                h2 { id: "daybridge-process-title", "{daybridge.process_heading}" }
                ol {
                    for (index, step) in daybridge.steps.iter().enumerate() {
                        li {
                            span { "{index + 1}" }
                            div {
                                h3 { "{step[0]}" }
                                p { "{step[1]}" }
                            }
                        }
                    }
                }
            }
            section { class: "daybridge-closing", "aria-labelledby": "daybridge-closing-title",
                div {
                    h2 { id: "daybridge-closing-title", "{daybridge.closing_heading}" }
                    p { "{daybridge.closing_body}" }
                }
                a {
                    class: "nav-btn nav-btn--primary daybridge-cta",
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
    fn renders_the_daily_fee_and_service_commitments() {
        fn app() -> Element {
            rsx! {
                DaybridgeHome {
                    content: HomeContent {
                        heading: "A way through divorce.".into(),
                        lead: "Move toward the other side.".into(),
                        contact_href: "mailto:contact@example.com".into(),
                        contact_label: "Start a conversation".into(),
                        ..HomeContent::default()
                    },
                    daybridge: DaybridgeContent {
                        fee_label: "Attorney fee".into(),
                        fee_price: "$10".into(),
                        fee_unit: " / day".into(),
                        motions_heading: "Motion drafts in five business days".into(),
                        costs_heading: "You pay case costs separately".into(),
                        ..DaybridgeContent::default()
                    },
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("$10"), "{html}");
        assert!(html.contains("five business days"), "{html}");
        assert!(html.contains("case costs separately"), "{html}");
    }
}
