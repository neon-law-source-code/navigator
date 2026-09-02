//! The firm home page (`/`) — the statement of the offering the firm leads
//! with, then the four practices as equal doors.
//!
//! The page publishes no service catalog and no price. It leads with one
//! statement (impact litigation, an open docket, a team), then a grid of
//! practice boxes so litigation, company counsel, technology, and one-time
//! filings are all reachable from `/`. Every fee is quoted through `/contact`.
//!
//! The only state is the static copy ([`HomeContent`]), resolved by the portal
//! router at router-build time and injected via `ServeConfig::context_providers`;
//! the page resolves no per-request data.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PracticeCard, PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

pub use crate::components::PracticeMark;

/// The self-contained home stylesheet, hoisted alongside `theme.css`.
pub const HOME_STYLESHEET_HREF: &str = "/public/css/home.css";

/// One run of practice prose; `emphasis` renders it as `<strong>`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CopyRun {
    pub text: String,
    pub emphasis: bool,
    /// Where this run links, if it links. `Some` renders an inline anchor
    /// instead of bare text, so a sentence can name another page of the site
    /// without breaking out of the paragraph.
    pub href: Option<String>,
}

/// One `<source>` of the hero `<picture>` — the MIME type the browser tests
/// for, and the width-keyed candidates it chooses from.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HeroSource {
    pub mime: String,
    pub srcset: String,
}

/// The hero photograph, resolved to plain URLs.
///
/// Resolved server-side rather than here: the variant URLs come from
/// `views::assets`, which reads `NAVIGATOR_ASSET_BASE_URL` to decide whether
/// the bytes live on the local `/public` mount or in the deployment's public
/// assets bucket. A wasm view cannot answer that question, so the router
/// answers it once and injects the result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HeroPicture {
    /// `<source>` elements in negotiation order: AVIF, WebP, JPEG.
    pub sources: Vec<HeroSource>,
    /// The `<img>` `src` every browser understands.
    pub fallback_src: String,
    /// What the photograph shows. A real description rather than an empty
    /// `alt`: the picture is the page's subject, not decoration behind it.
    pub alt: String,
    pub sizes: String,
}

/// The firm's engagements, in the firm's own words.
///
/// A heading and the paragraphs under it. Deliberately not a list of cards: the
/// shape of the section is itself a claim about how many offerings the reader is
/// choosing between, and the page leads with one.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ServiceSection {
    pub heading: String,
    pub body: Vec<Vec<CopyRun>>,
}

/// One practice the home page points at, as a box at the foot of the page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PracticeLink {
    /// The mark the box opens on, drawn by the view and hidden from assistive
    /// technology: the heading under it already names the practice, so a screen
    /// reader announcing "balance scale" would only repeat it.
    pub mark: PracticeMark,
    pub heading: String,
    pub body: String,
    pub href: String,
}

/// The static home copy — resolved brand-safely at router-build time and
/// injected into the render context.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HomeContent {
    pub head_title: String,
    pub meta_description: String,
    /// The hero photograph the page opens on. `None` when the deployment
    /// publishes no hero: the page then opens on the statement alone
    /// rather than over a broken image.
    pub hero: Option<HeroPicture>,
    /// The practice statement under the hero.
    pub heading: String,
    pub lead: String,
    pub contact_href: String,
    pub contact_label: String,
    /// The one service, in prose. `None` leaves the page at the statement.
    pub service: Option<ServiceSection>,
    /// The heading over the practice boxes. Empty when there are no boxes.
    pub practices_heading: String,
    /// The other practices, as boxes at the foot of the page. Empty renders no
    /// section at all rather than an empty grid.
    pub practices: Vec<PracticeLink>,
}

/// The [`HomeContent`] injected into the render context by the portal router.
#[derive(Clone, Default)]
pub struct InjectedHome(pub HomeContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HomePageView {
    pub chrome: PublicChrome,
    pub content: HomeContent,
}

/// Resolve the chrome and the static home content.
#[server]
pub async fn home_page_view() -> Result<HomePageView, ServerFnError> {
    let content = consume_context::<InjectedHome>().0;
    Ok(HomePageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn HomePageEntry() -> Element {
    let resource = use_server_future(home_page_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        HomePage { chrome: view.chrome, content: view.content }
    }
}

/// The pure home page. Prop-driven, so it server-renders and unit-tests without
/// a server future.
#[component]
pub fn HomePage(chrome: PublicChrome, content: HomeContent) -> Element {
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
        document::Stylesheet { href: HOME_STYLESHEET_HREF }
        PublicShell { header, footer,
            // The hero: the photograph, and nothing over it. The wordmark used
            // to sit on the picture, which said the firm's name a third time —
            // the header mark and the browser tab already do — and cost the
            // photograph its middle. The page's `<h1>` is the practice
            // statement below it, which is the first thing on the page that
            // says something a reader does not already know.
            section { class: "home-hero",
                if let Some(hero) = content.hero.as_ref() {
                    picture { class: "home-hero__picture",
                        for source in hero.sources.iter() {
                            // `srcset`/`sizes` are not in Dioxus's `source`
                            // element definition, so they are written as raw
                            // attributes rather than typed ones.
                            source {
                                r#type: "{source.mime}",
                                "srcset": "{source.srcset}",
                                "sizes": "{hero.sizes}",
                            }
                        }
                        img {
                            class: "home-hero__image",
                            src: "{hero.fallback_src}",
                            alt: "{hero.alt}",
                            sizes: "{hero.sizes}",
                            // The hero is the largest paint on the page; keep
                            // it out of lazy loading so it is not deferred
                            // behind the fold.
                            fetchpriority: "high",
                        }
                    }
                }
            }
            section { class: "home-statement",
                // No glow behind the statement. The hero above it is now the
                // page's decoration, and the wash bled past the photograph's
                // edge into the margin, which read as a rendering fault rather
                // than as lighting.
                h1 { class: "home-statement__heading", "{content.heading}" }
                p { class: "home-statement__lead", "{content.lead}" }
                a {
                    class: "nav-btn nav-btn--primary home-statement__cta",
                    href: "{content.contact_href}",
                    "{content.contact_label}"
                }
            }
            if let Some(service) = content.service.as_ref() {
                ServiceProse { service: service.clone() }
            }
            if !content.practices.is_empty() {
                PracticeLinks {
                    heading: content.practices_heading.clone(),
                    practices: content.practices.clone(),
                }
            }
        }
    }
}

/// The engagements section, in prose: the heading and the paragraphs under it.
#[component]
fn ServiceProse(service: ServiceSection) -> Element {
    rsx! {
        section { class: "neon-card home-service", "aria-labelledby": "home-service-heading",
            h2 { id: "home-service-heading", class: "home-service__heading", "{service.heading}" }
            for paragraph in service.body.iter() {
                p { class: "home-service__paragraph",
                    for run in paragraph.iter() {
                        if let Some(href) = run.href.as_ref() {
                            a { class: "home-service__link", href: "{href}", "{run.text}" }
                        } else if run.emphasis {
                            strong { "{run.text}" }
                        } else {
                            "{run.text}"
                        }
                    }
                }
            }
        }
    }
}

/// The practices, as boxes at the foot of the page.
///
/// The whole box is the link. The section labels itself so the boxes are not
/// unlabelled regions between the prose and the footer.
#[component]
fn PracticeLinks(heading: String, practices: Vec<PracticeLink>) -> Element {
    rsx! {
        section { class: "home-practices", "aria-labelledby": "home-practices-heading",
            h2 { id: "home-practices-heading", class: "home-practices__heading", "{heading}" }
            div { class: "home-practices__grid",
                for (index , practice) in practices.iter().enumerate() {
                    PracticeCard {
                        mark: practice.mark,
                        heading: practice.heading.clone(),
                        body: practice.body.clone(),
                        href: practice.href.clone(),
                        heading_id: format!("home-practice-heading-{index}"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent {
                        head_title: "Home".to_string(),
                        meta_description: "AI enablement for law firms.".to_string(),
                        hero: Some(HeroPicture {
                            sources: vec![
                                HeroSource {
                                    mime: "image/avif".to_string(),
                                    srcset: "/public/img/berkeley-bay/berkeley-bay-400w.avif 400w, \
                                             /public/img/berkeley-bay/berkeley-bay-1200w.avif 1200w"
                                        .to_string(),
                                },
                                HeroSource {
                                    mime: "image/jpeg".to_string(),
                                    srcset: "/public/img/berkeley-bay/berkeley-bay-1200w.jpg 1200w"
                                        .to_string(),
                                },
                            ],
                            fallback_src: "/public/img/berkeley-bay/berkeley-bay-1200w.jpg"
                                .to_string(),
                            alt: "The San Francisco Bay seen from the Berkeley hills.".to_string(),
                            sizes: "100vw".to_string(),
                        }),
                        heading: "AI enablement for law firms".to_string(),
                        lead: "Our clients are law firms.".to_string(),
                        contact_href: "mailto:contact@neonlaw.com".to_string(),
                        contact_label: "Contact us".to_string(),
                        practices_heading: "The rest of what we do".to_string(),
                        practices: vec![PracticeLink {
                            mark: PracticeMark::Scales,
                            heading: "Litigation".to_string(),
                            body: "We try cases on both sides of the v.".to_string(),
                            href: "/litigation".to_string(),
                        }],
                        service: Some(ServiceSection {
                            heading: "What we do".to_string(),
                            body: vec![
                                vec![
                                    CopyRun {
                                        text: "AI reaches the matter ".to_string(),
                                        emphasis: false,
                                        href: None,
                                    },
                                    CopyRun {
                                        text: "through the law firm".to_string(),
                                        emphasis: true,
                                        href: None,
                                    },
                                ],
                                vec![CopyRun {
                                    text: "we deploy ".to_string(),
                                    emphasis: false,
                                    href: None,
                                }, CopyRun {
                                    text: "Neon Law Navigator".to_string(),
                                    emphasis: false,
                                    href: Some("/navigator".to_string()),
                                }],
                            ],
                        }),
                    },
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn renders_the_practice_statement_and_contact_cta() {
        let out = html();
        assert!(
            out.contains("AI enablement for law firms"),
            "the practice statement: {out}"
        );
        assert!(out.contains("Our clients are law firms."), "lead");
        assert!(
            out.contains(r#"href="mailto:contact@neonlaw.com""#),
            "CTA links to the firm inbox"
        );
        assert!(out.contains("Contact us"), "CTA label");
    }

    /// The photograph carries no text. The wordmark used to sit on it, which
    /// said the firm's name a third time — the header mark and the browser tab
    /// already do — and cost the picture its middle. The page's one `<h1>` is
    /// therefore the practice statement, which is the first thing on the page
    /// that tells a reader something they did not already know.
    #[test]
    fn the_page_h1_is_the_practice_statement_and_the_photograph_carries_no_text() {
        let out = html();
        assert_eq!(out.matches("<h1").count(), 1, "one h1: {out}");
        assert!(
            out.contains(r#"<h1 class="home-statement__heading""#),
            "the h1 is the statement: {out}"
        );
        for gone in ["home-hero__wordmark", "home-hero__scrim"] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
        let hero = out.find("home-hero__picture").expect("the photograph");
        let statement = out.find("home-statement").expect("the statement");
        assert!(hero < statement, "the photograph leads the page: {out}");
    }

    #[test]
    fn the_hero_photograph_renders_responsively_with_a_real_description() {
        let out = html();
        // A `<picture>`, not a bare `<img>`: the hero is the page's largest
        // paint, and a phone must not download the 1200px variant.
        assert!(
            out.contains("<picture"),
            "the hero negotiates formats: {out}"
        );
        assert!(
            out.contains(r#"type="image/avif""#),
            "AVIF is offered first: {out}"
        );
        assert!(
            out.contains("berkeley-bay-400w.avif 400w"),
            "the candidates are keyed by width: {out}"
        );
        assert!(
            out.contains(r#"src="/public/img/berkeley-bay/berkeley-bay-1200w.jpg""#),
            "the <img> fallback is the JPEG every browser reads: {out}"
        );
        assert!(
            out.contains(r#"alt="The San Francisco Bay seen from the Berkeley hills.""#),
            "the photograph is described, not hidden behind an empty alt: {out}"
        );
    }

    #[test]
    fn a_deployment_with_no_published_hero_opens_on_the_statement() {
        // The bytes live in a bucket, not in git, so an unpublished hero is a
        // real state rather than a bug — and it must degrade to the statement on
        // the brand surface, never to a broken image.
        let out = statement_only_html();
        assert!(!out.contains("<picture"), "no empty picture: {out}");
        assert!(!out.contains("home-hero__scrim"), "no scrim: {out}");
        assert!(
            out.contains("home-statement__heading"),
            "the statement still leads: {out}"
        );
    }

    /// The page renders one offering, in prose, under one `<h2>`.
    ///
    /// The shape is the claim: a grid of cards tells a reader there are several
    /// things to choose between, and the firm does one thing. This is what keeps
    /// a practice card from growing back.
    #[test]
    fn the_service_is_one_prose_section_and_not_a_grid_of_cards() {
        let out = html();
        assert!(out.contains("What we do"), "the section heading: {out}");
        // One card for the engagements prose, plus one per practice box. What
        // must not come back is a card *per offering* in the prose itself.
        assert_eq!(
            out.matches(r#"class="neon-card home-service""#).count(),
            1,
            "exactly one engagements card: {out}"
        );
        assert_eq!(
            out.matches("<h2").count(),
            2,
            "one h2 for the prose, one for the boxes: {out}"
        );
        assert!(
            out.contains(r#"aria-labelledby="home-service-heading""#),
            "the section is labelled by its own heading: {out}"
        );
        // Matched on the full class attribute, not the bare word: the practice
        // boxes at the foot of the page use `home-practice__heading`, which
        // contains the retired card's class name as a substring. A loose match
        // here would fail on markup that is correct.
        for gone in [
            r#"class="practice-grid""#,
            r#"class="practice__heading""#,
            r#"class="litigation__heading""#,
            r#"class="firm-chip""#,
        ] {
            assert!(!out.contains(gone), "{gone} must not render: {out}");
        }
        let statement = out.find("home-statement").expect("the statement");
        let service = out.find("home-service").expect("the service section");
        assert!(statement < service, "the statement leads: {out}");
    }

    #[test]
    fn service_prose_emphasises_the_phrases_the_firm_sets_in_bold() {
        let out = html();
        assert!(
            out.contains("<strong>through the law firm</strong>"),
            "the emphasised phrase is bold: {out}"
        );
        assert!(
            !out.contains("<strong>AI reaches the matter"),
            "the plain run stays plain: {out}"
        );
    }

    /// A linking run renders as an inline anchor rather than breaking the
    /// paragraph.
    ///
    /// The copy names Navigator mid-sentence and links its page, which is what
    /// `CopyRun::href` exists for. Without it the only way to link from this
    /// section would be a separate call-to-action under the prose, which is a
    /// different thing on the page than a word in a sentence.
    #[test]
    fn a_linking_run_renders_as_an_inline_anchor() {
        let out = html();
        assert!(
            out.contains(r#"<a class="home-service__link" href="/navigator">"#),
            "the linking run is an anchor: {out}"
        );
        assert!(
            out.contains("Neon Law Navigator</a>"),
            "the anchor carries the run's text: {out}"
        );
    }

    /// The access-to-justice line came off the page.
    ///
    /// It closed the section as a separate ruled-off paragraph, and it is gone
    /// deliberately rather than by an edit that lost it. This is what keeps the
    /// markup that framed it from coming back empty.
    #[test]
    fn the_section_carries_no_commitment_line() {
        let out = html();
        assert!(
            !out.contains("home-service__commitment"),
            "no commitment paragraph renders: {out}"
        );
        assert!(
            !out.contains("committed to using AI to improve access to justice"),
            "the retired commitment line is gone: {out}"
        );
    }

    #[test]
    fn the_service_section_stays_out_of_the_markup_when_there_is_none() {
        let out = statement_only_html();
        assert!(!out.contains("home-service"), "no empty section: {out}");
        assert!(!out.contains("neon-card"), "no empty card: {out}");
    }

    /// The boxes at the foot of the page point at the practice pages.
    ///
    /// The whole box is the link. It used to end in a "The litigation practice"
    /// label and the box was inert; with the label gone the box has to carry the
    /// link itself, or the practices would be named on the page with no way to
    /// reach them.
    ///
    /// The anchor names itself by its heading rather than by its contents. A
    /// link whose accessible name is the heading *and* the sentence is read out
    /// in full before a reader learns where it goes.
    #[test]
    fn each_practice_box_is_itself_the_link() {
        let out = html();
        assert!(out.contains("home-practices"), "the section renders: {out}");
        assert!(
            out.contains(r#"aria-labelledby="home-practices-heading""#),
            "the section labels itself: {out}"
        );
        assert!(
            out.contains(r#"<a class="neon-card home-practice" href="/litigation""#),
            "the box is the anchor: {out}"
        );
        assert!(
            out.contains(r#"aria-labelledby="home-practice-heading-0""#),
            "the anchor is named by its heading: {out}"
        );
        assert!(
            out.contains(r#"<h3 id="home-practice-heading-0""#),
            "each box heading carries its own id: {out}"
        );
        // Boxes, not an enumeration: no `<ul>`/`<li>` around them.
        assert!(
            !out.contains("<ul class=\"home-practices__grid\""),
            "the boxes are not a list: {out}"
        );
        // The retired "read more" labels. Each was a second thing to click in a
        // box that is now entirely clickable.
        for retired in [
            "The litigation practice",
            "The fractional GC practice",
            "The legal services schedule",
            "home-practice__link",
        ] {
            assert!(!out.contains(retired), "{retired} must not render: {out}");
        }
        // The mark is decorative — the heading beside it names the practice, so
        // a screen reader must not read the glyph out as well, and it stays out
        // of the tab order.
        assert!(
            out.contains(r#"class="home-practice__mark""#),
            "the mark renders: {out}"
        );
        assert!(
            out.contains(r#"aria-hidden="true""#) && out.contains(r#"focusable="false""#),
            "the mark is hidden from assistive technology: {out}"
        );
        // Stroked in `currentColor`, which is what lets it be white on the dark
        // theme — a colour emoji could not be recoloured at all.
        assert!(
            out.contains(r#"stroke="currentColor""#),
            "the mark takes the card's colour: {out}"
        );
        assert!(
            out.contains("M12 3v18"),
            "the litigation box carries the scales' beam: {out}"
        );
    }

    /// The boxes take their heading from the content rather than the view.
    ///
    /// It used to be the literal "Our legal practice" in the markup, which
    /// stopped being true when the boxes started carrying the fractional CTO
    /// engagement beside the two legal practices. The heading is copy, so it
    /// lives with the rest of the copy.
    #[test]
    fn the_practice_boxes_take_their_heading_from_the_content() {
        let out = html();
        assert!(
            out.contains("The rest of what we do"),
            "the injected heading renders: {out}"
        );
        assert!(
            !out.contains("Our legal practice"),
            "the hard-coded heading is gone: {out}"
        );
    }

    #[test]
    fn the_practice_boxes_stay_out_of_the_markup_when_there_are_none() {
        let out = statement_only_html();
        assert!(!out.contains("home-practices"), "no empty section: {out}");
    }

    /// The boxes sit at the foot of the page, under the engagements prose.
    ///
    /// Order is the claim: the page leads with one offering, and these say the
    /// firm practices law too. Above the prose they would read as the page
    /// offering four things.
    #[test]
    fn the_practice_boxes_sit_under_the_engagements_prose() {
        let out = html();
        let service = out.find("home-service").expect("the engagements section");
        let practices = out.find("home-practices").expect("the practice boxes");
        assert!(service < practices, "prose then boxes: {out}");
    }

    #[test]
    fn the_statement_carries_no_glow_behind_it() {
        // The wash bled past the hero photograph's edge into the page margin,
        // which reads as a rendering fault. This pins that the glow does not
        // come back with the next copy edit.
        let out = html();
        assert!(
            !out.contains("firm-glow"),
            "no glow on the home page: {out}"
        );
    }

    #[test]
    fn wraps_the_page_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome");
    }

    /// The page with nothing but its defaults: no hero, no service section.
    fn statement_only_html() -> String {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent::default(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }
}
