//! The one place Navigator decides whether a self-referencing action pin is
//! stale.
//!
//! The reusable workflows and composite actions in `.github/` name this
//! repository's OWN actions by an absolute tag rather than by the ref the
//! caller used, so nothing but a deliberate edit moves them. A pin left behind
//! ships a gate no consumer can run: `26.9.17-rc.1` published
//! `project-gate.yml` still pointing `navigator-install` at `26.9.16`, a tag
//! predating the action, and every job needing the CLI died at action
//! resolution.
//!
//! # One rule, two callers
//!
//! `ci.yml` asks this on every pull request and the `cut-release` preflight
//! asks it again before a bump is pushed, and both reach it through
//! [`COMMAND`]. That is the whole reason the rule is a tested function rather
//! than a `grep` in each script: the pull request is where a stale pin is
//! still free to fix, the release cut is where it becomes irreversible, and
//! two transcriptions of one match are two chances for those paths to disagree
//! about what a stale pin is.
//!
//! # What counts as a pin
//!
//! Every ref on a self-referencing `uses:` key is a pin, and a pin that is not
//! [`Report::workspace_version`] is stale. Two exclusions, each one deliberate
//! and each one narrow:
//!
//! - **A comment.** Every composite action documents its own caller in a
//!   header comment, and a release that rewrote those examples would be
//!   editing prose.
//! - **A named placeholder**, and only the ones in [`PLACEHOLDERS`]. `@YY.M.D`
//!   stands in for a version rather than naming one, so rewriting it would
//!   destroy the example.
//!
//! EVERYTHING ELSE IS A PIN, including a ref that is not a release version at
//! all. `@26.9`, `@1`, `@26.9.17+build`, and `@main` each name something other
//! than the workspace version, so each is exactly the failure this check
//! exists to catch — and a rule that recognised only well-formed versions
//! would wave all four through while reporting a clean scan.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};

/// The command both callers run. Named here so a guard can assert that neither
/// the preflight nor `ci.yml` has grown a second copy of the matching rule.
pub const COMMAND: &str = "ops release pins";

/// The checked-in configuration a self-reference may appear in.
///
/// `.github` is read WHOLE — `actions/` as much as `workflows/` — because the
/// invariant is about this repository referring to itself, not about one
/// directory. `docs/examples` is the same decision written a third time:
/// `docs/project-repositories.md` calls
/// `docs/examples/sample-portal-publish.yml` the caller the scaffold emits, so
/// a reader copies it into a Project repository and inherits whatever tag it
/// names. It sat at `26.9.14` for three releases while the check watched only
/// `.github/workflows/`.
pub const SCANNED_ROOTS: &[&str] = &[".github", "docs/examples"];

/// The prefix that makes a `uses:` value a reference to this repository.
const SELF_REFERENCE: &str = "neon-law-source-code/navigator/.github/";

/// The quote characters YAML may wrap a `uses:` value in.
const QUOTES: [char; 2] = ['"', '\''];

/// The refs that stand in for a version rather than naming one.
///
/// Exhaustive on purpose. The rule is not "anything that fails to parse as a
/// version is decoration" — a malformed ref is the most likely stale pin there
/// is — so a new placeholder has to be named here before the check will ignore
/// it.
pub const PLACEHOLDERS: &[&str] = &["YY.M.D"];

/// Strip the whitespace and YAML quoting that can sit around a `uses:` value.
///
/// A tag read with its closing quote or a trailing comment still attached
/// parses as a non-version and escapes the check entirely — a miss that looks
/// exactly like a clean scan.
fn unquote(text: &str) -> &str {
    text.trim_matches(|character: char| character.is_whitespace() || QUOTES.contains(&character))
}

/// Whether everything before the reference is a YAML `uses:` KEY, rather than
/// prose that happens to end in those five characters.
///
/// The distinction is load-bearing because [`sweep`] writes: a matcher that
/// accepted `description: See uses: …@26.9.18` would not merely over-report, it
/// would rewrite the sentence. So the whole head has to be the key — optional
/// indentation, an optional `-` sequence indicator for a step, `uses:`, and
/// then nothing but the whitespace or quote separating it from its value.
fn is_uses_key(before: &str) -> bool {
    let mut head = before.trim_start();
    // `- uses: …` is a step in a sequence. One indicator; `- - uses:` is not a
    // shape YAML produces here.
    if let Some(rest) = head.strip_prefix('-') {
        head = rest.trim_start();
    }
    unquote(head) == "uses:"
}

/// One literal self-referencing pin, located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    /// Repository-relative path, with `/` separators on every platform.
    pub file: String,
    /// One-based line number, so the report is clickable.
    pub line: usize,
    /// What is referenced, e.g. `actions/navigator-install`.
    pub target: String,
    /// The tag it names.
    pub tag: String,
}

/// Every pin the scan found, and the version they all have to name.
#[derive(Debug, Clone)]
pub struct Report {
    /// `[workspace.package].version`.
    pub workspace_version: String,
    /// Every literal self-referencing pin, in walk order.
    pub pins: Vec<Pin>,
}

impl Report {
    /// The pins naming anything but [`Report::workspace_version`].
    pub fn stale(&self) -> impl Iterator<Item = &Pin> {
        self.pins
            .iter()
            .filter(|pin| pin.tag != self.workspace_version)
    }
}

/// The literal self-referencing pin on `line`, if it carries one.
///
/// Returns `(target, tag)`. `None` covers the three uninteresting cases: the
/// line does not reference this repository, it is a comment, or its tag is a
/// placeholder rather than a version.
#[must_use]
pub fn pin_on_line(line: &str) -> Option<(&str, &str)> {
    if line.trim_start().starts_with('#') {
        return None;
    }

    let (before, after) = line.split_once(SELF_REFERENCE)?;
    if !is_uses_key(before) {
        return None;
    }

    let (target, tag) = after.split_once('@')?;
    if !target.starts_with("actions/") && !target.starts_with("workflows/") {
        return None;
    }

    // A trailing comment follows whitespace, and the closing quote is YAML's.
    // Neither is part of the ref.
    let tag = unquote(tag.split_whitespace().next()?);
    if tag.is_empty() || PLACEHOLDERS.contains(&tag) {
        return None;
    }

    Some((target, tag))
}

/// Walk [`SCANNED_ROOTS`] under `root` and locate every literal pin.
pub fn scan(root: &Path) -> Result<Report> {
    let manifest = root.join("Cargo.toml");
    let workspace_version = crate::release::workspace_version(
        &std::fs::read_to_string(&manifest)
            .with_context(|| format!("read {}", manifest.display()))?,
    )?;

    let mut pins = Vec::new();
    for scanned in SCANNED_ROOTS {
        for entry in walkdir::WalkDir::new(root.join(scanned))
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            // A file this walk cannot read as UTF-8 holds no `uses:` key.
            let Ok(source) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let file = relative(root, entry.path());
            for (index, line) in source.lines().enumerate() {
                if let Some((target, tag)) = pin_on_line(line) {
                    pins.push(Pin {
                        file: file.clone(),
                        line: index + 1,
                        target: target.to_string(),
                        tag: tag.to_string(),
                    });
                }
            }
        }
    }

    Ok(Report {
        workspace_version,
        pins,
    })
}

/// The line with its pin moved to `version`, or `None` when the line carries
/// no stale pin.
///
/// Only the ref token is replaced. The opening quote, the closing quote, and
/// any trailing comment are the line's, not the pin's, and survive untouched.
fn rewritten_line(line: &str, version: &str) -> Option<String> {
    let (_, tag) = pin_on_line(line)?;
    if tag == version {
        return None;
    }

    let (before, after) = line.split_once(SELF_REFERENCE)?;
    let (target, rest) = after.split_once('@')?;
    let offset = rest.find(tag)?;
    Some(format!(
        "{before}{SELF_REFERENCE}{target}@{}{version}{}",
        &rest[..offset],
        &rest[offset + tag.len()..]
    ))
}

/// Move every stale pin under `root` to `version`, and report what moved.
///
/// This is the other half of [`scan`], and the reason the bump commit can BE
/// the sweep rather than a thing to remember alongside it: `ops release version`
/// calls this in the same breath as it writes the manifest, so the seven pins
/// travel with the version that made them stale instead of waiting for a human
/// to edit seven lines across three files and a checker to notice they did not.
///
/// It rewrites line by line and so writes `\n` endings. Every file under the
/// scanned roots is LF today, and only a file carrying a stale pin is written
/// at all, so nothing is normalised in passing — but a CRLF workflow added
/// later would come back LF.
pub fn sweep(root: &Path, version: &str) -> Result<Vec<Pin>> {
    let mut moved = Vec::new();

    for scanned in SCANNED_ROOTS {
        for entry in walkdir::WalkDir::new(root.join(scanned))
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
        {
            let Ok(source) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let file = relative(root, entry.path());
            let mut rewritten = String::with_capacity(source.len());
            let mut changed = false;

            for (index, line) in source.lines().enumerate() {
                match rewritten_line(line, version) {
                    Some(new_line) => {
                        // The OLD tag is what the report names: it is the value
                        // the operator has to recognise as the one left behind.
                        let (target, tag) = pin_on_line(line).expect("a rewritten line has a pin");
                        moved.push(Pin {
                            file: file.clone(),
                            line: index + 1,
                            target: target.to_string(),
                            tag: tag.to_string(),
                        });
                        rewritten.push_str(&new_line);
                        changed = true;
                    }
                    None => rewritten.push_str(line),
                }
                rewritten.push('\n');
            }

            // `lines()` drops the final terminator, so only rewrite a file that
            // ended with one — otherwise the sweep would add a byte of its own.
            if changed {
                if !source.ends_with('\n') {
                    rewritten.pop();
                }
                std::fs::write(entry.path(), &rewritten)
                    .with_context(|| format!("rewrite {file}"))?;
            }
        }
    }

    Ok(moved)
}

/// `path` relative to `root`, spelled with `/` on every platform.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `navigator ops release pins`.
pub fn run(root: &Path) -> ExitCode {
    let report = match scan(root) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("navigator: {COMMAND}: {error:#}");
            return ExitCode::from(2);
        }
    };

    let stale: Vec<&Pin> = report.stale().collect();
    if !stale.is_empty() {
        eprintln!(
            "navigator: {COMMAND}: these self-referencing pins do not name {}:",
            report.workspace_version
        );
        for pin in stale {
            eprintln!(
                "  {}:{}: {} pinned at {}",
                pin.file, pin.line, pin.target, pin.tag
            );
        }
        eprintln!(
            "A tag that predates the action it names publishes a gate no consumer can run. \
             Sweep every pin above to {}.",
            report.workspace_version
        );
        return ExitCode::from(2);
    }

    println!(
        "navigator: all {} self-referencing pins name {}",
        report.pins.len(),
        report.workspace_version
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn repository_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .canonicalize()
            .expect("canonicalize the repository root")
    }

    fn read(relative: &str) -> String {
        let path = repository_root().join(relative);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }

    /// The invariant itself, asked of the tree as committed. This is the
    /// assertion that fails on the pull request moving
    /// `[workspace.package].version` without sweeping the pins — the window
    /// the release cut used to be the first to notice.
    #[test]
    fn every_checked_in_self_reference_names_the_workspace_version() {
        let report = scan(&repository_root()).expect("scan the repository for pins");
        let stale: Vec<&Pin> = report.stale().collect();

        assert!(
            stale.is_empty(),
            "these self-referencing pins do not name {}: {stale:#?}",
            report.workspace_version
        );
    }

    /// The walk really reaches the composite actions, so the exclusions below
    /// are what keep those files quiet rather than the walk never opening
    /// them.
    #[test]
    fn the_walk_reads_composite_actions_and_not_only_workflows() {
        assert!(SCANNED_ROOTS.contains(&".github"));

        let root = repository_root();
        let action = root.join(".github/actions/gate/action.yml");
        assert!(
            action.is_file(),
            "{} is the composite action this test stands on",
            action.display()
        );

        let entries: Vec<String> = SCANNED_ROOTS
            .iter()
            .flat_map(|scanned| walkdir::WalkDir::new(root.join(scanned)))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| relative(&root, entry.path()))
            .collect();

        assert!(
            entries.contains(&".github/actions/gate/action.yml".to_string()),
            "the walk never reached the composite actions: {entries:#?}"
        );
    }

    /// A real pin, in the shape both reusable workflows write it.
    #[test]
    fn finds_a_literal_pin_on_a_uses_key() {
        assert_eq!(
            pin_on_line(
                "      - uses: neon-law-source-code/navigator/.github/actions/navigator-install@26.9.17"
            ),
            Some(("actions/navigator-install", "26.9.17"))
        );
        assert_eq!(
            pin_on_line(
                "    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.9.16"
            ),
            Some(("workflows/project-gate.yml", "26.9.16"))
        );
    }

    /// A composite action's header comment shows its own caller. Rewriting
    /// those on every release would edit prose, so a comment is never a pin —
    /// even when it names a literal version, which is the case the tag rule
    /// alone would not catch.
    #[test]
    fn a_comment_is_never_a_pin() {
        for line in [
            "#     - uses: neon-law-source-code/navigator/.github/actions/gate@YY.M.D",
            "  #   - uses: neon-law-source-code/navigator/.github/actions/gate@26.9.14",
            "# uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@26.1.1",
        ] {
            assert_eq!(pin_on_line(line), None, "`{line}` is a comment");
        }
    }

    /// A named placeholder stands in for a version rather than naming one, and
    /// a release that rewrote it would destroy the example.
    #[test]
    fn a_named_placeholder_is_never_a_pin() {
        for placeholder in PLACEHOLDERS {
            let line = format!(
                "    uses: neon-law-source-code/navigator/.github/workflows/project-gate.yml@{placeholder}"
            );
            assert_eq!(pin_on_line(&line), None, "`{line}` names no version");
        }
    }

    /// A ref that is not a release version is the most likely stale pin there
    /// is, so it must be REPORTED rather than waved through. A rule that
    /// recognised only well-formed versions would ignore every one of these
    /// and still print a clean scan: `@26.9` is a truncated bump, `@1` a
    /// major-only ref no release ever carried, `@26.9.17+build` a version
    /// carrying metadata an image tag cannot hold, and `@main` a moving ref
    /// that names whatever was merged last.
    #[test]
    fn a_ref_that_is_not_a_release_version_is_still_a_pin() {
        for tag in [
            "26.9",
            "1",
            "26.9.17+build",
            "main",
            "latest",
            "v1",
            "26.08.17",
        ] {
            let line =
                format!("      - uses: neon-law-source-code/navigator/.github/actions/gate@{tag}");
            assert_eq!(
                pin_on_line(&line),
                Some(("actions/gate", tag)),
                "`@{tag}` names something other than the workspace version and must be reported"
            );
        }
    }

    /// And the report treats them as stale, which is what makes the command
    /// exit non-zero on one.
    #[test]
    fn a_malformed_ref_is_stale_against_the_workspace_version() {
        let report = Report {
            workspace_version: "26.9.17".to_string(),
            pins: vec![Pin {
                file: ".github/actions/gate/action.yml".to_string(),
                line: 40,
                target: "actions/navigator-install".to_string(),
                tag: "26.9".to_string(),
            }],
        };

        assert_eq!(report.stale().count(), 1);
    }

    /// A quoted value and a trailing comment are YAML, not part of the tag.
    /// Left attached, either one reads as a non-version and escapes the check
    /// entirely — a miss that looks exactly like a clean scan.
    #[test]
    fn a_quoted_value_or_trailing_comment_still_yields_the_tag() {
        assert_eq!(
            pin_on_line(
                r#"      - uses: "neon-law-source-code/navigator/.github/actions/gate@26.9.16""#
            ),
            Some(("actions/gate", "26.9.16"))
        );
        assert_eq!(
            pin_on_line(
                "      - uses: neon-law-source-code/navigator/.github/actions/gate@26.9.16 # swept by the cut"
            ),
            Some(("actions/gate", "26.9.16"))
        );
    }

    /// The head has to be the `uses:` KEY, not prose ending in those five
    /// characters. This one is not about over-reporting: `sweep` WRITES, so a
    /// matcher that accepted a sentence would rewrite the sentence.
    #[test]
    fn a_scalar_that_merely_contains_uses_is_not_a_pin() {
        for line in [
            "    description: See uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18",
            "      - run: echo \"uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18\"",
            "    note: uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18 is the caller",
            "    inputs-uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18",
            "    x-uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18",
        ] {
            assert_eq!(pin_on_line(line), None, "`{line}` carries no `uses:` key");
            assert_eq!(
                rewritten_line(line, "26.9.19"),
                None,
                "`{line}` must never be rewritten"
            );
        }
    }

    /// And the shapes that ARE the key keep working, in both the forms this
    /// repository writes them.
    #[test]
    fn both_real_uses_key_shapes_are_pins() {
        for line in [
            "      - uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18",
            "    uses: neon-law-source-code/navigator/.github/actions/gate@26.9.18",
            "  -   uses:   neon-law-source-code/navigator/.github/actions/gate@26.9.18",
            r#"      - uses: "neon-law-source-code/navigator/.github/actions/gate@26.9.18""#,
        ] {
            assert_eq!(
                pin_on_line(line),
                Some(("actions/gate", "26.9.18")),
                "`{line}` is a `uses:` key"
            );
        }
    }

    /// The sweep leaves a file whose only mention is prose byte for byte.
    #[test]
    fn the_sweep_does_not_rewrite_prose() {
        let root = tempfile::tempdir().expect("a temporary checkout");
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[workspace.package]\nversion = \"26.9.19\"\n",
        )
        .expect("write the manifest");
        let dir = root.path().join(".github/actions/prose");
        std::fs::create_dir_all(&dir).expect("create the directory");
        let file = dir.join("action.yml");
        let prose = "description: See uses: \
                     neon-law-source-code/navigator/.github/actions/gate@26.9.18 for the gate\n";
        std::fs::write(&file, prose).expect("write the prose");

        let moved = sweep(root.path(), "26.9.19").expect("sweep the fixture");

        assert!(moved.is_empty(), "prose is not a pin: {moved:#?}");
        assert_eq!(
            std::fs::read_to_string(&file).expect("re-read"),
            prose,
            "the sentence must come back byte for byte"
        );
    }

    /// Prose that mentions the reference is not a `uses:` key, and neither is
    /// a reference to some other repository.
    #[test]
    fn prose_and_foreign_actions_are_never_pins() {
        for line in [
            "See neon-law-source-code/navigator/.github/actions/gate@26.9.17 for the gate.",
            "      - uses: actions/checkout@26.9.17",
            "      - uses: neon-law-source-code/navigator/.github/CODEOWNERS@26.9.17",
            "      - uses: neon-law-source-code/navigator/.github/actions/gate",
        ] {
            assert_eq!(pin_on_line(line), None, "`{line}` is not a self-reference");
        }
    }

    /// A stale pin is reported with the file and line that carries it.
    #[test]
    fn stale_names_only_the_pins_that_miss_the_version() {
        let report = Report {
            workspace_version: "26.9.17".to_string(),
            pins: vec![
                Pin {
                    file: ".github/workflows/project-gate.yml".to_string(),
                    line: 88,
                    target: "actions/navigator-install".to_string(),
                    tag: "26.9.17".to_string(),
                },
                Pin {
                    file: ".github/actions/gate/action.yml".to_string(),
                    line: 40,
                    target: "actions/navigator-install".to_string(),
                    tag: "26.9.16".to_string(),
                },
            ],
        };

        let stale: Vec<&Pin> = report.stale().collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].file, ".github/actions/gate/action.yml");
        assert_eq!(stale[0].tag, "26.9.16");
    }

    /// A checkout with one stale pin, so the command can be asked what it
    /// does with one.
    fn fixture(pin_tag: &str) -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("a temporary checkout");
        std::fs::write(
            root.path().join("Cargo.toml"),
            "[workspace.package]\nversion = \"26.9.17\"\n",
        )
        .expect("write the manifest");

        let actions = root.path().join(".github/actions/gate");
        std::fs::create_dir_all(&actions).expect("create the composite action directory");
        std::fs::write(
            actions.join("action.yml"),
            format!(
                "# - uses: neon-law-source-code/navigator/.github/actions/gate@YY.M.D\nruns:\n                   steps:\n    - uses:                  neon-law-source-code/navigator/.github/actions/navigator-install@{pin_tag}\n"
            ),
        )
        .expect("write the composite action");

        root
    }

    /// The command answers `0` when every pin names the version and `2` when
    /// one does not. The stale case is the one a release cut is asking about,
    /// and a non-zero exit is the whole contract both callers rely on.
    #[test]
    fn the_command_exits_zero_on_a_swept_checkout_and_two_on_a_stale_one() {
        let swept = fixture("26.9.17");
        assert_eq!(run(swept.path()), ExitCode::SUCCESS);

        let stale = fixture("26.9.16");
        assert_eq!(run(stale.path()), ExitCode::from(2));
    }

    /// A checkout with no manifest cannot be judged, and the command says so
    /// rather than reporting a clean scan of nothing.
    #[test]
    fn a_checkout_without_a_manifest_is_an_error_rather_than_a_pass() {
        let empty = tempfile::tempdir().expect("a temporary directory");

        assert!(scan(empty.path()).is_err());
        assert_eq!(run(empty.path()), ExitCode::from(2));
    }

    /// The sweep is what makes the bump commit complete. It moves the stale
    /// pins and leaves everything else on the line alone — the quoting, the
    /// trailing comment, the placeholders, and the comment examples that a
    /// release rewriting them would destroy.
    #[test]
    fn the_sweep_moves_stale_pins_and_edits_nothing_else() {
        let root = fixture("26.9.16");
        let action = root.path().join(".github/actions/gate/action.yml");
        let before = std::fs::read_to_string(&action).expect("read the fixture");
        assert!(
            before.contains("@YY.M.D"),
            "the fixture carries a comment example"
        );

        let moved = sweep(root.path(), "26.9.17").expect("sweep the fixture");

        assert_eq!(moved.len(), 1);
        assert_eq!(
            moved[0].tag, "26.9.16",
            "the report names the tag left behind"
        );

        let after = std::fs::read_to_string(&action).expect("read the swept fixture");
        assert!(after.contains("navigator-install@26.9.17"));
        assert!(!after.contains("@26.9.16"));
        assert!(
            after.contains("# - uses: neon-law-source-code/navigator/.github/actions/gate@YY.M.D"),
            "the comment example must survive the sweep verbatim: {after}"
        );
        assert!(after.ends_with('\n'), "the trailing newline is preserved");

        // And the invariant now holds, which is the whole point.
        let report = scan(root.path()).expect("re-scan the swept checkout");
        assert_eq!(report.stale().count(), 0);
    }

    /// A sweep with nothing to do writes nothing and reports nothing, so
    /// re-running the bump is not a source of spurious diffs.
    #[test]
    fn a_sweep_with_nothing_stale_is_a_no_op() {
        let root = fixture("26.9.17");
        let action = root.path().join(".github/actions/gate/action.yml");
        let before = std::fs::read_to_string(&action).expect("read the fixture");

        let moved = sweep(root.path(), "26.9.17").expect("sweep the fixture");

        assert!(moved.is_empty());
        assert_eq!(
            std::fs::read_to_string(&action).expect("re-read"),
            before,
            "an already-swept checkout must come back byte for byte"
        );
    }

    /// Quoting and a trailing comment belong to the line, not the pin.
    #[test]
    fn the_sweep_replaces_the_ref_and_leaves_the_line_around_it() {
        assert_eq!(
            rewritten_line(
                r#"      - uses: "neon-law-source-code/navigator/.github/actions/gate@26.9.16""#,
                "26.9.17"
            )
            .as_deref(),
            Some(r#"      - uses: "neon-law-source-code/navigator/.github/actions/gate@26.9.17""#)
        );
        assert_eq!(
            rewritten_line(
                "      - uses: neon-law-source-code/navigator/.github/actions/gate@26.9.16 # pinned",
                "26.9.17"
            )
            .as_deref(),
            Some("      - uses: neon-law-source-code/navigator/.github/actions/gate@26.9.17 # pinned")
        );
        assert_eq!(
            rewritten_line(
                "# - uses: neon-law-source-code/navigator/.github/actions/gate@YY.M.D",
                "26.9.17"
            ),
            None,
            "a comment example is not a pin and must not be rewritten"
        );
    }

    /// One command, two callers. The preflight must not carry its own matcher,
    /// or the release path and the pull-request path can disagree about what a
    /// stale pin is.
    #[test]
    fn the_cut_release_preflight_delegates_to_the_command() {
        let preflight = read(".agents/skills/cut-release/scripts/preflight.sh");

        assert!(
            preflight.contains(COMMAND),
            "preflight.sh must run `{COMMAND}`"
        );
        assert!(
            !preflight.contains(SELF_REFERENCE),
            "preflight.sh still matches pins itself; the rule belongs in `release_pins` alone"
        );
    }

    /// The pull request is the last place a stale pin is still free to fix, so
    /// the same command is a required check there — and it has to sit in a job
    /// that ACTUALLY RUNS on the pull requests that can break it.
    ///
    /// Presence in the file is not enough. `rust` is scheduled by a path
    /// classifier that names no scanned root, and the merge aggregate accepts a
    /// skipped `rust`, so the check would be bypassable by exactly the PR that
    /// edits a composite action. It belongs in `gate`, which carries no `if:`.
    #[test]
    fn the_always_run_ci_gate_runs_the_same_command() {
        let workflow: serde_yaml::Value =
            serde_yaml::from_str(&read(".github/workflows/ci.yml")).expect("ci.yml parses as YAML");
        let gate = &workflow["jobs"]["gate"];

        assert!(
            gate.get("if").is_none(),
            "the `gate` job has grown an `if:`; the pin check is no longer unconditional"
        );

        let runs_it = gate["steps"]
            .as_sequence()
            .expect("the gate job has steps")
            .iter()
            .any(|step| {
                step.get("run")
                    .and_then(serde_yaml::Value::as_str)
                    .is_some_and(|run| run.contains(COMMAND))
            });
        assert!(
            runs_it,
            "the always-run `gate` job must run `{COMMAND}`: a PR touching only a scanned root \
             does not schedule `rust`, and the merge aggregate accepts a skipped `rust`"
        );
    }
}
