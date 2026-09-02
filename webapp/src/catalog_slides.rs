//! `/{category}/{slug}/slides` — the light-table grid of
//! every slide in a material, migrated to Dioxus SSR (#956 Phase 4).
//!
//! Almost everything dynamic here is client-side and deliberately so. Progress
//! lives in `localStorage` and is never sent anywhere: `workshop-progress.js`
//! reads the `data-workshop-progress` hooks and reveals the certificate form
//! once every slide has been seen. That gate is **a courtesy, not an access
//! control** — completion is client-trusted by design, because the
//! alternative is telemetry on how someone reads.
//!
//! The `PageLayout` loaded that script on every render; a Dioxus page
//! loads only what it names, so [`CatalogSlidesPage`] hoists it explicitly.
//! Without it the certificate form stays hidden forever.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::catalog_slide_body::{CatalogSlideBody, InjectedPracticeCatalog};
use crate::components::{PublicShell, SiteHeader, SiteNavLink, CATALOG_STYLESHEET_HREF};
use crate::home::PracticeLink;
use crate::public_chrome::{PublicChrome, PublicFooter};

/// The first-party script that paints slide-seen state and reveals the
/// certificate gate.
pub const WORKSHOP_PROGRESS_SCRIPT_HREF: &str = "/public/js/workshop-progress.js";

/// One slide's thumbnail in the grid.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SlideThumb {
    /// 1-based slide number, across the whole material.
    pub number: usize,
    pub title: String,
    /// Rendered HTML of the slide face, shown shrunk into the thumbnail.
    pub body_html: String,
    pub href: String,
}

/// One chapter's worth of thumbnails.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct SlideChapter {
    pub number: usize,
    pub title: String,
    pub slides: Vec<SlideThumb>,
}

/// The light table's resolved content.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct LightTableContent {
    pub workshop_title: String,
    /// The material slug, the key `workshop-progress.js` stores progress under.
    pub slug: String,
    /// Back to the material hub.
    pub material_href: String,
    pub chapters: Vec<SlideChapter>,
    /// Total slide count — the certificate gate unlocks once this many slides
    /// have been seen.
    pub total: usize,
    /// Where the certificate form posts.
    pub certificate_action: String,
    /// Double-submit CSRF token for the certificate form. Minted per request.
    pub csrf_token: String,
}

/// The [`LightTableContent`] the portal pre-layer injects.
#[derive(Clone, Default)]
pub struct InjectedLightTable(pub LightTableContent);

/// Everything the page renders.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CatalogSlidesView {
    pub chrome: PublicChrome,
    pub content: LightTableContent,
    pub practices: Vec<PracticeLink>,
}

/// Resolve the shared chrome and this material's light table.
#[server]
pub async fn catalog_slides_view() -> Result<CatalogSlidesView, ServerFnError> {
    let content =
        dioxus_fullstack_core::FullstackContext::extract::<axum::Extension<InjectedLightTable>, _>(
        )
        .await
        .map(|axum::Extension(c)| c.0)
        .unwrap_or_default();
    let practices = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedPracticeCatalog>,
        _,
    >()
    .await
    .map(|axum::Extension(c)| c.0)
    .unwrap_or_default();
    Ok(CatalogSlidesView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
        practices,
    })
}

/// The page's route entry.
#[component]
pub fn CatalogSlidesEntry() -> Element {
    let resource = use_server_future(catalog_slides_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        CatalogSlidesPage {
            chrome: view.chrome,
            content: view.content,
            practices: view.practices,
        }
    }
}

/// The pure light table.
#[component]
pub fn CatalogSlidesPage(
    chrome: PublicChrome,
    content: LightTableContent,
    #[props(default)] practices: Vec<PracticeLink>,
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
        document::Title { "{chrome.brand_name} | {content.workshop_title} | Slides" }
        document::Meta { name: "description", content: "Every slide in the workshop, at a glance." }
        document::Stylesheet { href: CATALOG_STYLESHEET_HREF }
        // Without this nothing on the page is ever checked, the count stays at
        // zero, and the certificate gate never opens.
        document::Script { src: WORKSHOP_PROGRESS_SCRIPT_HREF, defer: true }
        PublicShell { header, footer,
            article {
                class: "workshop-lighttable",
                "data-workshop-progress": "lighttable",
                "data-workshop-slug": "{content.slug}",
                "data-total": "{content.total}",
                nav { "aria-label": "Back to workshop",
                    a { href: "{content.material_href}", "← {content.workshop_title}" }
                }
                header { class: "lighttable-header",
                    h1 { "{content.workshop_title}" }
                }
                p { class: "catalog-empty",
                    "Open any slide to read it. View them all to unlock your certificate."
                }
                div { class: "lighttable-chapters",
                    for chapter in content.chapters.iter() {
                        LightTableChapter {
                            chapter: chapter.clone(),
                            practices: practices.clone(),
                        }
                    }
                }
                CertificateGate {
                    action: content.certificate_action.clone(),
                    csrf_token: content.csrf_token.clone(),
                }
            }
        }
    }
}

/// One chapter's thumbnails.
#[component]
fn LightTableChapter(chapter: SlideChapter, practices: Vec<PracticeLink>) -> Element {
    rsx! {
        section {
            class: "workshop-chapter",
            "data-workshop-chapter": "{chapter.title}",
            header { class: "workshop-chapter__header",
                span { class: "catalog-badge", "{chapter.number}" }
                div {
                    p { class: "catalog-eyebrow", "Chapter {chapter.number}" }
                    h2 { class: "workshop-chapter__title", "{chapter.title}" }
                }
            }
            div { class: "lighttable-grid",
                for slide in chapter.slides.iter() {
                    a {
                        class: "slide-thumb",
                        href: "{slide.href}",
                        "aria-label": "Open slide {slide.number}: {slide.title}",
                        "data-slide": "{slide.number}",
                        // The shrunk slide face is decoration; the readable
                        // label is the caption below it.
                        div {
                            class: "slide-thumb__preview",
                            "aria-hidden": "true",
                            inert: true,
                            CatalogSlideBody {
                                title: slide.title.clone(),
                                body_html: slide.body_html.clone(),
                                practices: practices.clone(),
                            }
                        }
                        div { class: "slide-thumb__caption", "{slide.number}. {slide.title}" }
                    }
                }
            }
        }
    }
}

/// The certificate request form, hidden until `workshop-progress.js` sees that
/// every slide has been opened.
///
/// Completion is client-trusted on purpose — the alternative is telemetry on
/// how someone reads — so this gate is a courtesy, not an access control, and
/// the server does not re-check it.
#[component]
fn CertificateGate(action: String, csrf_token: String) -> Element {
    rsx! {
        section {
            class: "workshop-certificate",
            "data-cert-gate": true,
            hidden: true,
            h2 { "You finished — claim your certificate" }
            p {
                "Enter your name and email and Neon Law will send a PDF certificate of completion."
            }
            form { class: "admin-form workshop-certificate__form", method: "post", action: "{action}",
                input { r#type: "hidden", name: "csrf_token", value: "{csrf_token}" }
                div { class: "workshop-certificate__field",
                    label { r#for: "cert-name", "Your name" }
                    input {
                        r#type: "text",
                        id: "cert-name",
                        name: "name",
                        required: true,
                        maxlength: "120",
                        placeholder: "Jane Q. Student",
                    }
                }
                div { class: "workshop-certificate__field",
                    label { r#for: "cert-email", "Email" }
                    input {
                        r#type: "email",
                        id: "cert-email",
                        name: "email",
                        required: true,
                        maxlength: "254",
                        placeholder: "you@example.com",
                    }
                }
                button { class: "nav-btn nav-btn--primary", r#type: "submit", "Email my certificate" }
            }
            p { class: "catalog-empty", "We use your email only to send this certificate." }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn table() -> LightTableContent {
        LightTableContent {
            workshop_title: "Using Neon Law Navigator".to_string(),
            slug: "use-the-navigator".to_string(),
            material_href: "/workshops/use-the-navigator".to_string(),
            chapters: vec![SlideChapter {
                number: 1,
                title: "Intro".to_string(),
                slides: vec![
                    SlideThumb {
                        number: 1,
                        title: "Install".to_string(),
                        body_html: "<h3>Install</h3>".to_string(),
                        href: "/workshops/use-the-navigator/step/1".to_string(),
                    },
                    SlideThumb {
                        number: 2,
                        title: "Notarize".to_string(),
                        body_html: "<h3>Notarize</h3>".to_string(),
                        href: "/workshops/use-the-navigator/step/2".to_string(),
                    },
                ],
            }],
            total: 2,
            certificate_action: "/workshops/use-the-navigator/certificate".to_string(),
            csrf_token: "tok-123".to_string(),
        }
    }

    fn html() -> String {
        fn app() -> Element {
            rsx! {
                CatalogSlidesPage { chrome: PublicChrome::default(), content: table() }
            }
        }
        ssr(app)
    }

    #[test]
    fn every_hook_workshop_progress_js_reads_is_present() {
        // This page is inert without the script, and the script finds
        // everything by these attributes. A rename here is a silent regression:
        // the page still renders, no progress is counted, and the certificate
        // never unlocks.
        let out = html();
        for hook in [
            r#"data-workshop-progress="lighttable""#,
            r#"data-workshop-slug="use-the-navigator""#,
            r#"data-total="2""#,
            "data-cert-gate",
            r#"data-slide="1""#,
            r#"data-workshop-chapter="Intro""#,
        ] {
            assert!(out.contains(hook), "missing hook {hook}: {out}");
        }
    }

    #[test]
    fn each_slide_links_to_its_own_step() {
        let out = html();
        assert!(out.contains("1. Install"), "first caption: {out}");
        assert!(out.contains("2. Notarize"), "second caption: {out}");
        for n in [1, 2] {
            assert!(
                out.contains(&format!(r#"href="/workshops/use-the-navigator/step/{n}""#)),
                "slide {n} href: {out}"
            );
        }
        assert!(
            out.contains(r#"aria-label="Open slide 1: Install""#),
            "slide links have distinct names: {out}"
        );
    }

    #[test]
    fn the_shrunk_slide_face_is_inert_and_hidden_from_assistive_tech() {
        // The preview is a decorative miniature; the caption is the readable
        // label, so a screen reader should hear the caption only and its
        // embedded links cannot receive focus.
        let out = html();
        assert!(
            out.contains(r#"class="slide-thumb__preview" aria-hidden="true" inert=true"#),
            "preview is decorative and inert: {out}"
        );
    }

    #[test]
    fn the_certificate_gate_starts_hidden_and_carries_its_csrf_token() {
        let out = html();
        assert!(
            out.contains("workshop-certificate") && out.contains("hidden"),
            "the gate renders hidden: {out}"
        );
        // The field is named `csrf_token`, not `_csrf` — the POST handler reads
        // that spelling, so renaming it breaks the submit.
        assert!(
            out.contains(r#"name="csrf_token""#) && out.contains(r#"value="tok-123""#),
            "hidden CSRF field: {out}"
        );
        assert!(
            out.contains(r#"action="/workshops/use-the-navigator/certificate""#)
                && out.contains(r#"method="post""#),
            "the form posts to the certificate endpoint: {out}"
        );
    }

    #[test]
    fn the_certificate_form_keeps_the_admin_form_hook() {
        // Styling-free class the nightly deploy gate selects on.
        let out = html();
        assert!(out.contains("admin-form"), "e2e form hook: {out}");
    }

    /// The certificate is the firm's artifact. `workflows::email::certificate`
    /// renders it under the firm brand and asserts the same absence, so a gate
    /// promising a certificate from the retired nonprofit would name a sender
    /// the email it produces never claims.
    #[test]
    fn the_gate_names_no_retired_organization_as_the_sender() {
        let out = html();
        assert!(
            !out.contains("Foundation"),
            "the retired nonprofit cannot be the promised sender: {out}"
        );
        assert!(
            out.contains("Neon Law will send a PDF certificate of completion"),
            "the firm is the sender the gate names: {out}"
        );
    }
}
