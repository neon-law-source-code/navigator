//! Brand home pages, company counsel, and lifetime estate planning.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    is_external_href, PracticeCard, PublicShell, SiteHeader, SiteNavLink, SocialMeta,
    TestimonialCard, TestimonialSection, THEME_STYLESHEET_HREF,
};
use crate::public_chrome::{PublicChrome, PublicFooter};

pub use crate::components::PracticeMark;
mod company;
pub use company::CompanyContent;
mod estate;
pub use estate::EstateContent;
mod privacy;
pub use privacy::PrivacyContent;

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

/// The firm's engagements, in the firm's own words.
///
/// A heading and the paragraphs under it, drawn as a full-width band so the
/// heading shares the statement's left edge. Deliberately not a card or a
/// list of cards: the shape of the section is itself a claim about how many
/// offerings the reader is choosing between, and the page leads with one.
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
    /// Optional identity mark for a parent-brand directory card.
    #[serde(default)]
    pub logo_href: String,
    /// Optional display face for a parent-brand directory card.
    #[serde(default)]
    pub font_family: String,
}

/// The decorative mark a provenance step opens on, drawn by the view and
/// hidden from assistive technology: the label beside it already names the
/// step.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum ProvenanceMark {
    /// The request a person sends: a document.
    #[default]
    Request,
    /// The licensed attorney's verification: a shield with a check.
    Attorney,
    /// The record uploaded to the chain: linked blocks.
    Chain,
}

/// One step of the flow a request follows, drawn as a node on a rail.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProvenanceStep {
    pub mark: ProvenanceMark,
    pub label: String,
    pub detail: String,
}

/// One row of the ledger illustration: a place a request went, and the record
/// that followed it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProvenanceLedgerRow {
    pub label: String,
    pub status: String,
}

/// How a request becomes a durable record: the flow, the ledger illustration
/// beside it, and the prose under both. Rendered only by a brand that keeps
/// such a record.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProvenanceSection {
    pub overline: String,
    pub heading: String,
    /// A second heading line set in the brand gradient; empty renders none.
    pub heading_accent: String,
    pub lead: String,
    pub steps: Vec<ProvenanceStep>,
    pub ledger_heading: String,
    pub ledger_caption: String,
    pub ledger: Vec<ProvenanceLedgerRow>,
    pub pillars: Vec<ProvenancePillar>,
    pub notes: Vec<Vec<CopyRun>>,
}

/// One tile under the flow: what the record is for.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProvenancePillar {
    pub heading: String,
    pub body: String,
}

/// The static home copy — resolved brand-safely at router-build time and
/// injected into the render context.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HomeContent {
    pub head_title: String,
    pub meta_description: String,
    /// The question the page opens on. It is the page's one `<h1>` and the
    /// first thing a reader sees under the header: nothing sits above it.
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
    /// How a request becomes a record. `None` renders no section, so a brand
    /// that keeps no such record says nothing about one.
    #[serde(default)]
    pub provenance: Option<ProvenanceSection>,
    /// Company counsel presentation; absent for other house brands.
    #[serde(default)]
    pub company: Option<CompanyContent>,
    /// The firm's notice and sign-in, followed by any catalogued practice
    /// cards and the shared footer, without the marketing header.
    #[serde(default)]
    pub bare: Option<BareStatement>,
    /// The lifetime estate-planning offer, authored in the brand catalog.
    #[serde(default)]
    pub estate: Option<EstateContent>,
    /// The annual privacy product, authored in the brand catalog.
    #[serde(default)]
    pub privacy: Option<PrivacyContent>,
}

/// The firm's notice and sign-in. When [`HomeContent::bare`] is set, this
/// leads into any practice cards and the shared footer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct BareStatement {
    pub heading: String,
    pub paragraph: String,
    /// A second sentence with one inline link — the existing client's way
    /// in. Empty renders no second sentence.
    pub sign_in: Vec<CopyRun>,
}

/// The [`HomeContent`] injected into the render context by the portal router.
#[derive(Clone, Default)]
pub struct InjectedHome(pub HomeContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HomePageView {
    pub chrome: PublicChrome,
    pub content: HomeContent,
    #[serde(default)]
    pub testimonials: Vec<TestimonialCard>,
}

/// Resolve the chrome and the static home content.
#[server]
pub async fn home_page_view() -> Result<HomePageView, ServerFnError> {
    let content =
        crate::public_chrome::copy_from_request_or_context(consume_context::<InjectedHome>)
            .await
            .0;
    let surreal = consume_context::<store::surreal::SurrealDb>();
    let testimonials = store::testimonials::published_for_home(&surreal, 6)
        .await
        .map_err(|error| ServerFnError::new(error.to_string()))?
        .into_iter()
        .map(home_testimonial_card)
        .collect();
    Ok(HomePageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
        testimonials,
    })
}

/// Map a published store row onto the homepage card. Attribution is only the
/// client's chosen label; a blank choice publishes the quote without a name,
/// title, or profile image.
#[cfg(feature = "server")]
fn home_testimonial_card(
    testimonial: store::testimonials::PublishedTestimonial,
) -> TestimonialCard {
    TestimonialCard {
        quote: testimonial.quote,
        attribution: testimonial.attribution_label.unwrap_or_default(),
        detail: None,
        profile_image_url: None,
        product_label: None,
    }
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
        HomePage {
            chrome: view.chrome,
            content: view.content,
            testimonials: view.testimonials,
        }
    }
}

/// The pure home page. Prop-driven, so it server-renders and unit-tests without
/// a server future.
#[component]
pub fn HomePage(
    chrome: PublicChrome,
    content: HomeContent,
    #[props(default)] testimonials: Vec<TestimonialCard>,
) -> Element {
    if let Some(bare) = content.bare.clone() {
        return rsx! {
            document::Title { "{content.head_title}" }
            document::Meta { name: "description", content: "{content.meta_description}" }
            SocialMeta {
                title: content.head_title.clone(),
                description: content.meta_description.clone(),
                site_name: chrome.brand_name.clone(),
                image: chrome.social_image.clone(),
            }
            // The base theme (background/text from tokens) and the resolved
            // brand's palette and typeface. Every other public page picks
            // these up from `PublicShell` and `PublicFooter`; this page
            // renders neither, so it hoists both itself.
            document::Stylesheet { href: THEME_STYLESHEET_HREF }
            document::Stylesheet { href: crate::brand_style::BRAND_STYLESHEET_HREF }
            document::Stylesheet { href: HOME_STYLESHEET_HREF }
            document::Stylesheet { href: "/public/css/vesta.css" }
            document::Stylesheet { href: "{chrome.tokens_href}" }
            // The theme root without `PublicShell`'s own marker: the holding
            // page is deliberately not a public marketing page (no header,
            // no support-chat widget), but the one shared footer under it
            // needs the theme's anchor and chip rules, which hang off this
            // class.
            div { class: "nav-theme",
                main { class: "holding-page",
                    h1 { class: "holding-page__heading", "{bare.heading}" }
                    p { class: "holding-page__paragraph", "{bare.paragraph}" }
                    if !bare.sign_in.is_empty() {
                        p { class: "holding-page__paragraph",
                            for run in bare.sign_in.iter() {
                                if let Some(href) = run.href.as_ref() {
                                    a { class: "holding-page__link", href: "{href}", "{run.text}" }
                                } else if run.emphasis {
                                    strong { "{run.text}" }
                                } else {
                                    "{run.text}"
                                }
                            }
                        }
                    }
                    if !content.practices.is_empty() {
                        PracticeLinks {
                            heading: content.practices_heading.clone(),
                            practices: content.practices.clone(),
                        }
                    }
                }
                // The same footer every other page of the firm's sites
                // carries — its office, its family of brands, its membership,
                // and the legal strip — so a reader on the holding host finds
                // the firm's address and the way to its other sites, and the
                // footer is one thing everywhere rather than everywhere but
                // here.
                PublicFooter { chrome: chrome.clone() }
            }
        };
    }
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
        if content.estate.is_some() {
            document::Stylesheet { href: "/public/css/vesta.css" }
        }
        if content.privacy.is_some() {
            document::Stylesheet { href: "/public/css/privacy.css" }
        }
        PublicShell { header, footer,
            if let Some(company) = content.company.as_ref() {
                company::CompanyHome { content: content.clone(), company: company.clone() }
            } else if let Some(estate) = content.estate.as_ref() {
                estate::EstateHome { content: content.clone(), estate: estate.clone() }
            } else if let Some(privacy) = content.privacy.as_ref() {
                privacy::PrivacyHome { content: content.clone(), privacy: privacy.clone() }
            } else {
            // The page opens on the question. No photograph above it and no
            // glow behind it: the question is the page, so it is the first
            // thing under the header and set large enough to read as such.
            section { class: "home-statement",
                h1 { class: "home-statement__heading", "{content.heading}" }
                p { class: "home-statement__lead", "{content.lead}" }
                a {
                    class: "nav-btn nav-btn--primary home-statement__cta",
                    href: "{content.contact_href}",
                    target: if is_external_href(&content.contact_href) { Some("_blank") } else { None },
                    rel: if is_external_href(&content.contact_href) { Some("noopener noreferrer") } else { None },
                    "{content.contact_label}"
                }
            }
            if let Some(service) = content.service.as_ref() {
                ServiceProse { service: service.clone() }
            }
            if let Some(provenance) = content.provenance.as_ref() {
                ProvenanceBand { provenance: provenance.clone() }
            }
            TestimonialSection {
                heading: "What clients say".to_string(),
                lead: "Shared by clients who asked us to publish their experience.".to_string(),
                cards: testimonials,
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
}

/// The engagements section, in prose: a full-width band, not a card, so the
/// heading shares the statement's left edge.
///
/// A linked run renders as a **classless** `<a>`, and that is load-bearing
/// rather than incidental. `theme.css` gives every inline prose link its
/// non-colour cue through `.nav-theme :is(p, li) > a:not([class])` — keyed on
/// the absence of a class precisely so no new prose page has to be remembered
/// into an allow-list. A class here, even a decorative one, opts these links
/// out of that rule and leaves them distinguishable by colour alone, which is
/// the `link-in-text-block` violation axe reports — and did report, on this
/// page in the dark scheme, where the shared link stop is 1.50:1 against body
/// text. The class this replaces styled nothing observable: `.nav-theme a`
/// carries one type selector more, so it won both the colour and
/// `text-decoration: none`, leaving a declared thickness and offset shaping a
/// line that was never drawn.
#[component]
fn ServiceProse(service: ServiceSection) -> Element {
    rsx! {
        section { class: "home-service", "aria-labelledby": "home-service-heading",
            h2 { id: "home-service-heading", class: "home-service__heading", "{service.heading}" }
            for paragraph in service.body.iter() {
                p { class: "home-service__paragraph",
                    for run in paragraph.iter() {
                        if let Some(href) = run.href.as_ref() {
                            a { href: "{href}", "{run.text}" }
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

/// How a request becomes a record: the flow on a rail, the ledger beside it,
/// and the notes under both.
///
/// The steps are an `<ol>` because their order is the claim — verification
/// comes before the record, never after. The ledger is an illustration, and
/// its caption says so in words; the bars inside it are decoration and stay
/// out of the accessibility tree. Every mark is stroked in `currentColor` so
/// one drawing serves both schemes.
#[component]
fn ProvenanceBand(provenance: ProvenanceSection) -> Element {
    rsx! {
        section {
            class: "neon-card home-provenance",
            "aria-labelledby": "home-provenance-heading",
            div { class: "home-provenance__glow", "aria-hidden": "true" }
            header { class: "home-provenance__header",
                p { class: "firm-eyebrow home-provenance__overline", "{provenance.overline}" }
                h2 { id: "home-provenance-heading", class: "home-provenance__heading",
                    "{provenance.heading}"
                    if !provenance.heading_accent.is_empty() {
                        " "
                        span { class: "home-provenance__heading-accent", "{provenance.heading_accent}" }
                    }
                }
                if !provenance.lead.is_empty() {
                    p { class: "home-provenance__lead", "{provenance.lead}" }
                }
            }
            div { class: "home-provenance__grid",
                if !provenance.steps.is_empty() {
                    ol { class: "home-provenance__steps",
                        for (index , step) in provenance.steps.iter().enumerate() {
                            li {
                                class: "home-provenance__step",
                                style: "--home-provenance-index: {index}",
                                ProvenanceMarkGlyph { mark: step.mark }
                                h3 { class: "home-provenance__step-label", "{step.label}" }
                                p { class: "home-provenance__step-detail", "{step.detail}" }
                            }
                        }
                    }
                }
                if !provenance.ledger.is_empty() {
                    figure {
                        class: "home-provenance__ledger",
                        "aria-labelledby": "home-provenance-ledger-heading",
                        figcaption { class: "home-provenance__ledger-caption",
                            h3 {
                                id: "home-provenance-ledger-heading",
                                class: "home-provenance__ledger-heading",
                                "{provenance.ledger_heading}"
                            }
                            if !provenance.ledger_caption.is_empty() {
                                p { class: "home-provenance__ledger-note", "{provenance.ledger_caption}" }
                            }
                        }
                        ol { class: "home-provenance__rows",
                            for (index , row) in provenance.ledger.iter().enumerate() {
                                li {
                                    class: "home-provenance__row",
                                    style: "--home-provenance-index: {index}",
                                    span { class: "home-provenance__row-label", "{row.label}" }
                                    span { class: "home-provenance__bar", "aria-hidden": "true" }
                                    span { class: "home-provenance__status", "{row.status}" }
                                }
                            }
                        }
                    }
                }
            }
            if !provenance.pillars.is_empty() {
                ul { class: "home-provenance__pillars",
                    for (index , pillar) in provenance.pillars.iter().enumerate() {
                        li {
                            class: "home-provenance__pillar",
                            style: "--home-provenance-index: {index}",
                            h3 { class: "home-provenance__pillar-heading", "{pillar.heading}" }
                            p { class: "home-provenance__pillar-body", "{pillar.body}" }
                        }
                    }
                }
            }
            if !provenance.notes.is_empty() {
                div { class: "home-provenance__notes",
                    for paragraph in provenance.notes.iter() {
                        p { class: "home-provenance__note",
                            for run in paragraph.iter() {
                                if let Some(href) = run.href.as_ref() {
                                    a { href: "{href}", "{run.text}" }
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
    }
}

/// Draw one provenance mark, stroked in `currentColor` and hidden from
/// assistive technology.
#[component]
fn ProvenanceMarkGlyph(mark: ProvenanceMark) -> Element {
    let class = match mark {
        ProvenanceMark::Request => "home-provenance__mark home-provenance__mark--request",
        ProvenanceMark::Attorney => "home-provenance__mark home-provenance__mark--attorney",
        ProvenanceMark::Chain => "home-provenance__mark home-provenance__mark--chain",
    };
    rsx! {
        span { class: "{class}",
            svg {
                class: "home-provenance__glyph",
                xmlns: "http://www.w3.org/2000/svg",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                "stroke-width": "1.5",
                "stroke-linecap": "round",
                "stroke-linejoin": "round",
                "aria-hidden": "true",
                "focusable": "false",
                match mark {
                    ProvenanceMark::Request => rsx! {
                        path { d: "M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9Z" }
                        path { d: "M14 3v6h6" }
                        path { d: "M8 13h8" }
                        path { d: "M8 17h5" }
                    },
                    ProvenanceMark::Attorney => rsx! {
                        path { d: "M12 3 4 6v6c0 4.4 3.4 8.1 8 9 4.6-.9 8-4.6 8-9V6Z" }
                        path { d: "m9 12 2 2 4-4" }
                    },
                    ProvenanceMark::Chain => rsx! {
                        rect { x: "3", y: "9", width: "6", height: "6", rx: "1" }
                        rect { x: "15", y: "9", width: "6", height: "6", rx: "1" }
                        path { d: "M9 12h6" }
                        path { d: "M12 9v-4" }
                        path { d: "M12 15v4" }
                    },
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
                        logo_href: practice.logo_href.clone(),
                        font_family: practice.font_family.clone(),
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

    fn testimonial_html() -> String {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent::default(),
                    testimonials: vec![TestimonialCard {
                        quote: "The firm made a hard problem manageable.".into(),
                        attribution: "Synthetic Client".into(),
                        detail: Some("Founder".into()),
                        profile_image_url: None,
                        product_label: Some("Synthetic matter".into()),
                    }],
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent {
                        head_title: "Home".to_string(),
                        meta_description: "AI enablement for law firms.".to_string(),
                        heading: "AI enablement for law firms".to_string(),
                        lead: "Our clients are law firms.".to_string(),
                        contact_href: "mailto:contact@neonlaw.com".to_string(),
                        contact_label: "Contact us".to_string(),
                        practices_heading: "The rest of what we do".to_string(),
                        practices: vec![PracticeLink {
                            mark: PracticeMark::Scales,
                            heading: "Litigation".to_string(),
                            body: "We try cases on both sides of the v.".to_string(),
                            href: "/disputes".to_string(),
                            logo_href: String::new(),
                            font_family: String::new(),
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
                        provenance: None,
                        privacy: None,
                        company: None,
                        bare: None,
                        estate: None,
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

    #[test]
    fn renders_home_testimonials_when_the_store_returns_published_rows() {
        let out = testimonial_html();
        assert!(out.contains("testimonial-section"), "{out}");
        assert!(
            out.contains("The firm made a hard problem manageable."),
            "{out}"
        );
        assert!(out.contains("Synthetic Client"), "{out}");
    }

    #[test]
    fn blank_public_attribution_omits_the_byline() {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent::default(),
                    testimonials: vec![TestimonialCard {
                        quote: "The quote stands alone.".into(),
                        attribution: String::new(),
                        detail: None,
                        profile_image_url: None,
                        product_label: None,
                    }],
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = dioxus_ssr::render(&dom);
        assert!(out.contains("The quote stands alone."), "{out}");
        assert!(!out.contains("testimonial-card__name"), "{out}");
        assert!(!out.contains("testimonial-card__avatar"), "{out}");
    }

    /// The page opens on the question, and nothing sits above it. A skyline
    /// photograph used to lead the page; it said nothing a reader came for and
    /// pushed the question toward the fold. The one `<h1>` is the question,
    /// and it is the first thing inside the shell's `<main>`.
    #[test]
    fn the_page_opens_on_the_question_with_no_photograph_above_it() {
        let out = html();
        assert_eq!(out.matches("<h1").count(), 1, "one h1: {out}");
        assert!(
            out.contains(r#"<h1 class="home-statement__heading""#),
            "the h1 is the question: {out}"
        );
        for gone in ["<picture", "<img", "home-hero"] {
            assert!(!out.contains(gone), "{gone} is gone: {out}");
        }
        let main = out.find("<main").expect("the shell's main");
        let statement = out
            .find(r#"<section class="home-statement""#)
            .expect("the statement");
        let between = &out[main..statement];
        assert!(
            !between.contains("<section") && !between.contains("<div"),
            "nothing renders between the shell and the question: {between}"
        );
    }

    /// The page renders one offering, in prose, under one `<h2>`.
    ///
    /// The shape is the claim: a grid of cards tells a reader there are several
    /// things to choose between, and the firm does one thing. The engagements
    /// copy sits in a full-width band so its heading shares the statement's
    /// left edge; a card around that prose is what this guards against.
    #[test]
    fn the_service_is_one_prose_section_and_not_a_grid_of_cards() {
        let out = html();
        assert!(out.contains("What we do"), "the section heading: {out}");
        assert_eq!(
            out.matches(r#"class="home-service""#).count(),
            1,
            "exactly one engagements band: {out}"
        );
        assert!(
            !out.contains(r#"class="neon-card home-service""#),
            "the engagements section is a band, not a card: {out}"
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
            out.contains(r#"<a href="/navigator">"#),
            "the linking run is an anchor: {out}"
        );
        assert!(
            out.contains("Neon Law Navigator</a>"),
            "the anchor carries the run's text: {out}"
        );
    }

    /// Every link inside a prose paragraph on this page carries no class.
    ///
    /// `theme.css` cues inline prose links through
    /// `.nav-theme :is(p, li) > a:not([class])`, so a class on one of these
    /// anchors — even a decorative one — opts it out of the WCAG 1.4.1
    /// underline and leaves colour as the only signal that the run leaves the
    /// page. That is axe's `link-in-text-block`, and it is how this page failed
    /// the public accessibility gate on the 26.9.10 release in the dark scheme,
    /// where the shared link stop is 1.50:1 against body text.
    ///
    /// Scoped to the paragraphs rather than the whole document: the page's
    /// *controls* are anchors too — the statement's filled call to action, each
    /// practice card — and those carry the class that styles them and bring
    /// their own affordance. A run inside a sentence has neither.
    ///
    /// The gate that caught it needs a live KIND cluster and a browser; this
    /// reads the rendered markup, so the regression is caught in the ordinary
    /// workspace run.
    #[test]
    fn a_link_inside_a_prose_paragraph_carries_no_class() {
        let out = html();
        let mut linked = 0;
        for class in ["home-service__paragraph", "home-provenance__note"] {
            for chunk in out.split(&format!(r#"class="{class}""#)).skip(1) {
                let paragraph = chunk.split_once("</p>").map_or(chunk, |(head, _)| head);
                assert!(
                    !paragraph.contains("<a class="),
                    "a link inside `.{class}` must carry no class, or it opts \
                     out of the WCAG 1.4.1 underline rule: {paragraph}"
                );
                linked += paragraph.matches("<a href=").count();
            }
        }
        assert!(
            linked > 0,
            "the fixture must still link from inside a sentence, or the \
             assertion above passes on prose that has no link to check: {out}"
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

    #[test]
    fn a_bare_emphasised_run_renders_as_strong_markup() {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent {
                        head_title: "Holding page".to_string(),
                        meta_description: "A holding page.".to_string(),
                        bare: Some(BareStatement {
                            heading: "Holding page".to_string(),
                            paragraph: "A statement.".to_string(),
                            sign_in: vec![CopyRun {
                                text: "Existing client".to_string(),
                                emphasis: true,
                                href: None,
                            }],
                        }),
                        ..HomeContent::default()
                    },
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = dioxus_ssr::render(&dom);

        assert!(
            out.contains("<strong>Existing client</strong>"),
            "a bare emphasised run is strong: {out}"
        );
    }

    /// A bare holding page carries the one shared footer under its statement
    /// — with the firm's identity and legal strip — and no header, so a
    /// reader on the holding host finds the firm's address and its other
    /// sites the same way they would on any other page.
    #[test]
    fn a_bare_page_carries_the_shared_footer_and_no_header() {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome {
                        legal_entity: "Shook Law PLLC".to_string(),
                        disclaimer: "Attorney advertisement.".to_string(),
                        copyright_year: 2026,
                        ..PublicChrome::default()
                    },
                    content: HomeContent {
                        head_title: "Holding page".to_string(),
                        meta_description: "A holding page.".to_string(),
                        bare: Some(BareStatement {
                            heading: "Holding page".to_string(),
                            paragraph: "A statement.".to_string(),
                            sign_in: Vec::new(),
                        }),
                        ..HomeContent::default()
                    },
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = dioxus_ssr::render(&dom);

        let statement = out.find(r#"class="holding-page""#).expect("the statement");
        let footer = out
            .find(r#"role="contentinfo""#)
            .expect("the shared footer");
        assert!(statement < footer, "statement, then footer: {out}");
        assert!(out.contains("© 2026 Shook Law PLLC"), "{out}");
        assert!(!out.contains(r#"class="site-header""#), "no header: {out}");
        assert!(
            !out.contains(crate::components::PUBLIC_SHELL_MARKER),
            "not a public shell page (no chat widget): {out}"
        );
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
            out.contains(r#"<a class="neon-card home-practice" href="/disputes""#),
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

    /// The provenance section: an ordered flow, a labelled ledger figure, and
    /// the notes under both. The steps are an `<ol>` because their order is
    /// the claim; the bars are decoration and stay out of the accessibility
    /// tree; the glyphs are stroked in `currentColor` and hidden.
    #[test]
    fn the_provenance_section_renders_an_ordered_flow_a_ledger_figure_and_notes() {
        let out = provenance_html();
        assert!(
            out.contains(r#"class="neon-card home-provenance""#),
            "the section is one card: {out}"
        );
        assert!(
            out.contains(r#"aria-labelledby="home-provenance-heading""#)
                && out.contains(r#"<h2 id="home-provenance-heading""#),
            "the section is labelled by its own heading: {out}"
        );
        assert!(
            out.contains(r#"<ol class="home-provenance__steps""#),
            "the flow is ordered: {out}"
        );
        assert_eq!(
            out.matches(r#"class="home-provenance__step""#).count(),
            3,
            "three steps: {out}"
        );
        assert!(
            out.contains(r#"style="--home-provenance-index: 2""#),
            "each step carries its index for the stagger: {out}"
        );
        for glyph in ["mark--request", "mark--attorney", "mark--chain"] {
            assert!(out.contains(glyph), "{glyph} renders: {out}");
        }
        assert!(
            out.contains(r#"stroke="currentColor""#) && out.contains(r#"focusable="false""#),
            "the glyphs take the text colour and hide from assistive technology: {out}"
        );
        assert!(
            out.contains(r#"<figure class="home-provenance__ledger" aria-labelledby="home-provenance-ledger-heading""#)
                && out.contains(r#"<h3 id="home-provenance-ledger-heading""#),
            "the ledger is a figure named by its heading: {out}"
        );
        assert!(
            out.contains("An illustration, not a count."),
            "the caption says what the ledger is: {out}"
        );
        assert!(
            out.contains(r#"<span class="home-provenance__bar" aria-hidden="true">"#),
            "the bars are decoration: {out}"
        );
        assert!(out.contains(">Attested<"), "the status is real text: {out}");
        assert!(
            out.contains(r#"<span class="home-provenance__heading-accent">"#)
                && out.contains("then recorded on Solana."),
            "the accent line sits inside the one h2: {out}"
        );
        assert!(
            out.contains(r#"<ul class="home-provenance__pillars""#)
                && out.contains(r#"<h3 class="home-provenance__pillar-heading">"#),
            "the pillars are a list of tiles: {out}"
        );
        assert!(
            out.contains(r#"<p class="home-provenance__note">"#)
                && out.contains("<strong>lawyer-attested nodes</strong>"),
            "the notes render with their emphasis: {out}"
        );
        let service = out.find("home-service").expect("the service section");
        let provenance = out.find("home-provenance").expect("the provenance section");
        let practices = out.find("home-practices").expect("the practice boxes");
        assert!(
            service < provenance && provenance < practices,
            "prose, then the record, then the boxes: {out}"
        );
        assert!(
            !out.contains("firm-glow"),
            "the section's wash is its own, clipped inside the card: {out}"
        );
    }

    #[test]
    fn the_provenance_section_stays_out_of_the_markup_when_there_is_none() {
        for out in [html(), statement_only_html()] {
            assert!(!out.contains("home-provenance"), "no empty section: {out}");
        }
    }

    /// The `html()` fixture plus a provenance section.
    fn provenance_html() -> String {
        fn app() -> Element {
            rsx! {
                HomePage {
                    chrome: PublicChrome::default(),
                    content: HomeContent {
                        heading: "Ask companies to delete your data.".to_string(),
                        contact_href: "mailto:contact@neonlaw.com".to_string(),
                        contact_label: "Contact us".to_string(),
                        service: Some(ServiceSection {
                            heading: "What this practice does".to_string(),
                            body: vec![vec![CopyRun {
                                text: "We help.".to_string(),
                                emphasis: false,
                                href: None,
                            }]],
                        }),
                        company: None,
                        provenance: Some(ProvenanceSection {
                            overline: "How the record works".to_string(),
                            heading: "Verified by a lawyer,".to_string(),
                            heading_accent: "then recorded on Solana.".to_string(),
                            lead: "We verify the request first.".to_string(),
                            steps: vec![
                                ProvenanceStep {
                                    mark: ProvenanceMark::Request,
                                    label: "You send the request".to_string(),
                                    detail: "Name the company.".to_string(),
                                },
                                ProvenanceStep {
                                    mark: ProvenanceMark::Attorney,
                                    label: "A licensed attorney verifies it".to_string(),
                                    detail: "Reviewed before it goes out.".to_string(),
                                },
                                ProvenanceStep {
                                    mark: ProvenanceMark::Chain,
                                    label: "We upload the record to Solana".to_string(),
                                    detail: "A hash, never the request.".to_string(),
                                },
                            ],
                            ledger_heading: "Where your data has been removed from".to_string(),
                            ledger_caption: "An illustration, not a count.".to_string(),
                            ledger: vec![ProvenanceLedgerRow {
                                label: "A data broker".to_string(),
                                status: "Attested".to_string(),
                            }],
                            pillars: vec![ProvenancePillar {
                                heading: "Privacy".to_string(),
                                body: "Only a hash goes on the chain.".to_string(),
                            }],
                            notes: vec![vec![
                                CopyRun {
                                    text: "Our ".to_string(),
                                    emphasis: false,
                                    href: None,
                                },
                                CopyRun {
                                    text: "lawyer-attested nodes".to_string(),
                                    emphasis: true,
                                    href: None,
                                },
                                CopyRun {
                                    text: " are long-term provenance.".to_string(),
                                    emphasis: false,
                                    href: None,
                                },
                            ]],
                        }),
                        practices_heading: "The work".to_string(),
                        practices: vec![PracticeLink {
                            mark: PracticeMark::Gavel,
                            heading: "Data-deletion requests".to_string(),
                            body: "One scoped request.".to_string(),
                            href: "/services".to_string(),
                            logo_href: String::new(),
                            font_family: String::new(),
                        }],
                        ..HomeContent::default()
                    },
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    /// The page with nothing but its defaults: no service section, no boxes.
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

    #[cfg(feature = "server")]
    #[test]
    fn home_card_uses_chosen_attribution_only() {
        let card = home_testimonial_card(store::testimonials::PublishedTestimonial {
            id: uuid::Uuid::nil(),
            quote: "Chosen quote.".into(),
            attribution_label: Some("Chosen Label".into()),
        });
        assert_eq!(card.quote, "Chosen quote.");
        assert_eq!(card.attribution, "Chosen Label");
        assert!(card.detail.is_none());
        assert!(card.profile_image_url.is_none());
        assert!(card.product_label.is_none());
    }

    #[cfg(feature = "server")]
    #[test]
    fn home_card_blank_attribution_carries_no_identity() {
        let card = home_testimonial_card(store::testimonials::PublishedTestimonial {
            id: uuid::Uuid::nil(),
            quote: "Standalone quote.".into(),
            attribution_label: None,
        });
        assert_eq!(card.quote, "Standalone quote.");
        assert!(card.attribution.is_empty());
        assert!(card.detail.is_none());
        assert!(card.profile_image_url.is_none());
    }
}
