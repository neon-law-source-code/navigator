//! The Foundation's public-disclosure surface, migrated to Dioxus SSR
//! (#956 Phase 4).
//!
//! Three pages: the hub at `/transparency`, one governance document
//! at `/foundation/transparency/{slug}`, and one quarterly board-minutes
//! page at `/foundation/transparency/minutes/{slug}`.
//!
//! The hub separates the documents a 501(c)(3) **must** make public under IRC
//! §6104(d) — the exemption application, the IRS determination letter, and the
//! annual returns — from the records the Foundation publishes *voluntarily*
//! (bylaws, the conflict of interest policy, and board minutes). Federal law
//! does not require those latter documents to be public, so the copy is careful
//! never to claim it does. Treat the wording here as reviewed legal copy: it is
//! carried over verbatim from the page and should not be reworded without
//! Legal Council.
//!
//! Per-request content: the documents are loaded from
//! `server/content/foundation/`, which `webapp` cannot reach, so the portal
//! route's pre-layer resolves them and injects a wasm-safe carrier. That layer
//! also owns the 404s — for an unknown slug, and for a slug whose category does
//! not match the route it arrived on.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{PublicShell, SiteHeader, SiteNavLink, SocialMeta};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The hub's `<meta description>`, as the page set it.
const INDEX_DESCRIPTION: &str = "Public disclosures of the Neon Law Foundation — the IRS \
                                 determination letter, bylaws, conflict of interest policy, \
                                 and board meeting minutes.";
/// The per-document fallback `<meta description>` when front matter has none.
const DOC_FALLBACK_DESCRIPTION: &str = "A Neon Law Foundation transparency document.";
/// The hub's canonical path.
pub const TRANSPARENCY_CANONICAL: &str = "/foundation/transparency";

/// One document as it appears in a list — a title, a one-line description, and
/// the link to open it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct DocLink {
    pub href: String,
    pub title: String,
    pub description: String,
}

/// The hub's content: the two voluntary lanes, already ordered by the portal
/// pre-layer (governance by priority, minutes newest-first).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TransparencyIndexContent {
    pub governance: Vec<DocLink>,
    pub minutes: Vec<DocLink>,
}

/// The full content of one transparency document page.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TransparencyDocContent {
    pub title: String,
    /// One-line front-matter summary, used for `<meta description>`; falls back
    /// to a generic line when empty.
    pub description: String,
    /// Canonical path, e.g. `/foundation/transparency/bylaws`.
    pub canonical_path: String,
    /// Rendered HTML body (already sanitized; NOT raw markdown).
    pub body_html: String,
}

/// The hub content the portal pre-layer injects.
#[derive(Clone, Default)]
pub struct InjectedTransparencyIndex(pub TransparencyIndexContent);

/// The document content the portal pre-layer injects.
#[derive(Clone, Default)]
pub struct InjectedTransparencyDoc(pub TransparencyDocContent);

/// Everything the hub renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TransparencyIndexView {
    pub chrome: PublicChrome,
    pub content: TransparencyIndexContent,
}

/// Everything a document page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct TransparencyDocView {
    pub chrome: PublicChrome,
    pub content: TransparencyDocContent,
}

/// Resolve the hub from the injected extension and the Foundation chrome.
#[server]
pub async fn transparency_index_view() -> Result<TransparencyIndexView, ServerFnError> {
    let content = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedTransparencyIndex>,
        _,
    >()
    .await
    .map_or_else(
        |_| TransparencyIndexContent::default(),
        |axum::Extension(injected)| injected.0,
    );
    Ok(TransparencyIndexView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// Resolve one document from the injected extension and the Foundation chrome.
#[server]
pub async fn transparency_doc_view() -> Result<TransparencyDocView, ServerFnError> {
    let content = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedTransparencyDoc>,
        _,
    >()
    .await
    .map_or_else(
        |_| TransparencyDocContent::default(),
        |axum::Extension(injected)| injected.0,
    );
    Ok(TransparencyDocView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The hub's route entry.
#[component]
pub fn TransparencyIndexEntry() -> Element {
    let resource = use_server_future(transparency_index_view)?;
    // Clone the view out of the read guard before rendering so the borrow does
    // not outlive it (the `rsx!` output escapes this scope).
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    transparency_index_body(&view)
}

/// A document page's route entry.
#[component]
pub fn TransparencyDocEntry() -> Element {
    let resource = use_server_future(transparency_doc_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    transparency_doc_body(&view)
}

/// The public-shell header for a Foundation transparency page.
fn transparency_header(chrome: &PublicChrome) -> Element {
    rsx! {
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
    }
}

/// The shared unified footer for a Foundation transparency page.
fn transparency_footer(chrome: &PublicChrome) -> Element {
    rsx! {
        PublicFooter { chrome: chrome.clone() }
    }
}

/// The head every transparency page shares: title, description, canonical, and
/// the share card. The canonical is a single `rel`+`href` pair per page, so it
/// is safe to emit through `document::Link` — the dedupe that rules that out for
/// the hreflang alternates only bites when two links share `href`+`rel`.
fn transparency_head(
    chrome: &PublicChrome,
    title: &str,
    description: &str,
    canonical: &str,
) -> Element {
    let head_title = format!("{} | {}", chrome.brand_name, title);
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: "{description}" }
        document::Link { rel: "canonical", href: "{canonical}" }
        SocialMeta {
            title: head_title.clone(),
            description: description.to_string(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
    }
}

/// `GET /transparency` — the public-disclosure hub.
///
/// The section split is the point of the page: "Required public disclosures"
/// names what §6104(d) compels, and "Published voluntarily" states plainly that
/// those documents are **not** required to be public. None of the required
/// disclosures is linked, because none is published yet — a §6104(d) page that
/// links to a 404 is worse than one that says "ask us".
pub fn transparency_index_body(view: &TransparencyIndexView) -> Element {
    let chrome = &view.chrome;
    let header = transparency_header(chrome);
    let footer = transparency_footer(chrome);
    let head = transparency_head(
        chrome,
        "Transparency",
        INDEX_DESCRIPTION,
        TRANSPARENCY_CANONICAL,
    );
    let foundation_name = chrome.foundation_name.clone();
    rsx! {
        {head}
        PublicShell { header, footer,
            article { class: "transparency",
                h1 { "Transparency" }
                p {
                    "The "
                    "{foundation_name}"
                    " is a Nevada nonprofit corporation "
                    "recognized by the IRS as a 501(c)(3) tax-exempt organization. We publish "
                    "here the documents the law requires us to make public — and, going further, "
                    "the governance records we choose to share."
                }

                RequiredDisclosures {}
                VoluntaryDocuments { governance: view.content.governance.clone() }
                BoardMinutes { minutes: view.content.minutes.clone() }

                p { class: "nav-muted",
                    "Looking for a record that isn't posted here? Request it through the "
                    a { href: "/contact", "contact page" }
                    "."
                }
            }
        }
    }
}

/// The §6104(d) section: what federal law compels the Foundation to make
/// public. None of these is linked, because none is published yet — a §6104(d)
/// page that links to a 404 is worse than one that says "ask us". Static copy,
/// so it takes no props.
#[component]
fn RequiredDisclosures() -> Element {
    rsx! {
        section { class: "transparency-required",
            h2 { "Required public disclosures" }
            p { class: "nav-muted",
                "Federal law (Internal Revenue Code §6104(d)) requires a 501(c)(3) to make "
                "these available for public inspection. Posting them here also satisfies "
                "that duty for anyone who asks."
            }
            ul { class: "transparency-list",
                li {
                    "IRS determination letter — the letter recognizing the Foundation's "
                    "501(c)(3) status. Available on request through the "
                    a { href: "/contact", "contact page" }
                    " while we prepare it for publication here."
                }
                li {
                    "Exemption application (IRS Form 1023) and supporting documents — "
                    "available on request through the "
                    a { href: "/contact", "contact page" }
                    " while we prepare the filing for publication here."
                }
                li {
                    "Annual returns (IRS Form 990-series) — the three most recent returns "
                    "will be posted here once filed."
                }
            }
        }
    }
}

/// The voluntary section: records the Foundation publishes because it chooses
/// to. The emphasized "not" is load-bearing legal copy — the page must never
/// imply these documents are legally mandated.
#[component]
fn VoluntaryDocuments(governance: Vec<DocLink>) -> Element {
    let is_empty = governance.is_empty();
    rsx! {
        section { class: "transparency-voluntary",
            h2 { "Published voluntarily" }
            p { class: "nav-muted",
                "The documents below are "
                em { "not" }
                " required to be public. The "
                "Foundation publishes them because transparency about how it governs "
                "itself is part of the mission."
            }
            if is_empty {
                p { "Governance documents will be posted here soon." }
            } else {
                ul { class: "transparency-list",
                    for doc in governance.iter() {
                        GovernanceItem { doc: doc.clone() }
                    }
                }
            }
            p {
                "The Foundation also publishes the standard agreements it uses to engage "
                "its team — an at-will employment agreement and an independent-contractor "
                "agreement — as open "
                a { href: "/notations", "Notations" }
                " any nonprofit can reuse."
            }
        }
    }
}

/// The quarterly board minutes. The heading and its note always render; the list
/// appears only once a quarter is published, so an empty section never shows a
/// bare list.
#[component]
fn BoardMinutes(minutes: Vec<DocLink>) -> Element {
    let has_minutes = !minutes.is_empty();
    rsx! {
        section { class: "transparency-minutes", id: "minutes",
            h2 { "Board meeting minutes" }
            p { class: "nav-muted",
                "Minutes of the Foundation's regular quarterly board meetings. Approved "
                "minutes are published as they are finalized."
            }
            if has_minutes {
                ul { class: "transparency-list transparency-minutes-list",
                    for doc in minutes.iter() {
                        li {
                            a { href: "{doc.href}", "{doc.title}" }
                        }
                    }
                }
            }
        }
    }
}

/// One governance document's line on the hub: the link, then its blurb when the
/// front matter carries one.
#[component]
fn GovernanceItem(doc: DocLink) -> Element {
    // The page joined the blurb with a literal " — " separator, which rsx!
    // cannot express against an interpolation, so it is built here.
    let blurb = (!doc.description.is_empty()).then(|| format!(" — {}", doc.description));
    rsx! {
        li {
            a { href: "{doc.href}", "{doc.title}" }
            if let Some(blurb) = blurb {
                "{blurb}"
            }
        }
    }
}

/// `GET /foundation/transparency/{slug}` and its `minutes/{slug}` twin — one
/// document as a centered letter.
pub fn transparency_doc_body(view: &TransparencyDocView) -> Element {
    let chrome = &view.chrome;
    let header = transparency_header(chrome);
    let footer = transparency_footer(chrome);
    let description = if view.content.description.is_empty() {
        DOC_FALLBACK_DESCRIPTION
    } else {
        &view.content.description
    };
    let head = transparency_head(
        chrome,
        &view.content.title,
        description,
        &view.content.canonical_path,
    );
    let body_html = view.content.body_html.clone();
    rsx! {
        {head}
        PublicShell { header, footer,
            article { class: "transparency-doc",
                p {
                    a { href: TRANSPARENCY_CANONICAL, "← All Foundation documents" }
                }
                h1 { "{view.content.title}" }
                // Already-rendered, already-sanitized HTML from the content
                // loader — the page used `PreEscaped` for the same reason.
                div { dangerous_inner_html: "{body_html}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chrome() -> PublicChrome {
        PublicChrome {
            // The shared header, which is the firm's — see `notations`.
            brand_name: "Neon Law".to_string(),
            home_href: "/".to_string(),
            logo_href: "/public/logo-firm.svg".to_string(),
            firm_name: "Neon Law".to_string(),
            foundation_name: "Neon Law Foundation".to_string(),
            ..PublicChrome::default()
        }
    }

    fn link(slug: &str, title: &str, desc: &str) -> DocLink {
        DocLink {
            href: format!("{TRANSPARENCY_CANONICAL}/{slug}"),
            title: title.to_string(),
            description: desc.to_string(),
        }
    }

    fn index_html(content: TransparencyIndexContent) -> String {
        dioxus_ssr::render_element(transparency_index_body(&TransparencyIndexView {
            chrome: chrome(),
            content,
        }))
    }

    fn doc_html(content: TransparencyDocContent) -> String {
        dioxus_ssr::render_element(transparency_doc_body(&TransparencyDocView {
            chrome: chrome(),
            content,
        }))
    }

    #[test]
    fn the_hub_separates_required_from_voluntary_and_does_not_overclaim() {
        let out = index_html(TransparencyIndexContent::default());
        assert!(out.contains("Required public disclosures"), "{out}");
        assert!(out.contains("Published voluntarily"), "{out}");
        // The determination letter is named as a required disclosure, and
        // offered on request rather than linked: the Foundation has not
        // published the PDF, and a §6104(d) page must not link to a 404.
        assert!(out.contains("IRS determination letter"), "{out}");
        assert!(!out.contains("determination-letter.pdf"), "{out}");
        // The voluntary section must say these are NOT required — the page may
        // never imply bylaws or minutes are legally mandated.
        assert!(out.contains("<em>not</em>"), "the emphasized denial: {out}");
        assert!(out.contains("required to be public"), "{out}");
    }

    #[test]
    fn an_empty_hub_still_names_the_required_disclosures() {
        let out = index_html(TransparencyIndexContent::default());
        assert!(
            out.contains("Governance documents will be posted here soon."),
            "governance empty state: {out}"
        );
        // The required section is not conditional — it must render whether or
        // not any voluntary document exists.
        assert!(out.contains("Internal Revenue Code §6104(d)"), "{out}");
    }

    #[test]
    fn the_hub_lists_governance_with_blurbs_and_minutes_without() {
        let out = index_html(TransparencyIndexContent {
            governance: vec![
                link("bylaws", "Bylaws", "How the board operates."),
                link("conflict-of-interest", "Conflict of Interest Policy", ""),
            ],
            minutes: vec![DocLink {
                href: "/foundation/transparency/minutes/26q2".to_string(),
                title: "Board Meeting Minutes — Q2 2026".to_string(),
                description: String::new(),
            }],
        });
        assert!(
            out.contains(r#"href="/foundation/transparency/bylaws""#),
            "{out}"
        );
        assert!(out.contains("How the board operates."), "blurb rendered");
        // A document with no front-matter description gets no dangling " — ".
        assert!(
            !out.contains("Conflict of Interest Policy</a> — <"),
            "no empty blurb separator: {out}"
        );
        assert!(
            out.contains(r#"href="/foundation/transparency/minutes/26q2""#),
            "minutes keep their own prefix: {out}"
        );
    }

    #[test]
    fn the_minutes_list_is_absent_when_no_quarter_is_published() {
        let out = index_html(TransparencyIndexContent::default());
        assert!(out.contains("Board meeting minutes"), "heading still shown");
        assert!(
            !out.contains("transparency-minutes-list"),
            "no empty list rendered: {out}"
        );
    }

    #[test]
    fn a_document_renders_its_body_verbatim_and_links_back() {
        let out = doc_html(TransparencyDocContent {
            title: "Bylaws".to_string(),
            description: "How the board operates.".to_string(),
            canonical_path: "/foundation/transparency/bylaws".to_string(),
            body_html: "<h2>Article I</h2><p>The board.</p>".to_string(),
        });
        assert!(out.contains("<h2>Article I</h2>"), "body verbatim: {out}");
        assert!(
            !out.contains("&lt;h2"),
            "the body must not be escaped: {out}"
        );
        assert!(
            out.contains(r#"href="/foundation/transparency""#),
            "back link to the hub: {out}"
        );
        assert!(
            out.contains("← All Foundation documents"),
            "back-link label"
        );
    }

    #[test]
    fn every_page_wears_the_foundation_chrome() {
        for out in [
            index_html(TransparencyIndexContent::default()),
            doc_html(TransparencyDocContent::default()),
        ] {
            assert!(out.contains("site-header"), "header chrome: {out}");
            assert!(out.contains("site-footer__legal"), "unified footer chrome");
            assert!(
                out.contains(r#"href="/foundation""#),
                "Foundation home link: {out}"
            );
        }
    }
}
