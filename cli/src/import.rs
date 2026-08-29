//! Catalog seeding: walk a directory of validated template markdown files and
//! persist each as a `templates` row, registering every question code
//! referenced by the file's `questionnaire:` and `workflow:` maps as a
//! `questions` row (creating each on first sight).
//!
//! Catalog seeding is intentionally idempotent: re-seeding the same
//! directory must not produce duplicate rows. Templates version through
//! `store::templates::save_version` (a no-op when the spec is
//! byte-for-byte unchanged); `question.code` carries a unique index, so
//! we look up by code before inserting and skip whenever a matching row
//! already exists.
//!
//! The CLI calls this from the `site seed` subcommand and integration tests;
//! the fixture repository lives at `templates/<category>/<name>.md`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rules::{navigator_default_rules_with_codes, DefaultFileFilter, FileFilter, Violation};
use serde::Deserialize;
use walkdir::WalkDir;

/// Load every known question code from the `questions` table and
/// return them as a strict registry suitable for passing to
/// `F104FlowQuestionCodes::new`. Used by `navigator validate` after a
/// directory has been imported so N104 can flag unknown codes.
pub async fn load_question_codes(
    surreal: &store::surreal::SurrealDb,
) -> anyhow::Result<Vec<String>> {
    let rows = store::questions::list_all(surreal).await?;
    let mut codes: BTreeSet<String> = rules::canonical_question_codes().into_iter().collect();
    codes.extend(rows.into_iter().map(|q| q.code));
    Ok(codes.into_iter().collect())
}

/// Outcome of a single site-seed run: how many templates and questions
/// were created, plus any rule violations keyed by path.
#[derive(Debug, Default)]
pub struct ImportReport {
    pub templates_created: usize,
    pub questions_created: usize,
    pub files_skipped_due_to_violations: usize,
    pub violations: Vec<Violation>,
}

#[derive(Debug, Deserialize)]
struct TemplateFrontmatter {
    title: Option<String>,
    respondent_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
    /// forms-registry code (`form:` key) for a government-form template.
    #[serde(default)]
    form: Option<String>,
    /// Declared notation kind from the `kind:` key; mirrors
    /// `store::template_source::Frontmatter` — see [`persist_template`]
    /// for why this must reach `store::templates::save_version`.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default)]
    workflow: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// Walk `dir`, validate every `*.md` (with the default Neon Law Navigator rule
/// set minus N104 question-code validation since we're populating the
/// registry as we go), and insert one `templates` row + one `questions`
/// row per referenced question code. Files with any rule violation
/// are skipped — they're recorded in [`ImportReport::violations`] for
/// the caller to report.
pub async fn import_directory(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    dir: &Path,
) -> anyhow::Result<ImportReport> {
    let mut report = ImportReport::default();
    let validation_rules = navigator_default_rules_with_codes(&[]);
    // Catalog seeding applies the notation rule set to every candidate `.md`.
    // Its explicit filter keeps repository prose such as `templates/README.md`
    // and a top-level `CLAUDE.md` outside the catalog. Files with template
    // structure still reach the rules, which report a missing `kind:`.
    let filter = DefaultFileFilter {
        excluded_names: [
            "README.md",
            "CLAUDE.md",
            "CODE_OF_CONDUCT.md",
            "LICENSE.md",
            "ERD.md",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        // `github` is the engineering intake shelf (`kind: github`): a
        // questionnaire that renders a GitHub issue or pull request body.
        // It is not a legal template — it has no `code`, jurisdiction, or
        // respondent — so it must never become a `templates` row. Because
        // import deliberately skips `kind:` classification, excluding the
        // directory is what keeps it out.
        excluded_directories: ["AgentDocumentation", "Blog", rules::GITHUB_SHELF]
            .into_iter()
            .map(str::to_string)
            .collect(),
    };
    for entry in WalkDir::new(dir).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !filter.include_file(path) {
            continue;
        }
        let contents = std::fs::read_to_string(path)?;
        let file = rules::SourceFile {
            path: PathBuf::from(path),
            contents: contents.clone(),
        };
        let file_violations: Vec<Violation> = validation_rules
            .iter()
            .flat_map(|r| r.lint(&file))
            .collect();
        // Only blocking (Error-severity) violations skip a file. Yellow
        // advisories like N112 ("step allowed but not built yet") apply
        // to nearly every template's lawyer_review gate and must not stop
        // it from importing.
        let has_errors = file_violations
            .iter()
            .any(|v| rules::severity_for_code(v.code) == rules::Severity::Error);
        if has_errors {
            report.files_skipped_due_to_violations += 1;
            report.violations.extend(file_violations);
            continue;
        }
        if let Some(parsed) = parse_frontmatter(&contents) {
            persist_template(surreal, storage, path, &parsed, &mut report).await?;
        }
    }
    Ok(report)
}

fn parse_frontmatter(contents: &str) -> Option<TemplateFrontmatter> {
    let fm = rules::frontmatter::extract(contents)?;
    serde_yaml::from_str(fm).ok()
}

/// Derive the template code from frontmatter or fall back to the
/// filename stem.
fn template_code(path: &Path, fm: &TemplateFrontmatter) -> String {
    fm.code.clone().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string()
    })
}

async fn persist_template(
    surreal: &store::surreal::SurrealDb,
    storage: &std::sync::Arc<dyn cloud::StorageService>,
    path: &Path,
    fm: &TemplateFrontmatter,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    let code = template_code(path, fm);
    // The body lives in a content-addressed asset, not an inline column.
    // Ingest the file contents and reference the asset — `ingest_content`
    // dedups by SHA-256, so re-seeding unchanged bytes costs no new write.
    let body = std::fs::read_to_string(path)?;
    let asset_id =
        store::assets::ingest_content(surreal, storage, body.as_bytes(), "text/markdown").await?;
    // Route through `save_version` (the same seam `template_source`'s
    // repo-backed import uses) rather than a raw insert: it appends a new
    // version when the spec changed, is a no-op when it didn't, and — the
    // reason this issue cares — actually carries `kind`/`form_code`
    // through instead of dropping them on the floor. `source_commit_sha`
    // is `None`: the workspace catalog is seeded from bundled files, not a
    // repo commit, and the version-identity tuple deliberately excludes it
    // anyway (provenance only).
    let saved = store::templates::save_version(
        surreal,
        None,
        &code,
        store::templates::Version {
            title: fm.title.clone().unwrap_or_else(|| code.clone()),
            respondent_type: fm
                .respondent_type
                .clone()
                .unwrap_or_else(|| "entity".into()),
            asset_id: Some(asset_id),
            form_code: fm.form.clone(),
            kind: fm.kind.clone(),
            source_commit_sha: None,
        },
    )
    .await?;
    if saved.was_written() {
        report.templates_created += 1;
    }

    // Collect every question code referenced by either map. State keys
    // may carry a `__label` suffix; the question code is the prefix.
    // `BEGIN` and `END` are control states and never registered;
    // `lawyer_review` is a workflow state but we register it so a later
    // `N104` pass keyed on the populated `questions` table doesn't
    // flag it as unknown.
    let mut codes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for map in [&fm.questionnaire, &fm.workflow].into_iter().flatten() {
        for state in map.keys() {
            if state == "BEGIN" || state == "END" {
                continue;
            }
            let prefix = state.split_once("__").map_or(state.as_str(), |(p, _)| p);
            codes.insert(prefix.to_string());
        }
    }
    for q_code in codes {
        if store::questions::find_by_code(surreal, &q_code)
            .await?
            .is_some()
        {
            continue;
        }
        // `find_or_create` rather than a bare create: two imports racing on
        // one auto-imported code would otherwise surface `CodeTaken`.
        store::questions::find_or_create(
            surreal,
            &store::questions::NewQuestion::new(
                q_code.clone(),
                format!("(auto-imported) {q_code}"),
                "string",
            ),
        )
        .await?;
        report.questions_created += 1;
    }
    Ok(())
}
