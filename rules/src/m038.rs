//! `M038` — no space inside inline code spans. Mirrors
//! markdownlint MD038. Flags `` ` foo ` ``.

use crate::{line_byte_range, Rule, SourceFile, TextEdit, Violation};

pub struct M038NoSpaceInCode;

impl M038NoSpaceInCode {
    pub const CODE: &'static str = "M038";
}

/// The backtick run a line ends without closing, which the **next** line may
/// close.
///
/// A `CommonMark` code span may cross a line break inside a paragraph, and
/// `S102`'s reflow routinely produces one: it packs to 120 columns without
/// regard for where a span begins. Read a line at a time, that span's
/// *closing* run looks like an opener and the prose up to the next backtick
/// reads as a padded code span, so trimming it eats the real spaces on either
/// side.
///
/// The carry is deliberately **one line**. An unmatched run is far more often
/// a stray literal backtick or a fence marker than a span that wraps, and a
/// state that travelled further would mute the rule for every line until a
/// run of the same length turned up — including for genuinely padded spans of
/// a different length. So a run that the next line does not close is spent,
/// and that line is scanned from its start.
type OpenRun = Option<usize>;

/// One complete code span within a single line.
struct Span {
    /// The whole span, opening run through closing run.
    outer: std::ops::Range<usize>,
    /// The content between the two runs.
    inner: std::ops::Range<usize>,
}

/// The byte index just past the first run of exactly `ticks` backticks, or
/// `None` when `line` holds no such run. A run of a different length is not
/// a closer, so it is stepped over.
fn close_run(line: &str, ticks: usize) -> Option<usize> {
    let bytes = line.as_bytes();
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

/// Every complete code span on `line`, and the run the line leaves open for
/// the next one.
///
/// When `incoming` names a run and this line closes it, that closing run is
/// stepped over without being reported: whether a span that crosses a line
/// break is padded is a property of the whole span, not of the fragment
/// sitting on this line, and `M038` does not judge it. When this line does
/// **not** close it, the run above was not a span opener — see [`OpenRun`] —
/// so the line is read from its start like any other.
fn scan_line(line: &str, incoming: OpenRun) -> (Vec<Span>, OpenRun) {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = incoming
        .and_then(|ticks| close_run(line, ticks))
        .unwrap_or(0);
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
        let start = i + ticks;
        let Some(offset) = close_run(&line[start..], ticks) else {
            // An unclosed run may open a span the next line closes.
            return (spans, Some(ticks));
        };
        let close_end = start + offset;
        spans.push(Span {
            outer: i..close_end,
            inner: start..close_end - ticks,
        });
        i = close_end;
    }
    (spans, None)
}

/// The line above the byte at `start`, without its line ending.
///
/// The fix needs only its predecessor, so it is taken from the text already
/// in hand rather than by replaying the file — a violation late in a long
/// document costs the same as one on line two.
fn previous_line(contents: &str, start: usize) -> Option<&str> {
    let before = contents[..start].strip_suffix('\n')?;
    let before = before.strip_suffix('\r').unwrap_or(before);
    Some(before.rsplit('\n').next().unwrap_or(before))
}

/// Whether a span's content carries the padding `M038` refuses. An all-space
/// span is left alone, since there is no content to tighten around.
fn is_padded(inner: &str) -> bool {
    !inner.is_empty()
        && (inner.starts_with(' ') || inner.ends_with(' '))
        && inner.bytes().any(|b| b != b' ')
}

/// Rebuild `line` with the inner padding of every code span removed
/// (`` ` foo ` `` → `` `foo` ``). A span is left untouched when trimming
/// would push a backtick up against the fence (the markdownlint
/// backtick-padding exception, e.g. ``` `` ` `` ```), or when the span is
/// all whitespace.
fn strip_code_padding(line: &str, incoming: OpenRun) -> String {
    let (spans, _) = scan_line(line, incoming);
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    for span in spans {
        out.push_str(&line[cursor..span.outer.start]);
        let inner = &line[span.inner.clone()];
        let trimmed = inner.trim_matches(' ');
        let safe = !trimmed.starts_with('`') && !trimmed.ends_with('`');
        if is_padded(inner) && !trimmed.is_empty() && safe {
            let ticks = &line[span.outer.start..span.inner.start];
            out.push_str(ticks);
            out.push_str(trimmed);
            out.push_str(ticks);
        } else {
            out.push_str(&line[span.outer.clone()]);
        }
        cursor = span.outer.end;
    }
    out.push_str(&line[cursor..]);
    out
}

impl Rule for M038NoSpaceInCode {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let mut open: OpenRun = None;
        let mut violations = Vec::new();
        for (idx, line) in file.contents.lines().enumerate() {
            let (spans, next) = scan_line(line, open);
            open = next;
            if spans.iter().any(|s| is_padded(&line[s.inner.clone()])) {
                violations.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line: idx + 1,
                    range: line_byte_range(&file.contents, idx + 1),
                    message: "Inline code span must not have leading or trailing whitespace"
                        .to_string(),
                });
            }
        }
        violations
    }

    fn fix(&self, file: &SourceFile, violation: &Violation) -> Option<TextEdit> {
        let line = &file.contents[violation.range.clone()];
        let incoming = previous_line(&file.contents, violation.range.start)
            .and_then(|above| scan_line(above, None).1);
        let fixed = strip_code_padding(line, incoming);
        (fixed != *line).then_some(TextEdit {
            range: violation.range.clone(),
            new_text: fixed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{strip_code_padding, M038NoSpaceInCode};
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
    fn an_unclosed_run_is_spent_after_one_line() {
        // A fence opener is an unmatched run too. It must not swallow the
        // rest of the file looking for a closer of its own length: the
        // padded span two lines down is real and gets flagged.
        let body = "```text\nbody\n````\nNext ` padded ` here.\n";
        let v = M038NoSpaceInCode.lint(&f(body));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].line, 4);
    }

    #[test]
    fn fix_refuses_when_trimming_would_abut_a_backtick() {
        // A span padded so its content is a literal backtick must keep
        // its spaces — collapsing them would merge fences. `strip` is a
        // no-op here, so `fix()` yields nothing to change.
        let line = "before `` ` `` after";
        assert_eq!(strip_code_padding(line, None), line);
    }
}
