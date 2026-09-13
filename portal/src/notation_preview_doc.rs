//! Build one `/notations/{slug}` show-page document from a template's own
//! Markdown — the content [`crate::dioxus_app::notation_preview_router`]
//! serves.
//!
//! Two callers assemble the same thing from different sources and must not
//! drift: the firm's public site, which compiles the bundled notations in
//! with `include_str!`, and `navigator notations preview`, which reads one
//! file off disk in whatever repository the author is standing in. A page
//! that renders differently depending on which of those produced it would
//! make the preview useless as a check on the published page, so the
//! projection lives here once and both call it.
//!
//! Everything is read from the template itself. The body becomes the
//! paragraph-stepping stage [`views::harvard_outline`] builds for every
//! notation; the declared `questionnaire:` becomes the "Try answering this"
//! demo's ordered steps; the declared `workflow:` becomes the sample-run
//! diagram's graph. All three readers are pure functions over the file — no
//! store, no runtime, no Notation — which is what lets a template be
//! previewed before it has ever been imported, let alone bound to a matter.

use webapp::notation_demo::DemoQuestion;
use webapp::notation_preview::PreviewDoc;
use webapp::notation_workflow::WorkflowStateView;

/// The template's declared questionnaire, in order, ready for the "Try
/// answering this" demo — parsed with no live Notation and no domain-crate
/// dependency (see `views::questionnaire_preview`'s own doc comment for why
/// that reader exists separately from `workflows::notation_session`).
#[must_use]
pub fn demo_questions(frontmatter: &str) -> Vec<DemoQuestion> {
    views::questionnaire_preview::parse(frontmatter)
        .into_iter()
        .map(|q| {
            let interactive = q.is_interactive();
            DemoQuestion {
                code: q.code,
                answer_type: q.answer_type,
                prompt: q.prompt,
                choices: q.choices,
                interactive,
            }
        })
        .collect()
}

/// The template's declared `workflow:` state machine, ready for the sample
/// "Workflow" runs — parsed with no live Restate invocation and no
/// domain-crate dependency (see `views::workflow_preview`).
#[must_use]
pub fn demo_workflow(frontmatter: &str) -> Vec<WorkflowStateView> {
    views::workflow_preview::parse(frontmatter)
        .into_iter()
        .map(|s| WorkflowStateView {
            name: s.name,
            transitions: s.transitions.into_iter().map(|t| (t.event, t.to)).collect(),
        })
        .collect()
}

/// Project one notation's Markdown onto its show page.
///
/// `slug` is the `{slug}` path segment the router matches, and `source_href`
/// is where "View source" points — a GitHub blob URL for a bundled notation,
/// the local path for a file being previewed. `origin_url` comes from the
/// template's own frontmatter, so a government form links to the government's
/// blank and a letter, which declares none, links nowhere.
#[must_use]
pub fn from_markdown(slug: &str, source_href: &str, src: &str) -> PreviewDoc {
    let doc = views::harvard_outline::parse(src);
    let frontmatter = doc.frontmatter.clone().unwrap_or_default();
    let origin_url = views::harvard_outline::frontmatter_field(&frontmatter, "origin_url");
    PreviewDoc {
        slug: slug.to_string(),
        title: doc.title.clone(),
        source_href: source_href.to_string(),
        demo_questions: demo_questions(&frontmatter),
        demo_workflow: demo_workflow(&frontmatter),
        frontmatter,
        stage_html: views::harvard_outline::stage_html(&doc),
        origin_url,
    }
}

#[cfg(test)]
mod tests {
    use super::from_markdown;

    /// A template with both blocks projects onto a page carrying both
    /// sections: the questionnaire's declared order and the workflow's
    /// declared graph, each read from this file alone.
    #[test]
    fn reads_both_declared_blocks_out_of_one_template() {
        let src = "---\ntitle: Sample Letter\ncode: sample__letter\nquestionnaire:\n  BEGIN:\n    \
                   _: custom_text__client_name\n  custom_text__client_name:\n    _: END\n  END: \
                   {}\ncustom_questions:\n  client_name:\n    prompt: What is your \
                   name?\nworkflow:\n  BEGIN:\n    intake_submitted: \
                   lawyer_review\n  lawyer_review:\n    approved: END\n  END: {}\n---\n\n# Sample \
                   Letter\n\nBody prose.\n";

        let doc = from_markdown("sample-letter", "/tmp/sample.md", src);

        assert_eq!(doc.slug, "sample-letter");
        assert_eq!(doc.source_href, "/tmp/sample.md");
        assert_eq!(
            doc.demo_questions
                .iter()
                .map(|q| q.code.as_str())
                .collect::<Vec<_>>(),
            vec!["custom_text__client_name"],
            "the demo walks the template's own declared question order"
        );
        assert_eq!(doc.demo_questions[0].prompt, "What is your name?");
        assert!(
            doc.demo_workflow.iter().any(|s| s.name == "lawyer_review"),
            "the diagram carries every reachable declared workflow state"
        );
        assert!(doc.frontmatter.contains("code: sample__letter"));
        assert!(
            !doc.stage_html.is_empty(),
            "every notation body has a stage"
        );
        assert!(
            doc.origin_url.is_none(),
            "a letter declares no government blank to link to"
        );
    }

    /// A form's `origin_url` reaches the page, and a template with neither
    /// declared block renders neither section rather than an empty one.
    #[test]
    fn carries_origin_url_and_omits_undeclared_sections() {
        let src = "---\ntitle: Some Form\ncode: us__some_form\norigin_url: \
                   https://example.gov/f.pdf\n---\n\n# Some Form\n\nCover prose.\n";

        let doc = from_markdown("some-form", "blob://x", src);

        assert_eq!(doc.origin_url.as_deref(), Some("https://example.gov/f.pdf"));
        assert!(doc.demo_questions.is_empty());
        assert!(doc.demo_workflow.is_empty());
    }
}
