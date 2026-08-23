//! `navigator template github` — drive the engineering intake notations under
//! `templates/github/`.
//!
//! Two commands over one shelf:
//!
//! - `render` fills a notation's `{{…}}` placeholders from `--answer`
//!   pairs and prints the resulting Markdown. Local and DB-free, the way
//!   `navigator template render` is: it validates the file against the same rule
//!   set first, so a notation that would fail `validate` never renders.
//!   This is the command that produces a pull-request body for a human to
//!   paste — it opens nothing.
//! - `open-issue` renders `create_issue.md` the same way and then opens
//!   the issue through [`workflows::github::IssueOpener`] — the *same*
//!   seam the `github_issue__*` workflow step dispatches through, so the
//!   CLI and the durable step cannot drift into two different clients.
//!
//! Neither command shells out to the `gh` CLI. The issue is created with
//! one authenticated `POST /repos/{owner}/{repo}/issues` from `reqwest`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use workflows::github::{
    default_repo_from_env, issue_opener_from_env, GithubIssuePayload, IssueRequest,
};

/// The `templates/github/` notation a command operates on. The shelf is
/// closed (rule `N119`), so this is an enum rather than a free path — a
/// third value would be a rule violation anyway.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Notation {
    /// `templates/github/create_issue.md`
    CreateIssue,
    /// `templates/github/create_pull_request.md`
    CreatePullRequest,
}

impl Notation {
    /// The filename stem, which is also the notation's identity.
    #[must_use]
    pub fn stem(self) -> &'static str {
        match self {
            Self::CreateIssue => "create_issue",
            Self::CreatePullRequest => "create_pull_request",
        }
    }

    /// Path to the notation, relative to the workspace root.
    #[must_use]
    pub fn path(self) -> PathBuf {
        PathBuf::from("templates")
            .join(rules::GITHUB_SHELF)
            .join(format!("{}.md", self.stem()))
    }
}

/// The workspace root: these commands read the shipped notations, so walk
/// up from the current directory to the first ancestor carrying
/// `templates/github`. Mirrors `forms_sync::workspace_root`.
///
/// # Errors
///
/// Returns an error naming the directory searched when no ancestor holds
/// the shelf, so the fix ("run from the checkout") is obvious.
pub fn workspace_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("current directory: {e}"))?;
    cwd.ancestors()
        .find(|p| p.join("templates").join(rules::GITHUB_SHELF).is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            format!(
                "no `templates/{}` directory above {} — run from the workspace checkout",
                rules::GITHUB_SHELF,
                cwd.display(),
            )
        })
}

/// Read, validate, and fill `notation`'s body from `answers`.
///
/// Validation uses the same classified rule set as `validate`, so the
/// GitHub notation is held to `N119` and the questionnaire-grammar rules
/// before anything is rendered. Warning-severity advisories are printed
/// but do not block, mirroring `navigator template render`.
fn render_body(
    root: &Path,
    notation: Notation,
    answers: &[(String, String)],
) -> Result<String, ExitCode> {
    let path = root.join(notation.path());
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        eprintln!("navigator: read {}: {e}", path.display());
        ExitCode::from(2)
    })?;

    let source = rules::SourceFile {
        path: notation.path(),
        contents: contents.clone(),
    };
    let violations: Vec<rules::Violation> = rules::navigator_classified_rules(&source)
        .iter()
        .flat_map(|r| r.lint(&source))
        .collect();
    let errors = violations
        .iter()
        .filter(|v| rules::severity_for_code(v.code) == rules::Severity::Error)
        .count();
    for v in &violations {
        eprintln!(
            "  {}:{} {}: {}",
            v.path.display(),
            v.line,
            v.code,
            v.message
        );
    }
    if errors > 0 {
        eprintln!("navigator: {errors} validation error(s); not rendering");
        return Err(ExitCode::from(1));
    }

    let context: BTreeMap<String, String> = answers.iter().cloned().collect();
    let body = views::notation::fill(strip_frontmatter(&contents), &context);
    Ok(body)
}

/// Everything after the leading frontmatter block.
fn strip_frontmatter(contents: &str) -> &str {
    let Some(after_open) = contents.strip_prefix("---\n") else {
        return contents;
    };
    if let Some(end) = after_open.find("\n---\n") {
        return after_open[end + "\n---\n".len()..].trim_start_matches('\n');
    }
    after_open.strip_suffix("\n---").map_or(contents, |_| "")
}

/// The `{{…}}` tokens no `--answer` filled, in first-seen order.
///
/// Loop directives (`{{#for …}}` / `{{/for}}`) are not placeholders, so
/// they are skipped. `render` reports these and continues — a half-filled
/// draft is a legitimate thing to print — but `open-issue` refuses on
/// them, because a literal `{{custom_text__observed_problem}}` posted to
/// GitHub is a public artifact that cannot be un-sent.
fn unfilled_placeholders(body: &str) -> Vec<&str> {
    let mut unfilled: Vec<&str> = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let token = after[..end].trim();
        if !token.starts_with('#') && !token.starts_with('/') && !unfilled.contains(&token) {
            unfilled.push(token);
        }
        rest = &after[end + 2..];
    }
    unfilled
}

/// Report unfilled placeholders without failing — the `render` contract.
fn warn_on_unfilled(body: &str) {
    let unfilled = unfilled_placeholders(body);
    if !unfilled.is_empty() {
        eprintln!(
            "navigator: {} placeholder(s) left unfilled and will render verbatim: {}",
            unfilled.len(),
            unfilled.join(", "),
        );
    }
}

/// The notation's frontmatter `title`, used as the issue title default.
fn declared_title(root: &Path, notation: Notation) -> Option<String> {
    let contents = std::fs::read_to_string(root.join(notation.path())).ok()?;
    let fm = rules::frontmatter::extract(&contents)?;
    rules::frontmatter::field(fm, "title").filter(|t| !t.is_empty())
}

/// `navigator template github render` — print the filled notation body.
pub fn run_render(
    root: &Path,
    notation: Notation,
    answers: &[(String, String)],
    out: Option<&Path>,
) -> ExitCode {
    let body = match render_body(root, notation, answers) {
        Ok(body) => body,
        Err(code) => return code,
    };
    warn_on_unfilled(&body);
    match out {
        Some(path) => {
            if let Err(e) = std::fs::write(path, &body) {
                eprintln!("navigator: write {}: {e}", path.display());
                return ExitCode::from(2);
            }
            eprintln!("navigator: wrote {} ({} bytes)", path.display(), body.len());
        }
        None => print!("{body}"),
    }
    ExitCode::SUCCESS
}

/// `navigator template github open-issue` — render `create_issue.md` and open it.
///
/// `dry_run` renders and reports the target without calling GitHub, so the
/// exact request can be inspected before anything is created.
pub async fn run_open_issue(
    root: &Path,
    answers: &[(String, String)],
    repo: Option<&str>,
    title: Option<&str>,
    labels: &[String],
    dry_run: bool,
) -> ExitCode {
    let body = match render_body(root, Notation::CreateIssue, answers) {
        Ok(body) => body,
        Err(code) => return code,
    };
    // Refuse rather than warn: an issue is a public artifact that cannot
    // be un-sent, so a literal `{{custom_text__observed_problem}}` must
    // never reach GitHub. `render` still prints a half-filled draft.
    let unfilled = unfilled_placeholders(&body);
    if !unfilled.is_empty() {
        eprintln!(
            "navigator: refusing to open an issue with {} unanswered question(s): {}\n\
             navigator: pass --answer <code>=<value> for each, or use `navigator template github render` \
             to inspect the draft",
            unfilled.len(),
            unfilled.join(", "),
        );
        return ExitCode::from(2);
    }
    let title = title
        .map(str::to_string)
        .or_else(|| declared_title(root, Notation::CreateIssue))
        .unwrap_or_else(|| "Engineering issue".to_string());

    let payload = GithubIssuePayload {
        repo: repo.map(str::to_string),
        title,
        body,
        labels: labels.to_vec(),
    };
    let request = match IssueRequest::from_payload(&payload, default_repo_from_env().as_deref()) {
        Ok(request) => request,
        Err(e) => {
            eprintln!("navigator: {e}");
            return ExitCode::from(2);
        }
    };

    if dry_run {
        eprintln!(
            "navigator: dry run — would open `{}` in {} ({} body bytes, labels: {})",
            request.title,
            request.slug(),
            request.body.len(),
            if request.labels.is_empty() {
                "none".to_string()
            } else {
                request.labels.join(", ")
            },
        );
        return ExitCode::SUCCESS;
    }

    match issue_opener_from_env().open_issue(&request).await {
        Ok(Some(issue)) => {
            println!("{}", issue.html_url);
            eprintln!(
                "navigator: opened issue #{} in {}",
                issue.number,
                request.slug()
            );
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!(
                "navigator: no GitHub token configured (set NAVIGATOR_GITHUB_TOKEN or \
                 GITHUB_TOKEN); nothing was opened"
            );
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("navigator: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{strip_frontmatter, ExitCode, Notation};

    #[test]
    fn each_notation_maps_to_its_file_on_the_closed_shelf() {
        assert_eq!(
            Notation::CreateIssue.path(),
            std::path::Path::new("templates/github/create_issue.md")
        );
        assert_eq!(
            Notation::CreatePullRequest.path(),
            std::path::Path::new("templates/github/create_pull_request.md")
        );
        // The stems are the ones `N119` pins, so the CLI and the rule
        // cannot name different files.
        let pinned: Vec<&str> = rules::GITHUB_NOTATIONS
            .iter()
            .map(|(stem, _)| *stem)
            .collect();
        assert!(pinned.contains(&Notation::CreateIssue.stem()));
        assert!(pinned.contains(&Notation::CreatePullRequest.stem()));
        assert_eq!(pinned.len(), 2, "the shelf is closed: {pinned:?}");
    }

    /// `open-issue` must refuse a body with unanswered questions: an
    /// issue is public and cannot be un-sent, so a literal
    /// `{{custom_text__observed_problem}}` must never reach GitHub. The
    /// dry run is enough to prove the gate, since it sits before any
    /// network call.
    #[tokio::test]
    async fn open_issue_refuses_a_body_with_unanswered_questions() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let code = super::run_open_issue(
            &root,
            // Only one of the seven questions answered.
            &[(
                "custom_text__observed_problem".to_string(),
                "Something is wrong.".to_string(),
            )],
            Some("neon-law-source-code/navigator"),
            None,
            &[],
            true,
        )
        .await;
        assert_eq!(
            code,
            ExitCode::from(2),
            "a partially answered issue must not be openable",
        );
    }

    /// The mirror: a fully answered body passes the gate and reaches the
    /// dry-run report.
    #[tokio::test]
    async fn open_issue_accepts_a_fully_answered_body() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let answers: Vec<(String, String)> = [
            ("custom_single_choice__change_surface", "infrastructure"),
            ("custom_yes_no__engineering_council", "no"),
            ("custom_text__observed_problem", "Something is wrong."),
            ("custom_text__grounded_scope", "Only this."),
            ("custom_text__acceptance_criteria", "It works."),
            ("custom_text__covering_tests", "This test."),
            ("custom_text__blast_radius", "cli/src/github.rs"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
        let code = super::run_open_issue(
            &root,
            &answers,
            Some("neon-law-source-code/navigator"),
            None,
            &[],
            true,
        )
        .await;
        assert_eq!(code, ExitCode::SUCCESS);
    }

    /// Loop directives are structure, not unanswered questions.
    #[test]
    fn loop_directives_are_not_counted_as_placeholders() {
        let body = "{{#for m in members}}{{m.name}}{{/for}} {{unfilled}}";
        assert_eq!(
            super::unfilled_placeholders(body),
            vec!["m.name", "unfilled"]
        );
    }

    #[test]
    fn frontmatter_is_stripped_leaving_the_body() {
        let doc = "---\nkind: github\ntitle: T\n---\n\n## Heading\n\nBody.\n";
        assert_eq!(strip_frontmatter(doc), "## Heading\n\nBody.\n");
    }

    /// The real shelf renders through the real rule set with real answers.
    /// This is the dogfooding path the docs describe, run as a test so it
    /// cannot rot.
    #[test]
    fn the_shipped_notations_render_with_answers_substituted() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        for (notation, code, value) in [
            (
                Notation::CreateIssue,
                "custom_text__observed_problem",
                "Engineering work has no intake notation.",
            ),
            (
                Notation::CreatePullRequest,
                "custom_text__change_summary",
                "Adds the github notation shelf.",
            ),
        ] {
            let answers = vec![
                (code.to_string(), value.to_string()),
                (
                    "custom_single_choice__change_surface".to_string(),
                    "infrastructure".to_string(),
                ),
            ];
            let body = super::render_body(&root, notation, &answers)
                .unwrap_or_else(|_| panic!("{} should render", notation.stem()));
            assert!(
                body.contains(value),
                "{} did not substitute the answer: {body}",
                notation.stem(),
            );
            assert!(
                body.contains("infrastructure"),
                "{} did not substitute the change surface: {body}",
                notation.stem(),
            );
            assert!(
                !body.contains(&format!("{{{{{code}}}}}")),
                "{} left the placeholder verbatim",
                notation.stem(),
            );
        }
    }
}
