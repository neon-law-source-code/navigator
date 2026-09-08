//! The firm fractional general counsel page (`/fractional-gc`) — accurate, efficient, and
//! speedy company counsel on a published flat fee.
//!
//! The page's argument is that recurring counsel should be simple to buy and
//! predictable to use, so the copy is organised around the base fee and what
//! the practice commits to.
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

use crate::components::pricing::{DayRateBadge, PricingCard, PricingSection};
use crate::components::{
    BillMarkGlyph, PracticeMark, PracticeMarkGlyph, PublicShell, SiteHeader, SiteNavLink,
    SocialMeta,
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
    /// The flat fee's published day rate. See
    /// [`crate::components::pricing::PricingCard::day_rate`].
    pub day_rate: Option<DayRateBadge>,
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
    pub fee_heading: String,
    /// How the base fee works, alongside the published pricing card below it.
    pub fee_body: String,
    /// The base package's own published pricing card.
    pub pricing: Vec<PricingOffer>,
    pub closing_heading: String,
    pub closing_body: String,
    pub closing_email: String,
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
        document::Stylesheet { href: crate::components::COMMITMENT_STYLESHEET_HREF }
        PublicShell { header, footer,
            SpeedHero { content: content.clone() }
            VirtueRow { virtues: content.virtues.clone() }
            FeeSection { content: content.clone() }
            ClosingCta { content }
        }
    }
}

/// The hero: the statement, the lead, and the one call to action.
///
/// It carried a turnaround dial beside it — the published figure drawn as a ring
/// with its qualifier beneath — which stated in a graphic what the Speedy virtue
/// below states in a sentence. That second grid column now carries the base
/// package's day rate instead, drawn from the same published [`PricingOffer`]
/// the fee section renders — the flat annual fee is what the numeral states in
/// a graphic what the fee section states in a sentence.
#[component]
fn SpeedHero(content: TransactionalContent) -> Element {
    let day_rate = content
        .pricing
        .first()
        .and_then(|offer| offer.day_rate.clone());
    rsx! {
        section { class: "speed-hero commitment-hero", "aria-labelledby": "speed-heading",
            div { class: "firm-glow speed-hero__glow", "aria-hidden": "true" }
            div { class: "speed-hero__statement commitment-hero__statement",
                PracticeMarkGlyph {
                    mark: PracticeMark::Handshake,
                    class: "speed-hero__mark commitment-hero__mark".to_string(),
                }
                p { class: "firm-eyebrow", "{content.eyebrow}" }
                h1 { id: "speed-heading", class: "speed-hero__heading commitment-hero__heading",
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
                p { class: "speed-hero__lead commitment-hero__lead", "{content.lead}" }
                a {
                    class: "nav-btn nav-btn--primary speed-hero__cta commitment-hero__cta",
                    href: "{content.cta_href}",
                    "{content.cta_label}"
                }
            }
            if let Some(badge) = day_rate {
                div { class: "speed-hero__rate commitment-hero__rate",
                    BillMarkGlyph {
                        src: badge.image_src.clone(),
                        class: "speed-hero__rate-mark commitment-hero__rate-mark".to_string(),
                    }
                    p { class: "speed-hero__rate-caption commitment-hero__rate-caption", "${badge.amount} a day" }
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
        ul { class: "speed-virtues commitment-benefit-grid",
            for (index , virtue) in virtues.iter().enumerate() {
                li { class: "neon-card speed-virtue commitment-benefit-card", style: "--speed-virtue-index: {index};",
                    p { class: "speed-virtue__word", "{virtue.word}" }
                    p { class: "speed-virtue__body", "{virtue.body}" }
                }
            }
        }
    }
}

/// The flat fee — published as the page's main pricing card — and what it
/// includes.
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
            day_rate: offer.day_rate.clone(),
        })
        .collect();
    rsx! {
        section { class: "speed-fee", "aria-labelledby": "speed-fee-heading",
            div { class: "speed-fee__head",
                h2 { id: "speed-fee-heading", class: "speed-heading", "{content.fee_heading}" }
                p { class: "speed-paragraph", "{content.fee_body}" }
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
        }
    }
}

/// The page's final invitation, kept short so the next step is obvious.
#[component]
fn ClosingCta(content: TransactionalContent) -> Element {
    rsx! {
        section { class: "speed-cta", "aria-labelledby": "speed-cta-heading",
            h2 { id: "speed-cta-heading", class: "speed-cta__heading", "{content.closing_heading}" }
            p { class: "speed-cta__body", "{content.closing_body}" }
            a { class: "speed-cta__link", href: "{content.cta_href}", "{content.closing_email}" }
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
            fee_heading: "One flat annual fee".to_string(),
            fee_body: "One predictable fee covers the recurring legal work.".to_string(),
            pricing: vec![PricingOffer {
                title: "Base package".to_string(),
                price: "$3,650".to_string(),
                cadence: Some("/year".to_string()),
                blurb: "That's just $10 a day.".to_string(),
                features: vec![
                    "Cap table management".to_string(),
                    "Employee and contractor agreements".to_string(),
                    "Basic taxes and state filings".to_string(),
                    "Corporate housekeeping".to_string(),
                    "Counsel on call".to_string(),
                    "DocuSign sent & tracked for you at $5 per contract".to_string(),
                ],
                day_rate: Some(DayRateBadge {
                    amount: 10,
                    image_src: "/public/img/ten-dollar-bill/ten-dollar-bill.jpg".to_string(),
                }),
            }],
            closing_heading: "Ready to build and sell?".to_string(),
            closing_body: "Invite us to be your Fractional GC.".to_string(),
            closing_email: "contact@neonlaw.com".to_string(),
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
            out.contains(r#"data-practice-mark="handshake""#)
                && out.contains("speed-hero__mark")
                && out.contains("commitment-hero"),
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

    /// The dial's old grid slot now carries the base package's day rate,
    /// drawn from the same figure the fee section publishes as a pricing
    /// card, so a fresh copy edit cannot let the two drift apart.
    #[test]
    fn the_hero_states_the_published_day_rate_as_a_graphic() {
        let out = html();
        assert!(
            out.contains("speed-hero__rate")
                && out.contains("commitment-hero__rate")
                && out.contains("ten-dollar-bill.jpg"),
            "the hero draws the $10 bill photo: {out}"
        );
        assert!(out.contains("$10 a day"), "{out}");
    }

    /// A page with no published pricing card has no figure to draw, so the
    /// hero renders no bill photo rather than a bare or stale one.
    #[test]
    fn no_pricing_card_means_no_hero_bill_mark() {
        fn app() -> Element {
            let mut view = content();
            view.pricing.clear();
            rsx! { TransactionalPage { chrome: PublicChrome::default(), content: view } }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = dioxus_ssr::render(&dom);
        assert!(!out.contains("speed-hero__rate"), "{out}");
        assert!(!out.contains("ten-dollar-bill.jpg"), "{out}");
    }

    #[test]
    fn publishes_its_flat_fee_pricing_cards() {
        // The base package is now published on the page as one pricing card
        // rather than quoted through `/contact`: the annual figure, the
        // per-day framing in its body, and the DocuSign per-contract line.
        let out = html();
        assert!(out.contains("One flat annual fee"), "the structure: {out}");
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
    fn carries_no_separately_priced_work_section() {
        let out = html();
        for gone in [
            "Priced separately",
            "Contracts with revisions",
            "Financings",
            "speed-separate",
            "It runs inside your sales cycle",
            "Discovery call",
            "speed-pipeline",
        ] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
    }

    #[test]
    fn ends_with_the_short_build_and_sell_invitation() {
        let out = html();
        assert!(out.contains("Ready to build and sell?"), "{out}");
        assert!(out.contains("Invite us to be your Fractional GC."), "{out}");
        assert!(out.contains(">contact@neonlaw.com</a>"), "{out}");
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
