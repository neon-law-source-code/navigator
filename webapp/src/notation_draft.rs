//! `/notations/drafts/{id}` — a pushed notation **draft** (LAW-29).
//!
//! The other half of `navigator notation preview`: a template pushed to
//! the Project it belongs to, rendered by the real portal rather than a
//! second local imitation kept in step with it by hand. Reuses
//! [`crate::notation_preview::NotationPreviewContent`] — the same parsed
//! shape a published notation and a local lint both already render from —
//! but in a different order: **questionnaire first, document body below
//! it**. The respondent answers before there is a document to read; the
//! author previewing the intake is the only other reader, and they are
//! previewing the intake. Leading with the body, as the published page
//! does, is backwards for both.
//!
//! Nothing here is bound to a real questionnaire run: the demo controls are
//! the same client-side-only stage [`crate::notation_demo::QuestionnaireDemo`]
//! renders for a local preview — no Notation, no Answer, no workflow
//! instance (`store::notation_drafts::create` never touches those tables).
//! A draft is stored and addressable so the URL is real and shareable; it
//! is not run.
//!
//! No auth layer, same as [`crate::notation_preview::NotationPreviewEntry`]
//! — the draft's id is the access control, exactly like a preview link. An
//! expired or unknown id renders not-found.
//!
//! The `{id}` resolves to a live draft in the router's own awaited
//! middleware (`portal::dioxus_app::inject_notation_draft`), mirroring how
//! [`crate::notation_preview`] resolves `{slug}` from a compiled-in `Vec` —
//! not through `ServeConfig::context_providers` and a `consume_context`
//! read inside the render task, the pattern the authenticated,
//! `admin.rs`-routed pages use. A public page giving itself a second,
//! independent `context_providers` closure destabilized the Dioxus
//! render-task pool shared by every public page on this router — a
//! resource conflict, not a logic bug, that surfaced several renders later
//! as a `cucumber` runtime-drop panic with no connection to this file on
//! its face. [`project_draft_source`] is the one piece portal's middleware
//! needs from here; it is `pub` for exactly that call.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::{Accordion, CodeBlock, PublicShell};
use crate::harvard_outline::{HARVARD_OUTLINE_SCRIPT_HREF, HARVARD_OUTLINE_STYLESHEET_HREF};
use crate::notation_demo::QuestionnaireDemo;
use crate::notation_preview::NotationPreviewContent;
use crate::public_chrome::PublicChrome;

/// What `portal::dioxus_app::inject_notation_draft` injects for the render
/// task to read back — a distinct type (rather than a bare
/// `Option<NotationPreviewContent>`) so its *presence* on the request can
/// mean "this was the draft route", the same role
/// [`crate::notation_preview::InjectedNotationPreview`] plays for the
/// preview route. Both routes render through the same shared
/// [`crate::App`], and `content: None` here (an unknown or expired id) has
/// to stay distinguishable from this extension never having been inserted
/// at all (some other route entirely).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct InjectedNotationDraft(pub Option<NotationPreviewContent>);

/// The resolved draft, or `None` for an unknown or expired id. `matched`
/// is `true` exactly when the current request was the draft route at all;
/// see [`InjectedNotationDraft`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct NotationDraftView {
    pub chrome: PublicChrome,
    pub content: Option<NotationPreviewContent>,
    pub matched: bool,
}

/// Project a draft's raw Markdown source onto the same content shape a
/// published notation and a local lint both render from. Pure — the same
/// projection `portal::notation_preview_doc::from_markdown` runs, minus
/// the `source_href`/GitHub-link concern a draft has no use for. It lives
/// here, `pub`, rather than in `portal::notation_preview_doc`, because
/// `portal` depends on `webapp` and not the other way around, and
/// `portal`'s draft-resolving middleware needs to call it directly.
#[cfg(feature = "server")]
#[must_use]
pub fn project_draft_source(source: &str) -> NotationPreviewContent {
    let doc = views::harvard_outline::parse(source);
    let frontmatter = doc.frontmatter.clone().unwrap_or_default();
    let origin_url = views::harvard_outline::frontmatter_field(&frontmatter, "origin_url");
    let demo_questions = views::questionnaire_preview::parse(&frontmatter)
        .into_iter()
        .map(|q| {
            let interactive = q.is_interactive();
            crate::notation_demo::DemoQuestion {
                code: q.code,
                answer_type: q.answer_type,
                prompt: q.prompt,
                choices: q.choices,
                interactive,
            }
        })
        .collect();
    let demo_workflow = views::workflow_preview::parse(&frontmatter)
        .into_iter()
        .map(|s| crate::notation_workflow::WorkflowStateView {
            name: s.name,
            transitions: s.transitions.into_iter().map(|t| (t.event, t.to)).collect(),
        })
        .collect();
    NotationPreviewContent {
        title: doc.title.clone(),
        source_href: String::new(),
        demo_questions,
        demo_workflow,
        frontmatter,
        stage_html: views::harvard_outline::stage_html(&doc),
        origin_url,
    }
}

/// Read back the draft `portal::dioxus_app::inject_notation_draft` already
/// resolved for `{id}` — mirroring
/// [`crate::notation_preview::notation_preview_view`]'s synchronous
/// `Extension` read exactly, since that middleware's awaited store lookup
/// is what makes this one synchronous. `matched` is `false` (and `content`
/// `None`) on every request that was not the draft route at all — this
/// server fn runs unconditionally on every render through the shared
/// [`crate::App`], including a preview request, since Dioxus hooks run
/// every pass regardless of which branch's result [`crate::App`] uses.
#[server]
pub async fn notation_draft_view() -> Result<NotationDraftView, ServerFnError> {
    let injected = dioxus_fullstack_core::FullstackContext::extract::<
        axum::Extension<InjectedNotationDraft>,
        _,
    >()
    .await
    .ok();
    Ok(NotationDraftView {
        chrome: crate::public_chrome::firm_public_chrome_from_context().await,
        content: injected.as_ref().and_then(|axum::Extension(d)| d.0.clone()),
        matched: injected.is_some(),
    })
}

/// The pure draft page: questionnaire first, rendered body below it.
/// Prop-driven, so it server-renders and unit-tests without a server
/// future.
#[component]
pub fn NotationDraftPage(chrome: PublicChrome, content: Option<NotationPreviewContent>) -> Element {
    let Some(content) = content else {
        return rsx! {
            PublicShell {
                header: rsx! {},
                footer: rsx! {},
                document::Title { "{chrome.brand_name} | Draft not found" }
                article { class: "notation-draft notation-draft--missing",
                    h1 { "Draft not found" }
                    p { "This draft does not exist, or it has expired." }
                }
            }
        };
    };
    #[cfg(not(target_arch = "wasm32"))]
    let title = {
        let head_title = format!("{} | Draft | {}", chrome.brand_name, content.title);
        rsx! { document::Title { "{head_title}" } }
    };
    #[cfg(target_arch = "wasm32")]
    let title = rsx! {};
    rsx! {
        PublicShell { header: rsx! {}, footer: rsx! {},
            {title}
            document::Stylesheet { href: HARVARD_OUTLINE_STYLESHEET_HREF }
            document::Script { src: HARVARD_OUTLINE_SCRIPT_HREF, defer: true }
            article { class: "notation-draft",
                header { class: "notation-draft__header",
                    p { class: "notation-draft__badge", "Draft — not run" }
                    h1 { "{content.title}" }
                    p { class: "notation-draft__notice",
                        "This is a preview. No notation was created, no workflow started, \
                         and nothing was filed."
                    }
                }
                if !content.demo_questions.is_empty() {
                    section { class: "notation-draft__questionnaire",
                        h2 { "Questionnaire" }
                        QuestionnaireDemo { questions: content.demo_questions.clone() }
                    }
                }
                section { class: "notation-draft__body",
                    h2 { "Document" }
                    div { dangerous_inner_html: "{content.stage_html}" }
                }
                if !content.frontmatter.is_empty() {
                    Accordion { title: "Frontmatter".to_string(),
                        CodeBlock { code: content.frontmatter.clone(), lang: "yaml".to_string() }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notation_demo::DemoQuestion;
    use crate::notation_workflow::WorkflowStateView;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn draft_content() -> NotationPreviewContent {
        NotationPreviewContent {
            title: "Engagement Letter".to_string(),
            source_href: String::new(),
            frontmatter: "title: Engagement Letter\ncode: engagement__letter".to_string(),
            stage_html: "<article class=\"harvard-stage\" data-harvard-outline>\
                    <section class=\"harvard-unit harvard-unit--depth-1\" data-harvard-path=\"I\">\
                    <h2>Scope of the engagement</h2></section></article>"
                .to_string(),
            origin_url: None,
            demo_questions: vec![DemoQuestion {
                code: "custom_text__client_name".to_string(),
                answer_type: "custom_text".to_string(),
                prompt: "What is your name?".to_string(),
                choices: Vec::new(),
                interactive: true,
            }],
            demo_workflow: vec![WorkflowStateView {
                name: "BEGIN".to_string(),
                transitions: vec![("intake_submitted".to_string(), "lawyer_review".to_string())],
            }],
        }
    }

    /// LAW-29's ordering fix: the questionnaire renders before the body,
    /// not after — the respondent answers before there is a document to
    /// read, and the author previewing intake is the other reader.
    #[test]
    fn the_questionnaire_renders_before_the_document_body() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            rsx! { NotationDraftPage { chrome, content: Some(draft_content()) } }
        }
        let out = ssr(app);
        let questionnaire_at = out
            .find("Try answering this")
            .expect("questionnaire renders");
        let body_at = out
            .find("Scope of the engagement")
            .expect("document body renders");
        assert!(
            questionnaire_at < body_at,
            "questionnaire must render before the body: {out}"
        );
    }

    /// The page states plainly that nothing was created or run — it must
    /// never read like an executed instrument.
    #[test]
    fn the_page_states_it_is_a_draft_that_was_never_run() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            rsx! { NotationDraftPage { chrome, content: Some(draft_content()) } }
        }
        let out = ssr(app);
        assert!(out.contains("Draft"), "{out}");
        assert!(
            out.contains("no notation was created") || out.contains("no workflow started"),
            "{out}"
        );
    }

    #[test]
    fn an_unknown_or_expired_draft_renders_not_found() {
        fn app() -> Element {
            let chrome = PublicChrome {
                brand_name: "Neon Law".to_string(),
                ..PublicChrome::default()
            };
            rsx! { NotationDraftPage { chrome, content: None } }
        }
        let out = ssr(app);
        assert!(out.contains("Draft not found"), "{out}");
    }
}
