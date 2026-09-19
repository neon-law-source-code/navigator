//! The lifetime estate plan, with an optional future record of a signed version.
use super::HomeContent;
use dioxus::prelude::*;
use views::locales::EstateCopy;

#[component]
pub(super) fn EstateHome(content: HomeContent, estate: EstateCopy) -> Element {
    rsx! {
        div { class: "vesta-home",
            section { class: "vesta-hero", "aria-labelledby": "vesta-title",
                div {
                    p { class: "vesta-eyebrow", "{estate.eyebrow}" }
                    h1 { id: "vesta-title", "{content.heading}" }
                    p { class: "vesta-lead", "{content.lead}" }
                    div { class: "vesta-actions",
                        a { class: "nav-btn nav-btn--primary", href: "{content.contact_href}", "{content.contact_label}" }
                        a { class: "vesta-text-link", href: "#your-plan", "{estate.process_link}" }
                    }
                }
                aside { class: "vesta-plan", "aria-label": "{estate.plan_label}",
                    img { class: "vesta-plan__mark", src: "/public/brand/vesta.svg", alt: "", width: "36", height: "50" }
                    p { class: "vesta-eyebrow", "{estate.plan_label}" }
                    p { class: "vesta-price", "{estate.price}" }
                    p { class: "vesta-plan__term", "{estate.price_term}" }
                    ul { for feature in &estate.features { li { "{feature}" } } }
                    p { class: "vesta-note", "{estate.fee_note}" }
                }
            }
            section { id: "your-plan", class: "vesta-process", "aria-labelledby": "vesta-process-title",
                p { class: "vesta-eyebrow", "{estate.process_label}" }
                h2 { id: "vesta-process-title", "{estate.process_heading}" }
                ol { class: "vesta-steps",
                    for (index, step) in estate.steps.iter().enumerate() {
                        li {
                            span { class: "vesta-step__number", "aria-hidden": "true", "0{index + 1}" }
                            h3 { "{step[0]}" }
                            p { "{step[1]}" }
                        }
                    }
                }
            }
            section { class: "vesta-record", "aria-labelledby": "vesta-record-title",
                div {
                    p { class: "vesta-eyebrow", "{estate.record_label}" }
                    p { class: "vesta-record__price", "{estate.record_price}" }
                    p { "{estate.record_unit}" }
                    span { class: "vesta-status", "{estate.record_status}" }
                }
                div {
                    h2 { id: "vesta-record-title", "{estate.record_heading}" }
                    p { "{estate.record_body}" }
                    p { class: "vesta-note", "{estate.record_note}" }
                }
            }
            section { class: "vesta-closing", "aria-labelledby": "vesta-closing-title",
                div {
                    h2 { id: "vesta-closing-title", "{estate.closing_heading}" }
                    p { "{estate.closing_body}" }
                }
                a { class: "nav-btn nav-btn--primary", href: "{content.contact_href}", "{content.contact_label}" }
            }
        }
    }
}
