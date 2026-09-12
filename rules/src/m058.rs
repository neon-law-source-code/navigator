//! `M058` — tables must be preceded by a blank line. (Trailing
//! paragraph after a table is consumed by GFM as part of the table,
//! so we only flag the preceding case.) Mirrors MD058.

use crate::tables::{body_line_numbers, is_delimiter_row, is_table_row};
use crate::{line_byte_range, Rule, SourceFile, Violation};

pub struct M058BlanksAroundTables;

impl M058BlanksAroundTables {
    pub const CODE: &'static str = "M058";
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

impl Rule for M058BlanksAroundTables {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        let lines: Vec<&str> = file.contents.lines().collect();
        // A table only opens where the renderer would build one, so front
        // matter and fenced code are out: a table drawn inside a fence is
        // sample text, and its spacing is the author's business.
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
                // Found a table header at i. Check line above.
                if i > 0 && !is_blank(lines[i - 1]) {
                    violations.push(Violation {
                        code: Self::CODE,
                        path: file.path.clone(),
                        line: i + 1,
                        range: line_byte_range(&file.contents, i + 1),
                        message: "Table is not preceded by a blank line".to_string(),
                    });
                }
                // Advance past the entire table.
                let mut j = i + 2;
                while j < lines.len() && is_table_row(lines[j]) {
                    j += 1;
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
    use super::M058BlanksAroundTables;
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;
    fn f(b: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("t.md"),
            contents: b.to_string(),
        }
    }
    #[test]
    fn passes_when_blank_line_precedes_table() {
        let s = "Body.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert!(M058BlanksAroundTables.lint(&f(s)).is_empty());
    }
    #[test]
    fn flags_table_with_no_blank_above() {
        let s = "Body.\n| a | b |\n|---|---|\n| 1 | 2 |\n";
        let v = M058BlanksAroundTables.lint(&f(s));
        assert_eq!(v.len(), 1);
    }
    #[test]
    fn passes_when_table_starts_document() {
        let s = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert!(M058BlanksAroundTables.lint(&f(s)).is_empty());
    }

    /// A table drawn inside a fenced code block is sample text. The renderer
    /// never builds it, so its spacing is the author's business.
    #[test]
    fn ignores_a_table_inside_a_fence() {
        let s = concat!(
            "Body.\n",
            "\n",
            "```markdown\n",
            "| a | b |\n",
            "|---|---|\n",
            "| 1 | 2 |\n",
            "```\n",
        );
        assert!(M058BlanksAroundTables.lint(&f(s)).is_empty());
    }

    /// Front matter is YAML. A pipe-bearing value there opens no table.
    #[test]
    fn ignores_pipe_bearing_frontmatter() {
        let s = concat!(
            "---\n",
            "title: a | b\n",
            "summary: --- | ---\n",
            "---\n",
            "\n",
            "Body prose.\n",
        );
        assert!(M058BlanksAroundTables.lint(&f(s)).is_empty());
    }
}
