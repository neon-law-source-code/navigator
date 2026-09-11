//! `S102` — body lines (and folded-scalar content inside frontmatter)
//! should be packed as close to the 120-character limit as possible.
//! Flags a line whose **next** line begins with a word that would
//! still fit if appended (with a single separating space), so the
//! prose can be reflowed tighter.
//!
//! Companion to `S101`, which polices the upper bound. `S102` polices
//! the *lower* bound by asking: "could you have pulled the next line's
//! first word up here without going over?" If yes, the wrap is too
//! eager.
//!
//! Two contexts are linted:
//! 1. **Body prose** (outside fenced code blocks).
//! 2. **Folded block scalars** (`description: >`) inside YAML
//!    frontmatter — newlines fold to spaces there, so packing is
//!    value-preserving. Literal blocks (`|`) are deliberately
//!    excluded since their newlines are part of the parsed value.
//!
//! The rule skips:
//! - fenced code blocks (triple-backtick / triple-tilde)
//! - non-folded frontmatter (plain `key: value`, flow scalars,
//!   mappings, sequences, literal `|` blocks)
//! - blank lines
//! - ATX headings (`#`, `##`, …)
//! - table rows (`|`)
//! - block-quote lines (`>`)
//! - horizontal rules (`---`, `***`, `___`) and setext heading
//!   underlines (a line of only `=` or only `-`)
//! - lines ending in a markdown hard break (two trailing spaces or
//!   trailing backslash) — those breaks are intentional
//! - pairs whose two lines have different leading whitespace (the
//!   next line belongs to a different block)
//! - cases where the next line begins a new list item (`-`, `*`, `+`,
//!   or an ordered marker like `1.`)
//! - pairs of lines that aren't directly adjacent in the source (a
//!   fence or frontmatter sat between them)
//!
//! The rule autofixes: `fix` repacks the whole block around the
//! violation rather than pulling one word up, so `navigator validate
//! --fix` reaches the rule's own fixpoint in a single pass. It declines
//! the blocks where repacking would rewrite the document tree — an
//! indented code block, a closing hard break, or a pack that would leave
//! a list marker at the head of a continuation line.

use crate::frontmatter;
use crate::{line_byte_range, Rule, SourceFile, TextEdit, Violation};

pub struct S102LinePacking {
    pub max: usize,
}

impl S102LinePacking {
    pub const CODE: &'static str = "S102";
    pub const DEFAULT_MAX: usize = 120;
}

impl Default for S102LinePacking {
    fn default() -> Self {
        Self {
            max: Self::DEFAULT_MAX,
        }
    }
}

impl Rule for S102LinePacking {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let mut out = Vec::new();
        // Pass 1: body prose (the original behavior).
        let body = frontmatter::body_lines(&file.contents);
        self.scan_pairs(file, &body, &mut out);
        // Pass 2: folded `>` block scalars inside frontmatter. Each
        // region's content lines fold to a single string, so packing
        // is value-preserving. We build a per-region `(line_no, line)`
        // slice and run the same pair check over it.
        let all_lines: Vec<&str> = file.contents.lines().collect();
        for region in frontmatter::folded_scalar_lines(&file.contents) {
            let mut region_lines = Vec::new();
            for line_no in region {
                region_lines.push((line_no, all_lines[line_no - 1]));
            }
            self.scan_pairs(file, &region_lines, &mut out);
        }
        out
    }

    /// Reflow the whole block the flagged line sits in, greedily packing
    /// its words to `max`.
    ///
    /// A one-word-at-a-time edit would not converge: pulling line 2's
    /// first word up leaves line 2 short, so the next run flags it
    /// again. Repacking the entire block instead reaches the rule's own
    /// fixpoint in a single pass — after a greedy pack no line can
    /// absorb its successor's first word, because that is exactly the
    /// test that ended each line.
    ///
    /// A block spans the maximal run of lines around the violation that
    /// [`joinable`] links, so the words only move within one paragraph
    /// (or one folded scalar) and never cross a blank line, a heading, a
    /// fence, a table, a list marker, or an indent change. Every
    /// violation in a block would compute the same edit, so only the
    /// first one emits it — `cli`'s `fix_directory` and the LSP's
    /// `source.fixAll` both apply edits blind, and two identical
    /// overlapping ranges would corrupt the file.
    fn fix(&self, file: &SourceFile, violation: &Violation) -> Option<TextEdit> {
        let all_lines: Vec<&str> = file.contents.lines().collect();
        let mut contexts = vec![frontmatter::body_lines(&file.contents)];
        for region in frontmatter::folded_scalar_lines(&file.contents) {
            contexts.push(
                region
                    .map(|line_no| (line_no, all_lines[line_no - 1]))
                    .collect(),
            );
        }
        for lines in &contexts {
            let Some(index) = lines.iter().position(|(no, _)| *no == violation.line) else {
                continue;
            };
            return self.reflow(file, lines, index);
        }
        None
    }
}

impl S102LinePacking {
    fn scan_pairs(&self, file: &SourceFile, lines: &[(usize, &str)], out: &mut Vec<Violation>) {
        let max = self.max;
        for pair in lines.windows(2) {
            let (a_no, a) = pair[0];
            let (b_no, b) = pair[1];
            if !joinable((a_no, a), (b_no, b)) {
                continue;
            }
            let Some(first_word) = first_word_of(b) else {
                continue;
            };
            let a_chars = a.chars().count();
            let first_chars = first_word.chars().count();
            let joined = a_chars + 1 + first_chars;
            if joined <= max {
                out.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line: a_no,
                    range: line_byte_range(&file.contents, a_no),
                    message: format!(
                        "Line is {a_chars} characters; could absorb \"{first_word}\" from line \
                         {b_no} to reach {joined} (max {max})",
                    ),
                });
            }
        }
    }

    /// Repack `lines[start..=end]`, the maximal joinable block around
    /// `index`, into as few lines as `max` allows. Returns `None` when
    /// the block is owned by an earlier violation, when repacking would
    /// change what the Markdown means, or when it would change nothing.
    fn reflow(&self, file: &SourceFile, lines: &[(usize, &str)], index: usize) -> Option<TextEdit> {
        let max = self.max;
        if index + 1 >= lines.len() || !joinable(lines[index], lines[index + 1]) {
            return None;
        }
        let mut start = index;
        while start > 0 && joinable(lines[start - 1], lines[start]) {
            start -= 1;
        }
        let mut end = index + 1;
        while end + 1 < lines.len() && joinable(lines[end], lines[end + 1]) {
            end += 1;
        }
        // One edit per block: every violation in it would produce this
        // same range, so the first violating pair owns the reflow.
        for k in start..index {
            let first_word = first_word_of(lines[k + 1].1)?;
            if lines[k].1.chars().count() + 1 + first_word.chars().count() <= max {
                return None;
            }
        }
        // Four spaces of indent is an indented code block unless it
        // continues something — a list item, or a paragraph. CommonMark
        // decides that on the line above, and so does `M046`: with a
        // blank line (or nothing) above it, this block is code, and
        // packing it would rewrite someone's sample.
        if leading_ws_len(lines[start].1) >= 4 {
            let above = lines[start]
                .0
                .checked_sub(2)
                .and_then(|idx| file.contents.lines().nth(idx));
            if above.is_none_or(|line| line.trim().is_empty()) {
                return None;
            }
        }
        // A trailing hard break on the closing line is authored intent,
        // and packing would either swallow the two spaces or float the
        // backslash onto a line of its own.
        if has_hard_break(lines[end].1) {
            return None;
        }
        let first = lines[start].1;
        let indent = &first[..leading_ws_len(first)];
        let mut packed: Vec<String> = Vec::new();
        let mut current = String::new();
        for word in lines[start..=end]
            .iter()
            .flat_map(|(_, line)| line.split_whitespace())
        {
            if current.is_empty() {
                current = format!("{indent}{word}");
            } else if current.chars().count() + 1 + word.chars().count() <= max {
                current.push(' ');
                current.push_str(word);
            } else {
                packed.push(std::mem::take(&mut current));
                current = format!("{indent}{word}");
            }
        }
        if !current.is_empty() {
            packed.push(current);
        }
        // A word that lands first on a continuation line can turn prose
        // into a list item, a heading, or a table row. Leave those
        // blocks to a human rather than rewriting the document tree.
        if packed
            .iter()
            .skip(1)
            .any(|line| is_non_prose(line) || starts_with_list_marker(&line[indent.len()..]))
        {
            return None;
        }
        let opener = line_byte_range(&file.contents, lines[start].0);
        // Every line but the last carries a terminator, so the block's
        // opener names the document's line ending. Rejoining with `\n`
        // in a CRLF file would leave a lone LF behind.
        let eol = if file.contents[opener.end..].starts_with("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let range = opener.start..line_byte_range(&file.contents, lines[end].0).end;
        let new_text = packed.join(eol);
        if new_text == file.contents[range.clone()] {
            return None;
        }
        Some(TextEdit { range, new_text })
    }
}

/// Whether line `b` may be folded up into line `a` without changing
/// what the Markdown means. Shared by the lint and the fix so a block
/// is reflowed on exactly the grounds it was flagged on.
fn joinable((a_no, a): (usize, &str), (b_no, b): (usize, &str)) -> bool {
    // Only consecutive source lines — a fence or frontmatter sitting
    // between them means they're different blocks.
    if b_no != a_no + 1 {
        return false;
    }
    if a.trim().is_empty() || b.trim().is_empty() {
        return false;
    }
    if has_hard_break(a) {
        return false;
    }
    if is_non_prose(a) || is_non_prose(b) {
        return false;
    }
    let a_indent = leading_ws_len(a);
    let b_indent = leading_ws_len(b);
    if a_indent != b_indent {
        return false;
    }
    !starts_with_list_marker(&b[b_indent..])
}

fn first_word_of(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

fn leading_ws_len(line: &str) -> usize {
    line.bytes()
        .take_while(|b| *b == b' ' || *b == b'\t')
        .count()
}

fn has_hard_break(line: &str) -> bool {
    // Two trailing spaces or a single trailing backslash are CommonMark
    // hard-break markers that the author put there on purpose. Don't
    // suggest unwrapping them.
    let trimmed = line.trim_end_matches('\r');
    trimmed.ends_with("  ") || trimmed.ends_with('\\')
}

fn is_non_prose(line: &str) -> bool {
    let s = line.trim_start();
    if s.is_empty() {
        return true;
    }
    if s.starts_with('#') || s.starts_with('|') || s.starts_with('>') {
        return true;
    }
    if s.starts_with("```") || s.starts_with("~~~") {
        return true;
    }
    is_setext_underline(s) || is_horizontal_rule(s)
}

/// A run of `=` or `-` alone on a line underlines the paragraph above
/// it into a setext heading. Folding it up would read as literal text
/// and delete the heading. `is_horizontal_rule` catches three or more
/// `-`; this catches the rest, and every `=`.
fn is_setext_underline(s: &str) -> bool {
    let stripped = s.trim_end();
    !stripped.is_empty()
        && (stripped.chars().all(|c| c == '=') || stripped.chars().all(|c| c == '-'))
}

fn is_horizontal_rule(s: &str) -> bool {
    let stripped = s.trim_end();
    let Some(c0) = stripped.chars().next() else {
        return false;
    };
    if !matches!(c0, '-' | '*' | '_') {
        return false;
    }
    let count = stripped.chars().filter(|c| *c == c0).count();
    if count < 3 {
        return false;
    }
    stripped.chars().all(|c| c == c0 || c == ' ' || c == '\t')
}

fn starts_with_list_marker(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    if matches!(bytes[0], b'-' | b'*' | b'+') && bytes[1] == b' ' {
        return true;
    }
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i + 1 < bytes.len() && matches!(bytes[i], b'.' | b')') && bytes[i + 1] == b' ' {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::S102LinePacking;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    fn file(contents: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("t.md"),
            contents: contents.to_string(),
        }
    }

    /// Apply the one edit the rule offers for a document, the way
    /// `cli`'s `fix_directory` and the LSP's `source.fixAll` do.
    fn fixed(body: &str) -> String {
        let f = file(body);
        let rule = S102LinePacking::default();
        let mut edits: Vec<crate::TextEdit> = rule
            .lint(&f)
            .iter()
            .filter_map(|v| rule.fix(&f, v))
            .collect();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let mut out = f.contents.clone();
        for edit in &edits {
            out.replace_range(edit.range.clone(), &edit.new_text);
        }
        out
    }

    #[test]
    fn fix_packs_a_paragraph_onto_as_few_lines_as_the_limit_allows() {
        assert_eq!(
            fixed("Short line.\nAnother short line.\nAnd a third.\n"),
            "Short line. Another short line. And a third.\n"
        );
    }

    #[test]
    fn fix_wraps_at_the_limit_rather_than_packing_onto_one_line() {
        // Six 30-character words: 120 exactly holds three (30 + 1 + 30 +
        // 1 + 30 = 92, and a fourth would reach 123), so the greedy pack
        // is 3 + 3.
        let word = "w".repeat(30);
        let body = format!("{word}\n{word}\n{word}\n{word}\n{word}\n{word}\n");
        let out = fixed(&body);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "{out:?}");
        for line in &lines {
            assert!(line.chars().count() <= 120, "{line:?}");
        }
    }

    #[test]
    fn fix_reaches_the_rules_own_fixpoint_in_one_pass() {
        let out = fixed("Short line.\nAnother short line.\nAnd a third.\n");
        assert!(
            S102LinePacking::default().lint(&file(&out)).is_empty(),
            "{out:?}"
        );
        assert_eq!(fixed(&out), out, "fix is not idempotent");
    }

    #[test]
    fn fix_preserves_the_blocks_indentation() {
        assert_eq!(
            fixed("  Indented line.\n  And its continuation.\n"),
            "  Indented line. And its continuation.\n"
        );
    }

    #[test]
    fn fix_stops_at_a_blank_line_and_leaves_neighbouring_paragraphs_alone() {
        assert_eq!(
            fixed("One a.\nOne b.\n\nTwo a.\nTwo b.\n"),
            "One a. One b.\n\nTwo a. Two b.\n"
        );
    }

    #[test]
    fn fix_never_crosses_a_fenced_code_block() {
        let body = "Prose a.\nProse b.\n```\nlet x = 1;\nlet y = 2;\n```\n";
        assert_eq!(
            fixed(body),
            "Prose a. Prose b.\n```\nlet x = 1;\nlet y = 2;\n```\n"
        );
    }

    #[test]
    fn fix_leaves_a_heading_and_its_following_line_apart() {
        assert_eq!(fixed("# Title\nBody line.\n"), "# Title\nBody line.\n");
    }

    #[test]
    fn fix_does_not_fold_a_list_item_into_its_predecessor() {
        assert_eq!(fixed("- alpha\n- beta\n"), "- alpha\n- beta\n");
    }

    #[test]
    fn fix_refuses_when_a_word_would_start_a_continuation_line_as_a_list_marker() {
        // "abc" fits on the first line at exactly 120, which pushes "2."
        // to the head of the second — turning prose into an ordered list
        // item. Left for a human instead.
        let head = "x".repeat(116);
        let body = format!("{head}\nabc 2. beta\n");
        assert_eq!(fixed(&body), body, "reflow changed the document tree");
    }

    #[test]
    fn leaves_a_setext_heading_underline_alone() {
        let body = "Title\n=====\n\nSub\n---\n";
        assert!(
            S102LinePacking::default().lint(&file(body)).is_empty(),
            "a setext underline is heading syntax, not a short line"
        );
        assert_eq!(fixed(body), body);
    }

    #[test]
    fn fix_refuses_an_indented_code_block() {
        // Blank line above, four spaces of indent: this is code, and
        // joining its lines would change what the sample runs.
        let body = "Prose.\n\n    let x = 1;\n    let y = 2;\n";
        assert!(!S102LinePacking::default().lint(&file(body)).is_empty());
        assert_eq!(fixed(body), body, "packed an indented code block");
    }

    #[test]
    fn fix_still_packs_an_indented_list_continuation() {
        // Same four-space indent, but the line above opens a list item,
        // so this is wrapped prose rather than code.
        assert_eq!(
            fixed("-   item\n    continuation a\n    continuation b\n"),
            "-   item\n    continuation a continuation b\n"
        );
    }

    #[test]
    fn fix_refuses_to_swallow_a_trailing_hard_break() {
        // The closing line's two trailing spaces are authored intent.
        let body = "Short line.\nEnds in a break.  \nNext.\n";
        assert_eq!(fixed(body), body);
    }

    #[test]
    fn fix_packs_a_folded_scalar_without_touching_the_rest_of_the_frontmatter() {
        let body =
            "---\nkind: onboarding\ndescription: >\n  Folded a.\n  Folded b.\n---\n\nBody.\n";
        assert_eq!(
            fixed(body),
            "---\nkind: onboarding\ndescription: >\n  Folded a. Folded b.\n---\n\nBody.\n"
        );
    }

    #[test]
    fn fix_keeps_crlf_line_endings_when_the_block_still_wraps() {
        let word = "w".repeat(40);
        let lf = format!("{word}\n{word}\n{word}\n{word}\n");
        let crlf = lf.replace('\n', "\r\n");
        let out = fixed(&crlf);
        assert!(!out.replace("\r\n", "").contains('\n'), "{out:?}");
        assert_eq!(out, fixed(&lf).replace('\n', "\r\n"));
    }

    #[test]
    fn only_the_first_violation_in_a_block_carries_the_edit() {
        // Three flagged lines in one paragraph must yield exactly one
        // edit: `fix_directory` applies edits blind, and two overlapping
        // ranges over the same block would corrupt the file.
        let f = file("Short line.\nAnother short line.\nAnd a third.\n");
        let rule = S102LinePacking::default();
        let violations = rule.lint(&f);
        assert_eq!(violations.len(), 2);
        let edits: Vec<_> = violations.iter().filter_map(|v| rule.fix(&f, v)).collect();
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert_eq!(edits[0].range.start, 0);
    }

    #[test]
    fn reports_its_code() {
        assert_eq!(S102LinePacking::default().code(), "S102");
    }

    #[test]
    fn flags_short_line_that_could_absorb_next_word() {
        let body = "Short line.\nAnother short line.\n";
        let v = S102LinePacking::default().lint(&file(body));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].code, "S102");
        assert_eq!(v[0].line, 1);
        assert!(v[0].message.contains("Another"));
    }

    #[test]
    fn passes_when_joining_would_exceed_max() {
        // 110-char first line + space + 12-char first word of next line
        // would be 123 — over the default 120-char max.
        let first = "x".repeat(110);
        let second = "yyyyyyyyyyyy and more.";
        let body = format!("{first}\n{second}\n");
        let v = S102LinePacking::default().lint(&file(&body));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn passes_when_line_is_exactly_at_the_limit_and_next_word_fits_exactly() {
        // Edge: 113 chars + ' ' + 'word' (4) = 118 <= 120 → flag.
        let first = "x".repeat(113);
        let body = format!("{first}\nword foo bar\n");
        let v = S102LinePacking::default().lint(&file(&body));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn skips_blank_lines() {
        let body = "A line.\n\nB line.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_when_a_is_a_heading() {
        let body = "## A heading\nA body paragraph follows.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_when_b_is_a_heading() {
        let body = "Some prose.\n## Heading\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_when_b_is_a_list_marker() {
        let body = "Lead-in sentence.\n- next bullet\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_when_b_is_an_ordered_list_marker() {
        let body = "Lead-in sentence.\n1. first item\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_when_indent_differs() {
        // Bullet header (no indent) vs continuation (2-space indent).
        let body = "- **Item title.**\n  Continuation prose.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn flags_pair_of_continuation_lines_with_matching_indent() {
        let body = "- **Item title.**\n  First continuation line.\n  Second continuation line.\n";
        let v = S102LinePacking::default().lint(&file(body));
        assert_eq!(v.len(), 1);
        // Only the first continuation gets flagged (it could absorb
        // "Second" from line 3); the second has no successor.
        assert_eq!(v[0].line, 2);
    }

    #[test]
    fn skips_inside_fenced_code_block() {
        let body = "Before.\n\n```\nshort\nlines inside fence\n```\n\nAfter.\n";
        // Pairs inside the fence are skipped by body_lines. The
        // "Before." / "After." pair is non-adjacent in source — they
        // belong to different blocks — and is also separated by a
        // fence, so no flag should fire.
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_when_a_ends_with_hard_break() {
        // Two trailing spaces on `a` mark a CommonMark hard break.
        let body = "Hard break here.  \nNext line of stanza.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_table_rows() {
        let body = "| col | val |\n| --- | --- |\n| a | b |\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_blockquotes() {
        let body = "> A quote line.\n> Continuation of the quote.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_horizontal_rules() {
        let body = "Before rule.\n---\nAfter rule.\n";
        // `---` is a horizontal rule (since the prior line is body,
        // not a setext title — but the rule treats it as non-prose
        // either way). No flag from pairs touching it.
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_plain_key_value_frontmatter() {
        // Plain `key: value` lines aren't a folded block — different
        // keys can't legally absorb each other's text.
        let body = "---\ntitle: Short\ndescription: Short too\n---\n\nBody.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn skips_literal_block_scalar_in_frontmatter() {
        // Literal `|` preserves newlines as part of the value; packing
        // would change what downstream code reads.
        let body = "---\ndescription: |\n  one\n  two\n---\n\nBody.\n";
        assert!(S102LinePacking::default().lint(&file(body)).is_empty());
    }

    #[test]
    fn flags_short_lines_inside_folded_scalar_frontmatter() {
        // Folded `>` scalars collapse newlines to spaces — packing is
        // value-preserving and S102 should suggest it.
        let body = "---\ndescription: >\n  short\n  text\n---\n\nBody.\n";
        let v = S102LinePacking::default().lint(&file(body));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].line, 3);
        assert!(v[0].message.contains("text"));
    }

    #[test]
    fn respects_configurable_max() {
        // With max = 30, "AAAAA" (5) + ' ' + "BBBB" (4) = 10 — under
        // 30 → flag. With default 120 it'd still flag, so set max
        // high enough that the join would exceed it.
        let rule = S102LinePacking { max: 8 };
        let body = "AAAAA\nBBBB CCCC\n";
        // 5 + 1 + 4 = 10 > 8 → no flag.
        assert!(rule.lint(&file(body)).is_empty());
    }

    #[test]
    fn flags_with_helpful_message_naming_the_next_word_and_target_line() {
        let body = "Short.\nNext-word here.\n";
        let v = S102LinePacking::default().lint(&file(body));
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("Next-word"));
        assert!(v[0].message.contains("line 2"));
    }

    #[test]
    fn counts_unicode_scalars_not_bytes() {
        // 118 Greek alphas + ' ' + "α" (1 char) = 120 ≤ 120 → flag.
        let first = "α".repeat(118);
        let body = format!("{first}\nα more.\n");
        let v = S102LinePacking::default().lint(&file(&body));
        assert_eq!(v.len(), 1);
    }
}
