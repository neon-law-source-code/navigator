//! Every command the CLI names in its own operator-facing text must resolve.
//!
//! A remediation message that tells an operator to run a command which does
//! not exist is unambiguously a defect: following it produces a second error
//! instead of a fix. `project create` shipped exactly that — it refused an
//! unknown client with "create the client first (`cli person create` …)", and
//! there is no `person` subcommand: a spelling that reads plausibly and
//! resolves nowhere, which is the class of thing a reader cannot catch and a
//! test catches for free.
//!
//! Neither was caught, because nothing read these strings. Both are one-line
//! copy fixes; the drift is the thing worth a test, since the next behaviour
//! change will strand the next string the same way. So the invariant is
//! asserted rather than remembered.
//!
//! ## What counts as operator-facing
//!
//! Two surfaces, and nothing else:
//!
//! 1. **Runtime string literals** anywhere under `cli/src` — the remediation
//!    messages an operator reads when a command refuses, and the text the
//!    scaffolder writes into the files it generates. These are printed
//!    verbatim, so a wrong command here is one an operator actually reads.
//! 2. **`///` doc comments in `cli/src/main.rs`**, the clap surface — but for a
//!    narrower reason than it looks. **clap truncates an arg's help at the
//!    first sentence boundary**, and `-h` and `--help` render identically, so
//!    most of a multi-sentence doc comment is never shown to anybody. Every
//!    stale spelling found here sat in a later sentence and was therefore
//!    invisible at the terminal. They are still worth guarding, because a doc
//!    comment naming a command that does not exist is wrong on its own terms
//!    and is what the next author reads and copies — but it is a source-truth
//!    guard at this layer, *not* a claim about what an operator sees. Do not
//!    justify it as "help output"; that was the mistake this comment replaces.
//!
//! Deliberately out of scope: `//` and `//!` comments outside `main.rs`. Those
//! are developer prose about a module, not text the CLI shows anyone, and they
//! carry the same unresolvable `cli <path>` spelling in another dozen places.
//! Widening the scan to them is a copy sweep, not a guard, and can follow.
//!
//! ## How a path is resolved
//!
//! The walk consumes tokens against the real binary's subcommand tree. A token
//! that is unknown while the current command still *has* subcommands is the
//! failure. Reaching a leaf ends the walk only if that leaf takes a positional
//! argument, because then the remaining tokens are the argument:
//! `navigator site import person` is correct — `person` is `import`'s
//! `<MODEL_NAME>` value. A leaf that takes no positional has nothing an
//! operator could type after it, so a trailing token there is the same defect
//! as an unknown subcommand: `navigator ops deployments check` names a `check`
//! that does not exist, and `ops deployments` accepts only `--deployments-dir`.
//! Both facts are read off the command's own `--help` rather than listed here.
//!
//! One known blind spot: an invocation whose backtick pair never closes on one
//! logical line is invisible to the scan. Contiguous `///` blocks and
//! backslash-continued string literals are joined before scanning, which is
//! what closes that gap for every present case.

use assert_cmd::Command;
use std::collections::BTreeMap;
use std::path::Path;

/// The two prefixes the codebase writes an invocation with. `navigator` is the
/// installed binary; `cli` is the crate spelling from `cargo run -p cli --`,
/// which takes the same path after the `--`. Both must resolve against the
/// same tree, so `cli assets build` is as wrong as `navigator assets build` —
/// the command lives at `ops assets build`.
const PREFIXES: &[&str] = &["navigator", "cli"];

/// A floor on what the scan finds, so an extractor that quietly stops matching
/// fails loudly instead of passing over an empty set. The real count is
/// comfortably above this; the number only has to be high enough that reaching
/// it proves the walk read real files.
const MINIMUM_INVOCATIONS_SCANNED: usize = 30;

/// One command invocation found in operator-facing text.
#[derive(Debug, Clone)]
struct Invocation {
    /// Path relative to the workspace, with forward slashes, so the failure
    /// message is the same string on Windows and on CI.
    file: String,
    /// Where the logical line starts, which for a joined `///` block is the
    /// block's first line rather than the one carrying the invocation. The
    /// failure message quotes the invocation itself, so it stays findable.
    line: usize,
    /// The invocation as written, without its backticks.
    literal: String,
    /// The leading run of subcommand-shaped tokens, prefix word dropped.
    path: Vec<String>,
}

/// A logical line: a contiguous `///` block or a backslash-continued string
/// literal, joined back into the one string an operator sees.
struct Unit {
    line: usize,
    text: String,
    is_doc: bool,
}

/// Collapse a file into the logical lines worth scanning. `//!` and plain `//`
/// comments are dropped here rather than filtered later, so a stray backtick
/// in one cannot swallow a following invocation.
fn units(source: &str) -> Vec<Unit> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim_start();

        if let Some(first) = trimmed.strip_prefix("///") {
            // A doc block is one comment as far as the reader is concerned, so
            // join it before looking for backticks: clap re-wraps the prose
            // and an invocation can straddle two source lines.
            let start = i;
            let mut text = String::from(first.trim());
            i += 1;
            while i < lines.len() {
                match lines[i].trim_start().strip_prefix("///") {
                    Some(next) => {
                        text.push(' ');
                        text.push_str(next.trim());
                        i += 1;
                    }
                    None => break,
                }
            }
            out.push(Unit {
                line: start + 1,
                text,
                is_doc: true,
            });
            continue;
        }

        if trimmed.starts_with("//") {
            i += 1;
            continue;
        }

        // A Rust string continuation eats the next line's leading whitespace,
        // so join with nothing to reproduce the real string.
        let start = i;
        let mut text = String::from(lines[i]);
        while text.trim_end().ends_with('\\') && i + 1 < lines.len() {
            let kept = text.trim_end();
            text = kept[..kept.len() - 1].to_string();
            i += 1;
            text.push_str(lines[i].trim_start());
        }
        out.push(Unit {
            line: start + 1,
            text,
            is_doc: false,
        });
        i += 1;
    }

    out
}

/// Every closed backtick span in `text`. An unclosed span ends the scan of
/// that line rather than consuming the rest of it.
fn backticked(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        match after.find('`') {
            Some(close) => {
                out.push(&after[..close]);
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// Shell and prose characters that wrap a command word without being part of
/// it: `` `tag=$(navigator ops release-default-tag)` `` names a real command,
/// and so does a sentence ending "…run `navigator ops assets build`."
///
/// `<`, `>`, `[`, `]`, and `-` are deliberately *not* here. They are what make
/// `<SEED_FILE>` and `--dry-run` stop the run instead of being mistaken for
/// subcommands, which is the whole reason the scan knows where a path ends.
const WRAPPING_PUNCTUATION: &[char] = &['(', ')', '$', ',', '.', ';', ':', '"', '\''];

/// A token clap could have named a subcommand: lowercase ASCII letters,
/// digits, and hyphens. `<CODE>`, `--flag`, and prose stop the run.
fn is_subcommand_shaped(token: &str) -> bool {
    let mut chars = token.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    token
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Does this token name the binary? Read from its last word, so a command
/// embedded in shell (`tag=$(navigator`) is found rather than skipped.
fn names_the_binary(token: &str) -> bool {
    let last = token
        .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .next()
        .unwrap_or_default();
    PREFIXES.contains(&last)
}

/// The subcommand path a backticked span names, or `None` when the span is not
/// an invocation of this binary at all.
fn subcommand_path(literal: &str) -> Option<Vec<String>> {
    let tokens: Vec<&str> = literal.split_whitespace().collect();
    let prefix = tokens.iter().position(|token| names_the_binary(token))?;

    let mut rest = &tokens[prefix + 1..];
    // `cargo run -p cli -- ops gcp setup` is the spelling the docs teach, and
    // cargo's separator is not a subcommand.
    if rest.first() == Some(&"--") {
        rest = &rest[1..];
    }

    let path: Vec<String> = rest
        .iter()
        .map(|token| token.trim_matches(|c| WRAPPING_PUNCTUATION.contains(&c)))
        .take_while(|token| is_subcommand_shaped(token))
        .map(str::to_string)
        .collect();
    (!path.is_empty()).then_some(path)
}

/// Every invocation in one file's operator-facing text. `doc_comments_count`
/// is true only for `main.rs`, where a `///` block is `--help` output.
fn invocations_in(file: &str, source: &str, doc_comments_count: bool) -> Vec<Invocation> {
    units(source)
        .into_iter()
        .filter(|unit| doc_comments_count || !unit.is_doc)
        .flat_map(|unit| {
            backticked(&unit.text)
                .into_iter()
                .filter_map(|literal| {
                    subcommand_path(literal).map(|path| Invocation {
                        file: file.to_string(),
                        line: unit.line,
                        literal: literal.to_string(),
                        path,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// What one `--help` page says a command accepts next.
#[derive(Debug, Clone)]
struct Node {
    /// The subcommands it offers, empty for a leaf.
    children: Vec<String>,
    /// Whether its `Usage:` line shows a positional argument, so a trailing
    /// token could be a value rather than a mistyped subcommand.
    takes_positional: bool,
}

/// Does this `--help` page's `Usage:` line show a positional argument?
///
/// clap writes a positional as `<NAME>` when required and `[NAME]` when
/// optional, and uses the same brackets for the two synthetic entries
/// `[OPTIONS]` and `[COMMAND]`, which are not arguments an operator supplies.
fn takes_positional(help: &str) -> bool {
    let Some(usage) = help
        .lines()
        .find(|line| line.trim_start().starts_with("Usage:"))
    else {
        return false;
    };

    usage.split_whitespace().any(|token| {
        let inner = token
            .trim_start_matches(['<', '['])
            .trim_end_matches(['>', ']'])
            .trim_end_matches("...");
        let bracketed = token.starts_with('<') || token.starts_with('[');
        bracketed && !inner.is_empty() && inner != "OPTIONS" && inner != "COMMAND"
    })
}

/// Parse the `Commands:` block out of one `--help` page. Shares its shape with
/// `help.rs`: a command row is indented exactly two spaces.
fn command_names(help: &str) -> Vec<String> {
    let mut in_commands = false;
    let mut names = Vec::new();

    for line in help.lines() {
        match line.trim() {
            "Commands:" => {
                in_commands = true;
                continue;
            }
            "Options:" if in_commands => break,
            _ => {}
        }

        if in_commands {
            if let Some(row) = line
                .strip_prefix("  ")
                .filter(|rest| !rest.starts_with(char::is_whitespace))
            {
                if let Some(name) = row.split_whitespace().next() {
                    names.push(name.to_string());
                }
            }
        }
    }

    names
}

/// What the real binary accepts at `path`, memoized so a tree of invocations
/// costs one `--help` run per distinct node rather than one per invocation.
fn node_at(cache: &mut BTreeMap<Vec<String>, Node>, path: &[String]) -> Node {
    if let Some(hit) = cache.get(path) {
        return hit.clone();
    }

    let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
    args.push("--help");
    let output = Command::cargo_bin("navigator")
        .expect("build the navigator binary")
        .args(&args)
        .output()
        .expect("run navigator --help");
    assert!(
        output.status.success(),
        "`navigator {} --help` failed:\n{}",
        path.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );

    let help = String::from_utf8_lossy(&output.stdout);
    let node = Node {
        children: command_names(&help),
        takes_positional: takes_positional(&help),
    };
    cache.insert(path.to_vec(), node.clone());
    node
}

/// Where the walk currently is, spelled the way an operator would type it.
fn spelled(path: &[String]) -> String {
    if path.is_empty() {
        "navigator".to_string()
    } else {
        format!("navigator {}", path.join(" "))
    }
}

/// Walk `path` against the real tree. `Err` names the token that does not
/// resolve and what the binary accepts there instead.
fn resolve(cache: &mut BTreeMap<Vec<String>, Node>, path: &[String]) -> Result<(), String> {
    let mut walked: Vec<String> = Vec::new();

    for token in path {
        let node = node_at(cache, &walked);

        if node.children.is_empty() {
            // A leaf that takes a positional consumes the rest as a value:
            // `site import person` names `import`'s <MODEL_NAME>. A leaf that
            // takes none has nothing an operator could type after it.
            return if node.takes_positional {
                Ok(())
            } else {
                Err(format!(
                    "`{}` has no subcommands and takes no argument, so `{token}` \
                     is not something an operator can type after it",
                    spelled(&walked)
                ))
            };
        }

        if !node.children.contains(token) {
            return Err(format!(
                "`{token}` is not a subcommand of `{}`, which offers: {}",
                spelled(&walked),
                node.children.join(", ")
            ));
        }
        walked.push(token.clone());
    }

    Ok(())
}

/// Every invocation in every operator-facing string under `cli/src`, in a
/// stable order — `walkdir` yields NTFS and ext4 directories differently, so
/// the failure list must not depend on which one ran it.
fn scan_cli_src() -> Vec<Invocation> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();

    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "rs"))
    {
        let relative = entry
            .path()
            .strip_prefix(&root)
            .expect("walked under src")
            .to_string_lossy()
            .replace('\\', "/");
        let file = format!("cli/src/{relative}");
        let source = std::fs::read_to_string(entry.path()).expect("read a cli source file");
        // `main.rs` is the clap surface, so its `///` blocks are `--help`.
        found.extend(invocations_in(&file, &source, relative == "main.rs"));
    }

    found.sort_by(|a, b| (&a.file, a.line, &a.literal).cmp(&(&b.file, b.line, &b.literal)));
    found
}

/// The guard. Every command the CLI names where an operator can read it must
/// be a command the CLI actually has.
#[test]
fn every_command_named_in_operator_facing_text_resolves() {
    let invocations = scan_cli_src();
    assert!(
        invocations.len() >= MINIMUM_INVOCATIONS_SCANNED,
        "the scan found only {} invocations under cli/src — the extractor has \
         stopped matching, so a green run would prove nothing",
        invocations.len()
    );

    let mut cache = BTreeMap::new();
    let failures: Vec<String> = invocations
        .iter()
        .filter_map(|invocation| {
            resolve(&mut cache, &invocation.path).err().map(|why| {
                format!(
                    "{}:{}: `{}` — {why}",
                    invocation.file, invocation.line, invocation.literal
                )
            })
        })
        .collect();

    assert!(
        failures.is_empty(),
        "{} operator-facing string(s) name a command that does not resolve. An \
         operator following one of these hits a second error instead of a fix:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The parser's own assumption, checked rather than trusted.
///
/// Everything here rests on `command_names` reading clap's `Commands:` block.
/// If that ever stops parsing — a clap format change, a different indent, a
/// width change — it returns an empty list for every page, every node reads as
/// a leaf, and the guard degrades in the worst possible direction: it would
/// report failures against the *strings* when the defect is in the parser, and
/// would wave through a genuinely stale subcommand as "an argument after a
/// leaf". So the root node is asserted to have the membership `help.rs`
/// independently pins, which turns a parser break into a legible failure here
/// instead of a misleading one everywhere else.
#[test]
fn the_help_parser_still_reads_the_top_level_commands() {
    let mut cache = BTreeMap::new();
    let root = node_at(&mut cache, &[]);

    for expected in [
        "dev",
        "erd",
        "forms",
        "github",
        "notations",
        "ops",
        "project",
        "site",
        "validate",
    ] {
        assert!(
            root.children.iter().any(|name| name == expected),
            "the `Commands:` parser read {:?} at the top level and missed \
             `{expected}` — it has stopped reading clap's output, so every node \
             would read as a leaf and this guard would blame the wrong thing",
            root.children
        );
    }
}

/// The guard's own failure path, kept covered rather than checked once by
/// hand: `cli person create` is the string this file was written for, and a
/// guard that can only pass is worth nothing.
#[test]
fn the_guard_rejects_a_command_that_does_not_exist() {
    let mut cache = BTreeMap::new();

    let stale = subcommand_path("cli person create").expect("an invocation");
    let why = resolve(&mut cache, &stale).expect_err("`person` is not a subcommand");
    assert!(
        why.contains("`person` is not a subcommand of `navigator`"),
        "the failure must name the token and where it was looked up: {why}"
    );

    // A command that is real, just nested deeper, must not resolve at the
    // wrong depth. Being real elsewhere must not make it resolve here.
    let shallow = subcommand_path("cli notation create").expect("an invocation");
    resolve(&mut cache, &shallow).expect_err("`notation` is nested under `site`");

    let real = subcommand_path("navigator site notation create").expect("an invocation");
    resolve(&mut cache, &real).expect("`site notation create` resolves");
}

/// Tokens after a leaf that takes a positional are arguments. `site import`'s
/// `<MODEL_NAME>` is a glossary term, and a remediation naming one must not
/// read as a missing subcommand.
#[test]
fn an_argument_after_a_leaf_command_is_not_a_missing_subcommand() {
    let mut cache = BTreeMap::new();
    let path = subcommand_path("navigator site import person").expect("an invocation");
    resolve(&mut cache, &path).expect("`person` is import's model-name argument");
}

/// The other half of that rule, and the reason it is not simply "stop at a
/// leaf": `ops deployments` is a leaf whose only input is `--deployments-dir`,
/// so a trailing word after it is as unrunnable as an unknown subcommand.
/// Without this, a whole class of stale spelling passes silently.
#[test]
fn a_trailing_word_on_a_leaf_that_takes_no_argument_does_not_resolve() {
    let mut cache = BTreeMap::new();
    let path = subcommand_path("navigator ops deployments check").expect("an invocation");
    let why = resolve(&mut cache, &path).expect_err("`ops deployments` takes no positional");
    assert!(
        why.contains("takes no argument"),
        "the failure must say why the trailing word cannot be typed: {why}"
    );
}

#[test]
fn the_scan_reads_an_invocation_out_of_a_continued_remediation_message() {
    let source = r#"
fn refuse() -> String {
    format!(
        "no person with email `{email}` — the client must exist first \
         (`navigator site import person <seed-file>`)"
    )
}
"#;
    let found = invocations_in("cli/src/example.rs", source, false);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].literal, "navigator site import person <seed-file>");
    assert_eq!(found[0].path, vec!["site", "import", "person"]);
}

#[test]
fn the_scan_reads_an_invocation_split_across_a_doc_comment_block() {
    let source = "\
    /// Prints the tag so a caller can capture it: `tag=$(navigator ops
    /// release-default-tag)`.
    Thing,
";
    let found = invocations_in("cli/src/main.rs", source, true);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, vec!["ops", "release-default-tag"]);
}

/// `cargo run -p cli -- <path>` is the spelling the workspace docs teach, so a
/// string that prints it must be read as the invocation it is.
#[test]
fn the_scan_reads_the_cargo_run_spelling() {
    let source = "    assert!(prose.contains(\"x\"), \"must print `cargo run -p cli -- ops gcp setup --project-id`\");\n";

    let found = invocations_in("cli/src/example.rs", source, false);
    assert_eq!(found.len(), 1, "{found:?}");
    // Cargo's `--` is dropped and `--project-id` ends the path.
    assert_eq!(found[0].path, vec!["ops", "gcp", "setup"]);
}

/// A placeholder must end a path rather than be read as a subcommand, or every
/// remediation that shows the operator what to substitute would fail.
#[test]
fn a_placeholder_ends_a_path_instead_of_becoming_a_subcommand() {
    let path = subcommand_path("navigator project create --code <CODE>").expect("an invocation");
    assert_eq!(path, vec!["project", "create"]);
}

/// Developer prose is out of scope, and staying out of scope is part of the
/// contract: widening the scan to `//` and `//!` is a copy sweep across
/// another dozen module docs, so it must be a decision rather than a drift.
#[test]
fn plain_comments_outside_the_clap_surface_are_not_scanned() {
    let source = "//! `cli assets build` — transcode curated source photos.\n\
                  // Lay out a slug directory the way `cli assets build` does.\n\
                  /// Entry point for `cli assets build`.\n";

    assert!(invocations_in("cli/src/assets.rs", source, false).is_empty());
    // The same `///` line is in scope in main.rs, where clap reads it.
    assert_eq!(invocations_in("cli/src/main.rs", source, true).len(), 1);
}
