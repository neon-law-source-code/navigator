//! The lifetime estate plan, with an optional future record of a signed version.
use super::HomeContent;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Copy loaded from the brand home catalog.
///
/// This mirrors `views::locales::EstateCopy` rather than naming it, for the
/// same reason [`super::CompanyContent`] does: `views` is gated behind the
/// `server` feature, and this type is a field of [`HomeContent`], which the
/// wasm client build compiles. `neon::locales::home` maps the catalog shape
/// onto this one at the one seam that already reads the YAML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EstateContent {
    pub eyebrow: String,
    pub process_link: String,
    pub plan_label: String,
    pub price: String,
    pub price_term: String,
    pub features: Vec<String>,
    pub fee_note: String,
    pub video_label: String,
    pub video_src: String,
    pub transcript_label: String,
    pub video_transcript: String,
    pub process_label: String,
    pub process_heading: String,
    pub steps: Vec<[String; 2]>,
    pub record_label: String,
    pub record_price: String,
    pub record_unit: String,
    pub record_status: String,
    pub record_heading: String,
    pub record_body: String,
    pub record_note: String,
    pub closing_heading: String,
    pub closing_body: String,
}

#[component]
pub(super) fn EstateHome(content: HomeContent, estate: EstateContent) -> Element {
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
            section { class: "vesta-video", "aria-labelledby": "vesta-video-title",
                h2 { id: "vesta-video-title", "{estate.video_label}" }
                video {
                    controls: true,
                    playsinline: true,
                    preload: "metadata",
                    poster: "/public/brand/vesta.svg",
                    src: "{estate.video_src}",
                    "aria-label": "{estate.video_label}",
                    a { href: "{estate.video_src}", "{estate.video_label}" }
                }
                details {
                    summary { "{estate.transcript_label}" }
                    p { "{estate.video_transcript}" }
                }
            }
            section { id: "your-plan", class: "vesta-process", "aria-labelledby": "vesta-process-title",
                p { class: "vesta-eyebrow", "{estate.process_label}" }
                h2 { id: "vesta-process-title", "{estate.process_heading}" }
                ol { class: "vesta-steps",
                    for step in &estate.steps {
                        li {
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
