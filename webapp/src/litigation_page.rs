//! The firm litigation page (`/litigation`) — the disputes practice, told from
//! both sides of the v.
//!
//! The page states one thing: the firm tries cases fast, for the party bringing
//! the claim and for the party answering it. A statement and the body.
//!
//! **Speed is the page's claim, and the body states it as method rather than as
//! outcome.** A page that leads with speed is one number away from advertising
//! a result, which is why `publishes_no_currency_amount` and the server-side
//! quantified-efficiency guard both bind harder here than they did under the
//! previous "zealous advocates" framing.
//!
//! **The body is the firm's own filed copy, and the page is what is left after
//! subtracting everything else.** It was a Rule 23 explainer with six
//! certification-element cards, a class-formation graphic, a chip strip, a phase
//! rail, an authority strip, and a fee section — each a reasonable answer to a
//! question a prospective client does not walk in with. What the firm wanted
//! said fits in a few paragraphs, so a few paragraphs are the page.
//!
//! **A paragraph may be added; a section may not.** The third body paragraph —
//! how the firm actually runs a matter — arrived as prose inside the existing
//! card rather than as a heading, a rail, or a feature grid, which is the shape
//! everything this page shed once had. `renders_two_sections_and_no_more` is the
//! guard that keeps that distinction, so it is the one to read before adding
//! copy here.
//!
//! **The one call to action lives in the body card, not as a section of its
//! own.** It is the same email and phone channels [`PublicFooter`] closes every
//! page with — `mailto_href` and `tel_href` are the footer's own helpers,
//! reused rather than restated so the subject line and the dialled digits stay
//! one source — laid out as `.zeal-cta`, a centered row the page's own
//! stylesheet places rather than the footer's three-column office grid.
//! `renders_the_contact_channels_side_by_side` is the guard.
//!
//! The body once named fee *arrangements* — contingency, monthly, "no cost due
//! if we lose" — on the reasoning that for this practice the arrangement is
//! part of the offer. Those paragraphs have been replaced and the sentence went
//! with them; whether to restore one is tracked in
//! `neon::firm_pages::resolve_litigation_content`, which holds the copy. Fee
//! *amounts* stay off the page regardless, and `publishes_no_currency_amount`
//! holds that.
//!
//! **The regulated copy is the footer's, not this page's.** This page used to
//! carry its own past-results disclaimer under the body, duplicating what the
//! shared footer already says on every firm page. One disclaimer, stated once,
//! site-wide, is the arrangement now: `views::brand::DEFAULT_BRANDING`'s
//! `firm_disclaimer` opens with "Attorney advertisement." and carries both the
//! no-advice and the past-results lines, and `PublicFooter` renders it here
//! along with everywhere else. `carries_the_regulated_copy_in_the_footer` is
//! the guard — it asserts the notice reaches the reader, which is what the
//! rule actually requires, rather than asserting it appears twice.
//!
//! Like [`crate::home`], the only state is the static copy
//! ([`LitigationContent`]), resolved by the portal router at router-build time
//! and injected via `ServeConfig::context_providers`.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    mailto_href, tel_href, Icon, IconName, PracticeMark, PracticeMarkGlyph, PublicShell,
    SiteHeader, SiteNavLink, SocialMeta,
};
use crate::home::CopyRun;
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The self-contained litigation stylesheet, hoisted after the brand layer.
pub const LITIGATION_STYLESHEET_HREF: &str = "/public/css/litigation.css";

/// One word of the hero statement, carrying its own reveal order.
///
/// The heading is split into words in the *content* rather than in CSS because
/// each word needs its own element to stagger, and a view that split a string
/// would have to guess where the emphasis falls.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HeroWord {
    pub text: String,
    /// Set on the words the firm sets in its own colour — "Litigation
    /// attorneys".
    pub accent: bool,
}

/// The static litigation copy — resolved brand-safely at router-build time and
/// injected into the render context.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LitigationContent {
    pub head_title: String,
    pub meta_description: String,
    pub eyebrow: String,
    /// The page `<h1>`, one word per element so the reveal can stagger.
    pub heading: Vec<HeroWord>,
    pub lead: String,
    pub cta_href: String,
    pub cta_label: String,
    /// The practice in the firm's own words: who it represents on the company
    /// side, who it represents against powerful corporations, and how it runs
    /// the matter once it takes one. This is the whole body of the page.
    ///
    /// Runs rather than plain strings, because the last paragraph names
    /// Navigator and links its page. [`CopyRun`] is the same linked-run type
    /// the home page's prose uses and the same one the firm's `plain()` and
    /// `link()` content helpers already produce, so no page has to accept raw
    /// HTML to carry a link.
    pub body: Vec<Vec<CopyRun>>,
}

/// The [`LitigationContent`] injected into the render context by the portal
/// router.
#[derive(Clone, Default)]
pub struct InjectedLitigation(pub LitigationContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LitigationPageView {
    pub chrome: PublicChrome,
    pub content: LitigationContent,
}

/// Resolve the chrome and the static litigation content.
#[server]
pub async fn litigation_page_view() -> Result<LitigationPageView, ServerFnError> {
    let content = consume_context::<InjectedLitigation>().0;
    Ok(LitigationPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn LitigationPageEntry() -> Element {
    let resource = use_server_future(litigation_page_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        LitigationPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure litigation page. Prop-driven, so it server-renders and unit-tests
/// without a server future.
#[component]
pub fn LitigationPage(chrome: PublicChrome, content: LitigationContent) -> Element {
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
        document::Stylesheet { href: LITIGATION_STYLESHEET_HREF }
        PublicShell { header, footer,
            ZealHero { content: content.clone() }
            section { class: "neon-card zeal-body", "aria-labelledby": "zeal-heading",
                for paragraph in content.body.iter() {
                    p { class: "zeal-paragraph zeal-paragraph--lead",
                        for run in paragraph.iter() {
                            if let Some(href) = run.href.as_ref() {
                                a { class: "zeal-paragraph__link", href: "{href}", "{run.text}" }
                            } else if run.emphasis {
                                strong { "{run.text}" }
                            } else {
                                "{run.text}"
                            }
                        }
                    }
                }
                // The same two channels the footer closes every page with, not
                // a second copy of them: `mailto_href` and `tel_href` are the
                // footer's own helpers, so the subject line and the dialled
                // digits can never drift between the two places they render.
                // What differs here is layout, not content: side by side and
                // centered, because this is the page's own ask rather than the
                // sign-off every page carries.
                //
                // Each channel is conditional on the chrome actually carrying
                // one, the same guard `PublicFooter` applies, so a chrome with
                // no phone number does not render a dead `tel:` link with
                // nothing to dial.
                ul { class: "zeal-cta",
                    if !chrome.firm_email.is_empty() {
                        li { class: "site-footer__office site-footer__office--channel",
                            a {
                                class: "site-footer__channel-link",
                                href: mailto_href(&chrome.firm_email),
                                Icon { name: IconName::EnvelopeFill }
                                span { "{chrome.firm_email}" }
                            }
                        }
                    }
                    if !chrome.firm_phone.is_empty() {
                        li { class: "site-footer__office site-footer__office--channel",
                            a {
                                class: "site-footer__channel-link",
                                href: tel_href(&chrome.firm_phone),
                                Icon { name: IconName::TelephoneFill }
                                span { "{chrome.firm_phone}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The hero: the statement, the lead, and the one call to action.
///
/// One column. It carried a class-formation graphic beside it — dots arriving
/// and closing into a frame — which was a picture of the one thing this page no
/// longer leads with, so the statement now has the width to itself.
#[component]
fn ZealHero(content: LitigationContent) -> Element {
    rsx! {
        section { class: "zeal-hero", "aria-labelledby": "zeal-heading",
            div { class: "firm-glow zeal-hero__glow", "aria-hidden": "true" }
            div { class: "zeal-hero__statement",
                PracticeMarkGlyph {
                    mark: PracticeMark::Scales,
                    class: "zeal-hero__mark".to_string(),
                }
                p { class: "firm-eyebrow", "{content.eyebrow}" }
                h1 { id: "zeal-heading", class: "zeal-hero__heading",
                    for (index , word) in content.heading.iter().enumerate() {
                        span {
                            class: if word.accent { "zeal-word zeal-word--accent" } else { "zeal-word" },
                            style: "--zeal-word-index: {index};",
                            // No trailing space: the word gaps are a margin in
                            // the stylesheet. An inline-block collapses the
                            // whitespace at its own end, so a space here would
                            // render as nothing and run the words together.
                            "{word.text}"
                        }
                    }
                }
                p { class: "zeal-hero__lead", "{content.lead}" }
                a {
                    class: "nav-btn nav-btn--primary zeal-hero__cta",
                    href: "{content.cta_href}",
                    "{content.cta_label}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> CopyRun {
        CopyRun {
            text: text.to_string(),
            emphasis: false,
            href: None,
        }
    }

    fn link(text: &str, href: &str) -> CopyRun {
        CopyRun {
            text: text.to_string(),
            emphasis: false,
            href: Some(href.to_string()),
        }
    }

    fn content() -> LitigationContent {
        LitigationContent {
            head_title: "Neon Law | Litigation".to_string(),
            meta_description: "Litigation on both sides of the v.".to_string(),
            eyebrow: "Litigation, plaintiff and defense".to_string(),
            heading: vec![
                HeroWord {
                    text: "Litigation".to_string(),
                    accent: true,
                },
                HeroWord {
                    text: "attorneys".to_string(),
                    accent: true,
                },
                HeroWord {
                    text: "built".to_string(),
                    accent: false,
                },
                HeroWord {
                    text: "for".to_string(),
                    accent: false,
                },
                HeroWord {
                    text: "speed.".to_string(),
                    accent: false,
                },
            ],
            lead: "We do as much as we can, as early as we can.".to_string(),
            cta_href: "/contact".to_string(),
            cta_label: "Contact us".to_string(),
            body: vec![
                vec![plain(
                    "We represent those who haven\u{2019}t been justly seen. We have litigated \
                     trademark and copyright disputes, prison rights litigation, and divorce.",
                )],
                vec![plain(
                    "There is little we will not take on. We have handled restraining orders and \
                     domestic violence matters.",
                )],
                vec![
                    plain("All litigation cases run on "),
                    link("Neon Law Navigator", "/navigator"),
                    plain(", the firm\u{2019}s case system."),
                ],
            ],
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                LitigationPage { chrome: PublicChrome::default(), content: content() }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    /// A chrome carrying real contact channels, for the one test that needs to
    /// see them render as more than an empty `mailto:`/`tel:` href.
    fn chrome_with_contact() -> PublicChrome {
        PublicChrome {
            firm_email: "counsel@neonlaw.com".to_string(),
            firm_phone: "+1 555 010 0100".to_string(),
            ..Default::default()
        }
    }

    fn html_with(chrome: PublicChrome) -> String {
        let mut dom = VirtualDom::new_with_props(
            LitigationPage,
            LitigationPageProps {
                chrome,
                content: content(),
            },
        );
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn leads_with_the_speed_statement() {
        let out = html();
        assert!(
            out.contains(r#"data-practice-mark="scales""#) && out.contains("zeal-hero__mark"),
            "the hero reuses the four-card litigation mark: {out}"
        );
        assert_eq!(out.matches("<h1").count(), 1, "one h1: {out}");
        assert!(out.contains("built"), "the statement: {out}");
        assert!(out.contains("speed."), "the statement: {out}");
        // The eyebrow names both sides of the v. rather than one: the firm
        // brings cases and it answers them, and a page that advertised only the
        // plaintiff side would send half of its readers away.
        assert!(
            out.contains("Litigation, plaintiff and defense"),
            "the eyebrow names both sides: {out}"
        );
        assert!(
            out.contains(r#"href="/contact""#),
            "the CTA routes to contact"
        );
    }

    /// The page publishes no em dash.
    ///
    /// The firm's style call for this page, and a guard rather than a habit
    /// because an em dash is the punctuation a copy edit reaches for by reflex:
    /// the surrounding module doc comments are full of them, so a sentence
    /// moved from a comment into the content carries one in silently.
    #[test]
    fn publishes_no_em_dash() {
        let out = html();
        assert!(!out.contains('\u{2014}'), "no em dash: {out}");
    }

    #[test]
    fn every_hero_word_carries_its_own_reveal_index() {
        // The stagger is data, not a CSS nth-child guess: a copy edit that adds
        // a word has to keep working, so the index rides on the element.
        let out = html();
        for index in 0..5 {
            assert!(
                out.contains(&format!("--zeal-word-index: {index};")),
                "word {index} carries its reveal order: {out}"
            );
        }
    }

    /// The page is a statement, one card of prose, and the disclaimer.
    ///
    /// Asserted as a count rather than a list of names because the failure this
    /// guards against is a section being *added*. This page grew a Rule 23
    /// explainer, a chip strip, a phase rail, an authority strip, and a fee
    /// section one reasonable-looking addition at a time.
    #[test]
    fn renders_two_sections_and_no_more() {
        let out = html();
        assert_eq!(
            out.matches("<section").count(),
            2,
            "the hero and the body, and nothing else: {out}"
        );
        assert_eq!(out.matches("<h1").count(), 1, "one h1: {out}");
        assert_eq!(
            out.matches("<h2").count(),
            0,
            "the body needs no heading of its own: {out}"
        );
    }

    /// The page's call to action is the same two channels the footer closes
    /// every page with, rendered a second time in its own centered row rather
    /// than the footer's three-column grid.
    ///
    /// This asserts the reuse rather than the layout: both links carry the
    /// same `mailto_href`/`tel_href` a real chrome resolves, which is what
    /// keeps this row from drifting out of sync with the footer's own. The
    /// `.zeal-cta` wrapper (not `.site-footer__offices`) is what keeps the
    /// footer's grid-column rules from pulling these two tiles into its
    /// vertical stack; the centering itself is a CSS concern this test does
    /// not reach into.
    #[test]
    fn renders_the_contact_channels_side_by_side() {
        let out = html_with(chrome_with_contact());
        assert!(
            out.contains(r#"class="zeal-cta""#),
            "the CTA row wraps in its own class, not the footer's office grid: {out}"
        );
        assert_eq!(
            out.matches(r#"class="site-footer__channel-link""#).count(),
            4,
            "two in the page's own CTA, two in the footer: {out}"
        );
        assert!(
            out.contains("mailto:counsel@neonlaw.com"),
            "the email channel resolves the same href the footer uses: {out}"
        );
        assert!(
            out.contains("tel:+15550100100"),
            "the phone channel resolves the same href the footer uses: {out}"
        );
        // Still no third section: the CTA is markup inside the existing body
        // card, not a section of its own.
        assert_eq!(out.matches("<section").count(), 2, "still the two: {out}");
    }

    /// Every body paragraph renders, in order: who the firm represents, the
    /// breadth of what it takes on, then how the firm runs the matter. The
    /// middle one is what an edit is most likely to drop, because the first
    /// reads like a complete practice description on its own.
    #[test]
    fn renders_every_paragraph_in_order() {
        let out = html();
        let seen = out
            .find("We represent those who")
            .expect("the who-we-represent paragraph");
        let breadth = out
            .find("There is little we will not take on")
            .expect("the breadth paragraph");
        let system = out
            .find("All litigation cases run on")
            .expect("the case-system paragraph");
        assert!(seen < breadth, "in the filed order: {out}");
        assert!(
            breadth < system,
            "how the work runs comes after who the firm represents: {out}"
        );
    }

    /// The body renders however many paragraphs the content carries.
    ///
    /// Asserted against the fixture length rather than a literal, because the
    /// page has lost and gained paragraphs and the view must not cap them: the
    /// guard that matters is [`renders_two_sections_and_no_more`], which stops a
    /// paragraph from arriving as a new *section*.
    #[test]
    fn renders_one_paragraph_per_body_entry() {
        let out = html();
        assert_eq!(
            out.matches("zeal-paragraph--lead").count(),
            content().body.len(),
            "one paragraph element per body entry: {out}"
        );
        // The last paragraph names Navigator and links its page. A copy edit
        // that flattens the runs back to plain text loses the link silently,
        // because the sentence still reads correctly without it.
        assert!(
            out.contains(r#"href="/navigator""#),
            "the Navigator mention links its page: {out}"
        );
    }

    /// Everything the page shed, guarded by the markup that carried it: the
    /// class figure and its dots, the creed, the Rule 23 element cards, the chip
    /// strip, the phase rail, and the authority strip.
    #[test]
    fn carries_none_of_the_sections_it_shed() {
        let out = html();
        for gone in [
            "zeal-figure",
            "zeal-dot",
            "zeal-creed",
            "zeal-element",
            "zeal-area",
            "zeal-rail",
            "zeal-authority",
            "zeal-referral",
        ] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
    }

    /// The regulated notice reaches the reader through the shared footer, and
    /// the page states it once rather than twice.
    ///
    /// What the rule requires is that a reader who could infer a result sees the
    /// notice — not that a given page renders its own copy of it. So this
    /// asserts the requirement (the footer's legal strip is present) and asserts
    /// the removed duplicate stays removed. The wording itself lives in
    /// `views::brand`, which is where a change to it belongs; a fixture here
    /// repeating that sentence would only go stale against it.
    #[test]
    fn carries_the_regulated_copy_in_the_footer() {
        let out = html();
        assert!(
            out.contains("site-footer__legal"),
            "the footer's legal strip, which carries the firm disclaimer: {out}"
        );
        assert!(
            !out.contains("zeal-disclaimer"),
            "the page-level duplicate is gone: {out}"
        );
        for gone in [
            "The law this practice runs on",
            "When we are not the right answer",
            "zeal-authority",
            "zeal-referral",
        ] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
    }

    /// No amount, ever. A fee *arrangement* may be named, because for this
    /// practice the arrangement is part of the offer and a reader needs to know
    /// whether a case costs anything to bring. A figure is different: a rate or
    /// a percentage on a marketing page goes stale against what the firm charges
    /// and reads as a binding quote.
    #[test]
    fn publishes_no_currency_amount() {
        let out = html();
        assert!(!out.contains('$'), "no currency amount: {out}");
        assert!(!out.contains('%'), "no percentage: {out}");
    }

    #[test]
    fn publishes_no_outcome_promise_or_superlative() {
        // The words a future copy edit could reintroduce without noticing. Each
        // is either an outcome promise or an unsubstantiable superlative, and
        // one of them on a lawyer-advertising page is a bar problem.
        let out = html().to_lowercase();
        for banned in [
            "guarantee you",
            "we will win",
            "best",
            "premier",
            "world-class",
            "industry-leading",
            "fastest",
            "top-tier",
        ] {
            assert!(!out.contains(banned), "{banned} must not render: {out}");
        }
    }

    #[test]
    fn wraps_the_page_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome");
    }
}
