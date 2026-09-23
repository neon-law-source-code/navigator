//! Notation renderer — fills in a template body with a context map.
//!
//! A template body is plain text with three placeholder grammars, all
//! evaluated by [`fill`] (the shared evaluator this render path and the
//! form-fill path both meet):
//!
//! - **Bare or dotted** — `{{code}}`, `{{type__role}}`, and
//!   `{{type__role.field}}` substitute the context value for that key.
//! - **Conditional** — `{{#if custom_yes_no__approved}} … {{/if}}` includes
//!   a clause only while the answer is truthy. `{{#if state=value}}` matches
//!   one exact answer.
//! - **Iterator** — `{{#for x in people__members}} … {{x.name}} … {{/for}}`
//!   walks an aggregate answer (a JSON array stored under the state key) and
//!   renders the inner block once per row, resolving `{{x.part}}` against
//!   that row.
//! - **Dotted `row.part`** — inside a loop, `{{x.part}}` reads a field off
//!   the current row (the same `row`/`part` access `forms::resolve_reauthored`
//!   does).
//!
//! Unfilled placeholders are *not* an error; rendering a partly-filled
//! notation is a valid intermediate state. Tests use the "no `{{` left in
//! the output" assertion to detect missing keys.

use std::collections::BTreeMap;

/// Evaluate `body` against `context` — expand `{{#for …}}` iterators over
/// aggregate answers, then substitute every remaining `{{key}}`. The
/// pure string half of [`render_filled_in`], shared so the render path
/// gains the iteration + dotted `row.part` capability of the form-fill path.
#[must_use]
pub fn fill(body: &str, context: &BTreeMap<String, String>) -> String {
    forms::notation::fill(body, context)
}

/// Evaluate conditions against `context` while substituting direct values
/// from `display`. Preview controls use stored choice values for conditions
/// and human-readable choice labels in the document.
#[must_use]
pub fn fill_with_display(
    body: &str,
    context: &BTreeMap<String, String>,
    display: &BTreeMap<String, String>,
) -> String {
    forms::notation::fill_with_display(body, context, display)
}

/// Render `body` with `context` evaluated into it (bare substitution plus
/// `{{#for …}}` iteration — see [`fill`]). The result is wrapped in an
/// `<article class="notation">` container with one `<p>` per paragraph
/// (blank-line-separated).
#[must_use]
pub fn render_filled_in(body: &str, context: &BTreeMap<String, String>) -> String {
    let filled = fill(body, context);
    let paragraphs: Vec<&str> = filled
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let mut html = String::from("<article class=\"notation\">");
    for paragraph in paragraphs {
        html.push_str("<p>");
        html.push_str(&html_escape(&collapse_whitespace(paragraph)));
        html.push_str("</p>");
    }
    html.push_str("</article>");
    html
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Replace any run of whitespace (spaces, newlines, tabs) with a
/// single space so a multi-line paragraph in the template renders
/// as one flowing paragraph in HTML.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::render_filled_in;
    use rules::Rule;
    use std::path::PathBuf;

    fn ctx(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn substitutes_single_placeholder() {
        let html = render_filled_in("Hello {{client_name}}.", &ctx(&[("client_name", "Libra")]));
        assert!(html.contains("Hello Libra."), "got: {html}");
        assert!(!html.contains("{{"));
    }

    #[test]
    fn substitutes_a_dotted_placeholder() {
        let filled = super::fill(
            "Client: {{person__client.name}} <{{person__client.email}}>.",
            &ctx(&[
                ("person__client.name", "Libra Prime"),
                ("person__client.email", "libra@example.com"),
            ]),
        );
        assert_eq!(filled, "Client: Libra Prime <libra@example.com>.");
    }

    #[test]
    fn substitutes_multiple_placeholders_across_paragraphs() {
        let body = "\
I, {{client_name}}, hire the firm for {{product_description}}.

The retainer covers the project {{project_name}}.";
        let html = render_filled_in(
            body,
            &ctx(&[
                ("client_name", "Libra"),
                ("product_description", "estate planning"),
                ("project_name", "Estate planning — Libra"),
            ]),
        );
        assert!(html.contains("I, Libra, hire the firm for estate planning."));
        assert!(html.contains("The retainer covers the project Estate planning — Libra."));
        // Two paragraphs → two <p> tags.
        let p_count = html.matches("<p>").count();
        assert_eq!(p_count, 2, "two paragraphs expected, html: {html}");
    }

    #[test]
    fn collapses_intra_paragraph_newlines_into_single_spaces() {
        let body = "I,\n{{client_name}},\nhire the firm.";
        let html = render_filled_in(body, &ctx(&[("client_name", "Libra")]));
        assert!(html.contains("I, Libra, hire the firm."));
    }

    #[test]
    fn leaves_unfilled_placeholders_in_output_for_grep() {
        let html = render_filled_in("Hi {{missing}}.", &ctx(&[]));
        assert!(html.contains("{{missing}}"));
    }

    #[test]
    fn empty_body_renders_empty_article() {
        let html = render_filled_in("", &ctx(&[]));
        assert!(html.contains("<article class=\"notation\">"));
        assert!(!html.contains("<p>"));
    }

    #[test]
    fn wraps_in_notation_article_class() {
        let html = render_filled_in("body", &ctx(&[]));
        assert!(html.contains("<article class=\"notation\">"), "got: {html}");
    }

    #[test]
    fn escapes_filled_template_text() {
        let html = render_filled_in(
            "Hello {{client_name}}.",
            &ctx(&[("client_name", "<Libra & Co.>")]),
        );
        assert!(
            html.contains("Hello &lt;Libra &amp; Co.&gt;."),
            "got: {html}"
        );
    }

    #[test]
    fn for_loop_iterates_an_aggregate_answer_with_dotted_row_part() {
        let body = "Members: {{#for m in people__members}}{{m.name}} of {{m.city}}; {{/for}}done.";
        let context = ctx(&[(
            "people__members",
            r#"[{"name": "Aries", "city": "Las Vegas"}, {"name": "Libra", "city": "Reno"}]"#,
        )]);
        let filled = super::fill(body, &context);
        assert_eq!(
            filled, "Members: Aries of Las Vegas; Libra of Reno; done.",
            "got: {filled}"
        );
    }

    #[test]
    fn for_loop_over_a_missing_aggregate_renders_nothing() {
        let filled = super::fill(
            "[{{#for m in people__members}}{{m.name}}{{/for}}]",
            &ctx(&[]),
        );
        assert_eq!(filled, "[]");
    }

    #[test]
    fn nested_for_loops_match_balanced_closes() {
        // The inner `{{/for}}` must not terminate the outer loop.
        let body =
            "{{#for g in groups}}[{{g.title}}: {{#for m in groups}}{{m.title}} {{/for}}]{{/for}}";
        let context = ctx(&[("groups", r#"[{"title": "A"}, {"title": "B"}]"#)]);
        let filled = super::fill(body, &context);
        assert_eq!(filled, "[A: A B ][B: A B ]", "got: {filled}");
    }

    #[test]
    fn for_loop_body_with_non_ascii_does_not_panic() {
        // A loop body carrying non-ASCII bytes (em-dash, curly quote,
        // accented letter — routine in legal copy) once panicked the
        // byte-at-a-time close scan mid-codepoint. It must render.
        let body = "{{#for m in people__members}}— {{m.name}} “résumé” … {{/for}}";
        let context = ctx(&[(
            "people__members",
            r#"[{"name": "Aríes"}, {"name": "Libra"}]"#,
        )]);
        let filled = super::fill(body, &context);
        assert_eq!(filled, "— Aríes “résumé” … — Libra “résumé” … ");
    }

    #[test]
    fn nested_for_loops_with_non_ascii_match_balanced_closes() {
        // The mid-codepoint hazard also has to stay safe when a non-ASCII
        // byte sits between a nested open and the balancing close.
        let body = "{{#for g in groups}}«{{#for m in groups}}{{m.title}}·{{/for}}»{{/for}}";
        let context = ctx(&[("groups", r#"[{"title": "Á"}, {"title": "B"}]"#)]);
        let filled = super::fill(body, &context);
        assert_eq!(filled, "«Á·B·»«Á·B·»", "got: {filled}");
    }

    #[test]
    fn bare_and_loop_placeholders_compose() {
        let body = "{{title}}: {{#for m in people__members}}{{m.name}} {{/for}}";
        let context = ctx(&[
            ("title", "Roster"),
            ("people__members", r#"[{"name": "Aries"}]"#),
        ]);
        assert_eq!(super::fill(body, &context), "Roster: Aries ");
    }

    #[test]
    fn conditional_clauses_follow_truthy_and_exact_answers() {
        let body = "{{#if custom_yes_no__approved}}Approved.{{/if}}{{#if custom_single_choice__law=nevada}} Nevada.{{/if}}";
        assert_eq!(super::fill(body, &ctx(&[])), "");
        assert_eq!(
            super::fill(
                body,
                &ctx(&[
                    ("custom_yes_no__approved", "true"),
                    ("custom_single_choice__law", "nevada"),
                ]),
            ),
            "Approved. Nevada."
        );
        assert_eq!(
            super::fill(
                body,
                &ctx(&[
                    ("custom_yes_no__approved", "false"),
                    ("custom_single_choice__law", "california"),
                ]),
            ),
            ""
        );
    }

    #[test]
    fn nested_conditionals_match_balanced_closes() {
        let body = "{{#if outer}}A{{#if inner}}B{{/if}}C{{/if}}";
        assert_eq!(
            super::fill(body, &ctx(&[("outer", "yes"), ("inner", "yes")])),
            "ABC"
        );
        assert_eq!(super::fill(body, &ctx(&[("outer", "yes")])), "AC");
    }

    #[test]
    fn n115_valid_body_uses_the_shared_evaluator() {
        let body = "Client: {{person__client.name}}\n\
Members:\n{{#for m in people__members}}- {{m.name}} from {{m.city}}\n{{/for}}";
        let source = rules::SourceFile {
            path: PathBuf::from("test.md"),
            contents: format!(
                "---\nquestionnaire:\n  BEGIN:\n    _: person__client\n  \
person__client:\n    _: people__members\n  people__members:\n    _: END\n  END: {{}}\n---\n\n{body}\n"
            ),
        };
        assert!(
            rules::F115PathResolution.lint(&source).is_empty(),
            "fixture must be accepted by N115"
        );

        let filled = super::fill(
            body,
            &ctx(&[
                ("person__client.name", "Libra Prime"),
                (
                    "people__members",
                    r#"[{"name":"Aries","city":"Las Vegas"},{"name":"Virgo","city":"Reno"}]"#,
                ),
            ]),
        );

        assert!(filled.contains("Client: Libra Prime"));
        assert!(filled.contains("- Aries from Las Vegas"));
        assert!(filled.contains("- Virgo from Reno"));
        assert!(
            !filled.contains("{{"),
            "all N115-valid data grammar should render: {filled}"
        );
    }
}
