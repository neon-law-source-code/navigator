//! The firm fractional general counsel page (`/fractional-gc`) — accurate, efficient, and
//! speedy company counsel on a published flat fee.
//!
//! The page's whole argument is that legal work belongs inside the sales cycle
//! rather than beside it, so the copy is organised around what the base fee
//! includes, what the practice commits to, and what sits outside the retainer.
//! The base fee itself is published on the page as a small set of flat-fee
//! pricing card (annual cadence, framed daily) rather than
//! quoted through `/contact`. Every turnaround is written as a commitment
//! about the firm's own work product — never about whether a deal closes,
//! which the firm does not control.
//!
//! Like [`crate::home`], the only state is the static copy
//! ([`TransactionalContent`]), resolved by the portal router at router-build
//! time and injected via `ServeConfig::context_providers`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::pricing::{PricingCard, PricingSection};
use crate::components::{
    PracticeMark, PracticeMarkGlyph, PublicShell, SiteHeader, SiteNavLink, SocialMeta,
};
use crate::litigation_page::HeroWord;
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The self-contained transactional stylesheet, hoisted after the brand layer.
pub const TRANSACTIONAL_STYLESHEET_HREF: &str = "/public/css/transactional.css";

/// One of the three words the practice is named by, with the sentence that
/// makes it a fact rather than an adjective.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Virtue {
    pub word: String,
    pub body: String,
}

/// One line item the flat monthly fee covers.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Included {
    pub name: String,
    pub body: String,
}

/// One stage of the customer's own sales cycle, and the legal step that runs
/// inside it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SalesStage {
    pub stage: String,
    pub legal_step: String,
}

/// One kind of work that is quoted separately from the retainer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SeparateWork {
    pub name: String,
    pub body: String,
    pub href: Option<String>,
    pub link_label: Option<String>,
}

/// One flat-fee pricing card for the base retainer itself. Mapped onto
/// [`PricingCard`] at render time, which supplies the shared "Navigator-UX"
/// pricing-card treatment used elsewhere in the app.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PricingOffer {
    pub title: String,
    pub price: String,
    pub cadence: Option<String>,
    pub blurb: String,
    pub features: Vec<String>,
}

/// The static transactional copy — resolved brand-safely at router-build time
/// and injected into the render context.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TransactionalContent {
    pub head_title: String,
    pub meta_description: String,
    pub eyebrow: String,
    pub heading: Vec<HeroWord>,
    pub lead: String,
    pub cta_href: String,
    pub cta_label: String,
    /// Accurate, Efficient, Speedy — the three the practice is named by.
    pub virtues: Vec<Virtue>,
    /// What an MSA is, spelled out — the page uses the term, so it defines it.
    pub msa_term: String,
    pub msa_definition: String,
    pub fee_heading: String,
    /// How the base fee works, alongside the published pricing cards below it.
    pub fee_body: String,
    /// The base package's own published pricing card.
    pub pricing: Vec<PricingOffer>,
    /// A short note on intake capacity. Not a gate: there is no waitlist or
    /// form behind it, just a plain statement that slots are limited.
    pub availability_note: Option<String>,
    pub included_heading: String,
    pub included: Vec<Included>,
    pub cycle_heading: String,
    pub cycle_body: String,
    pub cycle: Vec<SalesStage>,
    pub separate_heading: String,
    pub separate_body: String,
    pub separate: Vec<SeparateWork>,
}

/// The [`TransactionalContent`] injected into the render context by the portal
/// router.
#[derive(Clone, Default)]
pub struct InjectedTransactional(pub TransactionalContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TransactionalPageView {
    pub chrome: PublicChrome,
    pub content: TransactionalContent,
}

/// Resolve the chrome and the static transactional content.
#[server]
pub async fn transactional_page_view() -> Result<TransactionalPageView, ServerFnError> {
    let content = consume_context::<InjectedTransactional>().0;
    Ok(TransactionalPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn TransactionalPageEntry() -> Element {
    let resource = use_server_future(transactional_page_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        TransactionalPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure transactional page. Prop-driven, so it server-renders and
/// unit-tests without a server future.
#[component]
pub fn TransactionalPage(chrome: PublicChrome, content: TransactionalContent) -> Element {
    let header = rsx! {
        SiteHeader {
            brand_name: chrome.brand_name.clone(),
            home_href: chrome.home_href.clone(),
            logo_href: chrome.logo_href.clone(),
            destinations: chrome
                .destinations
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
            utility: chrome
                .utility
                .iter()
                .map(|link| SiteNavLink::new(link.label.clone(), link.href.clone()))
                .collect(),
        }
    };
    let footer = rsx! {
        PublicFooter { chrome: chrome.clone() }
    };
    rsx! {
        document::Title { "{content.head_title}" }
        document::Meta { name: "description", content: "{content.meta_description}" }
        SocialMeta {
            title: content.head_title.clone(),
            description: content.meta_description.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        document::Stylesheet { href: crate::brand_style::BRAND_STYLESHEET_HREF }
        document::Stylesheet { href: TRANSACTIONAL_STYLESHEET_HREF }
        PublicShell { header, footer,
            SpeedHero { content: content.clone() }
            VirtueRow { virtues: content.virtues.clone() }
            FeeSection { content: content.clone() }
            CycleSection { content: content.clone() }
            SeparateSection { content: content.clone() }
        }
    }
}

/// The hero: the statement, the lead, and the one call to action.
///
/// It carried a turnaround dial beside it — the published figure drawn as a ring
/// with its qualifier beneath — which stated in a graphic what the Speedy virtue
/// below states in a sentence.
#[component]
fn SpeedHero(content: TransactionalContent) -> Element {
    rsx! {
        section { class: "speed-hero", "aria-labelledby": "speed-heading",
            div { class: "firm-glow speed-hero__glow", "aria-hidden": "true" }
            div { class: "speed-hero__statement",
                PracticeMarkGlyph {
                    mark: PracticeMark::Handshake,
                    class: "speed-hero__mark".to_string(),
                }
                p { class: "firm-eyebrow", "{content.eyebrow}" }
                h1 { id: "speed-heading", class: "speed-hero__heading",
                    for word in content.heading.iter() {
                        span {
                            class: if word.accent { "speed-word speed-word--accent" } else { "speed-word" },
                            // No trailing space: the word gaps are a margin in
                            // the stylesheet, the same convention `/litigation`
                            // uses for the same reason.
                            "{word.text}"
                        }
                    }
                }
                p { class: "speed-hero__lead", "{content.lead}" }
                a {
                    class: "nav-btn nav-btn--primary speed-hero__cta",
                    href: "{content.cta_href}",
                    "{content.cta_label}"
                }
            }
        }
    }
}

/// Accurate, Efficient, Speedy — each word with the sentence that makes it a
/// commitment instead of an adjective.
#[component]
fn VirtueRow(virtues: Vec<Virtue>) -> Element {
    if virtues.is_empty() {
        return rsx! {};
    }
    rsx! {
        ul { class: "speed-virtues",
            for (index , virtue) in virtues.iter().enumerate() {
                li { class: "neon-card speed-virtue", style: "--speed-virtue-index: {index};",
                    p { class: "speed-virtue__word", "{virtue.word}" }
                    p { class: "speed-virtue__body", "{virtue.body}" }
                }
            }
        }
    }
}

/// The flat fee — published as pricing cards — what it includes, and the term
/// the page defines.
#[component]
fn FeeSection(content: TransactionalContent) -> Element {
    let pricing_cards: Vec<PricingCard> = content
        .pricing
        .iter()
        .map(|offer| PricingCard {
            title: offer.title.clone(),
            price: offer.price.clone(),
            cadence: offer.cadence.clone(),
            blurb: offer.blurb.clone(),
            features: offer.features.clone(),
            cta_label: content.cta_label.clone(),
            cta_href: content.cta_href.clone(),
            featured_label: None,
        })
        .collect();
    rsx! {
        section { class: "neon-card speed-fee", "aria-labelledby": "speed-fee-heading",
            div { class: "speed-fee__head",
                h2 { id: "speed-fee-heading", class: "speed-heading", "{content.fee_heading}" }
                p { class: "speed-paragraph", "{content.fee_body}" }
                dl { class: "speed-definition",
                    dt { class: "speed-definition__term", "{content.msa_term}" }
                    dd { class: "speed-definition__body", "{content.msa_definition}" }
                }
            }
            if !pricing_cards.is_empty() {
                // One column per card, so a single card fills the row instead
                // of sitting in a fixed-3-column grid with two empty tracks.
                // `PricingSection` clamps to 4 anyway, so a length that
                // cannot fit a `u8` just falls back to that clamp.
                PricingSection {
                    cols_lg: u8::try_from(pricing_cards.len()).unwrap_or(4),
                    cards: pricing_cards,
                }
            }
            if let Some(note) = &content.availability_note {
                p { class: "speed-paragraph speed-fee__availability", "{note}" }
            }
            div { class: "speed-fee__included",
                h3 { class: "speed-subheading", "{content.included_heading}" }
                ul { class: "speed-included",
                    for (index , item) in content.included.iter().enumerate() {
                        li { class: "speed-included__item", style: "--speed-item-index: {index};",
                            span { class: "speed-included__tick", "aria-hidden": "true" }
                            div {
                                p { class: "speed-included__name", "{item.name}" }
                                p { class: "speed-included__body", "{item.body}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The sales cycle, with the legal step that runs inside each stage rather
/// than after it. The travelling pulse is the page's second piece of motion.
#[component]
fn CycleSection(content: TransactionalContent) -> Element {
    rsx! {
        section { class: "neon-card speed-cycle", "aria-labelledby": "speed-cycle-heading",
            h2 { id: "speed-cycle-heading", class: "speed-heading", "{content.cycle_heading}" }
            p { class: "speed-paragraph", "{content.cycle_body}" }
            ol { class: "speed-pipeline",
                span { class: "speed-pipeline__pulse", "aria-hidden": "true" }
                for (index , stage) in content.cycle.iter().enumerate() {
                    li { class: "speed-stage", style: "--speed-stage-index: {index};",
                        span { class: "speed-stage__node", "aria-hidden": "true" }
                        p { class: "speed-stage__name", "{stage.stage}" }
                        p { class: "speed-stage__legal", "{stage.legal_step}" }
                    }
                }
            }
        }
    }
}

/// What the retainer does not cover, and where that work is priced instead.
#[component]
fn SeparateSection(content: TransactionalContent) -> Element {
    rsx! {
        section { class: "neon-card speed-separate", "aria-labelledby": "speed-separate-heading",
            h2 { id: "speed-separate-heading", class: "speed-heading", "{content.separate_heading}" }
            p { class: "speed-paragraph", "{content.separate_body}" }
            ul { class: "speed-separate__list",
                for (index , work) in content.separate.iter().enumerate() {
                    li { class: "speed-separate__item", style: "--speed-separate-index: {index};",
                        p { class: "speed-separate__name", "{work.name}" }
                        p { class: "speed-paragraph", "{work.body}" }
                        if let (Some(href), Some(label)) = (work.href.clone(), work.link_label.clone()) {
                            a { class: "speed-separate__link", href: "{href}", "{label}" }
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

    fn content() -> TransactionalContent {
        TransactionalContent {
            head_title: "Neon Law | Transactional".to_string(),
            meta_description: "Company counsel on a flat monthly fee.".to_string(),
            eyebrow: "Transactional".to_string(),
            heading: vec![
                HeroWord {
                    text: "Accurate.".to_string(),
                    accent: true,
                },
                HeroWord {
                    text: "Efficient.".to_string(),
                    accent: false,
                },
                HeroWord {
                    text: "Speedy.".to_string(),
                    accent: false,
                },
            ],
            lead: "Company counsel that runs inside your sales cycle.".to_string(),
            cta_href: "mailto:contact@neonlaw.com".to_string(),
            cta_label: "Contact us".to_string(),
            virtues: vec![Virtue {
                word: "Accurate".to_string(),
                body: "A licensed attorney signs off on every document.".to_string(),
            }],
            msa_term: "MSA — master services agreement".to_string(),
            msa_definition: "The contract that sets the terms once.".to_string(),
            fee_heading: "One base package, flat fees for everything else".to_string(),
            fee_body: "One flat annual fee covers the base package below.".to_string(),
            pricing: vec![PricingOffer {
                title: "Base package".to_string(),
                price: "$3,650".to_string(),
                cadence: Some("/year".to_string()),
                blurb: "That's just $10 a day.".to_string(),
                features: vec!["DocuSign sent & tracked at $5 per contract".to_string()],
            }],
            availability_note: Some(
                "We take on a limited number of Fractional GC clients at a time.".to_string(),
            ),
            included_heading: "What the fee covers".to_string(),
            included: vec![Included {
                name: "Cap table management".to_string(),
                body: "We keep the ledger current.".to_string(),
            }],
            cycle_heading: "Inside the sales cycle".to_string(),
            cycle_body: "Legal runs in the stage, not after it.".to_string(),
            cycle: vec![SalesStage {
                stage: "Discovery call".to_string(),
                legal_step: "NDA out the same day.".to_string(),
            }],
            separate_heading: "Priced separately".to_string(),
            separate_body: "Two kinds of work sit outside the retainer.".to_string(),
            separate: vec![SeparateWork {
                name: "Litigation".to_string(),
                body: "Quoted per phase after a case assessment.".to_string(),
                href: Some("/litigation".to_string()),
                link_label: Some("The litigation practice".to_string()),
            }],
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                TransactionalPage { chrome: PublicChrome::default(), content: content() }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn leads_with_the_three_words_the_practice_is_named_by() {
        let out = html();
        assert!(
            out.contains(r#"data-practice-mark="handshake""#) && out.contains("speed-hero__mark"),
            "the hero reuses the four-card transactional mark: {out}"
        );
        assert_eq!(out.matches("<h1").count(), 1, "one h1: {out}");
        assert!(
            out.contains("Accurate.") && out.contains("Efficient.") && out.contains("Speedy."),
            "the statement: {out}"
        );
        assert!(
            out.contains(r#"speed-word speed-word--accent">Accurate.</span>"#),
            "Accurate. carries the firm's colour: {out}"
        );
        assert!(
            out.contains(r#"href="mailto:contact@neonlaw.com""#),
            "the CTA routes to contact"
        );
    }

    /// The dial came off. It drew the published turnaround as a ring with its
    /// qualifier beneath — the same commitment the Speedy virtue makes in a
    /// sentence, stated twice and once as a graphic.
    #[test]
    fn carries_no_turnaround_dial() {
        let out = html();
        for gone in ["speed-dial", "Measured from a complete intake"] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
    }

    #[test]
    fn defines_the_msa_it_makes_a_commitment_about() {
        let out = html();
        assert!(
            out.contains("master services agreement"),
            "the term is spelled out rather than assumed: {out}"
        );
    }

    #[test]
    fn publishes_its_flat_fee_pricing_cards() {
        // The base package is now published on the page as one pricing card
        // rather than quoted through `/contact`: the annual figure, the
        // per-day framing in its body, and the DocuSign per-contract line.
        let out = html();
        assert!(
            out.contains("One base package, flat fees for everything else"),
            "the structure: {out}"
        );
        assert!(
            out.contains("pricing-card"),
            "the pricing card renders: {out}"
        );
        for figure in ["$3,650", "/year", "$10 a day", "$5 per contract"] {
            assert!(out.contains(figure), "{figure} must publish: {out}");
        }
        // A single card gets a single grid column, so it fills the row
        // instead of sitting in a fixed-3-column grid with two empty tracks.
        assert!(
            out.contains("--pricing-cols: 1;"),
            "one card is one column, full width: {out}"
        );
    }

    #[test]
    fn routes_the_work_it_does_not_cover_to_where_it_is_priced() {
        let out = html();
        assert!(out.contains("Priced separately"), "the section: {out}");
        assert!(
            out.contains(r#"href="/litigation""#),
            "separate work links to its own page: {out}"
        );
    }

    /// Two sections came off this page: the engagement-letter block and the
    /// metered-contracts explainer. Both described terms of the engagement — how
    /// scope is fixed, how the invoice is itemised — rather than anything a
    /// reader weighs when deciding whether to call.
    #[test]
    fn carries_no_metered_contracts_section() {
        let out = html();
        for gone in ["Contracts are metered", "speed-metered", "metered on top"] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
    }

    /// The engagement-letter block came off the page. What it said is still
    /// true — scope and fee are fixed in a signed engagement letter — but it is
    /// a term of the engagement rather than something a marketing page has to
    /// close on, and the fee section already sends the number to `/contact`.
    #[test]
    fn carries_no_engagement_letter_block() {
        let out = html();
        assert!(
            !out.contains("Engagement letter governs"),
            "the engagement-letter block is gone: {out}"
        );
        assert!(
            !out.contains("speed-engagement"),
            "and its section with it: {out}"
        );
    }

    #[test]
    fn publishes_no_outcome_promise_or_superlative() {
        let out = html().to_lowercase();
        for banned in [
            "guarantee",
            "we will close",
            "fastest",
            "best-in-class",
            "world-class",
            "industry-leading",
            "premier",
            "cutting-edge",
        ] {
            assert!(!out.contains(banned), "{banned} must not render: {out}");
        }
    }

    #[test]
    fn the_virtue_row_stays_out_of_the_markup_when_there_are_none() {
        fn app() -> Element {
            rsx! {
                TransactionalPage {
                    chrome: PublicChrome::default(),
                    content: TransactionalContent::default(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = dioxus_ssr::render(&dom);
        assert!(!out.contains("speed-virtues"), "no empty row: {out}");
    }

    #[test]
    fn wraps_the_page_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome");
    }
}
