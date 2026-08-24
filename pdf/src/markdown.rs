//! Convert a notation template's Markdown body into Typst markup.
//!
//! Notation templates are authored in **Markdown** (`##` headings,
//! `**bold**`, `-` lists) — that is what the `rules` validator checks.
//! The [`crate::render`] pipeline, however, compiles **Typst**, whose
//! markup is close but not identical: emphasis is `*x*` not `**x**`,
//! headings are `=` not `#`, and a stray `#` or `$` in prose is a
//! function call or math delimiter. Feeding raw Markdown to Typst
//! therefore renders wrong or fails to compile.
//!
//! [`to_typst`] walks the [`pulldown_cmark`] event stream and emits the
//! equivalent Typst markup, escaping every character Typst would
//! otherwise treat as syntax. It covers the constructs that appear in
//! notation bodies (headings, paragraphs, strong/emphasis, ordered and
//! unordered lists, block quotes, inline code, links, tables, horizontal
//! rules); raw HTML is dropped rather than leaked as literal tags, with a
//! single exception — an HTML comment `<!-- pagebreak -->` maps to a Typst
//! `#pagebreak()`, so a body can force the next content (e.g. a signature
//! block) onto a fresh page.
//!
//! ## Answered and unanswered
//!
//! The caller substitutes `{{placeholder}}` tokens before conversion, and
//! this module marks both outcomes so a reader can tell them apart at a
//! glance. An **answered** value the caller wrapped with [`bold_answer`]
//! renders bold, so every fact particular to a matter stands out from the
//! boilerplate. An **unanswered** token — one still present at render time
//! — survives verbatim, wrapped in a yellow highlight, so an unfinished
//! document is unmistakably unfinished rather than looking like prose.

use pulldown_cmark::{Alignment, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Convert a Markdown `body` to Typst markup suitable for
/// [`crate::render`].
///
/// The output is fragment markup (no page setup or font rule) — the
/// caller wraps it in an [`crate::OutputFormat`] chrome preamble.
#[must_use]
pub fn to_typst(body: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(body, opts);

    let mut out = String::with_capacity(body.len() + body.len() / 8);
    // Ordered-list counters, one per nesting level. `None` marks an
    // unordered list at that depth.
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    // How many bracketed Typst content containers (block quote, table
    // cell, link, strikethrough) are currently open. A `#pagebreak()` is
    // illegal inside one, so the sentinel is honored only at depth 0.
    let mut container_depth: usize = 0;

    for event in parser {
        match event {
            Event::Start(tag) => {
                if opens_container(&tag) {
                    container_depth += 1;
                }
                start_tag(&mut out, &tag, &mut list_stack);
            }
            Event::End(tag) => {
                if closes_container(tag) {
                    container_depth = container_depth.saturating_sub(1);
                }
                end_tag(&mut out, tag, &mut list_stack);
            }
            Event::Text(text) => out.push_str(&escape_text_marking_placeholders(&text)),
            Event::Code(code) => {
                out.push_str("#raw(");
                out.push_str(&typst_string(&code));
                out.push(')');
            }
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push_str(" \\\n"),
            Event::Rule => out.push_str("\n#line(length: 100%)\n\n"),
            // The one HTML token honored: a top-level `<!-- pagebreak -->`
            // comment forces a Typst page break (used to start a signature
            // block on its own page). Inside a container (table cell, block
            // quote, …) a `#pagebreak()` is illegal and would fail the whole
            // render, so there the sentinel is dropped like any other HTML.
            // Every other raw/inline HTML token — plus footnotes, math, and
            // task markers — is dropped rather than leaked as literal tags.
            Event::Html(html) | Event::InlineHtml(html)
                if container_depth == 0 && is_pagebreak_comment(&html) =>
            {
                out.push_str("\n#pagebreak()\n\n");
            }
            _ => {}
        }
    }
    // Collapse any run of 3+ newlines the structure handlers produced
    // into the canonical paragraph break.
    normalize_blank_lines(&out)
}

fn start_tag(out: &mut String, tag: &Tag, list_stack: &mut Vec<Option<u64>>) {
    match tag {
        Tag::Heading { level, .. } => {
            out.push('\n');
            for _ in 0..heading_depth(*level) {
                out.push('=');
            }
            out.push(' ');
        }
        Tag::Strong => out.push('*'),
        Tag::Emphasis => out.push('_'),
        Tag::Strikethrough => out.push_str("#strike["),
        Tag::List(first) => list_stack.push(*first),
        Tag::Item => {
            // Indent nested items two spaces per level below the top.
            let depth = list_stack.len().saturating_sub(1);
            for _ in 0..depth {
                out.push_str("  ");
            }
            match list_stack.last_mut() {
                Some(Some(n)) => {
                    out.push_str(&n.to_string());
                    out.push_str(". ");
                    *n += 1;
                }
                _ => out.push_str("- "),
            }
        }
        Tag::BlockQuote(_) => out.push_str("#quote(block: true)[\n"),
        Tag::Link { dest_url, .. } => {
            out.push_str("#link(");
            out.push_str(&typst_string(dest_url));
            out.push_str(")[");
        }
        // A Markdown table becomes a centered Typst `#table(..)`: one
        // column per alignment slot, centered by default unless the
        // author set per-column alignment, then a flat cell stream Typst
        // lays out row by row. Header cells are wrapped in
        // `table.header(..)` so Typst treats them as the header row
        // (repeated when the table breaks across pages); each cell's
        // inline markup (strong, links, escaped currency) is emitted by
        // the same handlers as prose.
        Tag::Table(alignments) => {
            out.push_str("\n#align(center)[#table(\n  columns: ");
            out.push_str(&alignments.len().to_string());
            out.push_str(",\n");
            if alignments.iter().all(|a| *a == Alignment::None) {
                out.push_str("  align: center,\n");
            } else {
                out.push_str("  align: (");
                for (i, alignment) in alignments.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(typst_align(*alignment));
                }
                out.push_str("),\n");
            }
        }
        Tag::TableHead => out.push_str("  table.header("),
        Tag::TableRow => out.push_str("  "),
        Tag::TableCell => out.push('['),
        // Headings/paragraphs inside other blocks and unhandled tags
        // contribute their text via Text events; no wrapper needed.
        _ => {}
    }
}

/// Map a Markdown column alignment to the Typst `align` keyword. A
/// column with no explicit alignment (`None`) uses Navigator's default
/// centered table-cell alignment.
fn typst_align(alignment: Alignment) -> &'static str {
    match alignment {
        Alignment::None | Alignment::Center => "center",
        Alignment::Left => "left",
        Alignment::Right => "right",
    }
}

fn end_tag(out: &mut String, tag: TagEnd, list_stack: &mut Vec<Option<u64>>) {
    match tag {
        TagEnd::Heading(_) | TagEnd::Paragraph => out.push_str("\n\n"),
        TagEnd::Strong => out.push('*'),
        TagEnd::Emphasis => out.push('_'),
        TagEnd::Strikethrough | TagEnd::Link => out.push(']'),
        TagEnd::List(_) => {
            list_stack.pop();
            if list_stack.is_empty() {
                out.push('\n');
            }
        }
        TagEnd::Item | TagEnd::TableRow => out.push('\n'),
        TagEnd::BlockQuote(_) => out.push_str("]\n\n"),
        TagEnd::Table => out.push_str(")]\n\n"),
        TagEnd::TableHead => out.push_str("),\n"),
        TagEnd::TableCell => out.push_str("], "),
        _ => {}
    }
}

/// Typst supports six heading levels; clamp deeper Markdown headings to
/// the deepest Typst level rather than emitting an over-long `=` run.
fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Escape the characters Typst treats as markup syntax so prose text
/// renders verbatim. The set is the markup sigils that can fire
/// mid-line: a function/label call (`#`, `<`), math (`$`), emphasis
/// (`*`, `_`), raw (`` ` ``), reference (`@`), content brackets
/// (`[`, `]`), and the escape char itself (`\`).
/// Wrap a questionnaire answer so it renders **bold** where it is
/// substituted into a template body.
///
/// The two halves of the same idea: an *unanswered* `{{placeholder}}` gets
/// the yellow wash from [`escape_text_marking_placeholders`], and an
/// *answered* one gets bold. A reader skimming a retainer can then find
/// every fact particular to their matter — names, dates, the fee, the
/// scope — without reading the boilerplate around them.
///
/// The caller substitutes this into the Markdown body, so the value is
/// Markdown-escaped first: a name or a scope sentence carrying `*` or `_`
/// would otherwise unbalance the strong run and swallow the rest of the
/// paragraph. A blank value is returned untouched, because `****` is not
/// emphasis in Markdown — it is four asterisks on the page.
#[must_use]
pub fn bold_answer(value: &str) -> String {
    if value.trim().is_empty() {
        return value.to_string();
    }
    let mut escaped = String::with_capacity(value.len() + 4);
    escaped.push_str("**");
    for c in value.chars() {
        if matches!(c, '*' | '_' | '[' | ']' | '`' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped.push_str("**");
    escaped
}

/// The wash behind an unanswered `{{placeholder}}`. A soft highlighter
/// yellow: loud enough that nobody signs or sends a document with a blank
/// still in it, light enough that the token underneath stays legible in
/// print and in grayscale.
const PLACEHOLDER_HIGHLIGHT: &str = "#fff3a3";

/// Escape `s` for Typst, wrapping any surviving `{{placeholder}}` in a
/// yellow highlight.
///
/// A token still present at render time is one the questionnaire never
/// answered. Rendering it as ordinary prose makes a blank look like
/// intentional text — the exact failure that puts `{{person__client.name}}`
/// in front of a client. Marking it makes an unfinished document
/// unmistakably unfinished at a glance, while leaving it readable so an
/// attorney can see *which* answer is missing.
///
/// Only a whole `{{…}}` run on one text event is marked; a token broken
/// across events falls through to plain escaping rather than emitting
/// unbalanced markup.
fn escape_text_marking_placeholders(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(open) = rest.find("{{") {
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("}}") else {
            break;
        };
        out.push_str(&escape_text(&rest[..open]));
        out.push_str("#highlight(fill: rgb(\"");
        out.push_str(PLACEHOLDER_HIGHLIGHT);
        out.push_str("\"))[");
        out.push_str(&escape_text(&rest[open..open + 2 + close + 2]));
        out.push(']');
        rest = &after_open[close + 2..];
    }
    out.push_str(&escape_text(rest));
    out
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '@' | '[' | ']'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether `tag` opens a bracketed Typst content container — one whose
/// markup wraps its children in `[...]` (block quote, table cell, link,
/// strikethrough). A `#pagebreak()` emitted inside one is a Typst error
/// ("pagebreaks are not allowed inside of containers"), so [`to_typst`]
/// suppresses the pagebreak sentinel while any such container is open.
fn opens_container(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::BlockQuote(_) | Tag::TableCell | Tag::Link { .. } | Tag::Strikethrough
    )
}

/// The [`opens_container`] counterpart for a closing tag.
fn closes_container(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::BlockQuote(_) | TagEnd::TableCell | TagEnd::Link | TagEnd::Strikethrough
    )
}

/// Recognize the one HTML token [`to_typst`] honors: an HTML comment whose
/// body is `pagebreak` (case-insensitive, surrounding whitespace ignored),
/// e.g. `<!-- pagebreak -->`. It is invisible in the on-screen HTML preview
/// and maps to a Typst `#pagebreak()` in the PDF, letting a template force a
/// hard page break — for example, to start a signature block on a fresh page.
fn is_pagebreak_comment(html: &str) -> bool {
    let Some(inner) = html
        .trim()
        .strip_prefix("<!--")
        .and_then(|s| s.strip_suffix("-->"))
    else {
        return false;
    };
    inner.trim().eq_ignore_ascii_case("pagebreak")
}

/// Render `s` as a Typst double-quoted string literal, escaping the two
/// characters significant inside one. Used for `#raw(..)` / `#link(..)`
/// arguments, where the content is a string expression, not markup.
fn typst_string(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Squeeze runs of 3+ newlines down to exactly two (one blank line),
/// and trim leading/trailing whitespace, so the emitted Typst has
/// stable paragraph spacing regardless of how the handlers stacked
/// their `\n`s.
fn normalize_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newline_run = 0usize;
    for c in s.chars() {
        if c == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(c);
            }
        } else {
            newline_run = 0;
            out.push(c);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::to_typst;

    #[test]
    fn headings_become_equals_runs() {
        assert_eq!(to_typst("# Title"), "= Title");
        assert_eq!(to_typst("## Section"), "== Section");
        assert_eq!(to_typst("### Sub"), "=== Sub");
    }

    #[test]
    fn strong_collapses_to_single_asterisks() {
        // The crux: Markdown `**x**` must NOT survive as `**x**`, which
        // renders un-bolded in Typst.
        assert_eq!(to_typst("**bold**"), "*bold*");
    }

    #[test]
    fn emphasis_becomes_underscores() {
        assert_eq!(to_typst("*italic*"), "_italic_");
        assert_eq!(to_typst("_italic_"), "_italic_");
    }

    #[test]
    fn unordered_list_uses_typst_dash_markers() {
        let out = to_typst("- one\n- two\n");
        assert_eq!(out, "- one\n- two");
    }

    #[test]
    fn ordered_list_numbers_explicitly() {
        let out = to_typst("1. first\n2. second\n");
        assert_eq!(out, "1. first\n2. second");
    }

    #[test]
    fn inline_code_becomes_raw_call_not_backticks() {
        // A Typst backtick run would need matched delimiters; `#raw(..)`
        // is unambiguous and escapes nothing in the prose stream.
        assert_eq!(to_typst("`code`"), "#raw(\"code\")");
    }

    #[test]
    fn placeholder_tokens_pass_through_verbatim() {
        // `{{name}}` carries no Typst meaning; it must survive so the
        // caller can substitute it (before or after conversion).
        assert_eq!(to_typst("Hello `{{name}}`"), "Hello #raw(\"{{name}}\")");
        // The at-risk path is a *bare* token in prose, not backtick-wrapped:
        // `{` / `}` are deliberately absent from `escape_text` because Typst
        // markup treats them as literal characters (code is `#{..}`). So an
        // unfilled token must reach the page verbatim — escaping it to
        // `\{\{name\}\}` would corrupt the passthrough guarantee. It is
        // wrapped in the unanswered-placeholder highlight, but the token
        // inside is byte-for-byte what the caller substitutes on.
        assert_eq!(
            to_typst("Hello {{name}}"),
            "Hello #highlight(fill: rgb(\"#fff3a3\"))[{{name}}]"
        );
        // Even an own-line token and a dotted token (both present in real
        // templates) survive unescaped and compile.
        assert_eq!(
            to_typst("{{custom_clauses}}"),
            "#highlight(fill: rgb(\"#fff3a3\"))[{{custom\\_clauses}}]"
        );
        assert_eq!(
            to_typst("Signed {{client.signature}}"),
            "Signed #highlight(fill: rgb(\"#fff3a3\"))[{{client.signature}}]"
        );
    }

    #[test]
    fn typst_sigils_in_prose_are_escaped() {
        // A bare `#`/`$` in prose would otherwise start a Typst function
        // call or math block and break compilation.
        assert_eq!(to_typst(r"Pay $9,999 to #1"), r"Pay \$9,999 to \#1");
    }

    #[test]
    fn link_becomes_typst_link_call() {
        assert_eq!(
            to_typst("[neon](https://www.neonlaw.com)"),
            "#link(\"https://www.neonlaw.com\")[neon]"
        );
    }

    #[test]
    fn paragraphs_are_separated_by_one_blank_line() {
        assert_eq!(to_typst("one\n\ntwo"), "one\n\ntwo");
    }

    #[test]
    fn blockquote_wraps_in_typst_quote() {
        let out = to_typst("> noted");
        assert!(out.starts_with("#quote(block: true)["), "got: {out}");
        assert!(out.contains("noted"));
    }

    #[test]
    fn table_becomes_typst_table_call() {
        // A GFM table must reach Typst as `#table(..)`, not leak its
        // `| .. |` pipes into the page as literal prose. The header row
        // is wrapped in `table.header(..)`; body cells follow as a flat
        // stream that Typst lays out `columns`-wide.
        let out = to_typst("| A | B |\n| - | - |\n| 1 | 2 |");
        assert!(out.contains("#align(center)[#table("), "got: {out}");
        assert!(out.contains("columns: 2"), "got: {out}");
        assert!(out.contains("align: center"), "got: {out}");
        assert!(out.contains("table.header([A], [B], )"), "got: {out}");
        assert!(out.contains("[1], [2],"), "got: {out}");
    }

    #[test]
    fn table_cells_carry_inline_markup_and_escapes() {
        // Cell content flows through the same inline handlers as prose:
        // `**bold**` collapses to Typst `*bold*`, and a `$` (Typst math)
        // is escaped so a currency figure renders verbatim.
        let out = to_typst("| Filing | Fee |\n| - | - |\n| List | **$150.00** |");
        assert!(out.contains(r"[*\$150.00*]"), "got: {out}");
    }

    #[test]
    fn table_column_alignment_maps_to_typst_align() {
        // Explicit `:--` / `:--:` / `--:` alignment markers force the
        // tuple, mapping left/center/right per column.
        let out = to_typst("| A | B | C |\n| :-- | :--: | --: |\n| 1 | 2 | 3 |");
        assert!(out.contains("align: (left, center, right)"), "got: {out}");
        crate::render(&out).expect("converted aligned table must compile through Typst");
    }

    #[test]
    fn table_output_is_typst_compilable() {
        // The safety net: a real fee table (header, currency, a bold
        // total row) must compile all the way through Typst.
        let md = "\
| Nevada Secretary of State filing | Fee |\n\
| - | - |\n\
| Articles of Organization | $75.00 |\n\
| **Total state filing fees** | **$425.00** |";
        crate::render(&to_typst(md)).expect("a converted table must compile through Typst");
    }

    #[test]
    fn pagebreak_comment_becomes_typst_pagebreak() {
        // The one HTML token honored: `<!-- pagebreak -->` forces a page
        // break. Whitespace and case inside the comment don't matter.
        assert!(to_typst("<!-- pagebreak -->").contains("#pagebreak()"));
        assert!(to_typst("<!--pagebreak-->").contains("#pagebreak()"));
        assert!(to_typst("<!--  PageBreak  -->").contains("#pagebreak()"));
        // A page break between two paragraphs must compile through Typst.
        let typ = to_typst("First page.\n\n<!-- pagebreak -->\n\nSecond page.");
        assert!(typ.contains("#pagebreak()"), "got: {typ}");
        crate::render(&typ).expect("a page break must compile through Typst");
    }

    #[test]
    fn pagebreak_sentinel_inside_a_container_is_dropped_not_emitted() {
        // A `#pagebreak()` is illegal inside a Typst container and would
        // fail the whole render, so the sentinel is honored only at the top
        // level. Inside a table cell or a block quote it is dropped (like
        // any other HTML), and — crucially — the document still compiles.
        let table = to_typst("| a | b |\n|---|---|\n| <!-- pagebreak --> | y |\n");
        assert!(
            !table.contains("#pagebreak()"),
            "sentinel in a table cell must not emit a page break; got: {table}"
        );
        crate::render(&table).expect("a table with the sentinel in a cell must still compile");

        let quote = to_typst("> before\n>\n> <!-- pagebreak -->\n>\n> after");
        assert!(
            !quote.contains("#pagebreak()"),
            "sentinel in a block quote must not emit a page break; got: {quote}"
        );
        crate::render(&quote).expect("a block quote with the sentinel must still compile");

        // A top-level sentinel after a container still works — the depth
        // counter returns to 0 when the container closes.
        let after = to_typst("> quoted\n\n<!-- pagebreak -->\n\nAfter.");
        assert!(
            after.contains("#pagebreak()"),
            "a top-level sentinel after a container must still break; got: {after}"
        );
        crate::render(&after).expect("must compile");
    }

    #[test]
    fn non_pagebreak_html_comments_are_dropped() {
        // Only the pagebreak sentinel is honored; any other comment is
        // dropped, neither leaked as text nor treated as a page break.
        let out = to_typst("Before.\n\n<!-- TODO: revise -->\n\nAfter.");
        assert!(!out.contains("TODO"), "got: {out}");
        assert!(!out.contains("#pagebreak()"), "got: {out}");
    }

    #[test]
    fn output_is_typst_compilable() {
        // The real safety net: whatever we emit must compile.
        let md = "# Demand\n\nPay **now** to `{{party}}`:\n\n- item one\n- item two\n\n> heed this";
        let typ = to_typst(md);
        crate::render(&typ).expect("converted markdown must compile through Typst");
    }

    #[test]
    fn an_unanswered_placeholder_is_highlighted_and_prose_is_not() {
        // A token still present at render time is a blank. It must be
        // impossible to mistake for intentional prose, and still readable
        // so an attorney sees which answer is missing.
        let out = to_typst("Dear {{person__client.name}}, welcome.");
        assert!(
            out.contains("#highlight(fill: rgb(\"#fff3a3\"))["),
            "placeholder not highlighted: {out}"
        );
        assert!(out.contains("person\\_\\_client.name"), "token lost: {out}");
        // The surrounding prose is untouched.
        assert!(out.contains("Dear "), "{out}");
        assert!(out.contains(", welcome."), "{out}");
        // Text with no placeholder gains no markup at all.
        assert!(!to_typst("Plain prose.").contains("#highlight"));
    }

    #[test]
    fn several_placeholders_on_one_line_are_each_highlighted() {
        let out = to_typst("{{a}} and {{b}} and {{c}}");
        assert_eq!(out.matches("#highlight(fill:").count(), 3, "{out}");
        crate::render(&out).expect("must compile");
    }

    #[test]
    fn a_filled_answer_renders_bold() {
        // A substituted answer must be visually distinct from the boilerplate
        // it sits in: a reader skimming a retainer should be able to find
        // every fact particular to their matter without reading the prose.
        assert_eq!(
            super::bold_answer("Acme Robotics, Inc."),
            "**Acme Robotics, Inc.**"
        );
        let typ = to_typst(&format!("Client: {}.", super::bold_answer("Jordan Rivera")));
        assert!(typ.contains("*Jordan Rivera*"), "not strong: {typ}");
        crate::render(&typ).expect("must compile");
    }

    #[test]
    fn a_filled_answer_containing_markup_stays_bold_and_compiles() {
        // A client name or a scope sentence may contain `*` or `_`. Wrapping
        // it naively would unbalance the strong run and either swallow the
        // rest of the paragraph or fail to compile.
        for value in ["A*B", "under_score", "**already bold**", "50% * 2"] {
            let typ = to_typst(&format!("Value: {}.", super::bold_answer(value)));
            crate::render(&typ).unwrap_or_else(|e| panic!("`{value}` broke rendering: {e}"));
            // Typst strong is `*…*`; the run must open right where the
            // answer starts and close before the trailing period, with the
            // value's own sigils escaped inside rather than ending it early.
            let inner = typ
                .strip_prefix("Value: *")
                .and_then(|t| t.strip_suffix("*."))
                .unwrap_or_else(|| panic!("`{value}` did not open one strong run: {typ}"));
            assert!(
                !inner.is_empty() && !inner.contains("*."),
                "`{value}` closed its strong run early: {typ}"
            );
        }
    }

    #[test]
    fn an_empty_answer_is_not_wrapped_into_stray_asterisks() {
        // `****` is not strong emphasis in Markdown; it renders as four
        // literal asterisks on the page.
        assert_eq!(super::bold_answer(""), "");
        assert_eq!(super::bold_answer("   "), "   ");
    }

    #[test]
    fn an_unterminated_placeholder_falls_through_to_plain_escaping() {
        // `{{` with no closer must not emit an unbalanced `#highlight[`
        // that fails to compile.
        let out = to_typst("An open {{brace and nothing else.");
        assert!(!out.contains("#highlight"), "{out}");
        crate::render(&out).expect("must compile");
    }
}
