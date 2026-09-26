//! Company counsel: services, transparent fees, and the tools behind the work.

use super::HomeContent;
use crate::components::is_external_href;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Copy loaded from the brand home catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompanyContent {
    pub principles: Vec<[String; 2]>,
    pub services_heading: String,
    pub services_note: String,
    pub services_link: String,
    pub services: Vec<[String; 2]>,
    pub community_heading: String,
    pub community_body: String,
    pub community_links: Vec<[String; 2]>,
    pub booking_href: String,
    pub pricing_link: String,
    pub retainer_note: String,
    pub retainer_amount: u32,
    pub simulator_heading: String,
    pub simulator_body: String,
    pub simulator_days_label: String,
    pub simulator_reviews_label: String,
    pub simulator_plan_label: String,
    pub simulator_review_label: String,
    pub simulator_contract_label: String,
    pub simulator_total_label: String,
    pub simulator_note: String,
    pub pause_label: String,
    pub packages: Vec<String>,
    pub pricing_heading: String,
    pub video_label: String,
    pub video_src: String,
    pub membership_label: String,
    pub membership_price: String,
    pub membership_unit: String,
    pub membership_body: String,
    pub membership_features: Vec<String>,
    pub express_heading: String,
    pub express_price: String,
    pub express_unit: String,
    pub express_body: String,
    pub page_note: String,
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
            a {
                class: "company-text-link",
                href: "{href}",
                target: "_blank",
                rel: "noopener noreferrer",
                "{label} ↗"
            }
        },
        None => rsx! {
            span { class: "company-text-link company-text-link--unlinked", "{label}" }
        },
    }
}

fn parse_dollars(value: &str) -> u32 {
    value
        .chars()
        .filter_map(|character| character.to_digit(10))
        .fold(0, |amount, digit| {
            amount.saturating_mul(10).saturating_add(digit)
        })
}

fn format_dollars(amount: u32) -> String {
    let digits = amount.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    format!("${formatted}")
}

struct FeeScenario {
    class_name: String,
    plan_details: String,
    review_price: String,
    earned_fees: String,
    remaining_retainer: String,
    earned_share: u32,
}

#[component]
pub(super) fn CompanyHome(
    content: HomeContent,
    company: CompanyContent,
    #[props(default)] lead_capture_enabled: bool,
) -> Element {
    let booking_external = is_external_href(&company.booking_href);
    let daily_fee = parse_dollars(&company.membership_price);
    let review_fee = parse_dollars(&company.express_price);
    let contract_fee = company
        .drafting_packages
        .first()
        .map_or(0, |row| parse_dollars(&row[1]));
    let contract_value = format_dollars(contract_fee);
    let retainer_value = format_dollars(company.retainer_amount);
    let review_rows = [
        ["No review".to_string(), "$0".to_string()],
        ["One review".to_string(), company.express_price.clone()],
    ];
    let mut fee_scenarios = Vec::new();
    for days in [10_u32, 30] {
        for reviews in 0_u32..=1 {
            let plan_fees = days.saturating_mul(daily_fee);
            let review_fees = reviews.saturating_mul(review_fee);
            let earned_fees = contract_fee
                .saturating_add(plan_fees)
                .saturating_add(review_fees);
            let earned_share = earned_fees
                .saturating_mul(100)
                .checked_div(company.retainer_amount)
                .unwrap_or(0)
                .min(100);
            fee_scenarios.push(FeeScenario {
                class_name: format!("company-simulator__summary-case--{days}-{reviews}"),
                plan_details: format!(
                    "{days} days × {} = {}",
                    company.membership_price,
                    format_dollars(plan_fees)
                ),
                review_price: format_dollars(review_fees),
                earned_fees: format_dollars(earned_fees),
                remaining_retainer: format_dollars(
                    company.retainer_amount.saturating_sub(earned_fees),
                ),
                earned_share,
            });
        }
    }
    rsx! {
        div { class: "company-home",
            section { class: "company-hero",
                h1 { "{content.heading}" }
                p { class: "company-hero__lead", "{content.lead}" }
                div { class: "company-actions",
                    a {
                        class: "nav-btn nav-btn--primary",
                        href: "{company.booking_href}",
                        target: if booking_external { Some("_blank") } else { None },
                        rel: if booking_external { Some("noopener noreferrer") } else { None },
                        "{content.contact_label}"
                    }
                    if lead_capture_enabled {
                        a {
                            class: "nav-btn nav-btn--secondary home-lead-modal__trigger",
                            href: "/contact",
                            "aria-haspopup": "dialog",
                            "data-lead-modal-trigger": "true",
                            "Get in touch"
                        }
                    }
                    a { class: "company-text-link", href: "#pricing", "{company.pricing_link} ↗" }
                }
            }
            section { class: "deal-exhibition", "aria-label": "Moving legal documents",
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
                                    strong { "{package}" }
                                    span { class: "deal-package__lines" }
                                    span { class: "deal-package__seal",
                                        img { src: "/public/logo.svg", alt: "" }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "deal-exhibition__scanner", span {} span {} }
                }
            }
            div { class: "company-principles",
                for principle in company.principles.iter() {
                    article {
                        h2 { "{principle[0]}" }
                        p { "{principle[1]}" }
                    }
                }
            }
            div { class: "company-experience",
                if let Some(service) = content.service.clone() {
                    super::ServiceProse { service }
                }
                video { class: "company-video", controls: true, preload: "metadata", playsinline: true,
                    "aria-label": "{company.video_label}",
                    source { src: "{company.video_src}", r#type: "video/mp4" }
                }
            }
            section { id: "pricing", class: "company-pricing", "aria-labelledby": "company-pricing-title",
                h2 { id: "company-pricing-title", "{company.pricing_heading}" }
                div { class: "company-pricing__grid",
                    article { class: "company-foundation",
                        h3 { "{company.drafting_heading}" }
                        for package in company.drafting_packages.iter() {
                            p { class: "company-price", "{package[1]}" span { "{package[0]}" } }
                            p { "{package[2]}" }
                        }
                    }
                    article { class: "company-membership",
                        h3 { "{company.membership_label}" }
                        p { class: "company-price", "{company.membership_price}" span { "{company.membership_unit}" } }
                        p { "{company.membership_body}" }
                        ul { for feature in company.membership_features.iter() { li { "{feature}" } } }
                    }
                    article { class: "company-express",
                        h3 { "{company.express_heading}" }
                        p { class: "company-price", "{company.express_price}" span { "{company.express_unit}" } }
                        p { "{company.express_body}" }
                    }
                }
                p { class: "company-pricing__terms", "{company.page_note}" }
                p { class: "company-note", "{company.retainer_note}" }
                section { class: "company-simulator", "aria-labelledby": "company-simulator-title",
                    h3 { id: "company-simulator-title", "{company.simulator_heading}" }
                    p { class: "company-simulator__body", "{company.simulator_body}" }
                    fieldset {
                            legend { "{company.simulator_days_label}" }
                            div { class: "company-simulator__options",
                                label { class: "company-simulator__option", r#for: "company-plan-days-10",
                                    input { class: "nav-radio__input company-simulator__radio", id: "company-plan-days-10", r#type: "radio", name: "company-plan-days", value: "10", checked: true }
                                    span { "10 days" }
                                }
                                label { class: "company-simulator__option", r#for: "company-plan-days-30",
                                    input { class: "nav-radio__input company-simulator__radio", id: "company-plan-days-30", r#type: "radio", name: "company-plan-days", value: "30" }
                                    span { "30 days" }
                                }
                            }
                    }
                    fieldset {
                            legend { "{company.simulator_reviews_label}" }
                            div { class: "company-simulator__options",
                                for (index, row) in review_rows.iter().enumerate() {
                                    label { key: "{index}", class: "company-simulator__option", r#for: "company-review-count-{index}",
                                        input { class: "nav-radio__input company-simulator__radio", id: "company-review-count-{index}", r#type: "radio", name: "company-review-count", value: "{index}", checked: index == 1 }
                                        span { "{row[0]} · {row[1]}" }
                                    }
                                }
                            }
                    }
                    for scenario in fee_scenarios.iter() {
                            div { key: "{scenario.class_name}", class: "company-simulator__scenario {scenario.class_name}" ,
                                dl { class: "company-simulator__summary-case",
                                    div { dt { "Starting retainer" } dd { "{retainer_value}" span { "Held in trust" } } }
                                    div { dt { "{company.simulator_contract_label}" } dd { "{contract_value}" } }
                                    div { dt { "{company.simulator_plan_label}" } dd { "{scenario.plan_details}" } }
                                    div { dt { "{company.simulator_review_label}" } dd { "{scenario.review_price}" } }
                                }
                                div {
                                    class: "company-simulator__allocation",
                                    role: "group",
                                    "aria-label": "{company.simulator_total_label}",
                                    div {
                                        class: "company-simulator__chart",
                                        style: "--earned-share: {scenario.earned_share}%",
                                        role: "img",
                                        "aria-label": "After selected services: {scenario.earned_fees} earned, {scenario.remaining_retainer} remains held in trust.",
                                        span { class: "company-simulator__chart-center",
                                            strong { "{scenario.earned_share}%" }
                                            span { "earned" }
                                        }
                                    }
                                    ul { class: "company-simulator__legend",
                                        li {
                                            span { class: "company-simulator__swatch", "aria-hidden": "true" }
                                            span { "Earned charges" }
                                            strong { "{scenario.earned_fees}" }
                                        }
                                        li {
                                            span { class: "company-simulator__swatch company-simulator__swatch--trust", "aria-hidden": "true" }
                                            span { "Remaining in trust" }
                                            strong { "{scenario.remaining_retainer}" }
                                        }
                                    }
                                }
                            }
                    }
                    p { class: "company-note", "{company.simulator_note}" }
                }
            }
            section { class: "company-services", "aria-labelledby": "company-services-title",
                h2 { id: "company-services-title", "{company.services_heading}" }
                div { class: "company-services__intro",
                    p { "{company.services_note}" }
                    a { class: "company-text-link", href: "/notations#templates", "{company.services_link}" }
                }
                div { class: "company-services__grid",
                    for service in company.services.iter() {
                        article {
                            h3 { "{service[0]}" }
                            p { "{service[1]}" }
                        }
                    }
                }
            }
            section { class: "company-litigation", "aria-labelledby": "company-litigation-title",
                div {
                    h2 { id: "company-litigation-title", "{company.litigation_heading}" }
                    p { "{company.litigation_body}" }
                    a {
                        class: "company-text-link",
                        href: "{company.booking_href}",
                        target: if booking_external { Some("_blank") } else { None },
                        rel: if booking_external { Some("noopener noreferrer") } else { None },
                        "{company.litigation_link} ↗"
                    }
                }
                div {
                    p { class: "company-price", "{company.litigation_price}" }
                    p { "{company.litigation_unit}" }
                    p { class: "company-note", "{company.litigation_note}" }
                }
            }
            section { class: "company-navigator", "aria-labelledby": "company-navigator-title",
                div { class: "company-engineering",
                    div { class: "company-network",
                        div { class: "company-network__orbit", "aria-hidden": "true" }
                        div { class: "company-network__orbit company-network__orbit--inner", "aria-hidden": "true" }
                        img { class: "company-network__center", src: "/public/ferris.svg", alt: "Ferris, the Rust community’s crab mascot", width: "160", height: "106" }
                    }
                    label { class: "company-engineering__control",
                        input { r#type: "checkbox", class: "nav-checkbox" }
                        "{company.pause_label}"
                    }
                }
                div {
                    h2 { id: "company-navigator-title", "{company.navigator_heading}" }
                    p { "{company.navigator_body}" }
                    a { class: "company-text-link", href: "/navigator", "{company.navigator_link} ↗" }
                    p { class: "company-note company-source",
                        a {
                            href: "https://github.com/neon-law-source-code/navigator",
                            target: "_blank",
                            rel: "noopener noreferrer",
                            "{company.source_label}"
                        }
                        ". {company.source_note}"
                    }
                }
            }
            section { class: "company-community",
                h2 { "{company.community_heading}" }
                div {
                    p { "{company.community_body}" }
                    div { class: "company-actions",
                        for link in company.community_links.iter() {
                            a { class: "company-text-link", href: "{link[1]}", "{link[0]} ↗" }
                        }
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
                a {
                    class: "nav-btn nav-btn--primary",
                    href: "{company.booking_href}",
                    target: if booking_external { Some("_blank") } else { None },
                    rel: if booking_external { Some("noopener noreferrer") } else { None },
                    "{content.contact_label} ↗"
                }
            }
        }
    }
}
