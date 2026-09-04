//! `/notations/{slug}` — the public show page for one bundled notation.
//!
//! Every notation — letter or form — reuses the same highlight rendering
//! [`crate::harvard_outline`] built for the lawyer-tier recording stage at
//! `/app/outline`: the body is Markdown either way, so it steps
//! paragraph-by-paragraph with the arrow keys whether or not it carries
//! `I.`/`II.` headings. Wrapped in the public site chrome instead of the
//! `/app` navbar, with no auth or policy gate: this is the same content
//! already published as the notation's raw source on GitHub, just rendered
//! for a reader rather than a git browser.
//!
//! A form additionally gets a link to the government's own blank form (the
//! template's `origin_url`) when one is declared — `None` for a letter. The
//! fields the questionnaire fills in are not repeated here: the YAML
//! frontmatter rendered below already names them (`questionnaire:`,
//! `custom_questions:`), so a separate table would only duplicate it.
//!
//! Every notation gets its own path, the same shape as
//! [`crate::catalog_material`]'s `/workshops/{slug}` and
//! `/presentations/{slug}` — a shareable, bookmarkable URL per document
//! rather than one page keyed by a query string. An unknown slug 404s,
//! exactly as an unknown workshop or presentation material does. A
//! [`BackBreadcrumb`] returns to the `/notations` catalog, and "View source
//! on GitHub" sits on the title's own line rather than the `/notations`
//! catalog card — the card's own link now opens this page directly.
//!
//! Below the title, the page is four stacked [`Accordion`]s, each closed by
//! default so a reader lands on a short page rather than a wall of YAML and
//! Markdown: **Frontmatter** (the template's declared YAML as a
//! [`CodeBlock`]; omitted entirely for a template with none), **Body** (the
//! highlighted stage), **Questionnaire** (the "Try answering this" demo,
//! [`QuestionnaireDemo`]), and **Workflow** (the sample-run diagram,
//! [`WorkflowDiagram`]). The last two are omitted when the template declares
//! no `questionnaire:` or `workflow:` block, same as before this page had
//! accordions at all.
//!
//! The slug also drives the browser tab title for free: every
//! server-rendered route's `<title>` is replaced by `stamp_document_title`
//! (in `portal::dioxus_app`) with one derived mechanically from the URL
//! path, and `onboarding-letter` title-cases to "Onboarding Letter" — so
//! naming the slug after the document is what makes the sitewide tab-title
//! policy show the right thing, not a per-page override of it.
//!
//! The `{slug}` resolution happens in the portal's axum middleware rather
//! than inside [`notation_preview_view`], mirroring how
//! [`crate::contact_page::contact_page_view`] reads its content: a plain
//! synchronous `Extension`, resolved before the render.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{
    Accordion, BackBreadcrumb, CodeBlock, PublicShell, SiteHeader, SiteNavLink, SocialMeta,
    CATALOG_STYLESHEET_HREF,
};
use crate::harvard_outline::{HARVARD_OUTLINE_SCRIPT_HREF, HARVARD_OUTLINE_STYLESHEET_HREF};
use crate::notation_demo::{DemoQuestion, QuestionnaireDemo};
use crate::notation_workflow::{WorkflowDiagram, WorkflowStateView};
use crate::public_chrome::{PublicChrome, PublicFooter};

/// One bundled document ready to preview.
#[derive(Clone, Default)]
pub struct PreviewDoc {
    pub slug: String,
    pub title: String,
    pub source_href: String,
    /// The template's YAML frontmatter, verbatim. Empty renders nothing.
    pub frontmatter: String,
    /// The paragraph-highlighted stage, from
    /// `views::harvard_outline::stage_html` — every notation has one, since
    /// every notation's body is Markdown prose.
    pub stage_html: String,
    /// The government's own blank form, for a form template that declares
    /// `origin_url`. `None` for a letter.
    pub origin_url: Option<String>,
    /// The template's declared questionnaire, in order, from
    /// `views::questionnaire_preview::parse` — feeds the "Try answering
    /// this" demo (ENG-452). Empty for a template with no questionnaire
    /// block, which renders no demo section at all.
    pub demo_questions: Vec<DemoQuestion>,
    /// The template's declared `workflow:` state machine, from
    /// `views::workflow_preview::parse` — feeds the sample "Workflow" runs.
    /// Empty for a template with no workflow block, which renders no
    /// section at all.
    pub demo_workflow: Vec<WorkflowStateView>,
}

/// Everything the page renders, resolved ahead of the render — the router's
/// middleware picks the matching [`PreviewDoc`] by the `{slug}` path segment
/// and injects it for [`notation_preview_view`] to read back synchronously.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct NotationPreviewContent {
    pub title: String,
    pub source_href: String,
    /// The template's YAML frontmatter, verbatim. Empty renders nothing.
    pub frontmatter: String,
    pub stage_html: String,
    pub origin_url: Option<String>,
    pub demo_questions: Vec<DemoQuestion>,
    pub demo_workflow: Vec<WorkflowStateView>,
}

/// The [`NotationPreviewContent`] injected into the render context by the
/// portal router.
#[derive(Clone, Default)]
pub struct InjectedNotationPreview(pub NotationPreviewContent);

/// Everything the page renders, chrome included.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct NotationPreviewView {
    pub chrome: PublicChrome,
    pub content: NotationPreviewContent,
}

/// Read back the resolved content and the public chrome.
#[server]
pub async fn notation_preview_view() -> Result<NotationPreviewView, ServerFnError> {
    let content = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedNotationPreview>,
        _,
    >()
    .await
    .map(|axum::Extension(c)| c.0)
    .unwrap_or_default();
    Ok(NotationPreviewView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content,
    })
}

/// The page's route entry.
#[component]
pub fn NotationPreviewEntry() -> Element {
    let resource = use_server_future(notation_preview_view)?;
    let view = match &*resource.read() {
        Some(Ok(view)) => view.clone(),
        _ => return rsx! {},
    };
    rsx! {
        NotationPreviewPage { chrome: view.chrome, content: view.content }
    }
}

/// The pure preview page: the highlighted stage inside the public shell,
/// with an optional link to a form's blank government original. Prop-driven,
/// so it server-renders and unit-tests without a server future.
#[component]
pub fn NotationPreviewPage(chrome: PublicChrome, content: NotationPreviewContent) -> Element {
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
    let head_title = format!("{} | Notations | {}", chrome.brand_name, content.title);
    let description = format!("A preview of the Firm's {}.", content.title);
    rsx! {
        document::Title { "{head_title}" }
        document::Meta { name: "description", content: "{description}" }
        SocialMeta {
            title: head_title.clone(),
            description: description.clone(),
            site_name: chrome.brand_name.clone(),
            image: chrome.social_image.clone(),
        }
        document::Stylesheet { href: CATALOG_STYLESHEET_HREF }
        document::Stylesheet { href: HARVARD_OUTLINE_STYLESHEET_HREF }
        document::Script { src: HARVARD_OUTLINE_SCRIPT_HREF, defer: true }
        PublicShell { header, footer,
            article { class: "notation-preview",
                BackBreadcrumb { href: "/notations".to_string(), label: "Notations".to_string() }
                header { class: "notation-preview-header",
                    h1 { "{content.title}" }
                    div { class: "notation-preview-header__actions",
                        if let Some(origin_url) = &content.origin_url {
                            a { class: "nav-btn nav-btn--secondary", href: "{origin_url}",
                                "View the form"
                            }
                        }
                        a { class: "nav-btn nav-btn--secondary", href: "{content.source_href}",
                            "View on GitHub"
                        }
                    }
                }
                if !content.frontmatter.is_empty() {
                    Accordion { title: "Frontmatter".to_string(),
                        CodeBlock { code: content.frontmatter.clone(), lang: "yaml".to_string() }
                    }
                }
                Accordion { title: "Body".to_string(),
                    div { dangerous_inner_html: "{content.stage_html}" }
                }
                if !content.demo_questions.is_empty() {
                    Accordion { title: "Questionnaire".to_string(),
                        QuestionnaireDemo { questions: content.demo_questions.clone() }
                    }
                }
                if !content.demo_workflow.is_empty() {
                    Accordion { title: "Workflow".to_string(),
                        WorkflowDiagram { states: content.demo_workflow.clone() }
                    }
                }
            }
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

    fn letter_content() -> NotationPreviewContent {
        NotationPreviewContent {
            title: "Onboarding Letter".to_string(),
            source_href: "https://github.com/neon-law-source-code/navigator/blob/main/templates/notations/neon_law/shared/onboarding_letter.md".to_string(),
            frontmatter: "title: Onboarding Letter\ncode: onboarding__letter".to_string(),
            stage_html: "<article class=\"harvard-stage\" data-harvard-outline>\
                    <section class=\"harvard-unit harvard-unit--depth-1\" data-harvard-path=\"I\">\
                    <h2>Scope of the engagement</h2></section></article>"
                .to_string(),
            origin_url: None,
            demo_questions: Vec::new(),
            demo_workflow: Vec::new(),
        }
    }

    fn form_content() -> NotationPreviewContent {
        NotationPreviewContent {
            title: "Nevada LLC Formation".to_string(),
            source_href: "https://github.com/neon-law-source-code/navigator/blob/main/templates/notations/forms/united_states/nevada/state/nv__llc_formation.md".to_string(),
            frontmatter: "title: Nevada LLC Formation\ncode: nv__llc_formation".to_string(),
            stage_html: "<article class=\"harvard-stage\" data-harvard-outline>\
                    <section class=\"harvard-unit harvard-unit--depth-0\" data-harvard-path=\"\">\
                    <p>This engagement forms the company.</p></section></article>"
                .to_string(),
            origin_url: Some(
                "https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms"
                    .to_string(),
            ),
            demo_questions: Vec::new(),
            demo_workflow: Vec::new(),
        }
    }

    fn html() -> String {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            rsx! { NotationPreviewPage { chrome, content: letter_content() } }
        }
        ssr(app)
    }

    #[test]
    fn renders_the_highlighted_stage_for_the_requested_document() {
        let out = html();
        assert!(out.contains("Onboarding Letter"), "title: {out}");
        assert!(out.contains("data-harvard-outline"), "stage markup: {out}");
        assert!(
            out.contains("Scope of the engagement"),
            "stage content: {out}"
        );
    }

    #[test]
    fn links_to_the_raw_source_on_github_beside_the_title() {
        let out = html();
        assert!(
            out.contains(
                r#"href="https://github.com/neon-law-source-code/navigator/blob/main/templates/notations/neon_law/shared/onboarding_letter.md""#
            ),
            "github source link: {out}"
        );
        assert!(out.contains("View on GitHub"), "link label: {out}");
        assert!(
            out.contains(r#"class="notation-preview-header""#),
            "title and GitHub link share one header row: {out}"
        );
    }

    #[test]
    fn a_back_breadcrumb_returns_to_the_catalog() {
        let out = html();
        assert!(out.contains(r#"href="/notations""#), "back link: {out}");
        assert!(
            out.contains(r#"aria-label="Breadcrumb""#),
            "breadcrumb landmark: {out}"
        );
    }

    #[test]
    fn renders_the_frontmatter_as_a_code_block() {
        let out = html();
        assert!(out.contains("nav-code"), "code block wrapper: {out}");
        assert!(
            out.contains("onboarding__letter"),
            "frontmatter content: {out}"
        );
    }

    #[test]
    fn a_document_with_no_frontmatter_renders_no_code_block() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            let content = NotationPreviewContent {
                frontmatter: String::new(),
                ..letter_content()
            };
            rsx! { NotationPreviewPage { chrome, content } }
        }
        let out = ssr(app);
        assert!(!out.contains("nav-code"), "no code block: {out}");
    }

    #[test]
    fn wraps_the_page_in_the_public_shell_chrome() {
        let out = html();
        assert!(out.contains("site-header"), "header chrome: {out}");
        assert!(out.contains("site-footer__legal"), "footer chrome: {out}");
    }

    #[test]
    fn a_form_steps_through_its_own_body_and_links_the_blank_government_form() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            rsx! { NotationPreviewPage { chrome, content: form_content() } }
        }
        let out = ssr(app);
        assert!(out.contains("Nevada LLC Formation"), "title: {out}");
        assert!(
            out.contains("data-harvard-outline") && out.contains("This engagement forms"),
            "a form still steps through its own body: {out}"
        );
        assert!(
            out.contains(
                r#"href="https://www.nvsos.gov/businesses/commercial-recordings/forms-fees/all-business-forms""#
            ) && out.contains("View the form"),
            "link to the blank government form: {out}"
        );
        assert!(
            out.contains(r#"class="notation-preview-header__actions""#),
            "the form link and the GitHub link share one row: {out}"
        );
        // The frontmatter codeblock already names the questionnaire's
        // fields; a separate table would only duplicate it.
        assert!(
            !out.contains("notation-preview-cover-sheet"),
            "no separate fields table: {out}"
        );
    }

    #[test]
    fn a_form_with_no_origin_url_links_only_to_github() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            let content = NotationPreviewContent {
                origin_url: None,
                ..form_content()
            };
            rsx! { NotationPreviewPage { chrome, content } }
        }
        let out = ssr(app);
        assert_eq!(
            out.matches("nav-btn--secondary").count(),
            1,
            "only the GitHub link, no blank-form link: {out}"
        );
    }

    #[test]
    fn a_document_with_no_declared_questionnaire_shows_no_demo_section() {
        assert!(
            !html().contains("Try answering this"),
            "letter_content() carries no demo_questions: {}",
            html()
        );
    }

    #[test]
    fn a_document_with_a_declared_questionnaire_shows_the_demo_section() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            let content = NotationPreviewContent {
                demo_questions: vec![DemoQuestion {
                    code: "custom_single_choice__governing_law".to_string(),
                    answer_type: "custom_single_choice".to_string(),
                    prompt: "Which state's law governs this engagement?".to_string(),
                    choices: vec![("nevada".to_string(), "Nevada".to_string())],
                    interactive: true,
                }],
                ..letter_content()
            };
            rsx! { NotationPreviewPage { chrome, content } }
        }
        let out = ssr(app);
        assert!(out.contains("Try answering this"), "{out}");
        assert!(out.contains(r#"type="radio""#), "{out}");
    }

    #[test]
    fn frontmatter_and_body_each_render_as_their_own_collapsible_accordion() {
        let out = html();
        assert!(out.contains("<details"), "native disclosure: {out}");
        assert_eq!(
            out.matches("nav-accordion__toggle").count(),
            2,
            "frontmatter and body, each its own accordion, with no questionnaire or \
             workflow declared: {out}"
        );
    }

    #[test]
    fn a_document_with_no_declared_workflow_shows_no_workflow_section() {
        assert!(
            !html().contains("notation-workflow"),
            "letter_content() carries no demo_workflow: {}",
            html()
        );
    }

    #[test]
    fn a_document_with_a_declared_workflow_shows_the_workflow_accordion() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            let content = NotationPreviewContent {
                demo_workflow: vec![
                    WorkflowStateView {
                        name: "BEGIN".to_string(),
                        transitions: vec![("go".to_string(), "review".to_string())],
                    },
                    WorkflowStateView {
                        name: "review".to_string(),
                        transitions: vec![("done".to_string(), "END".to_string())],
                    },
                    WorkflowStateView {
                        name: "END".to_string(),
                        transitions: Vec::new(),
                    },
                ],
                ..letter_content()
            };
            rsx! { NotationPreviewPage { chrome, content } }
        }
        let out = ssr(app);
        assert!(out.contains("notation-workflow"), "{out}");
        assert!(out.contains("Workflow"), "accordion title: {out}");
        assert!(out.contains("review"), "task chip: {out}");
    }
}
