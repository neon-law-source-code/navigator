//! `N119` — the GitHub notation contract.
//!
//! A `kind: github` file is the engineering intake notation: the
//! questionnaire that gathers what a GitHub issue or pull request needs
//! before it is opened, and the body that renders the answers into the
//! text that gets posted. It reuses the questionnaire grammar the legal
//! notations use, so an author already knows how to read it — but it is
//! not a legal instrument, and this rule is what keeps the shelf from
//! drifting into one.
//!
//! Four things are pinned:
//!
//! 1. **The shelf is closed.** A GitHub notation lives at
//!    `templates/github/<stem>.md` and its stem is one of exactly two
//!    values — [`GITHUB_NOTATIONS`]. There is no third artifact to open on
//!    GitHub, so a third file is a mistake, not an extension point.
//! 2. **Every notation classifies its change surface.** Both files ask
//!    [`CHANGE_SURFACE_STATE`], whose options are the closed
//!    [`CHANGE_SURFACES`] set. The surface is what decides which gate the
//!    work has to clear, so it is asked before anything is written, and it
//!    is asked identically on the issue and the pull request — the answer
//!    carries from one to the other.
//! 3. **Every notation decides on the Engineering Council.** Both files
//!    ask [`ENGINEERING_COUNCIL_STATE`]. `CLAUDE.md` says councils are used
//!    "only when earned"; asking makes that judgment explicit and recorded
//!    rather than a thing someone remembers to consider.
//! 4. **Every notation carries narrative.** At least one `custom_text__*`
//!    state must exist — a classification with no prose is an issue with no
//!    body.
//!
//! The rule only fires on a file that declares `kind: github`, so it is
//! silent on every legal template and every content page.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{frontmatter, line_byte_range, Rule, SourceFile, Violation};

/// The directory a GitHub notation lives in, directly under `templates/`.
pub const GITHUB_SHELF: &str = "github";

/// The closed set of GitHub notations, as `(filename stem, what it opens)`.
///
/// Two artifacts can be opened on GitHub from a questionnaire, so the shelf
/// holds exactly two files. The second element is the hover/completion
/// description the LSP shows.
pub const GITHUB_NOTATIONS: &[(&str, &str)] = &[
    (
        "create_issue",
        "Opens a GitHub issue: the work is described but not yet done. The rendered body \
         becomes the issue body.",
    ),
    (
        "create_pull_request",
        "Opens a GitHub pull request: the work exists on a branch and is proposed for merge. \
         The rendered body becomes the pull request description.",
    ),
];

/// The questionnaire state that classifies what the change touches.
pub const CHANGE_SURFACE_STATE: &str = "custom_single_choice__change_surface";

/// The questionnaire state that decides whether to convene the Engineering
/// Council before the work starts.
pub const ENGINEERING_COUNCIL_STATE: &str = "custom_yes_no__engineering_council";

/// The closed set of change surfaces, as `(choice key, label, what it
/// means)`.
///
/// The surface answers "what part of the workspace does this change?", and
/// it is the field that decides which gate the work has to clear — so the
/// four values are the four gates, not a taxonomy for its own sake. The
/// third element is the explanation the LSP shows on hover, so an author
/// picking a value sees the consequence rather than guessing from the label.
pub const CHANGE_SURFACES: &[(&str, &str, &str)] = &[
    (
        "web",
        "Web feature",
        "A change a person sees in a browser — a route, handler, view, or content page under \
         `web/` or `views/`. Gate: the browser and accessibility suites \
         (`navigator dev browser-e2e`), plus a captured walkthrough on the pull request.",
    ),
    (
        "api",
        "API feature",
        "A change to a contract other code calls — an HTTP/JSON route, an AIDA tool in \
         `mcp/src/tools/`, or an A2A/MCP surface. Gate: the workspace test suite, and a \
         router that implements `portal::agent_router::AgentRouter` rather than forking the \
         AIDA catalog.",
    ),
    (
        "infrastructure",
        "Infrastructure",
        "A change to how the workspace builds, tests, deploys, or runs — the `cli`, the \
         Kubernetes manifests under `k8s/`, CI workflows, or the KIND dependency tier. Gate: \
         the affected command run locally, and the deploy path left reversible.",
    ),
    (
        "form",
        "Government form",
        "A change that vendors or re-authors a government PDF form under `templates/forms/` — \
         the blank PDF, its `.fields.toml` field map, and the catalog card that binds them. \
         Gate: the `forms` crate tests and `navigator validate templates`.",
    ),
];

/// The hover/completion description for a change-surface choice key, or
/// `None` when `token` is not one of [`CHANGE_SURFACES`].
#[must_use]
pub fn describe_change_surface(token: &str) -> Option<String> {
    CHANGE_SURFACES
        .iter()
        .find(|(key, _, _)| *key == token)
        .map(|(key, label, explanation)| format!("**`{key}`** — {label}\n\n{explanation}"))
}

/// The hover/completion description for a GitHub notation filename stem,
/// or `None` when `stem` is not one of [`GITHUB_NOTATIONS`].
#[must_use]
pub fn describe_github_notation(stem: &str) -> Option<String> {
    GITHUB_NOTATIONS
        .iter()
        .find(|(name, _)| *name == stem)
        .map(|(name, summary)| format!("**`{name}`** — {summary}"))
}

/// The two stems, for a diagnostic's "expected one of" list.
fn expected_stems() -> String {
    GITHUB_NOTATIONS
        .iter()
        .map(|(stem, _)| format!("`{stem}`"))
        .collect::<Vec<_>>()
        .join(" or ")
}

pub struct F119GithubNotation;

impl F119GithubNotation {
    pub const CODE: &'static str = "N119";
}

#[derive(Debug, Default, Deserialize)]
struct FrontmatterShape {
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default)]
    custom_questions: BTreeMap<String, CustomQuestionShape>,
}

#[derive(Debug, Default, Deserialize)]
struct CustomQuestionShape {
    #[serde(default)]
    choices: BTreeMap<String, String>,
}

impl Rule for F119GithubNotation {
    fn code(&self) -> &'static str {
        Self::CODE
    }

    fn description(&self) -> &'static str {
        crate::description_for_code(Self::CODE)
    }

    fn lint(&self, file: &SourceFile) -> Vec<Violation> {
        // Fires only on a declared GitHub notation. Everything else — a
        // legal template, a blog post, plain prose — is another family's
        // business.
        if crate::kind::declared(&file.contents) != Some(crate::Kind::Github) {
            return Vec::new();
        }

        let mut violations = Vec::new();
        Self::check_location(file, &mut violations);

        let Some(fm) = frontmatter::extract(&file.contents) else {
            return violations;
        };
        let Ok(parsed) = serde_yaml::from_str::<FrontmatterShape>(fm) else {
            return violations;
        };
        let Some(questionnaire) = parsed.questionnaire else {
            violations.push(Self::at_line(
                file,
                1,
                "A GitHub notation must declare a `questionnaire:` — it is the intake that \
                 gathers what the issue or pull request needs"
                    .to_string(),
            ));
            return violations;
        };

        Self::check_required_states(file, &questionnaire, &mut violations);
        Self::check_change_surface_choices(file, &parsed.custom_questions, &mut violations);
        violations
    }
}

impl F119GithubNotation {
    fn at_line(file: &SourceFile, line: usize, message: String) -> Violation {
        Violation {
            code: Self::CODE,
            path: file.path.clone(),
            line,
            range: line_byte_range(&file.contents, line),
            message,
        }
    }

    /// The shelf is closed: `templates/github/{create_issue,
    /// create_pull_request}.md` and nothing else.
    fn check_location(file: &SourceFile, violations: &mut Vec<Violation>) {
        let stem = file.path.file_stem().and_then(|s| s.to_str());
        // Exactly `templates/github/<file>.md` — one level below the shelf,
        // and the shelf itself directly below `templates/`. Matching the
        // parent directory's name alone would also accept `.github/`.
        let on_shelf = segments_under_templates(&file.path)
            .is_some_and(|segments| segments.len() == 2 && segments[0] == GITHUB_SHELF);

        if !on_shelf {
            violations.push(Self::at_line(
                file,
                1,
                format!(
                    "A `kind: github` notation must live directly under `templates/{GITHUB_SHELF}/`; \
                     found `{}`",
                    file.path.display()
                ),
            ));
        }

        if !stem.is_some_and(|s| GITHUB_NOTATIONS.iter().any(|(name, _)| *name == s)) {
            violations.push(Self::at_line(
                file,
                1,
                format!(
                    "A `kind: github` notation must be named {} — found `{}`. GitHub opens \
                     exactly two things from a questionnaire, so the shelf holds exactly \
                     those two files",
                    expected_stems(),
                    stem.unwrap_or_default()
                ),
            ));
        }
    }

    /// Both notations ask the same three things: what surface the change
    /// touches, whether the Engineering Council convenes, and at least one
    /// question whose answer is prose.
    fn check_required_states(
        file: &SourceFile,
        questionnaire: &BTreeMap<String, BTreeMap<String, String>>,
        violations: &mut Vec<Violation>,
    ) {
        for (state, why) in [
            (
                CHANGE_SURFACE_STATE,
                "the change surface decides which gate the work has to clear, and it is asked \
                 identically on the issue and the pull request so the answer carries across",
            ),
            (
                ENGINEERING_COUNCIL_STATE,
                "councils are used only when earned, so the decision is asked and recorded \
                 rather than left to whoever remembers to consider it",
            ),
        ] {
            if !questionnaire.contains_key(state) {
                violations.push(Self::at_line(
                    file,
                    questionnaire_line(&file.contents),
                    format!("A GitHub notation must ask `{state}` — {why}"),
                ));
            }
        }

        if !questionnaire
            .keys()
            .any(|state| state.starts_with("custom_text__"))
        {
            violations.push(Self::at_line(
                file,
                questionnaire_line(&file.contents),
                "A GitHub notation must ask at least one `custom_text__*` question — the \
                 narrative it renders into the body. A classification with no prose is an \
                 issue with no body"
                    .to_string(),
            ));
        }
    }

    /// The change surface's options are the closed [`CHANGE_SURFACES`] set:
    /// no missing value, no invented one.
    fn check_change_surface_choices(
        file: &SourceFile,
        custom_questions: &BTreeMap<String, CustomQuestionShape>,
        violations: &mut Vec<Violation>,
    ) {
        let Some(role) = CHANGE_SURFACE_STATE.split_once("__").map(|(_, r)| r) else {
            return;
        };
        // A missing definition is `N104`'s finding (every `custom_*` state
        // needs a `custom_questions` entry); N119 only judges the options of
        // one that exists, so the two never double-flag the same line.
        let Some(question) = custom_questions.get(role) else {
            return;
        };

        let declared: Vec<&str> = question.choices.keys().map(String::as_str).collect();
        let expected: Vec<&str> = CHANGE_SURFACES.iter().map(|(key, _, _)| *key).collect();

        let missing: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|key| !declared.contains(key))
            .collect();
        let unknown: Vec<&str> = declared
            .iter()
            .copied()
            .filter(|key| !expected.contains(key))
            .collect();

        let line = custom_question_line(&file.contents, role);
        if !missing.is_empty() {
            violations.push(Self::at_line(
                file,
                line,
                format!(
                    "`custom_questions.{role}.choices` is missing the change surface(s) {} — \
                     the four surfaces are the four gates, so none of them may be dropped",
                    quoted(&missing)
                ),
            ));
        }
        if !unknown.is_empty() {
            violations.push(Self::at_line(
                file,
                line,
                format!(
                    "`custom_questions.{role}.choices` declares unknown change surface(s) {} — \
                     expected exactly {}. Add the surface to the rules crate's \
                     `CHANGE_SURFACES` (with the gate it implies) before using it here",
                    quoted(&unknown),
                    quoted(&expected)
                ),
            ));
        }
    }
}

/// The path segments below the nearest `templates/` component, or `None`
/// when the file is not under a templates tree at all. Mirrors the walk
/// [`crate::F110JurisdictionPath`] uses for the legal shelves.
fn segments_under_templates(path: &std::path::Path) -> Option<Vec<&str>> {
    let mut components = path.components();
    components
        .by_ref()
        .find(|c| matches!(c, std::path::Component::Normal(seg) if *seg == "templates"))?;
    Some(
        components
            .filter_map(|c| match c {
                std::path::Component::Normal(seg) => seg.to_str(),
                _ => None,
            })
            .collect(),
    )
}

fn quoted(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The line of the top-level `questionnaire:` key, so a "missing state"
/// finding underlines the block it is missing from rather than line 1.
fn questionnaire_line(contents: &str) -> usize {
    contents
        .lines()
        .position(|line| line == "questionnaire:")
        .map_or(1, |idx| idx + 1)
}

/// The line of a `custom_questions.<role>:` entry, so a choices finding
/// underlines the question it is about.
fn custom_question_line(contents: &str, role: &str) -> usize {
    let key = format!("{role}:");
    for (idx, raw) in contents.lines().enumerate() {
        let trimmed = raw.trim_start();
        // Indented, so a body line reading `change_surface:` at column 0
        // cannot be mistaken for the frontmatter entry.
        if trimmed == key && trimmed.len() < raw.len() {
            return idx + 1;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::{
        describe_change_surface, describe_github_notation, F119GithubNotation, CHANGE_SURFACES,
        CHANGE_SURFACE_STATE, ENGINEERING_COUNCIL_STATE, GITHUB_NOTATIONS,
    };
    use crate::{Rule, SourceFile};
    use std::path::PathBuf;

    /// A GitHub notation that satisfies the whole contract — the baseline
    /// every negative test perturbs one field of.
    fn valid_frontmatter() -> String {
        "kind: github
title: Create a GitHub issue
questionnaire:
  BEGIN:
    _: custom_single_choice__change_surface
  custom_single_choice__change_surface:
    _: custom_yes_no__engineering_council
  custom_yes_no__engineering_council:
    _: custom_text__change_summary
  custom_text__change_summary:
    _: END
  END: {}
custom_questions:
  change_surface:
    prompt: What does this change touch?
    choices:
      web: Web feature
      api: API feature
      infrastructure: Infrastructure
      form: Government form
  engineering_council:
    prompt: Should the Engineering Council review this before work starts?
  change_summary:
    prompt: Describe the change.
"
        .to_string()
    }

    fn at(path: &str, fm: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            contents: format!("---\n{fm}---\n\nBody.\n"),
        }
    }

    fn valid_at(path: &str) -> SourceFile {
        at(path, &valid_frontmatter())
    }

    #[test]
    fn accepts_both_canonical_notations() {
        for (stem, _) in GITHUB_NOTATIONS {
            let v = F119GithubNotation.lint(&valid_at(&format!("templates/github/{stem}.md")));
            assert!(v.is_empty(), "{stem} should be clean: {v:?}");
        }
    }

    #[test]
    fn flags_a_third_file_on_the_shelf() {
        let v = F119GithubNotation.lint(&valid_at("templates/github/create_discussion.md"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert_eq!(v[0].code, "N119");
        assert!(v[0].message.contains("create_issue"), "{v:?}");
        assert!(v[0].message.contains("create_pull_request"), "{v:?}");
    }

    #[test]
    fn flags_a_github_notation_off_the_shelf() {
        let v = F119GithubNotation.lint(&valid_at("templates/neon_law/shared/create_issue.md"));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("templates/github/"), "{v:?}");
    }

    /// A directory named `github` is not the shelf — only `templates/github`
    /// is, so a `.github/` file cannot pass the location check by name.
    #[test]
    fn flags_a_github_notation_outside_the_templates_tree() {
        for path in [
            ".github/create_issue.md",
            "github/create_issue.md",
            "templates/github/nested/create_issue.md",
        ] {
            let v = F119GithubNotation.lint(&valid_at(path));
            assert!(
                v.iter().any(|v| v.message.contains("templates/github/")),
                "{path} should be off-shelf: {v:?}",
            );
        }
    }

    #[test]
    fn flags_a_missing_change_surface_question() {
        let fm = valid_frontmatter().replace(
            "  custom_single_choice__change_surface:\n    _: custom_yes_no__engineering_council\n",
            "",
        );
        let fm = fm.replace(
            "    _: custom_single_choice__change_surface",
            "    _: custom_yes_no__engineering_council",
        );
        let v = F119GithubNotation.lint(&at("templates/github/create_issue.md", &fm));
        assert!(
            v.iter().any(|v| v.message.contains(CHANGE_SURFACE_STATE)),
            "{v:?}"
        );
    }

    #[test]
    fn flags_a_missing_engineering_council_question() {
        let fm = valid_frontmatter().replace(
            "  custom_yes_no__engineering_council:\n    _: custom_text__change_summary\n",
            "",
        );
        let fm = fm.replace(
            "    _: custom_yes_no__engineering_council",
            "    _: custom_text__change_summary",
        );
        let v = F119GithubNotation.lint(&at("templates/github/create_issue.md", &fm));
        assert!(
            v.iter()
                .any(|v| v.message.contains(ENGINEERING_COUNCIL_STATE)),
            "{v:?}"
        );
        assert!(v.iter().any(|v| v.message.contains("only when earned")));
    }

    #[test]
    fn flags_a_notation_with_no_narrative_question() {
        let fm = valid_frontmatter()
            .replace("  custom_text__change_summary:\n    _: END\n", "")
            .replace("    _: custom_text__change_summary", "    _: END");
        let v = F119GithubNotation.lint(&at("templates/github/create_issue.md", &fm));
        assert!(
            v.iter().any(|v| v.message.contains("custom_text__*")),
            "{v:?}"
        );
    }

    #[test]
    fn flags_a_dropped_change_surface() {
        let fm = valid_frontmatter().replace("      form: Government form\n", "");
        let v = F119GithubNotation.lint(&at("templates/github/create_issue.md", &fm));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0]
            .message
            .contains("missing the change surface(s) `form`"));
    }

    #[test]
    fn flags_an_invented_change_surface() {
        let fm = valid_frontmatter().replace(
            "      form: Government form\n",
            "      form: Government form\n      mobile: Mobile app\n",
        );
        let v = F119GithubNotation.lint(&at("templates/github/create_issue.md", &fm));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("unknown change surface(s) `mobile`"));
        assert!(v[0].message.contains("CHANGE_SURFACES"));
    }

    #[test]
    fn undefined_change_surface_question_is_left_to_n104() {
        // N104 owns "every custom_* state needs a custom_questions entry".
        // N119 must stay silent so one mistake is not flagged twice.
        let fm = valid_frontmatter().replace(
            "  change_surface:
    prompt: What does this change touch?
    choices:
      web: Web feature
      api: API feature
      infrastructure: Infrastructure
      form: Government form
",
            "",
        );
        let v = F119GithubNotation.lint(&at("templates/github/create_issue.md", &fm));
        assert!(v.is_empty(), "N104's finding, not N119's: {v:?}");
    }

    #[test]
    fn silent_on_every_other_kind() {
        // The same frontmatter shape under a legal kind, at a path N119
        // would otherwise reject, must produce nothing.
        let fm = valid_frontmatter().replace("kind: github", "kind: letter");
        let v =
            F119GithubNotation.lint(&at("templates/neon_law/shared/offboarding_letter.md", &fm));
        assert!(v.is_empty(), "{v:?}");
        assert!(F119GithubNotation
            .lint(&at("docs/index.md", "title: Docs\n"))
            .is_empty());
    }

    #[test]
    fn flags_a_github_notation_with_no_questionnaire() {
        let v = F119GithubNotation.lint(&at(
            "templates/github/create_issue.md",
            "kind: github\ntitle: T\n",
        ));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].message.contains("must declare a `questionnaire:`"));
    }

    #[test]
    fn every_surface_and_notation_has_a_hover_description() {
        for (key, label, explanation) in CHANGE_SURFACES {
            let described = describe_change_surface(key)
                .unwrap_or_else(|| panic!("`{key}` needs a hover description"));
            assert!(described.contains(label), "{described}");
            assert!(described.contains(explanation), "{described}");
        }
        assert!(describe_change_surface("mobile").is_none());

        for (stem, summary) in GITHUB_NOTATIONS {
            let described = describe_github_notation(stem)
                .unwrap_or_else(|| panic!("`{stem}` needs a hover description"));
            assert!(described.contains(summary), "{described}");
        }
        assert!(describe_github_notation("create_discussion").is_none());
    }

    #[test]
    fn is_error_severity() {
        use crate::{severity_for_code, Severity};
        assert_eq!(severity_for_code("N119"), Severity::Error);
    }
}
