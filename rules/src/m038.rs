//! `M038` — no space inside inline code spans. Mirrors
//! markdownlint MD038. Flags `` ` foo ` ``.

use crate::{line_byte_range, Rule, SourceFile, TextEdit, Violation};

pub struct M038NoSpaceInCode;

impl M038NoSpaceInCode {
    pub const CODE: &'static str = "M038";
}

/// One code span, as byte ranges into the whole document.
struct Span {
    /// The whole span, opening run through closing run.
    outer: std::ops::Range<usize>,
    /// The content between the two runs.
    inner: std::ops::Range<usize>,
}

/// The byte index just past the first run of exactly `ticks` backticks, or
/// `None` when `text` holds no such run. A run of a different length is not
/// a closer, so it is stepped over.
fn close_run(text: &str, ticks: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if run == ticks {
                return Some(i + run);
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// The delimiter character and length of the fence this line opens, if it
/// opens one: up to three spaces of indent, then three or more backticks or
/// tildes. A backtick fence's info string may not itself contain a backtick.
fn opens_fence(line: &str) -> Option<(u8, usize)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let ch = rest.as_bytes().first().copied()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let run = rest.as_bytes().iter().take_while(|&&b| b == ch).count();
    if run < 3 || (ch == b'`' && rest[run..].contains('`')) {
        return None;
    }
    Some((ch, run))
}

/// Whether this line closes a fence opened with `ch` repeated `len` times:
/// the same character, up to three spaces of indent, a run at least as long,
/// and nothing after it but whitespace.
///
/// The indent limit is the closer's own, not the opener's. At four spaces the
/// run is content rather than a closing fence, so the block is still open and
/// what follows is still the sample being shown.
fn closes_fence(line: &str, ch: u8, len: usize) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let run = rest.as_bytes().iter().take_while(|&&b| b == ch).count();
    run >= len && rest[run..].trim().is_empty()
}

/// The byte ranges of the prose blocks of `contents` — the runs of lines
/// that are neither blank nor inside a fenced code block.
///
/// Two bounds, for two different reasons.
///
/// A `CommonMark` code span may cross a line break but never a blank line;
/// the paragraph ends there. Pairing backticks within the paragraph is what
/// keeps a stray backtick, or a fence marker, from hunting through the rest
/// of the file for a closer of its own length and muting the rule over
/// everything it crosses.
///
/// A fenced block is a sample, not prose. Its body is whatever the author
/// wrote, and `` ` padded ` `` inside it is the thing being shown rather than
/// a span to tighten. Excluding the block is also what keeps its delimiters
/// from pairing with each other as if they were one enormous code span.
fn blocks(contents: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut offset = 0;
    let mut fence: Option<(u8, usize)> = None;
    for line in contents.split_inclusive('\n') {
        let skip = if let Some((ch, len)) = fence {
            // Inside a fence every line is skipped, the closer included.
            if closes_fence(line, ch, len) {
                fence = None;
            }
            true
        } else {
            fence = opens_fence(line);
            fence.is_some() || line.trim().is_empty()
        };
        if skip {
            if let Some(open) = start.take() {
                out.push(open..offset);
            }
        } else if start.is_none() {
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(open) = start {
        out.push(open..contents.len());
    }
    out
}

/// Every code span in `contents`, pairing runs across line breaks but never
/// across a blank line. A run with no closer in its own block is a literal
/// backtick and is stepped over rather than left open.
///
/// Scanning the block rather than the line is what makes a wrapped span
/// whole. `S102`'s reflow packs prose to 120 columns without regard for
/// where a span begins, so a span routinely straddles one line break and
/// occasionally two. Read a line at a time, such a span's *closing* run
/// looks like an opener and the prose up to the next backtick reads as
/// padded code — so trimming it eats the real spaces on either side.
fn spans(contents: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for block in blocks(contents) {
        let text = &contents[block.clone()];
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'`' {
                i += 1;
                continue;
            }
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let start = i + ticks;
            let Some(offset) = close_run(&text[start..], ticks) else {
                i = start;
                continue;
            };
            let close_end = start + offset;
            out.push(Span {
                outer: block.start + i..block.start + close_end,
                inner: block.start + start..block.start + close_end - ticks,
            });
            i = close_end;
        }
    }
    out
}

/// The byte offset each line starts at, for turning a span's position into
/// the line number a violation reports.
fn line_starts(contents: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(contents.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

/// Whether a span's content carries the padding `M038` refuses. An all-space
/// span is left alone, since there is no content to tighten around.
fn is_padded(inner: &str) -> bool {
    !inner.is_empty()
        && (inner.starts_with(' ') || inner.ends_with(' '))
        && inner.bytes().any(|b| b != b' ')
}

/// Whether this span is one `M038` judges.
///
/// A span that wraps is not. Whether it is padded is a property of the whole
/// span, and the rule speaks a line at a time — it reports one line and
/// rewrites one line — so a wrapped span is recognized precisely so that it
/// can be stepped over intact.
fn judged(contents: &str, span: &Span) -> bool {
    !contents[span.outer.clone()].contains('\n') && is_padded(&contents[span.inner.clone()])
}

impl Rule for M038NoSpaceInCode {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let contents = &file.contents;
        let starts = line_starts(contents);
        let mut violations: Vec<Violation> = Vec::new();
        for span in spans(contents) {
            if !judged(contents, &span) {
                continue;
            }
            let line = starts.partition_point(|&start| start <= span.outer.start);
            // One violation per line, as the line is what gets rewritten.
            if violations.last().is_some_and(|last| last.line == line) {
                continue;
            }
            violations.push(Violation {
                code: Self::CODE,
                path: file.path.clone(),
                line,
                range: line_byte_range(contents, line),
                message: "Inline code span must not have leading or trailing whitespace"
                    .to_string(),
            });
        }
        violations
    }

    fn fix(&self, file: &SourceFile, violation: &Violation) -> Option<TextEdit> {
        let contents = &file.contents;
        let range = violation.range.clone();
        let mut out = String::with_capacity(range.len());
        let mut cursor = range.start;
        // Spans arrive in document order, so the cursor only moves forward.
        for span in spans(contents) {
            if span.outer.start < range.start || span.outer.end > range.end {
                continue;
            }
            out.push_str(&contents[cursor..span.outer.start]);
            let inner = &contents[span.inner.clone()];
            let trimmed = inner.trim_matches(' ');
            // Trimming is declined when it would push a backtick up against
            // the fence (the markdownlint backtick-padding exception, e.g.
            // ``` `` ` `` ```), or when the span is all whitespace.
            let safe = !trimmed.starts_with('`') && !trimmed.ends_with('`');
            if is_padded(inner) && !trimmed.is_empty() && safe {
                let ticks = &contents[span.outer.start..span.inner.start];
                out.push_str(ticks);
                out.push_str(trimmed);
                out.push_str(ticks);
            } else {
                out.push_str(&contents[span.outer.clone()]);
            }
            cursor = span.outer.end;
        }
        out.push_str(&contents[cursor..range.end]);
        (out != contents[range.clone()]).then_some(TextEdit {
            range,
            new_text: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::M038NoSpaceInCode;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;
    fn f(b: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("t.md"),
            contents: b.to_string(),
        }
    }
    #[test]
    fn passes_with_tight_code_span() {
        assert!(M038NoSpaceInCode.lint(&f("Use `foo` here.\n")).is_empty());
    }
    #[test]
    fn flags_space_inside_code_span() {
        let v = M038NoSpaceInCode.lint(&f("Use ` foo ` here.\n"));
        assert_eq!(v.len(), 1);
    }

    fn fixed(body: &str) -> String {
        let file = f(body);
        let v = M038NoSpaceInCode.lint(&file);
        let edit = M038NoSpaceInCode.fix(&file, &v[0]).expect("a fix");
        let mut out = file.contents.clone();
        out.replace_range(edit.range, &edit.new_text);
        out
    }

    #[test]
    fn fix_trims_code_span_padding() {
        assert_eq!(fixed("Use ` foo ` here.\n"), "Use `foo` here.\n");
        assert_eq!(fixed("`left ` and ` right`\n"), "`left` and `right`\n");
    }

    #[test]
    fn fix_is_idempotent() {
        let once = fixed("Use ` foo ` here.\n");
        assert!(M038NoSpaceInCode.lint(&f(&once)).is_empty());
    }

    #[test]
    fn a_code_span_crossing_a_line_keeps_the_spaces_around_its_closer() {
        // A code span may cross a line break inside a paragraph. Here it
        // opens with `` `site `` and closes after `upload` on the next
        // line. Read a line at a time, that closer looks like an *opener*,
        // so " can confirm this — before it, " reads as a padded span and
        // its real spaces are trimmed away, printing `upload`can` and
        // `it,`--host``. Nothing on either line is actually padded.
        let body = "an operator who has just run `site\n\
                    document upload` can confirm this — before it, `--host` was\n";
        assert!(
            M038NoSpaceInCode.lint(&f(body)).is_empty(),
            "no span here is padded, got: {:?}",
            M038NoSpaceInCode.lint(&f(body)),
        );
    }

    #[test]
    fn a_stray_backtick_does_not_mute_a_padded_span_of_another_length() {
        // The lone backtick is a literal, not the opener of a span that
        // wraps. Carrying it forward would make the scanner hunt for a
        // single-backtick closer and walk straight past the perfectly
        // ordinary padded double-backtick span underneath it.
        let v = M038NoSpaceInCode.lint(&f("`\nThen `` padded `` here.\n"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].line, 2);
    }

    #[test]
    fn a_span_crossing_two_line_breaks_keeps_the_prose_after_it_intact() {
        // The span opens on line 1 and closes on line 3, so line 2 carries
        // no backtick at all. Its real closer must not be read as an opener,
        // or " can confirm this — before it, " becomes padded code and loses
        // the spaces on either side — the very corruption this rule change
        // exists to stop, one line further out.
        let body = "prefix `alpha\nmiddle\nomega` can confirm this — before it, `--host` was\n";
        let v = M038NoSpaceInCode.lint(&f(body));
        assert!(v.is_empty(), "nothing here is padded, got {v:?}");
    }

    #[test]
    fn a_fenced_sample_is_not_prose_and_is_left_alone() {
        // The body of a fenced block is whatever the author is showing, so
        // `` ` padded ` `` there is the subject rather than a span to
        // tighten. Both delimiters, an info string, a blank body line, and a
        // closer longer than its opener.
        for body in [
            "~~~text\n` padded `\n~~~\n",
            "```text\n` padded `\n```\n",
            "```\n` padded `\n```\n",
            "```text\n\n` padded `\n```\n",
            "```text\n` padded `\n`````\n",
            "  ```text\n` padded `\n  ```\n",
        ] {
            let v = M038NoSpaceInCode.lint(&f(body));
            assert!(
                v.is_empty(),
                "fenced body must be left alone: {body:?} -> {v:?}"
            );
        }
    }

    #[test]
    fn a_four_space_run_does_not_close_a_fence() {
        // CommonMark allows a closing fence up to three spaces of indent.
        // At four it is content, not a closer, so the fence is still open
        // and the span below it is still the sample being shown.
        let body = "```text\nbody\n    ```\n` padded `\n```\n";
        let v = M038NoSpaceInCode.lint(&f(body));
        assert!(v.is_empty(), "still inside the fence, got {v:?}");
    }

    #[test]
    fn a_closer_indented_three_spaces_still_closes() {
        // Three is the limit, not the exclusion.
        let v = M038NoSpaceInCode.lint(&f("```text\nbody\n   ```\n` padded `\n"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].line, 4);
    }

    #[test]
    fn prose_after_a_fence_closes_is_still_linted() {
        // Exempting the block must not exempt the document. A closer longer
        // than its opener still closes, so the padded span below is prose.
        let v = M038NoSpaceInCode.lint(&f("```text\nbody\n````\nNext ` padded ` here.\n"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].line, 4);
    }

    #[test]
    fn a_lone_backtick_run_that_never_closes_is_not_a_fence() {
        // Two backticks are an inline run, not a fence opener, so the line
        // below stays prose and its padded span is still found.
        let v = M038NoSpaceInCode.lint(&f("``\nThen ` padded ` here.\n"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].line, 2);
    }

    #[test]
    fn fix_refuses_when_trimming_would_abut_a_backtick() {
        // A span padded so its content is a literal backtick must keep its
        // spaces — collapsing them would merge fences. The span is still
        // reported, but the fix declines to touch it.
        let file = f("before `` ` `` after\n");
        let v = M038NoSpaceInCode.lint(&file);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(
            M038NoSpaceInCode.fix(&file, &v[0]).is_none(),
            "the fix must decline rather than merge the fences"
        );
    }
}
