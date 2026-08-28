//! The band vocabulary every marketing page is built from, and the one
//! renderer that draws it.
//!
//! **Why one module for several pages.** The firm's marketing pages are the
//! same handful of shapes in a different order: a hero, prose bands, card
//! grids, a numbered walk, and a closing call to action. Modelling those shapes
//! once ([`Band`]) and letting each page order them is what keeps a new page a
//! data change rather than a new component tree.
//!
//! Copy lives in the Rust that renders it, per the workspace's English-only
//! rule: there is no catalog and no key lookup. The portal router resolves each
//! page's content at router-build time and injects it, so no page here resolves
//! per-request data.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    PlatformMark, PlatformMarkGlyph, PracticeMark, PracticeMarkGlyph, PublicShell, SiteHeader,
    SiteNavLink, SocialMeta,
};
use crate::litigation_page::HeroWord;
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The marketing-page stylesheet, hoisted alongside `theme.css` and the shared
/// token layer.
pub const MARKETING_STYLESHEET_HREF: &str = "/public/css/marketing-page.css";

/// One run of prose. `emphasis` renders it as `<strong>`.
///
/// Marketing copy leans on a bolded clause per paragraph — "the lawyer is
/// responsible, and the lawyer signs" — so a paragraph is a sequence of runs
/// rather than a string. Modelling it as data keeps the copy wasm-safe and
/// keeps markup out of the content.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub text: String,
    pub emphasis: bool,
    /// When set, the run renders as an `<a>` to this href rather than as text.
    pub href: Option<String>,
}

impl Run {
    /// Plain prose.
    #[must_use]
    pub fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            emphasis: false,
            href: None,
        }
    }

    /// Prose the page bolds.
    #[must_use]
    pub fn strong(text: &str) -> Self {
        Self {
            text: text.to_string(),
            emphasis: true,
            href: None,
        }
    }

    /// An inline link — prose that navigates to `href`.
    #[must_use]
    pub fn link(text: &str, href: &str) -> Self {
        Self {
            text: text.to_string(),
            emphasis: false,
            href: Some(href.to_string()),
        }
    }
}

/// A paragraph: the runs that compose it.
pub type Paragraph = Vec<Run>;

/// One card in a card band — a program on the home page, or a commitment on an
/// audience page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Card {
    pub title: String,
    /// Short labels rendered as a chip row. Empty renders no row.
    pub chips: Vec<String>,
    pub body: Vec<Paragraph>,
    /// Optional deep link to the page that expands this card.
    pub href: Option<String>,
    pub href_label: Option<String>,
}

/// One entry in a numbered walk.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Step {
    pub title: String,
    pub body: Vec<Paragraph>,
}

/// One labeled place in a Project's connected-work diagram.
///
/// The public diagram names the work surface, not a particular account, URL,
/// or matter. That keeps the visual useful without publishing a client or a
/// provider credential.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct ProjectNetworkNode {
    pub label: String,
    pub detail: String,
}

/// One platform's download box.
///
/// Resolved server-side by the page that mounts the band — the href carries a
/// version, and only the server knows which release it is running — so this
/// struct holds finished strings rather than the coordinates to build them.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct Download {
    /// The platform's own word, for the box's `data-download-platform` hook.
    pub platform: String,
    /// The box's heading: `Linux`, `macOS`, `Windows`.
    pub label: String,
    /// Which machine the archive runs on, under the heading.
    pub detail: String,
    /// The archive's filename, shown so a reader can match what lands in their
    /// downloads folder to the box they clicked.
    pub filename: String,
    /// The absolute URL of the archive on the public GitHub Release.
    pub href: String,
    /// The line mark the box opens on.
    pub mark: PlatformMark,
}

/// The package-manager route, published beside the boxes.
///
/// Not a fourth box: it installs on one platform, it upgrades in place, and a
/// box that ran a shell command rather than downloading a file would lie about
/// what clicking it does.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PackageInstall {
    pub heading: String,
    pub body: Vec<Paragraph>,
    /// The commands a reader copies. Homebrew is one line: `brew install`
    /// of the tap-qualified formula also upgrades in place.
    pub commands: Vec<String>,
}

/// One horizontal band of a marketing page.
///
/// A page is an ordered list of these. Adding a band shape here is the only
/// way a page grows a new kind of section, which is what stops four pages from
/// drifting into four private layouts.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum Band {
    /// A centred statement: a large lead line over supporting prose. The
    /// mission band on the home page, and the opening argument on each
    /// audience page.
    Statement {
        /// Screen-reader heading for the band. Rendered visually hidden when
        /// `lead` is doing the visible work.
        heading: String,
        lead: String,
        body: Vec<Paragraph>,
    },
    /// A titled grid of cards.
    Cards {
        anchor: String,
        overline: String,
        heading: String,
        description: Option<String>,
        items: Vec<Card>,
    },
    /// A titled, numbered walk.
    Steps {
        anchor: String,
        overline: String,
        heading: String,
        description: Option<String>,
        items: Vec<Step>,
    },
    /// A Project-centered map of the work surfaces that meet around Navigator.
    ProjectNetwork {
        anchor: String,
        overline: String,
        heading: String,
        description: Option<String>,
        left: Vec<ProjectNetworkNode>,
        right: Vec<ProjectNetworkNode>,
        mcp_tools: Vec<String>,
        agentic_coding_tools: Vec<String>,
        saas_tools: Vec<String>,
    },
    /// The three CLI download boxes, and the package-manager route beside
    /// them.
    ///
    /// Its boxes wear `home.css`'s `.home-practice` treatment — the same object
    /// the firm's home page ends on, hover wash and all — so a reader who has
    /// been to the front page meets something they already know how to use.
    /// [`MarketingShell`] hoists that sheet for any page carrying this band.
    Downloads {
        anchor: String,
        overline: String,
        heading: String,
        description: Option<String>,
        /// The release every href in `items` names. Printed once, above the
        /// boxes, rather than three times inside them.
        version: String,
        /// Where a reader goes for an older release or the notes.
        archive_href: String,
        archive_label: String,
        items: Vec<Download>,
        package: Option<PackageInstall>,
    },
    /// The closing call to action. The firm publishes one route in on these
    /// pages — its inbox — so this carries an address rather than a form.
    Cta {
        heading: String,
        body: Option<String>,
        email: String,
        /// Optional subject line prefilled in the recipient's email client.
        email_subject: Option<String>,
    },
}

impl Band {
    /// Whether this band renders the CLI download boxes.
    ///
    /// A page asks so it can hoist `home.css`, whose `.home-practice` rules the
    /// boxes are written against. A Dioxus page loads only the sheets it names,
    /// so a band whose stylesheet nobody hoists renders as unstyled anchors —
    /// visible, clickable, and wrong.
    #[must_use]
    pub const fn is_downloads(&self) -> bool {
        matches!(self, Self::Downloads { .. })
    }
}

/// Everything one marketing page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct PageContent {
    pub head_title: String,
    pub meta_description: String,
    /// The page's `<h1>`.
    pub title: String,
    /// The first-party line mark above the title. Practice and product pages
    /// use the same SVGs as the four-card slide; audience pages omit it.
    pub hero_mark: Option<PracticeMark>,
    /// The line under the title.
    pub tagline: String,
    /// The `<h1>` as lines of words: the outer `Vec` is the line breaks the
    /// statement sets for itself, the inner one the words on that line, so the
    /// practice skin can set the opening ones in the firm's own colour the way
    /// `/litigation` does.
    ///
    /// Lines are data rather than a `<br>` in a string, because where a
    /// statement breaks is a typographic decision the copy makes — leaving it to
    /// the viewport gives a different reading at every width. Empty renders
    /// `tagline` as one plain run instead, which is what the marketing skin and
    /// any page that has not been given an accent split still do.
    pub hero_lines: Vec<Vec<HeroWord>>,
    /// The paragraph under the hero statement. Empty renders none.
    pub hero_lead: String,
    /// The one call to action in the hero. `None` renders none — the closing
    /// [`Band::Cta`] is still where a page's address lives.
    pub hero_cta: Option<HeroCta>,
    pub bands: Vec<Band>,
    /// Which visual language the page wears.
    pub skin: PageSkin,
}

/// The hero's one call to action: where it goes and what it says.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct HeroCta {
    pub href: String,
    pub label: String,
}

/// Which visual language a marketing page wears.
///
/// One renderer serves every marketing page, and they want different
/// typography: a campaign page and a practice page read differently. Rather
/// than fork the renderer, a page names its skin and the stylesheet keys off a
/// modifier class.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
pub enum PageSkin {
    /// The campaign look the `/navigator` platform page wears.
    #[default]
    Marketing,
    /// The firm's practice look — the serif statement, the glow, and the carded
    /// body the `/litigation` and `/fractional-gc` pages wear.
    Practice,
}

impl PageSkin {
    /// The modifier class this skin puts on the page root, if any.
    #[must_use]
    pub const fn modifier(self) -> &'static str {
        match self {
            Self::Marketing => "",
            Self::Practice => " fm-page--practice",
        }
    }
}

/// The [`PageContent`] the portal router injects for one marketing page.
#[derive(Clone, Default)]
pub struct InjectedMarketingPage(pub PageContent);

/// A marketing page's resolved view.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct MarketingPageView {
    pub chrome: PublicChrome,
    pub content: PageContent,
}

/// Resolve the public chrome and one marketing page's static copy.
#[server]
pub async fn marketing_page_view() -> Result<MarketingPageView, ServerFnError> {
    let content = consume_context::<InjectedMarketingPage>().0;
    Ok(MarketingPageView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// A marketing page's route entry.
#[component]
pub fn MarketingPageEntry() -> Element {
    let resource = use_server_future(marketing_page_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        MarketingPage { chrome: view.chrome, content: view.content }
    }
}

/// The chrome both page components wrap their bands in.
///
/// Taken as rendered `Element`s for the same reason [`PublicShell`] does: the
/// brand is resolved server-side, and the shell never branches on it.
#[component]
fn MarketingShell(
    chrome: PublicChrome,
    title: String,
    description: String,
    /// Hoist the firm's component layer (`brand-firm.css`) alongside this
    /// page's own rules. The practice skin is written against that layer's
    /// vocabulary — the card, the glow, the eyebrow — so a page wearing it
    /// needs the sheet that defines them. Marketing-skin pages do not.
    firm_components: bool,
    /// Hoist the home page's sheet (`home.css`). A page carrying a downloads
    /// band reuses that sheet's `.home-practice` box wholesale, and this is the
    /// only place in the render tree that can put it in the document head:
    /// `document::Stylesheet` is collected by the head collector rather than
    /// emitted as body markup, so it has to be named by the component that owns
    /// the page and not by the band deep inside it.
    home_components: bool,
    children: Element,
) -> Element {
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
        document::Title { "{title}" }
        document::Meta { name: "description", content: "{description}" }
        SocialMeta {
            title: title.clone(),
            description: description.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        // The palette comes from the shared token layer `PublicShell` hoists.
        // A practice-skin page also needs the firm's component layer, whose
        // card, glow, and eyebrow the skin styles against; it is hoisted first
        // so this page's own rules order after it.
        if firm_components {
            document::Stylesheet { href: crate::brand_style::BRAND_STYLESHEET_HREF }
        }
        document::Stylesheet { href: MARKETING_STYLESHEET_HREF }
        // After the marketing layer, because the download boxes take their
        // whole treatment from `home.css` and this page's own rules only
        // position them.
        if home_components {
            document::Stylesheet { href: crate::home::HOME_STYLESHEET_HREF }
        }
        PublicShell { header, footer, {children} }
    }
}

/// One marketing page. Prop-driven, so it server-renders and unit-tests
/// without a server future.
#[component]
pub fn MarketingPage(chrome: PublicChrome, content: PageContent) -> Element {
    rsx! {
        MarketingShell {
            chrome: chrome.clone(),
            title: content.head_title.clone(),
            description: content.meta_description.clone(),
            firm_components: content.skin == PageSkin::Practice || content.hero_mark.is_some(),
            home_components: content.bands.iter().any(Band::is_downloads),
            div { class: "fm-page{content.skin.modifier()}",
                section { class: "fm-hero fm-hero--page",
                    // The practice skin leads with the eyebrow and sets the
                    // tagline as the `<h1>`, the way `/litigation` does: on a
                    // practice page the statement is the headline and the
                    // practice name is the label above it. The marketing skin
                    // keeps the title as the headline.
                    if content.skin == PageSkin::Practice {
                        div { class: "firm-glow fm-hero__glow", "aria-hidden": "true" }
                        div { class: "fm-hero__inner",
                            if let Some(mark) = content.hero_mark {
                                PracticeMarkGlyph {
                                    mark,
                                    class: "fm-hero__mark".to_string(),
                                }
                            }
                            p { class: "firm-eyebrow", "{content.title}" }
                            h1 { class: "fm-hero__title",
                                if content.hero_lines.is_empty() {
                                    "{content.tagline}"
                                } else {
                                    for line in content.hero_lines.iter() {
                                        span { class: "fm-hero__line",
                                            for word in line.iter() {
                                                // No trailing space: the word gap
                                                // is a margin in the stylesheet.
                                                // Each word is its own
                                                // inline-block, which collapses
                                                // the whitespace at its own end,
                                                // so a space here would render as
                                                // nothing and run the words
                                                // together.
                                                span {
                                                    class: if word.accent { "fm-word fm-word--accent" } else { "fm-word" },
                                                    "{word.text}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if !content.hero_lead.is_empty() {
                                p { class: "fm-hero__lead", "{content.hero_lead}" }
                            }
                            if let Some(cta) = content.hero_cta.as_ref() {
                                a {
                                    class: "nav-btn nav-btn--primary fm-hero__cta",
                                    href: "{cta.href}",
                                    "{cta.label}"
                                }
                            }
                        }
                    } else {
                        div { class: "fm-hero__inner",
                            if let Some(mark) = content.hero_mark {
                                PracticeMarkGlyph {
                                    mark,
                                    class: "fm-hero__mark".to_string(),
                                }
                            }
                            h1 { class: "fm-hero__title", "{content.title}" }
                            p { class: "fm-hero__tagline", "{content.tagline}" }
                        }
                    }
                }
                Bands { items: content.bands.clone() }
            }
        }
    }
}

/// Render a page's bands in order.
#[component]
fn Bands(items: Vec<Band>) -> Element {
    rsx! {
        for band in items.iter() {
            match band {
                Band::Statement { heading, lead, body } => rsx! {
                    section { class: "fm-band fm-band--statement",
                        div { class: "fm-band__inner",
                            h2 { class: "fm-visually-hidden", "{heading}" }
                            if !lead.is_empty() {
                                p { class: "fm-statement__lead", "{lead}" }
                            }
                            div { class: "fm-statement__body",
                                for paragraph in body.iter() {
                                    Prose { runs: paragraph.clone() }
                                }
                            }
                        }
                    }
                },
                Band::Cards { anchor, overline, heading, description, items } => rsx! {
                    section { class: "fm-band fm-band--cards", id: "{anchor}",
                        div { class: "fm-band__inner",
                            BandHeading {
                                overline: overline.clone(),
                                heading: heading.clone(),
                                description: description.clone(),
                            }
                            ul { class: "fm-cards",
                                for card in items.iter() {
                                    li { class: "fm-card",
                                        h3 { class: "fm-card__title", "{card.title}" }
                                        if !card.chips.is_empty() {
                                            ul { class: "fm-chips",
                                                for chip in card.chips.iter() {
                                                    li { class: "fm-chip", "{chip}" }
                                                }
                                            }
                                        }
                                        div { class: "fm-card__body",
                                            for paragraph in card.body.iter() {
                                                Prose { runs: paragraph.clone() }
                                            }
                                        }
                                        if let (Some(href), Some(label)) =
                                            (card.href.as_ref(), card.href_label.as_ref())
                                        {
                                            a { class: "fm-card__link", href: "{href}", "{label}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Band::Steps { anchor, overline, heading, description, items } => rsx! {
                    section { class: "fm-band fm-band--steps", id: "{anchor}",
                        div { class: "fm-band__inner",
                            BandHeading {
                                overline: overline.clone(),
                                heading: heading.clone(),
                                description: description.clone(),
                            }
                            ol { class: "fm-steps",
                                for step in items.iter() {
                                    li { class: "fm-step",
                                        h3 { class: "fm-step__title", "{step.title}" }
                                        div { class: "fm-step__body",
                                            for paragraph in step.body.iter() {
                                                Prose { runs: paragraph.clone() }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Band::ProjectNetwork {
                    anchor,
                    overline,
                    heading,
                    description,
                    left,
                    right,
                    mcp_tools,
                    agentic_coding_tools,
                    saas_tools,
                } => rsx! {
                    section { class: "fm-band fm-band--project-network", id: "{anchor}",
                        div { class: "fm-band__inner",
                            BandHeading {
                                overline: overline.clone(),
                                heading: heading.clone(),
                                description: description.clone(),
                            }
                            figure { class: "fm-project-network",
                                div { class: "fm-project-network__map",
                                    ul { class: "fm-project-network__lane fm-project-network__lane--left",
                                        "aria-label": "Project resources to the left of Navigator",
                                        for node in left.iter() {
                                            li { class: "fm-project-network__node fm-project-network__node--left",
                                                h3 { "{node.label}" }
                                                p { "{node.detail}" }
                                            }
                                        }
                                    }
                                    div { class: "fm-project-network__core",
                                        img {
                                            class: "fm-project-network__wheel",
                                            src: "/public/navigator-wheel.svg",
                                            alt: "Neon Law Navigator wheel",
                                        }
                                        p { class: "fm-project-network__eyebrow", "The Project center" }
                                        h3 { "Navigator" }
                                        p { "Web API MCP CLI" }
                                    }
                                    ul { class: "fm-project-network__lane fm-project-network__lane--right",
                                        "aria-label": "Project resources to the right of Navigator",
                                        for node in right.iter() {
                                            li { class: "fm-project-network__node fm-project-network__node--right",
                                                h3 { "{node.label}" }
                                                p { "{node.detail}" }
                                            }
                                        }
                                    }
                                }
                                div { class: "fm-project-network__tool-panels",
                                    div { class: "fm-project-network__external",
                                        p { class: "fm-project-network__external-label", "MCPs" }
                                        ul { class: "fm-project-network__tools", "aria-label": "MCPs",
                                            for tool in mcp_tools.iter() {
                                                li { class: "fm-project-network__tool", "{tool}" }
                                            }
                                        }
                                    }
                                    div { class: "fm-project-network__external",
                                        p { class: "fm-project-network__external-label", "Agentic Legal Coding" }
                                        ul { class: "fm-project-network__tools", "aria-label": "Agentic legal coding tools",
                                            for tool in agentic_coding_tools.iter() {
                                                li { class: "fm-project-network__tool", "{tool}" }
                                            }
                                        }
                                    }
                                    div { class: "fm-project-network__external fm-project-network__external--saas",
                                        p { class: "fm-project-network__external-label", "SaaS" }
                                        ul { class: "fm-project-network__tools", "aria-label": "SaaS tools",
                                            for tool in saas_tools.iter() {
                                                li { class: "fm-project-network__tool", "{tool}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Band::Downloads {
                    anchor,
                    overline,
                    heading,
                    description,
                    version,
                    archive_href,
                    archive_label,
                    items,
                    package,
                } => rsx! {
                    section { class: "fm-band fm-band--downloads", id: "{anchor}",
                        div { class: "fm-band__inner",
                            BandHeading {
                                overline: overline.clone(),
                                heading: heading.clone(),
                                description: description.clone(),
                            }
                            p { class: "fm-downloads__version",
                                "Version "
                                // The version is the one string on this band a
                                // reader might retype into an issue or a shell,
                                // so it is set as code rather than as prose.
                                code { class: "fm-downloads__tag", "{version}" }
                                " · "
                                a { href: "{archive_href}", "{archive_label}" }
                            }
                            // `home-practices__grid` is the home page's own
                            // grid, reused rather than reproduced: it carries
                            // the three explicit columns AND the clipping
                            // context the hover wash needs, and its child
                            // selector is what arms `.home-practice`. A private
                            // copy under a download-flavoured name would be the
                            // same rules with a second place to forget.
                            div { class: "home-practices__grid fm-downloads__grid",
                                for item in items.iter() {
                                    a {
                                        key: "{item.platform}",
                                        class: "neon-card home-practice fm-download",
                                        href: "{item.href}",
                                        "data-download-platform": "{item.platform}",
                                        // The accessible name, written out.
                                        //
                                        // The home page labels its boxes by
                                        // their heading alone, because each one
                                        // carries a full sentence a reader does
                                        // not need read out to know where the
                                        // link goes. A download is the opposite
                                        // case: "Linux" does not say what
                                        // arrives, and the visible detail is
                                        // punctuation-separated fragments that
                                        // announce badly. So the name is a
                                        // sentence, and it names the release —
                                        // which the boxes themselves never
                                        // repeat, and which is the one fact a
                                        // reader wants confirmed before they
                                        // download anything.
                                        "aria-label": "Download Neon Law Navigator {version} for {item.label} — {item.detail}",
                                        // The archive is a file, not a page.
                                        // Without this a browser that can
                                        // preview the type navigates instead of
                                        // saving, and the reader loses the page.
                                        download: "{item.filename}",
                                        PlatformMarkGlyph {
                                            mark: item.mark,
                                            class: "home-practice__mark".to_string(),
                                        }
                                        h3 { class: "home-practice__heading", "{item.label}" }
                                        p { class: "home-practice__body", "{item.detail}" }
                                        // The filename, so what lands in the
                                        // downloads folder matches the box that
                                        // was clicked.
                                        p { class: "fm-download__file", "{item.filename}" }
                                    }
                                }
                            }
                            if let Some(package) = package.as_ref() {
                                div { class: "fm-package",
                                    h3 { class: "fm-package__heading", "{package.heading}" }
                                    div { class: "fm-package__body",
                                        for paragraph in package.body.iter() {
                                            Prose { runs: paragraph.clone() }
                                        }
                                    }
                                    // One `<pre>` per command. A reader
                                    // triple-clicks a line to select it, and two
                                    // commands in one block select together.
                                    for command in package.commands.iter() {
                                        pre { class: "fm-package__command",
                                            code { "{command}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                Band::Cta { heading, body, email, email_subject } => rsx! {
                    section { class: "fm-band fm-band--cta",
                        div { class: "fm-band__inner",
                            h2 { class: "fm-cta__heading", "{heading}" }
                            if let Some(body) = body.as_ref() {
                                p { class: "fm-cta__body", "{body}" }
                            }
                            MailAction { email: email.clone(), subject: email_subject.clone() }
                        }
                    }
                },
            }
        }
    }
}

/// A band's overline, heading, and optional standfirst.
#[component]
fn BandHeading(overline: String, heading: String, description: Option<String>) -> Element {
    rsx! {
        div { class: "fm-band__heading",
            p { class: "fm-overline", "{overline}" }
            h2 { class: "fm-band__title", "{heading}" }
            if let Some(description) = description.as_ref() {
                p { class: "fm-band__description", "{description}" }
            }
        }
    }
}

/// One paragraph of runs.
///
/// A linked run renders as a **classless** `<a>`, and that is load-bearing
/// rather than incidental. `theme.css` gives every inline prose link its
/// non-colour cue through `.nav-theme :is(p, li) > a:not([class])` — keyed on
/// the absence of a class precisely so no new prose page has to be remembered
/// into an allow-list. A class here, even a decorative one, opts these links
/// out of that rule and leaves them distinguishable by colour alone, which is
/// the `link-in-text-block` violation axe reports.
///
/// The classes that *do* belong on an anchor in this stylesheet — `fm-card__link`,
/// `fm-action__link` — are controls: a card's call to action and a filled
/// button. Those are styled, and they carry their own affordance. A run inside a
/// sentence is prose.
#[component]
fn Prose(runs: Paragraph) -> Element {
    rsx! {
        p {
            for run in runs.iter() {
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

/// A marketing page's one call to action: an inbox.
///
/// Rendered as a `mailto:` anchor rather than a form. Intake on these pages is
/// by conversation, and a contact form implies a queue behind it.
#[component]
fn MailAction(email: String, subject: Option<String>) -> Element {
    let href = subject.map_or_else(
        || format!("mailto:{email}"),
        |subject| {
            let subject =
                percent_encoding::utf8_percent_encode(&subject, percent_encoding::NON_ALPHANUMERIC);
            format!("mailto:{email}?subject={subject}")
        },
    );
    rsx! {
        p { class: "fm-action",
            a { class: "fm-action__link", href: "{href}", "{email}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> PublicChrome {
        PublicChrome {
            brand_name: "Neon Law".to_string(),
            home_href: "/".to_string(),
            logo_href: "/public/logo.svg".to_string(),
            social_image: "https://example.test/og.png".to_string(),
            ..PublicChrome::default()
        }
    }

    fn render(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    /// A downloads band with all three boxes and the package route.
    fn sample_downloads() -> Band {
        Band::Downloads {
            anchor: "download".to_string(),
            overline: "Download".to_string(),
            heading: "Run Navigator yourself".to_string(),
            description: Some("Pick your platform.".to_string()),
            version: "26.8.20".to_string(),
            archive_href: "https://github.com/neon-law-source-code/navigator/releases".to_string(),
            archive_label: "every release".to_string(),
            items: crate::cli_release::PLATFORMS
                .iter()
                .map(|platform| Download {
                    platform: platform.slug.to_string(),
                    label: platform.label.to_string(),
                    detail: platform.detail.to_string(),
                    filename: crate::cli_release::asset_filename("26.8.20", platform),
                    href: crate::cli_release::asset_href("26.8.20", platform),
                    mark: platform.mark,
                })
                .collect(),
            package: Some(PackageInstall {
                heading: "Install with Homebrew".to_string(),
                body: vec![vec![Run::plain("On a Mac this is the route we recommend.")]],
                commands: vec![crate::cli_release::HOMEBREW_INSTALL_COMMAND.to_string()],
            }),
        }
    }

    fn downloads_html() -> String {
        fn app() -> Element {
            rsx! {
                Bands { items: vec![sample_downloads()] }
            }
        }
        render(app)
    }

    /// Each box is one anchor at the version's real GitHub Release asset, in
    /// the order the page lays them out: Linux, macOS, Windows.
    ///
    /// The whole box being the anchor is what makes the hover wash mean
    /// something — `home.css` arms `.home-practice` on `a:hover`, so a box with
    /// a link *inside* it would light up nowhere.
    #[test]
    fn each_box_is_one_anchor_at_the_release_asset_for_its_platform() {
        let out = downloads_html();
        let positions: Vec<usize> = ["linux", "macos", "windows"]
            .iter()
            .map(|slug| {
                let href = format!(
                    "https://github.com/neon-law-source-code/navigator/releases/download/26.8.20/\
                     navigator-26.8.20-{slug}."
                )
                .replace(char::is_whitespace, "");
                out.find(&href)
                    .unwrap_or_else(|| panic!("the {slug} box links its archive: {out}"))
            })
            .collect();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "Linux, then macOS in the middle, then Windows: {out}"
        );
        assert_eq!(
            out.matches(r#"class="neon-card home-practice fm-download""#)
                .count(),
            3,
            "three boxes, each one anchor: {out}"
        );
    }

    /// The boxes wear the home page's own classes.
    ///
    /// `home.css` puts the border, the lift, and the radial wash that swells
    /// across the whole box on `.home-practice`, and arms them through
    /// `.home-practices__grid > .home-practice` — the grid class is what
    /// establishes the clipping context, so a box outside that grid keeps the
    /// colours and loses the illumination. Both class names are therefore
    /// load-bearing rather than cosmetic, and a well-meaning rename to
    /// `fm-download__grid` alone would silently flatten the hover.
    #[test]
    fn the_boxes_wear_the_home_pages_illuminated_card() {
        let out = downloads_html();
        assert!(
            out.contains(r#"class="home-practices__grid fm-downloads__grid""#),
            "the grid is the home page's, which arms the hover wash: {out}"
        );
        for class in [
            "home-practice__mark",
            "home-practice__heading",
            "home-practice__body",
        ] {
            assert!(out.contains(class), "{class} renders: {out}");
        }
    }

    /// Each box's accessible name is a sentence that names the release and the
    /// platform, so a screen-reader user hears what arrives before they choose.
    ///
    /// The visible detail is punctuation-separated fragments (`x86_64 · glibc ·
    /// tar.gz`), which is right for the eye and wrong for the ear — an explicit
    /// label is what keeps the two audiences from having to share one string.
    #[test]
    fn each_box_announces_the_release_and_the_platform_it_is_for() {
        let out = downloads_html();
        for label in ["Linux", "macOS", "Windows"] {
            assert!(
                out.contains(&format!(
                    r#"aria-label="Download Neon Law Navigator 26.8.20 for {label} — "#
                )),
                "the {label} box names the release it hands over: {out}"
            );
        }
    }

    /// Every box carries `download` with the archive's filename, so the browser
    /// saves the file instead of navigating away from the page — and the
    /// filename is shown, so what lands in the downloads folder matches the box
    /// that was clicked.
    #[test]
    fn a_box_saves_the_archive_rather_than_navigating_to_it() {
        let out = downloads_html();
        for filename in [
            "navigator-26.8.20-linux.tar.gz",
            "navigator-26.8.20-macos.tar.gz",
            "navigator-26.8.20-windows.zip",
        ] {
            assert!(
                out.contains(&format!(r#"download="{filename}""#)),
                "{filename} is saved, not opened: {out}"
            );
            assert!(
                out.contains(&format!(r#"class="fm-download__file">{filename}<"#)),
                "{filename} is shown on its box: {out}"
            );
        }
    }

    /// The version is printed once, above the boxes, and every href carries it.
    /// A page that named one release and linked another would be worse than one
    /// that named none.
    #[test]
    fn the_version_the_band_prints_is_the_version_every_href_carries() {
        let out = downloads_html();
        assert!(
            out.contains(r#"class="fm-downloads__tag">26.8.20<"#),
            "the version is set as the string it is: {out}"
        );
        assert_eq!(
            out.matches("/releases/download/26.8.20/").count(),
            3,
            "all three hrefs name that release: {out}"
        );
    }

    /// The Homebrew route renders as one copy-paste command.
    #[test]
    fn the_homebrew_command_renders_once() {
        let out = downloads_html();
        assert_eq!(
            out.matches(r#"class="fm-package__command""#).count(),
            1,
            "one install command, because brew upgrades in place: {out}"
        );
        assert!(
            out.contains(crate::cli_release::HOMEBREW_INSTALL_COMMAND),
            "the install command names the tap: {out}"
        );
        assert!(
            !out.contains("brew upgrade "),
            "a second upgrade line would be a second spelling of the same formula: {out}"
        );
    }

    /// A page with no downloads band asks for no `home.css`, and one with a
    /// band asks for it.
    ///
    /// Asserted on the band data rather than on the rendered head: a
    /// `document::Stylesheet` is collected by the fullstack head collector and
    /// never appears in `dioxus_ssr::render` output. The covering assertion
    /// that the sheet actually reaches the document lives in
    /// `server/tests/firm_routes.rs`, against the real `/navigator` route.
    #[test]
    fn only_a_page_with_downloads_asks_for_the_home_stylesheet() {
        assert!(sample_downloads().is_downloads());
        assert!(!Band::Statement {
            heading: "Our mission".to_string(),
            lead: "A shortage of hours.".to_string(),
            body: vec![],
        }
        .is_downloads());
        assert!(sample_page().bands.iter().all(|band| !band.is_downloads()));
    }

    /// A page exercising every band shape in the vocabulary, in order.
    fn sample_page() -> PageContent {
        PageContent {
            hero_lines: Vec::new(),
            hero_lead: String::new(),
            hero_cta: None,
            head_title: "Fractional CTO — Neon Law".to_string(),
            meta_description: "The technology function, run by the firm.".to_string(),
            title: "Fractional CTO".to_string(),
            hero_mark: None,
            tagline: "The technology function, run by the firm.".to_string(),
            skin: PageSkin::Marketing,
            bands: vec![
                Band::Statement {
                    heading: "Our mission".to_string(),
                    lead: "Not a shortage of law. A shortage of hours.".to_string(),
                    body: vec![vec![
                        Run::plain("Routine matters should cost "),
                        Run::strong("very little to run"),
                    ]],
                },
                Band::Cards {
                    anchor: "what-we-do".to_string(),
                    overline: "The programs".to_string(),
                    heading: "What we do".to_string(),
                    description: None,
                    items: vec![Card {
                        title: "Company counsel".to_string(),
                        chips: vec!["Flat monthly fee".to_string()],
                        body: vec![vec![Run::plain("Cap table and employee agreements.")]],
                        href: Some("/fractional-gc".to_string()),
                        href_label: Some("See the practice".to_string()),
                    }],
                },
                Band::Steps {
                    anchor: "how-it-works".to_string(),
                    overline: "The engagement".to_string(),
                    heading: "How it works".to_string(),
                    description: Some("From a first email to a signed retainer.".to_string()),
                    items: vec![Step {
                        title: "You tell us about the matter".to_string(),
                        body: vec![vec![Run::plain("In your own words.")]],
                    }],
                },
                Band::ProjectNetwork {
                    anchor: "connected-project".to_string(),
                    overline: "The map".to_string(),
                    heading: "One Project".to_string(),
                    description: Some("The work, in one view.".to_string()),
                    left: vec![ProjectNetworkNode {
                        label: "Internal Slack".to_string(),
                        detail: "Firm conversation.".to_string(),
                    }],
                    right: vec![ProjectNetworkNode {
                        label: "Client portal".to_string(),
                        detail: "Client experience.".to_string(),
                    }],
                    mcp_tools: vec![
                        "Court Listener".to_string(),
                        "Descrybe".to_string(),
                        "Exa".to_string(),
                        "Midpage".to_string(),
                    ],
                    agentic_coding_tools: vec![
                        "Antigravity".to_string(),
                        "Claude Code".to_string(),
                        "Codex".to_string(),
                        "Cursor".to_string(),
                    ],
                    saas_tools: vec![
                        "Chatwoot".to_string(),
                        "Descript".to_string(),
                        "DocuSign".to_string(),
                        "Google Workspace".to_string(),
                        "Highlight".to_string(),
                        "Linear".to_string(),
                        "Mercury".to_string(),
                        "Xero".to_string(),
                    ],
                },
                Band::Cta {
                    heading: "Tell us about the matter.".to_string(),
                    body: None,
                    email: "support@neonlaw.com".to_string(),
                    email_subject: None,
                },
            ],
        }
    }

    fn page_html() -> String {
        fn app() -> Element {
            rsx! { MarketingPage { chrome: chrome(), content: sample_page() } }
        }
        render(app)
    }

    #[test]
    fn a_page_bolds_the_runs_marked_for_emphasis() {
        let out = page_html();
        assert!(
            out.contains("<strong>very little to run</strong>"),
            "an emphasised run renders as <strong>: {out}"
        );
    }

    #[test]
    fn linked_runs_render_as_inline_anchors_and_empty_leads_are_skipped() {
        fn app() -> Element {
            rsx! {
                Bands {
                    items: vec![Band::Statement {
                        heading: "Business filings".to_string(),
                        lead: String::new(),
                        body: vec![vec![
                            Run::plain("Business filings included in our "),
                            Run::link("fractional GC", "/fractional-gc"),
                            Run::plain(" projects."),
                        ]],
                    }],
                }
            }
        }
        let out = render(app);
        assert!(
            out.contains(r#"href="/fractional-gc""#),
            "an inline link run renders as an anchor: {out}"
        );
        assert!(
            out.contains("fractional GC</a>"),
            "the anchor carries the run's text: {out}"
        );
        // The anchor must be classless, or `theme.css`'s
        // `.nav-theme :is(p, li) > a:not([class])` stops matching it and the
        // link loses its non-colour cue — axe's `link-in-text-block`, which is
        // how `/services` failed the public accessibility gate.
        assert!(
            !out.contains(r"<a class="),
            "an inline prose link must carry no class, or it opts out of the \
             WCAG 1.4.1 underline rule: {out}"
        );
        assert!(
            !out.contains(r#"class="fm-statement__lead""#),
            "an empty lead renders no paragraph: {out}"
        );
    }

    #[test]
    fn a_page_renders_every_band_in_order() {
        let out = page_html();
        let statement = out.find("shortage of hours").expect("statement band");
        let cards = out.find("What we do").expect("cards band");
        let steps = out.find("How it works").expect("steps band");
        let network = out.find("One Project").expect("connected Project band");
        let cta = out.find("Tell us about the matter.").expect("cta band");
        assert!(
            statement < cards && cards < steps && steps < network && network < cta,
            "bands render in the order the content lists them: {out}"
        );
    }

    #[test]
    fn a_project_network_renders_the_wheel_and_accessible_resource_lanes() {
        let out = page_html();
        assert!(
            out.contains(r#"id="connected-project""#),
            "network anchor: {out}"
        );
        assert!(
            out.contains(r#"src="/public/navigator-wheel.svg""#),
            "the diagram uses Navigator's own wheel: {out}"
        );
        assert!(
            out.contains(r#"aria-label="Project resources to the left of Navigator""#)
                && out.contains(r#"aria-label="Project resources to the right of Navigator""#),
            "the two resource lanes are named: {out}"
        );
        for label in [
            "Internal Slack",
            "Client portal",
            "Navigator",
            "Web API MCP CLI",
            "MCPs",
            "Court Listener",
            "Agentic Legal Coding",
            "Antigravity",
            "Claude Code",
            "Codex",
            "Cursor",
            "SaaS",
            "DocuSign",
            "Google Workspace",
            "Descript",
            "Chatwoot",
            "Highlight",
            "Linear",
            "Mercury",
            "Xero",
        ] {
            assert!(out.contains(label), "missing diagram label {label}: {out}");
        }
    }

    #[test]
    fn card_bands_carry_their_anchor_so_the_nav_can_link_them() {
        // A page's own nav links `#what-we-do` and `#how-it-works`. A band
        // that renders no id turns both into no-ops that scroll nowhere.
        let out = page_html();
        assert!(out.contains(r#"id="what-we-do""#), "cards anchor: {out}");
        assert!(out.contains(r#"id="how-it-works""#), "steps anchor: {out}");
    }

    #[test]
    fn a_card_deep_links_to_the_page_that_expands_it() {
        let out = page_html();
        assert!(
            out.contains(r#"href="/fractional-gc""#),
            "the card links the page that expands it: {out}"
        );
        assert!(out.contains("See the practice"), "link label: {out}");
    }

    #[test]
    fn every_call_to_action_opens_the_firms_inbox() {
        // A CTA that renders no `mailto:` is a dead end on the band whose whole
        // job is to start a conversation.
        let out = page_html();
        assert_eq!(
            out.matches(r#"href="mailto:support@neonlaw.com""#).count(),
            1,
            "the closing CTA opens the inbox: {out}"
        );
    }

    #[test]
    fn a_call_to_action_can_prefill_an_email_subject() {
        fn app() -> Element {
            rsx! { MailAction {
                email: "contact@example.com".to_string(),
                subject: Some("Co-Counseling for Good with AI".to_string()),
            } }
        }

        let out = render(app);
        assert!(
            out.contains(
                r#"href="mailto:contact@example.com?subject=Co%2DCounseling%20for%20Good%20with%20AI""#
            ),
            "the mailto link carries the supplied subject: {out}"
        );
    }

    #[test]
    fn the_steps_band_is_an_ordered_list() {
        // "How it works" is a sequence, and a screen reader should hear it as
        // one. `<ul>` would drop the ordering the copy depends on.
        let out = page_html();
        assert!(out.contains(r#"<ol class="fm-steps""#), "ordered: {out}");
    }

    /// The two skins put the page's `<h1>` in different places, and that is the
    /// whole point of the flag.
    ///
    /// A campaign page reads as a campaign: the title is the headline. A
    /// practice page reads as a practice: the practice name is the label and
    /// the statement is the headline, the way `/litigation` reads. One renderer
    /// serves both, so this is what keeps a change to one from silently
    /// restyling the other.
    #[test]
    fn the_practice_skin_leads_with_the_eyebrow_and_the_marketing_skin_with_the_title() {
        fn page(skin: PageSkin) -> String {
            let content = PageContent {
                hero_lines: Vec::new(),
                hero_lead: String::new(),
                hero_cta: None,
                head_title: "T".to_string(),
                meta_description: "D".to_string(),
                title: "Fractional CTO".to_string(),
                hero_mark: Some(PracticeMark::Technology),
                tagline: "We run the technology function for law firms.".to_string(),
                bands: vec![],
                skin,
            };
            let mut dom = VirtualDom::new_with_props(
                MarketingPage,
                MarketingPageProps {
                    chrome: chrome(),
                    content,
                },
            );
            dom.rebuild_in_place();
            dioxus_ssr::render(&dom)
        }

        let practice = page(PageSkin::Practice);
        assert!(
            practice.contains("fm-page--practice"),
            "the practice skin marks the page root: {practice}"
        );
        assert!(
            practice.contains(r#"<p class="firm-eyebrow">Fractional CTO</p>"#),
            "the practice name is the eyebrow: {practice}"
        );
        assert!(
            practice.contains("We run the technology function for law firms."),
            "the statement is on the page: {practice}"
        );
        assert!(
            practice.contains("firm-glow"),
            "the practice skin carries the glow the practice pages wear: {practice}"
        );
        assert!(
            practice.contains(r#"data-practice-mark="technology""#),
            "the practice page carries the same technology mark as the product card: {practice}"
        );

        let marketing = page(PageSkin::Marketing);
        assert!(
            !marketing.contains("fm-page--practice"),
            "the marketing skin does not mark the root: {marketing}"
        );
        assert!(
            !marketing.contains("firm-eyebrow"),
            "the marketing skin renders no eyebrow: {marketing}"
        );
        assert!(
            !marketing.contains("firm-glow"),
            "the marketing skin carries no glow: {marketing}"
        );
        assert!(
            marketing.contains(r#"class="fm-hero__tagline""#),
            "the marketing skin keeps the tagline under the title: {marketing}"
        );
    }

    #[test]
    fn a_marketing_page_renders_its_title_and_bands_without_a_badge() {
        fn app() -> Element {
            let content = PageContent {
                hero_lines: Vec::new(),
                hero_lead: String::new(),
                hero_cta: None,
                head_title: "Fractional General Counsel — Neon Law".to_string(),
                meta_description: "Company counsel on a flat monthly fee.".to_string(),
                title: "Fractional General Counsel".to_string(),
                hero_mark: None,
                tagline: "Work that arrives scoped.".to_string(),
                bands: vec![Band::Cta {
                    heading: "Tell us about the company.".to_string(),
                    body: Some("What you are building, and who is on the cap table.".to_string()),
                    email: "support@neonlaw.com".to_string(),
                    email_subject: None,
                }],
                skin: PageSkin::Marketing,
            };
            rsx! { MarketingPage { chrome: chrome(), content } }
        }
        let out = render(app);
        assert!(out.contains("Fractional General Counsel"), "title: {out}");
        assert!(out.contains("Work that arrives scoped."), "tagline: {out}");
        assert!(out.contains("Tell us about the company."), "cta: {out}");
        assert!(
            !out.contains("fm-badge"),
            "a marketing page opens on its argument, and no page carries a badge \
             now that the renderer draws no hero badge at all: {out}"
        );
    }
}
