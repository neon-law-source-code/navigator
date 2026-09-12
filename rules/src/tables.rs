//! How the table rules read a GFM table row.
//!
//! `M055`, `M056`, `M058`, and `M060` all have to answer the same three
//! questions — is this line part of a table, is it the delimiter row, and
//! what are its cells — and each used to answer them with its own copy.
//! The copies drifted: three of them split on every `|`, so a cell holding
//! an escaped `\|` was torn in two, and all of them read the raw line
//! list, so a table drawn inside a fenced code block was measured as if
//! the renderer would build it.
//!
//! Cells are separated by unescaped `|`. The outer pipes delimit the row
//! rather than open a cell, so they are optional and contribute no column.
//! A backslash escapes the character after it, which is how a row
//! documenting a shell pipeline keeps `a \| b` in one cell.

use std::collections::BTreeSet;

use crate::frontmatter;

/// Split a row into its cells on unescaped `|`, discarding the optional
/// leading and trailing delimiter pipes.
///
/// Interior padding is preserved, because `M060` measures it.
#[must_use]
pub fn cells(line: &str) -> Vec<&str> {
    let row = line.trim();
    let mut out = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (index, character) in row.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '|' => {
                out.push(&row[start..index]);
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&row[start..]);
    if out.len() > 1 && out[0].is_empty() {
        out.remove(0);
    }
    if out.len() > 1 && out.last().is_some_and(|cell| cell.is_empty()) {
        out.pop();
    }
    if out.len() == 1 && out[0].is_empty() {
        out.clear();
    }
    out
}

/// How many columns a row declares.
#[must_use]
pub fn cell_count(line: &str) -> usize {
    cells(line).len()
}

/// The character offsets of the pipes that actually delimit columns.
///
/// Character offsets rather than byte offsets, so a row carrying an em-dash
/// still lines up with an all-ASCII neighbour. Escaped pipes are excluded:
/// they render as literal text inside a cell and delimit nothing.
#[must_use]
pub fn pipe_positions(line: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut escaped = false;
    for (index, character) in line.chars().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '|' => out.push(index),
            _ => {}
        }
    }
    out
}

/// Whether a line carries at least one unescaped `|`, the only thing that
/// can make it part of a table.
#[must_use]
pub fn is_table_row(line: &str) -> bool {
    !pipe_positions(line).is_empty()
}

/// Whether a line is a delimiter row: pipe-separated cells, each one
/// hyphens with an optional alignment colon at either end.
#[must_use]
pub fn is_delimiter_row(line: &str) -> bool {
    let cells = cells(line);
    if cells.is_empty() {
        return false;
    }
    cells.iter().all(|cell| {
        let dashes = cell.trim().trim_start_matches(':').trim_end_matches(':');
        !dashes.is_empty() && dashes.chars().all(|character| character == '-')
    })
}

/// The 1-based line numbers that are Markdown body: outside YAML front
/// matter and outside fenced code blocks.
///
/// A table is only a table where the renderer would build one. Inside a
/// fence it is sample text — often sample text demonstrating the very
/// mistake a rule exists to catch — and in front matter it is a YAML
/// scalar that happens to hold a pipe.
#[must_use]
pub fn body_line_numbers(contents: &str) -> BTreeSet<usize> {
    frontmatter::body_lines(contents)
        .into_iter()
        .map(|(line_no, _)| line_no)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{body_line_numbers, cell_count, cells, is_delimiter_row, is_table_row};

    #[test]
    fn outer_pipes_delimit_the_row_rather_than_opening_a_cell() {
        assert_eq!(cell_count("| a | b |"), 2);
        assert_eq!(cell_count("a | b"), 2);
        assert_eq!(cell_count("| a | b"), 2);
        assert_eq!(cell_count("a | b |"), 2);
    }

    #[test]
    fn an_empty_leading_column_is_still_a_column() {
        assert_eq!(cell_count("| | a | b |"), 3);
    }

    #[test]
    fn an_escaped_pipe_opens_no_cell() {
        assert_eq!(cell_count(r"| a \| b | c |"), 2);
        assert_eq!(cells(r"| a \| b | c |"), vec![r" a \| b ", " c "]);
    }

    #[test]
    fn interior_padding_survives_the_split() {
        assert_eq!(cells("| a | b |"), vec![" a ", " b "]);
        assert_eq!(cells("|a|b|"), vec!["a", "b"]);
    }

    #[test]
    fn a_row_needs_an_unescaped_pipe() {
        assert!(is_table_row("| a |"));
        assert!(is_table_row("a | b"));
        assert!(!is_table_row("no pipes here"));
        assert!(!is_table_row(r"escaped \| only"));
    }

    #[test]
    fn a_delimiter_row_is_hyphens_with_optional_colons() {
        assert!(is_delimiter_row("| --- | --- |"));
        assert!(is_delimiter_row("|---|---|"));
        assert!(is_delimiter_row("| :-- | :-: | --: |"));
        assert!(is_delimiter_row("--- | ---"));
        assert!(!is_delimiter_row("| a | b |"));
        assert!(!is_delimiter_row("| :: | -- |"));
        assert!(!is_delimiter_row("| | |"));
    }

    #[test]
    fn body_lines_exclude_fences_and_frontmatter() {
        let contents = concat!(
            "---\n",
            "title: a | b\n",
            "---\n",
            "\n",
            "Body.\n",
            "\n",
            "```text\n",
            "| fenced |\n",
            "```\n",
            "After.\n",
        );
        let body = body_line_numbers(contents);
        assert!(body.contains(&5), "the body prose line is body");
        assert!(body.contains(&10), "the line after the fence is body");
        assert!(!body.contains(&2), "front matter is not body");
        assert!(!body.contains(&8), "a fenced line is not body");
    }
}
