//! Company counsel: membership, notation packages, and an illustrative work stream.

use super::HomeContent;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Copy loaded from the brand home catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompanyContent {
    pub booking_href: String,
    pub pricing_link: String,
    pub hero_note: String,
    pub flow_caption: String,
    pub flow_steps: Vec<String>,
    pub packages: Vec<String>,
    pub pause_label: String,
    pub pricing_heading: String,
    pub video_label: String,
    pub video_src: String,
    pub membership_label: String,
    pub membership_price: String,
    pub membership_unit: String,
    pub membership_body: String,
    pub membership_features: Vec<String>,
    pub review_heading: String,
    pub review_body: String,
    pub review_columns: [String; 3],
    pub review_rows: Vec<[String; 3]>,
    pub review_note: String,
    pub drafting_heading: String,
    pub drafting_packages: Vec<[String; 3]>,
    pub closing_heading: String,
    pub closing_body: String,
    pub litigation_heading: String,
    pub litigation_link: String,
    pub litigation_price: String,
    pub litigation_unit: String,
    pub litigation_body: String,
    pub litigation_note: String,
    pub people_heading: String,
    pub people_body: String,
    pub immigration_label: String,
    pub estate_label: String,
    /// Where each sibling practice's name links, when it links at all.
    ///
    /// `None` while that practice is held out of launch
    /// (`views::brand::BrandKey::is_live`), in which case the name renders as
    /// plain text. The section's offer is real either way — the firm does
    /// arrange this work through its siblings — but a link to a host the
    /// deployment answers `404` for advertises an address rather than an
    /// offer. Same rule the footer's family row applies, derived from the
    /// same launch gate, so a launch flips both at once.
    pub immigration_href: Option<String>,
    pub estate_href: Option<String>,
    pub navigator_heading: String,
    pub navigator_body: String,
    pub navigator_link: String,
    pub source_label: String,
    pub source_note: String,
}

/// One sibling practice in the "for the people building it" section: a link
/// when that practice has launched, its bare name when it has not.
///
/// The arrow is part of the link affordance, so it goes with the link rather
/// than staying on a label that no longer leads anywhere.
#[component]
fn SiblingPractice(label: String, href: Option<String>) -> Element {
    match href {
        Some(href) => rsx! {
            a { class: "company-text-link", href: "{href}", "{label} ↗" }
        },
        None => rsx! {
            span { class: "company-text-link company-text-link--unlinked", "{label}" }
        },
    }
}

#[component]
pub(super) fn CompanyHome(content: HomeContent, company: CompanyContent) -> Element {
    rsx! {
        div { class: "company-home",
            section { class: "company-hero",
                h1 { "{content.heading}" }
                p { class: "company-hero__lead", "{content.lead}" }
                div { class: "company-actions",
                    a { class: "nav-btn nav-btn--primary", href: "{company.booking_href}", "{content.contact_label}" }
                    a { class: "company-text-link", href: "#pricing", "{company.pricing_link} ↗" }
                }
                p { class: "company-hero__terms", "{company.hero_note}" }
            }
            section { class: "deal-exhibition", "aria-label": "{company.flow_caption}",
                input { id: "pause-deal-flow", class: "nav-checkbox deal-exhibition__pause", r#type: "checkbox" }
                label { class: "deal-exhibition__control", r#for: "pause-deal-flow", "{company.pause_label}" }
                div { class: "deal-exhibition__scene", "aria-hidden": "true",
                    div { class: "deal-exhibition__grid" }
                    div { class: "deal-exhibition__belt deal-exhibition__belt--back" }
                    div { class: "deal-exhibition__belt deal-exhibition__belt--front" }
                    div { class: "deal-exhibition__track",
                        for (index, package) in company.packages.iter().cycle().take(company.packages.len() * 3).enumerate() {
                            div { class: "deal-package", key: "{index}",
                                div { class: "deal-package__top" }
                                div { class: "deal-package__side" }
                                div { class: "deal-package__face",
                                    span { class: "deal-package__number", "N / 0{index % company.packages.len() + 1}" }
                                    strong { "{package}" }
                                    span { class: "deal-package__lines" }
                                    span { class: "deal-package__seal", "NL" }
                                }
                            }
                        }
                    }
                    div { class: "deal-exhibition__scanner", span {} span {} }
                }
            }
            section { class: "company-pricing", "aria-labelledby": "company-pricing-title",
                h2 { id: "company-pricing-title", class: "nav-visually-hidden", "{company.pricing_heading}" }
                video { class: "company-video", controls: true, preload: "metadata", playsinline: true,
                    "aria-label": "{company.video_label}",
                    source { src: "{company.video_src}", r#type: "video/mp4" }
                }
                // The hero's "See pricing" link means the prices, not the
                // presentation video that opens this section, so the anchor
                // sits on the grid where the membership card begins.
                div { id: "pricing", class: "company-pricing__grid",
                    article { class: "company-membership",
                        h3 { "{company.membership_label}" }
                        p { class: "company-price", "{company.membership_price}" span { "{company.membership_unit}" } }
                        p { "{company.membership_body}" }
                        ul { for feature in company.membership_features.iter() { li { "{feature}" } } }
                    }
                    article { class: "company-review",
                        h3 { "{company.review_heading}" }
                        p {
                            if let Some((before, after)) = company.review_body.split_once("notation") {
                                "{before}"
                                a { href: "/notations", "notation" }
                                "{after}"
                            } else {
                                "{company.review_body}"
                            }
                        }
                        table {
                            caption { class: "company-table-caption", "{company.review_heading}" }
                            thead { tr { for column in company.review_columns.iter() { th { scope: "col", "{column}" } } } }
                            tbody { for row in company.review_rows.iter() {
                                tr { th { scope: "row", "{row[0]}" } td { "{row[1]}" } td { "{row[2]}" } }
                            } }
                        }
                        p { class: "company-note", "{company.review_note}" }
                    }
                }
            }
            section { class: "company-packages", "aria-labelledby": "company-packages-title",
                h2 { id: "company-packages-title", "{company.drafting_heading}" }
                div { class: "company-packages__grid",
                    for package in company.drafting_packages.iter() {
                        article {
                            h3 { "{package[0]}" }
                            p { class: "company-package-price", "{package[1]}" }
                            p { "{package[2]}" }
                        }
                    }
                }
            }
            section { class: "company-litigation", "aria-labelledby": "company-litigation-title",
                div {
                    h2 { id: "company-litigation-title", "{company.litigation_heading}" }
                    p { "{company.litigation_body}" }
                    a { class: "company-text-link", href: "{company.booking_href}", "{company.litigation_link} ↗" }
                }
                div {
                    p { class: "company-price", "{company.litigation_price}" }
                    p { "{company.litigation_unit}" }
                    p { class: "company-note", "{company.litigation_note}" }
                }
            }
            section { class: "company-navigator", "aria-labelledby": "company-navigator-title",
                div { class: "company-network", "aria-hidden": "true",
                    div { class: "company-network__orbit" }
                    div { class: "company-network__orbit company-network__orbit--inner" }
                    img { class: "company-network__center", src: "/public/navigator-wheel.svg", alt: "", width: "96", height: "96" }
                    for (index, step) in company.flow_steps.iter().enumerate() {
                        span { class: "company-network__node company-network__node--{index}", "{step}" }
                    }
                }
                div {
                    h2 { id: "company-navigator-title", "{company.navigator_heading}" }
                    p { "{company.navigator_body}" }
                    a { class: "company-text-link", href: "/navigator", "{company.navigator_link} ↗" }
                    p { class: "company-note company-source",
                        a { href: "https://github.com/neon-law-source-code/navigator", "{company.source_label}" }
                        ". {company.source_note}"
                    }
                }
            }
            section { class: "company-people", "aria-labelledby": "company-people-title",
                div {
                    h2 { id: "company-people-title", "{company.people_heading}" }
                    p { "{company.people_body}" }
                }
                div { class: "company-people__links",
                    SiblingPractice {
                        label: company.immigration_label.clone(),
                        href: company.immigration_href.clone(),
                    }
                    SiblingPractice {
                        label: company.estate_label.clone(),
                        href: company.estate_href.clone(),
                    }
                }
            }
            section { class: "company-closing",
                h2 { "{company.closing_heading}" span { "{company.closing_body}" } }
                a { class: "nav-btn nav-btn--primary", href: "{company.booking_href}", "{content.contact_label} ↗" }
            }
        }
    }
}
