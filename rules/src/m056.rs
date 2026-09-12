//! `M056` — a GFM table's delimiter row must carry the same number of
//! cells as its header row, and every body row must match them both.
//! Mirrors MD056, widened to the delimiter row.
//!
//! The delimiter row is the load-bearing one. GitHub-flavoured Markdown
//! builds a table only when the delimiter row's cell count equals the
//! header's; any mismatch demotes the whole block to paragraph text, so
//! the pipes render literally and the columns vanish. That is silent —
//! nothing errors, the page just reads wrong — which is exactly the
//! failure a gate exists to catch.
//!
//! Rows are read through [`crate::tables`], which is where the shared
//! GFM reading lives: cells separated by unescaped `|`, outer pipes
//! optional, `\|` a literal pipe inside a cell. Fenced code blocks and
//! YAML front matter are not Markdown body, so tables drawn there are
//! sample text and are skipped.

use crate::tables::{cell_count, is_delimiter_row, is_table_row};
use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

pub struct M056TableColumnCount;

impl M056TableColumnCount {
    pub const CODE: &'static str = "M056";
}

impl Rule for M056TableColumnCount {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        // `body_lines` drops front matter and fenced code, and carries
        // each surviving line's real number. Two entries are adjacent in
        // the document only when those numbers are consecutive, so a
        // fence sitting between a header and a delimiter row keeps them
        // from pairing.
        let lines = frontmatter::body_lines(&file.contents);
        let mut violations = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            let (header_no, header) = lines[index];
            let Some(&(delimiter_no, delimiter)) = lines.get(index + 1) else {
                break;
            };
            if delimiter_no != header_no + 1
                || !is_table_row(header)
                || !is_delimiter_row(delimiter)
            {
                index += 1;
                continue;
            }
            let expected = cell_count(header);
            let declared = cell_count(delimiter);
            if declared != expected {
                violations.push(Violation {
                    code: Self::CODE,
                    path: file.path.clone(),
                    line: delimiter_no,
                    range: line_byte_range(&file.contents, delimiter_no),
                    message: format!(
                        "Table delimiter row has {declared} cell(s); its header row has \
                         {expected}. A mismatch renders the block as paragraph text, not a table"
                    ),
                });
                // The block never became a table, so the lines under it
                // are paragraph text rather than body rows to measure.
                index += 2;
                while index < lines.len() && is_table_row(lines[index].1) {
                    index += 1;
                }
                continue;
            }
            let mut body = index + 2;
            let mut previous_no = delimiter_no;
            while let Some(&(line_no, line)) = lines.get(body) {
                if line_no != previous_no + 1 || !is_table_row(line) {
                    break;
                }
                let got = cell_count(line);
                if got != expected {
                    violations.push(Violation {
                        code: Self::CODE,
                        path: file.path.clone(),
                        line: line_no,
                        range: line_byte_range(&file.contents, line_no),
                        message: format!("Table row has {got} cell(s); expected {expected}"),
                    });
                }
                previous_no = line_no;
                body += 1;
            }
            index = body;
        }
        violations
    }
}

#[cfg(test)]
mod tests {
    use super::M056TableColumnCount;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;
    fn f(b: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("t.md"),
            contents: b.to_string(),
        }
    }
    #[test]
    fn passes_with_consistent_columns() {
        let s = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }
    #[test]
    fn flags_short_body_row() {
        let s = "| a | b |\n|---|---|\n| 1 |\n";
        let v = M056TableColumnCount.lint(&f(s));
        assert_eq!(v.len(), 1);
    }

    /// The shape that reached `main`: a header that gained a fourth column
    /// while its delimiter row kept three. GitHub-flavoured Markdown refuses
    /// to build a table from it and renders the whole block as paragraph
    /// text, so the gate has to catch the mismatch on the delimiter row.
    #[test]
    fn flags_a_delimiter_row_narrower_than_its_header() {
        let s = concat!(
            "| | Unset | Plain | Complete |\n",
            "| --- | --- | --- |\n",
            "| stdout | fmt | JSON | JSON |\n",
        );
        let v = M056TableColumnCount.lint(&f(s));
        assert_eq!(v.len(), 1, "expected one delimiter finding, got {v:?}");
        assert_eq!(v[0].line, 2);
        assert!(
            v[0].message.contains('3') && v[0].message.contains('4'),
            "message names neither count: {}",
            v[0].message
        );
    }

    #[test]
    fn flags_a_delimiter_row_wider_than_its_header() {
        let s = "| a | b |\n| --- | --- | --- |\n| 1 | 2 |\n";
        let v = M056TableColumnCount.lint(&f(s));
        assert_eq!(v.len(), 1, "expected one delimiter finding, got {v:?}");
        assert_eq!(v[0].line, 2);
    }

    /// The corrected shape passes: four header cells, four delimiter cells,
    /// and body rows to match.
    #[test]
    fn passes_the_repaired_four_column_shape() {
        let s = concat!(
            "| | Unset | Plain | Complete |\n",
            "| --- | --- | --- | --- |\n",
            "| stdout | fmt | JSON | JSON |\n",
            "| traces | \u{2014} | OTLP | OTLP |\n",
        );
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    /// A body row's own count is still measured against the header once the
    /// delimiter row agrees with it.
    #[test]
    fn flags_a_long_body_row_under_a_matching_delimiter() {
        let s = "| a | b |\n| --- | --- |\n| 1 | 2 | 3 |\n";
        let v = M056TableColumnCount.lint(&f(s));
        assert_eq!(v.len(), 1, "expected one body finding, got {v:?}");
        assert_eq!(v[0].line, 3);
    }

    /// A broken table drawn inside a fenced code block is sample text, not a
    /// table the renderer will build.
    #[test]
    fn ignores_a_broken_table_inside_a_fence() {
        let s = concat!(
            "Prose.\n",
            "\n",
            "```markdown\n",
            "| a | b | c |\n",
            "| --- | --- |\n",
            "| 1 |\n",
            "```\n",
        );
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    /// Front matter is YAML, not Markdown: a pipe-bearing value there never
    /// opens a table.
    #[test]
    fn ignores_pipe_bearing_frontmatter() {
        let s = concat!(
            "---\n",
            "title: a | b | c\n",
            "summary: --- | ---\n",
            "---\n",
            "\n",
            "Body prose.\n",
        );
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    /// A fence between the header and the delimiter row means they are not
    /// adjacent source lines, so no table opens.
    #[test]
    fn does_not_pair_rows_across_a_fence() {
        let s = concat!(
            "| a | b | c |\n",
            "```text\n",
            "sample\n",
            "```\n",
            "| --- | --- |\n",
        );
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    /// `\|` is a literal pipe inside a cell, not a cell separator. Counting
    /// it as one turns a correct table into a finding.
    #[test]
    fn escaped_pipes_do_not_open_a_cell() {
        let s = concat!(
            "| command | effect |\n",
            "| --- | --- |\n",
            "| `a \\| b` | pipes a into b |\n",
        );
        assert!(
            M056TableColumnCount.lint(&f(s)).is_empty(),
            "an escaped pipe was counted as a separator"
        );
    }

    /// The same escape applies to the header: a header carrying `\|` has
    /// fewer cells than a naive pipe count suggests, and the delimiter row
    /// that matches it must not be flagged.
    #[test]
    fn escaped_pipes_in_the_header_do_not_inflate_the_expected_count() {
        let s = "| a \\| b | c |\n| --- | --- |\n| 1 | 2 |\n";
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    /// A delimiter row is hyphens with optional alignment colons. A prose
    /// line that merely carries a pipe does not open a table under the line
    /// above it.
    #[test]
    fn prose_carrying_a_pipe_does_not_open_a_table() {
        let s = "Run `a | b` to pipe.\nThen read the output of `c | d | e`.\n";
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    #[test]
    fn accepts_alignment_colons_in_the_delimiter_row() {
        let s = "| a | b | c |\n| :-- | :-: | --: |\n| 1 | 2 | 3 |\n";
        assert!(M056TableColumnCount.lint(&f(s)).is_empty());
    }

    /// A table written without its outer pipes is still a table.
    #[test]
    fn counts_a_row_written_without_outer_pipes() {
        let s = "a | b | c\n--- | ---\n1 | 2 | 3\n";
        let v = M056TableColumnCount.lint(&f(s));
        assert_eq!(v.len(), 1, "expected one delimiter finding, got {v:?}");
        assert_eq!(v[0].line, 2);
    }

    /// One mismatched delimiter row reports once. The rows beneath it are
    /// paragraph text, not body rows of a table that never opened.
    #[test]
    fn reports_a_mismatched_delimiter_once_per_block() {
        let s = "| a | b | c |\n| --- | --- |\n| 1 |\n| 2 | 3 |\n\nAfter.\n";
        let v = M056TableColumnCount.lint(&f(s));
        assert_eq!(v.len(), 1, "expected one finding, got {v:?}");
        assert_eq!(v[0].line, 2);
    }
}
