//! CyberInjuryLaw's approved campaign, rendered and hydrated by Dioxus.

mod assessment;
mod content;
mod fees;
pub use content::CyberInjuryContent;

use super::HomeContent;
use crate::components::{BrandFavicon, SocialMeta, THEME_STYLESHEET_HREF};
use crate::public_chrome::{PublicChrome, PublicFooter};
use dioxus::prelude::*;

#[component]
pub(super) fn CyberInjuryHome(
    chrome: PublicChrome,
    content: HomeContent,
    copy: CyberInjuryContent,
) -> Element {
    rsx! {
        document::Title { "{content.head_title}" }
        document::Meta { name: "description", content: content.meta_description.clone() }
        SocialMeta { title: content.head_title.clone(), description: content.meta_description.clone(), site_name: chrome.brand_name.clone(), image: chrome.social_image.clone() }
        BrandFavicon { logo_href: chrome.logo_href.clone() }
        document::Stylesheet { href: THEME_STYLESHEET_HREF }
        document::Stylesheet { href: crate::brand_style::BRAND_STYLESHEET_HREF }
        document::Stylesheet { href: chrome.tokens_href.clone() }
        document::Stylesheet { href: "/public/css/cyber-injury-law.css" }
        div { class: "cyber-page",
        div { class: "cyber-site",
            a { class: "cyber-skip", href: "#cyber-main", "Skip to content" }
            div { class: "cyber-utility", span { "PERSONAL INJURY. RE-ENGINEERED." } span { "NEW YORK ATTITUDE. NEXT-GENERATION ADVOCACY." } }
            header { class: "cyber-header",
                a { class: "cyber-wordmark", href: "/", "aria-label": "CyberInjuryLaw home",
                    img { src: chrome.logo_href.clone(), alt: "", width: "44", height: "44" }
                    span { "Cyber" span { class: "cyber-lime", "Injury" } "Law.com" small { "HUMAN ADVOCATES. MACHINE ADVANTAGE." } }
                }
                nav { "aria-label": "Main navigation",
                    a { href: "#cyber-advantage", "The AI advantage" }
                    a { href: "#cyber-fees", "Our 20% fee" }
                    a { class: "cyber-button cyber-button--small", href: "#cyber-assessment", "Free case assessment" span { "aria-hidden": "true", "↗" } }
                }
                a { class: "cyber-booking", href: "/consultation", "aria-label": "Book a free consultation at CyberInjuryLaw.com",
                    img { src: "/public/brand/cyber-consultation-qr.svg", alt: "Scan to book: https://www.CyberInjuryLaw.com/consultation", width: "84", height: "84" }
                    span { "BOOK A FREE" strong { "CONSULTATION ↗" } small { "Scan or tap to book" } }
                }
            }
            main { id: "cyber-main",
                section { class: "cyber-hero", "aria-labelledby": "cyber-title",
                    div { class: "cyber-hero__image",
                        img { src: copy.hero_src.clone(), alt: "CyberInjuryLaw campaign portrait with chrome cybernetic arms", fetchpriority: "high", width: "1536", height: "1024" }
                        p { "BUILT TO FIGHT FOR YOU." small { "AI-ENHANCED CAMPAIGN PORTRAIT" } }
                    }
                    div { class: "cyber-hero__copy",
                        p { class: "cyber-eyebrow", span { class: "cyber-lime", "aria-hidden": "true", "✳ " } "{copy.eyebrow}" }
                        h1 { id: "cyber-title", for (index, line) in copy.hero_lines.iter().enumerate() { span { class: if index == 3 { "cyber-lime" } else { "" }, "{line}" } } }
                        p { class: "cyber-lead", "{content.lead}" }
                        a { class: "cyber-button", href: "#cyber-assessment", "{content.contact_label}" span { "aria-hidden": "true", "↗" } }
                        p { class: "cyber-hero__notes", "Instant preliminary assessment · Free consultation" }
                        p { class: "cyber-fine", "{copy.hero_note}" }
                    }
                    p { class: "cyber-fee-badge", "20% ATTORNEY FEE" small { "+ CASE EXPENSES" } }
                }
                div { class: "cyber-ticker", span { "LESS OVERHEAD." } span { "aria-hidden": "true", "✳" } span { "MORE FIGHT." } span { "aria-hidden": "true", "✳" } span { "YOUR RECOVERY COMES FIRST." } }
                section { id: "cyber-fees", class: "cyber-section cyber-split cyber-paper",
                    div { class: "cyber-intro",
                        p { class: "cyber-eyebrow", "THE MATH IS ON YOUR SIDE" }
                        h2 { for (index, line) in copy.fee_heading.iter().enumerate() { span { class: if index == 2 { "cyber-muted" } else { "" }, "{line}" } } }
                        p { "{copy.fee_body}" }
                        p { class: "cyber-fine", "{copy.fee_note}" }
                    }
                    fees::FeeCalculator { note: copy.calculation_note.clone() }
                }
                section { id: "cyber-advantage", class: "cyber-section cyber-advantage",
                    div { class: "cyber-advantage__heading",
                        div { p { class: "cyber-eyebrow", "TECHNOLOGY THAT DOES THE HEAVY LIFTING" } h2 { span { "{copy.discovery_heading[0]}" } span { class: "cyber-lime", "{copy.discovery_heading[1]}" } } }
                        p { "{copy.discovery_body}" }
                    }
                    div { class: "cyber-cards", for (index, card) in copy.discovery_cards.iter().enumerate() {
                        article { span { class: "cyber-glyph", "aria-hidden": "true", if index == 0 { "⌘" } else if index == 1 { "⌕" } else { "↗" } } h3 { "{card[0]}" } p { "{card[1]}" } span { class: "cyber-benefit", "{card[2]}" } }
                    } }
                    p { class: "cyber-fine", "{copy.discovery_note}" }
                }
                section { id: "cyber-assessment", class: "cyber-section cyber-split cyber-paper",
                    div { class: "cyber-intro",
                        p { class: "cyber-eyebrow", "YOUR NEXT MOVE STARTS HERE" }
                        h2 { for (index, line) in copy.assessment_heading.iter().enumerate() { span { class: if index == 0 { "" } else { "cyber-muted" }, "{line}" } } }
                        p { "{copy.assessment_body}" }
                        p { class: "cyber-privacy", "No name. No email. No pressure." br {} "Your answers stay in this browser tab." }
                    }
                    assessment::Assessment { copy: copy.clone(), consultation_href: content.contact_href }
                }
                section { class: "cyber-closing",
                    p { class: "cyber-eyebrow", "PERSONAL INJURY. PERSONAL POWER." }
                    h2 { span { "{copy.closing_lines[0]}" } span { "{copy.closing_lines[1]}" } }
                    a { class: "cyber-button cyber-button--dark", href: "#cyber-assessment", "Make your next move" span { "aria-hidden": "true", "↗" } }
                }
            }
            div { class: "cyber-campaign-footer",
                strong { "CYBERINJURYLAW" } span { "Attorney Advertising" }
                a { href: copy.ad_src, "View the Canal St campaign ↗" }
                p { "{copy.footer_note}" }
            }
        }
        div { class: "nav-theme", PublicFooter { chrome: chrome.clone() } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyber_home_server_renders_the_offer_and_interactive_controls() {
        let html = dioxus_ssr::render_element(rsx! {
            CyberInjuryHome {
                chrome: PublicChrome::default(),
                content: HomeContent { heading: "We use AI to put more money in your pocket".into(), contact_href: "/contact".into(), ..HomeContent::default() },
                copy: CyberInjuryContent { hero_lines: ["WE USE AI TO".into(), "PUT MORE".into(), "MONEY IN".into(), "YOUR POCKET.".into()], hero_src: "/public/img/cyber-injury-law/cyber-warrior.png".into(), ..CyberInjuryContent::default() },
            }
        });
        for required in [
            "WE USE AI TO",
            "YOUR POCKET.",
            "20% ATTORNEY FEE",
            "$76,000",
            "$63,650",
            "+$12,350",
            "cyber-recovery",
            "cyber-incident",
            "cyber-timing",
            "Attorney Advertising",
            "cyber-warrior.png",
        ] {
            assert!(html.contains(required), "missing {required}");
        }
        assert!(!html.contains("fonts.googleapis.com"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("dangerous_inner_html"));
    }
}
