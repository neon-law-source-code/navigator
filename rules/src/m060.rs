//! `M060` — tables must use one consistent column style per table:
//! `aligned` (pipes line up), `tight` (single-space cell padding), or
//! `compact` (no padding). Mirrors MD060's `any` mode.

use crate::tables::{body_line_numbers, cells, is_delimiter_row, is_table_row, pipe_positions};
use crate::{line_byte_range, Rule, SourceFile, Violation};

pub struct M060TableColumnStyle;

impl M060TableColumnStyle {
    pub const CODE: &'static str = "M060";
}

fn aligned(rows: &[&str]) -> bool {
    let target = pipe_positions(rows[0]);
    rows.iter().all(|r| pipe_positions(r) == target)
}

fn tight(rows: &[&str]) -> bool {
    rows.iter().all(|r| {
        cells(r).iter().all(|c| {
            if c.trim().is_empty() {
                return true;
            }
            c.starts_with(' ') && c.ends_with(' ') && !c.starts_with("  ") && !c.ends_with("  ")
        })
    })
}

fn compact(rows: &[&str]) -> bool {
    rows.iter().all(|r| {
        cells(r).iter().all(|c| {
            if c.is_empty() {
                return true;
            }
            !c.starts_with(' ') && !c.ends_with(' ')
        })
    })
}

impl Rule for M060TableColumnStyle {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let lines: Vec<&str> = file.contents.lines().collect();
        // A table inside a fence is sample text; its column style is the
        // author's business, not this rule's.
        let body = body_line_numbers(&file.contents);
        let mut violations = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let next = lines.get(i + 1).copied().unwrap_or("");
            if body.contains(&(i + 1))
                && body.contains(&(i + 2))
                && is_table_row(lines[i])
                && is_delimiter_row(next)
            {
                let mut rows: Vec<&str> = vec![lines[i], lines[i + 1]];
                let mut j = i + 2;
                while j < lines.len() && body.contains(&(j + 1)) && is_table_row(lines[j]) {
                    rows.push(lines[j]);
                    j += 1;
                }
                if !aligned(&rows) && !tight(&rows) && !compact(&rows) {
                    violations.push(Violation {
                        code: Self::CODE,
                        path: file.path.clone(),
                        line: i + 1,
                        range: line_byte_range(&file.contents, i + 1),
                        message:
                            "Table does not match any consistent column style (aligned/tight/compact)"
                                .to_string(),
                    });
                }
                i = j;
            } else {
                i += 1;
            }
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::M060TableColumnStyle;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;
    fn f(b: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("t.md"),
            contents: b.to_string(),
        }
    }
    #[test]
    fn passes_with_tight_table() {
        let s = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert!(M060TableColumnStyle.lint(&f(s)).is_empty());
    }
    #[test]
    fn passes_with_compact_table() {
        let s = "|a|b|\n|-|-|\n|1|2|\n";
        assert!(M060TableColumnStyle.lint(&f(s)).is_empty());
    }
    #[test]
    fn flags_mixed_padding() {
        let s = "| a |b|\n|---|---|\n|1 | 2|\n";
        let v = M060TableColumnStyle.lint(&f(s));
        assert!(!v.is_empty());
    }

    #[test]
    fn aligned_table_with_unicode_em_dash_does_not_misalign() {
        // The em-dash (3 bytes in UTF-8) used to shift pipe byte
        // offsets and trip `aligned`. With char-index counting the
        // table reads as aligned.
        let s = "\
| a    | b                |
| ---- | ---------------- |
| ok   | plain ascii      |
| also | with — em-dash   |
";
        assert!(M060TableColumnStyle.lint(&f(s)).is_empty());
    }

    /// `\|` is a literal pipe inside a cell, not a column boundary. Splitting
    /// on it tears one cell into two ragged halves and the table matches no
    /// style at all, so every table documenting a shell pipeline was a finding.
    #[test]
    fn an_escaped_pipe_stays_inside_its_cell() {
        let s = concat!(
            "| command | effect |\n",
            "| --- | --- |\n",
            "| `a \\| b` | pipes a into b |\n",
        );
        assert!(
            M060TableColumnStyle.lint(&f(s)).is_empty(),
            "an escaped pipe was read as a column boundary"
        );
    }

    /// A table inside a fence is sample text; its column style is the
    /// author's business.
    #[test]
    fn ignores_a_table_inside_a_fence() {
        let s = concat!(
            "Prose.\n",
            "\n",
            "```markdown\n",
            "| a |b|\n",
            "|---|---|\n",
            "|1 | 2|\n",
            "```\n",
        );
        assert!(M060TableColumnStyle.lint(&f(s)).is_empty());
    }
}
