//! Documented `navigator …` invocations in a Project repository must still
//! resolve against this binary's clap command tree.
//!
//! A contract that names `navigator template render` after the group moved to
//! `notations` is a procedure an agent follows into a usage error. Clap already
//! owns the live tree, so the gate walks it rather than a second roster.

use std::path::Path;

use clap::Command;

/// One documented invocation that does not resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedCli {
    pub line: usize,
    pub command: String,
    pub verb: String,
}

/// Walk `markdown` for `navigator …` invocations (fenced commands and inline
/// code) and return those whose next verb is not a clap subcommand.
#[must_use]
pub fn unresolved_invocations(markdown: &str, tree: &Command) -> Vec<UnresolvedCli> {
    invocations(markdown)
        .into_iter()
        .filter_map(|(line, command)| {
            unresolved_verb(&command, tree).map(|verb| UnresolvedCli {
                line,
                command,
                verb,
            })
        })
        .collect()
}

/// Command strings taken from fenced blocks and inline backticks.
#[must_use]
pub fn invocations(markdown: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (index, line) in markdown.lines().enumerate() {
        let line_no = index + 1;
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            if let Some(command) = fence_command(line) {
                out.push((line_no, command));
            }
            continue;
        }
        out.extend(inline_commands(line, line_no));
    }
    out
}

fn fence_command(line: &str) -> Option<String> {
    let trimmed = line.trim().trim_start_matches('$').trim();
    if trimmed == "navigator" || trimmed.starts_with("navigator ") {
        Some(trimmed.to_string())
    } else {
        None
    }
}

fn inline_commands(line: &str, line_no: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('`') else {
            break;
        };
        let inner = rest[..end].trim().trim_start_matches('$').trim();
        rest = &rest[end + 1..];
        if inner == "navigator" || inner.starts_with("navigator ") {
            out.push((line_no, inner.to_string()));
        }
    }
    out
}

/// The first argv word after `navigator` that clap does not recognize as a
/// subcommand, when the current command still has subcommands.
#[must_use]
pub fn unresolved_verb(command: &str, tree: &Command) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    if words.first().copied() != Some("navigator") {
        return None;
    }
    let mut current = tree;
    for word in words.iter().copied().skip(1) {
        if is_operand(word) {
            break;
        }
        match current.find_subcommand(word) {
            Some(sub) => current = sub,
            None if has_user_subcommands(current) => return Some(word.to_string()),
            None => break,
        }
    }
    None
}

fn is_operand(word: &str) -> bool {
    word.starts_with('-')
        || word.starts_with('<')
        || word.starts_with('[')
        || word == "."
        || word.contains('/')
        || (word.contains('.') && word != ".." && !word.chars().all(|c| c == '.'))
}

fn has_user_subcommands(cmd: &Command) -> bool {
    cmd.get_subcommands()
        .any(|sub| sub.get_name() != "help" && !sub.is_hide_set())
}

/// Markdown files a Project repository's contract can name a CLI verb in.
pub fn markdown_paths(root: &Path) -> Vec<std::path::PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name();
            name != ".git" && name != "node_modules" && name != "dist"
        })
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Command {
        Command::new("navigator")
            .subcommand(
                Command::new("notations")
                    .subcommand(Command::new("render"))
                    .subcommand(Command::new("format")),
            )
            .subcommand(
                Command::new("project")
                    .subcommand(Command::new("gate"))
                    .subcommand(
                        Command::new("repository")
                            .subcommand(Command::new("scaffold"))
                            .subcommand(Command::new("sync-skills")),
                    ),
            )
    }

    #[test]
    fn a_retired_group_is_unresolved_and_a_live_path_is_not() {
        let tree = tree();
        assert_eq!(
            unresolved_verb("navigator template render file.md", &tree).as_deref(),
            Some("template")
        );
        assert_eq!(
            unresolved_verb("navigator notations render file.md", &tree),
            None
        );
        assert_eq!(unresolved_verb("navigator notations format", &tree), None);
        // The retired group: `projects` was promoted out of `site` and made
        // singular, so both old spellings are unresolved at their first verb.
        assert_eq!(
            unresolved_verb("navigator projects gate", &tree).as_deref(),
            Some("projects")
        );
        assert_eq!(
            unresolved_verb("navigator site projects gate", &tree).as_deref(),
            Some("site")
        );
        // And the retired leaf under a live path is named at the leaf.
        assert_eq!(
            unresolved_verb("navigator project repository validate .", &tree).as_deref(),
            Some("validate")
        );
        assert_eq!(unresolved_verb("navigator project gate", &tree), None);
        assert_eq!(unresolved_verb("navigator project gate --ci", &tree), None);
        assert_eq!(unresolved_verb("navigator validate", &tree), None);
        assert_eq!(unresolved_verb("navigator validate . --ci", &tree), None);
    }

    #[test]
    fn fences_and_inline_code_are_both_invocations() {
        let md = concat!(
            "Run `navigator template render x`.\n\n",
            "```bash\n",
            "$ navigator project gate --ci\n",
            "```\n",
        );
        let found = unresolved_invocations(md, &tree());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].line, 1);
        assert_eq!(found[0].verb, "template");
        let all = invocations(md);
        assert!(
            all.iter()
                .any(|(_, cmd)| cmd == "navigator project gate --ci"),
            "{all:?}"
        );
    }
}
