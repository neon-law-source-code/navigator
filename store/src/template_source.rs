//! The shared `read-repo → validate → persist` engine.
//!
//! One library function that turns a template authored as a file in a
//! Project's git repo into a persisted, immutable Template version — the
//! single seam `notation create` runs before opening a notation, called
//! identically by `web` and `cli` (docs/project-repositories.md, the
//! Shared-engine invariant of issue #252). It is layered on the three
//! seams it must not reimplement:
//!
//! - [`repos::RepoStore`] — read `templates/<code>.md` from `refs/heads/main`
//!   at HEAD and pin the commit SHA it was read from.
//! - [`rules`] — validate with the same rule set `navigator validate` runs,
//!   and **refuse on any blocking (Error-severity) violation**. Unlike the
//!   `navigator site seed` batch seeder, which *skips* a bad file, a notation
//!   must never open from an invalid template, so `create` fails loudly.
//! - [`crate::templates::save_version`] — append an immutable, project-scoped
//!   version (retiring the prior one only when the bytes changed), pinned by
//!   its content-addressed body asset (the content hash) plus the commit SHA.
//!
//! The `(commit SHA + content hash)` pin a notation inherits is exactly this
//! version: `notation.template_id` → the row this writes → its `asset_id`
//! (content hash) and `source_commit_sha` (commit SHA).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use cloud::StorageService;
use serde::Deserialize;
use uuid::Uuid;

/// Directory in a Project repo that holds template blueprints
/// (docs/project-repositories.md §"Project repository shape").
const TEMPLATES_DIR: &str = "templates";

/// A Template version persisted from a Project's git repo.
#[derive(Debug)]
pub struct Persisted {
    /// The now-current Template row the notation will pin via `template_id`.
    pub template: crate::templates::Template,
    /// Commit SHA of `refs/heads/main` the version was read from.
    pub commit_sha: String,
    /// Whether a new version row was written. `false` means the repo bytes
    /// were byte-identical to the current version — nothing changed, and
    /// the existing (earlier-provenance) row is returned unchanged.
    pub written: bool,
}

/// Errors from [`persist_from_repo`].
#[derive(Debug, thiserror::Error)]
pub enum TemplateSourceError {
    #[error("project `{0}` git repo has no commits yet — nothing to read a template from")]
    RepoEmpty(Uuid),
    #[error("no template `{code}` in the project repo (expected `templates/{code}.md` at HEAD)")]
    TemplateNotFound { code: String },
    #[error("template `{code}` has {} blocking rule violation(s); refusing to open a notation from an invalid template", .violations.len())]
    Invalid {
        code: String,
        violations: Vec<rules::Violation>,
    },
    #[error("template `{0}` body is not valid UTF-8")]
    NotUtf8(String),
    #[error("repo: {0}")]
    Repo(#[from] repos::RepoError),
    #[error("asset: {0}")]
    Asset(#[from] crate::assets::AssetError),
    #[error("template: {0}")]
    Template(#[from] crate::templates::TemplateError),
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A store lookup this module makes through a seam that owns its own
    /// error type (projects, questions).
    #[error("{0}")]
    Store(String),
}

/// The template frontmatter fields that make up a version's spec, plus the
/// question codes to register. Mirrors the shape the batch importer parses.
#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    title: Option<String>,
    respondent_type: Option<String>,
    /// forms-registry code (`form:` key) for a government-form template.
    #[serde(default)]
    form: Option<String>,
    /// Declared notation kind (`retainer`/`letter`/`filing`) from the
    /// `kind:` key; `None` until declared.
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    questionnaire: Option<BTreeMap<String, BTreeMap<String, String>>>,
    #[serde(default)]
    workflow: Option<BTreeMap<String, BTreeMap<String, String>>>,
}

/// Read `templates/<code>.md` from the Project repo's HEAD, validate it
/// (refusing on any Error-severity violation), and persist it as an
/// immutable, project-scoped Template version pinned to `(commit SHA +
/// content hash)`.
///
/// Idempotent by policy: byte-identical bytes re-read at a later commit are
/// [`crate::templates::Saved::Unchanged`] — the commit SHA is provenance,
/// not identity, so it does not churn a version.
pub async fn persist_from_repo(
    surreal: &crate::surreal::SurrealDb,
    storage: &Arc<dyn StorageService>,
    repo: &repos::RepoStore,
    project_id: Uuid,
    code: &str,
) -> Result<Persisted, TemplateSourceError> {
    // Read the pinned commit and HEAD tree together, off the async pool —
    // both shell `git` (project_export.rs / provision_repo do the same).
    let project_code = crate::projects::find_by_id(surreal, project_id)
        .await
        .map_err(|error| TemplateSourceError::Store(error.to_string()))?
        .ok_or(TemplateSourceError::RepoEmpty(project_id))?
        .code;
    let repo = repo.clone();
    let (commit_sha, tree) = tokio::task::spawn_blocking(move || {
        let commit_sha = repo.head_oid_code(&project_code)?;
        let tree = repo.read_head_tree_code(&project_code)?;
        Ok::<_, repos::RepoError>((commit_sha, tree))
    })
    .await
    .map_err(|e| repos::RepoError::Io(std::io::Error::other(e.to_string())))??;

    let commit_sha = commit_sha.ok_or(TemplateSourceError::RepoEmpty(project_id))?;

    let rel_path = format!("{TEMPLATES_DIR}/{code}.md");
    let bytes = tree
        .into_iter()
        .find(|(path, _)| path == &rel_path)
        .map(|(_, bytes)| bytes)
        .ok_or_else(|| TemplateSourceError::TemplateNotFound {
            code: code.to_string(),
        })?;
    let contents =
        String::from_utf8(bytes).map_err(|_| TemplateSourceError::NotUtf8(code.to_string()))?;

    // Validate with the same rule set `navigator validate` runs. Refuse on
    // any blocking violation — a notation must never open from bad paper.
    //
    // The file is presented under the bare `<code>.md` name, not its
    // `templates/<code>.md` repo path: a Project repo template is a
    // standalone blueprint, not a file in the workspace catalog tree, so the
    // catalog-*location* rules (N110's `neon_law/`/`forms/` shelves +
    // jurisdiction taxonomy) go correctly inert — they key on a `templates/`
    // path segment. Every *content* rule still fires, including N116's
    // load-bearing "a `lawyer_review` precedes each outbound step".
    let source = rules::SourceFile {
        path: std::path::PathBuf::from(format!("{code}.md")),
        contents: contents.clone(),
    };
    let violations: Vec<rules::Violation> = rules::navigator_default_rules_with_codes(&[])
        .iter()
        .flat_map(|rule| rule.lint(&source))
        .filter(|v| rules::severity_for_code(v.code) == rules::Severity::Error)
        .collect();
    if !violations.is_empty() {
        return Err(TemplateSourceError::Invalid {
            code: code.to_string(),
            violations,
        });
    }

    let fm: Frontmatter = rules::frontmatter::extract(&contents)
        .and_then(|f| serde_yaml::from_str(f).ok())
        .unwrap_or_default();

    // The body is content-addressed — its content sha256 is the content hash.
    let asset_id =
        crate::assets::ingest_content(surreal, storage, contents.as_bytes(), "text/markdown")
            .await?;

    // Register every referenced question code so answers can FK a question
    // row (mirrors the batch importer's registry step).
    register_questions(surreal, &fm).await?;

    let saved = crate::templates::save_version(
        surreal,
        Some(project_id),
        code,
        crate::templates::Version {
            title: fm.title.unwrap_or_else(|| code.to_string()),
            respondent_type: fm.respondent_type.unwrap_or_else(|| "entity".into()),
            asset_id: Some(asset_id),
            form_code: fm.form,
            kind: fm.kind,
            source_commit_sha: Some(commit_sha.clone()),
        },
    )
    .await?;

    Ok(Persisted {
        written: saved.was_written(),
        template: saved.into_model(),
        commit_sha,
    })
}

/// Register every question code referenced by the `questionnaire:` and
/// `workflow:` maps as a `questions` row, creating each on first sight.
/// State keys may carry a `__label` suffix; the question code is the
/// prefix. `BEGIN` / `END` are control states and never registered.
async fn register_questions(
    surreal: &crate::surreal::SurrealDb,
    fm: &Frontmatter,
) -> Result<(), TemplateSourceError> {
    let mut codes: BTreeSet<String> = BTreeSet::new();
    for map in [&fm.questionnaire, &fm.workflow].into_iter().flatten() {
        for state in map.keys() {
            if state == "BEGIN" || state == "END" {
                continue;
            }
            let prefix = state.split_once("__").map_or(state.as_str(), |(p, _)| p);
            codes.insert(prefix.to_string());
        }
    }
    for code in codes {
        // `find_or_create` rather than read-then-insert: two imports racing
        // on one auto-registered code would otherwise surface `CodeTaken`,
        // and the existing row's template-authored prompt is left alone.
        crate::questions::find_or_create(
            surreal,
            &crate::questions::NewQuestion::new(
                code.clone(),
                format!("(auto-registered) {code}"),
                "string",
            ),
        )
        .await
        .map_err(|error| TemplateSourceError::Store(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template guaranteed to pass validation: the workspace corpus is
    /// validated clean in CI, so an existing corpus body is a stable valid
    /// fixture. Its `lawyer_review → generate_pdf` shape satisfies N116.
    const VALID_TEMPLATE: &str =
        include_str!("../../templates/notations/neon_law/shared/onboarding_letter.md");

    async fn fs_storage() -> Arc<dyn StorageService> {
        Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-template-source-test"))
                .await
                .unwrap(),
        )
    }

    async fn project_row(surreal: &crate::surreal::SurrealDb) -> crate::projects::Project {
        crate::projects::create(
            surreal,
            &crate::projects::NewProject {
                code: format!("template-source-{}", uuid::Uuid::now_v7()),
                name: "matter".into(),
                status: "open".into(),
                entity_id: crate::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap()
    }

    /// A bare repo under a fresh temp root, with the given files committed
    /// to `main`. Returns the store, the temp dir (kept alive), and the
    /// commit SHA.
    fn repo_with(
        project_code: &str,
        files: &[(&str, &[u8])],
    ) -> (repos::RepoStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = repos::RepoStore::new(dir.path());
        store.ensure_code(project_code).unwrap();
        if !files.is_empty() {
            store
                .commit_as_code(
                    project_code,
                    repos::Author {
                        name: "Tester",
                        email: "tester@example.com",
                    },
                    "add templates",
                    files,
                )
                .unwrap();
        }
        (store, dir)
    }

    #[tokio::test]
    async fn persists_a_project_scoped_version_pinned_to_commit_and_content() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;
        let project = project_row(&surreal).await;
        let project_id = project.id;
        let (repo, _dir) = repo_with(
            &project.code,
            &[("templates/amendment.md", VALID_TEMPLATE.as_bytes())],
        );
        let head = repo.head_oid_code(&project.code).unwrap().unwrap();

        let persisted = persist_from_repo(&surreal, &storage, &repo, project_id, "amendment")
            .await
            .expect("engine should persist a valid repo template");

        assert!(persisted.written, "first read writes a new version");
        assert_eq!(persisted.commit_sha, head, "pins the HEAD commit it read");
        let row = persisted.template;
        assert_eq!(row.code, "amendment");
        assert_eq!(
            row.project_id,
            Some(project_id),
            "project-scoped, not shared"
        );
        assert!(row.is_current);
        assert_eq!(row.source_commit_sha.as_deref(), Some(head.as_str()));
        assert!(
            row.asset_id.is_some(),
            "body ingested as a content-addressed asset"
        );

        // The body round-trips from object storage, and resolve finds it.
        let body = crate::templates::body(&surreal, &storage, &row)
            .await
            .unwrap();
        assert_eq!(body, VALID_TEMPLATE);
        let resolved = crate::templates::resolve(&surreal, Some(project_id), "amendment")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.id, row.id);
    }

    #[tokio::test]
    async fn refuses_an_invalid_template_loudly() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;
        let project = project_row(&surreal).await;
        let project_id = project.id;
        // A line well past the 120-char limit trips S101 (Error-severity).
        let invalid = format!("# Bad\n\n{}\n", "x".repeat(200));
        let (repo, _dir) = repo_with(&project.code, &[("templates/bad.md", invalid.as_bytes())]);

        let err = persist_from_repo(&surreal, &storage, &repo, project_id, "bad")
            .await
            .expect_err("an invalid template must refuse, not persist");
        match err {
            TemplateSourceError::Invalid { code, violations } => {
                assert_eq!(code, "bad");
                assert!(!violations.is_empty());
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
        // And nothing was written.
        assert!(crate::templates::resolve(&surreal, Some(project_id), "bad")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn errors_when_the_repo_has_no_commits() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;
        let project = project_row(&surreal).await;
        let project_id = project.id;
        let (repo, _dir) = repo_with(&project.code, &[]); // ensured, but no commit

        let err = persist_from_repo(&surreal, &storage, &repo, project_id, "amendment")
            .await
            .expect_err("an empty repo has nothing to read");
        assert!(matches!(err, TemplateSourceError::RepoEmpty(p) if p == project_id));
    }

    #[tokio::test]
    async fn errors_when_the_template_file_is_absent() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;
        let project = project_row(&surreal).await;
        let project_id = project.id;
        let (repo, _dir) = repo_with(
            &project.code,
            &[("templates/other.md", VALID_TEMPLATE.as_bytes())],
        );

        let err = persist_from_repo(&surreal, &storage, &repo, project_id, "amendment")
            .await
            .expect_err("no templates/amendment.md at HEAD");
        assert!(
            matches!(err, TemplateSourceError::TemplateNotFound { code } if code == "amendment")
        );
    }

    #[tokio::test]
    async fn re_reading_identical_bytes_is_unchanged_and_keeps_first_provenance() {
        let surreal = crate::surreal::test_support::mem().await;
        let storage = fs_storage().await;
        let project = project_row(&surreal).await;
        let project_id = project.id;
        let (repo, _dir) = repo_with(
            &project.code,
            &[("templates/amendment.md", VALID_TEMPLATE.as_bytes())],
        );
        let first = persist_from_repo(&surreal, &storage, &repo, project_id, "amendment")
            .await
            .unwrap();

        // A second, unrelated commit bumps HEAD but leaves the template
        // bytes identical.
        repo.commit_as_code(
            &project.code,
            repos::Author {
                name: "Tester",
                email: "tester@example.com",
            },
            "unrelated commit",
            &[("README.md", b"hello")],
        )
        .unwrap();
        let new_head = repo.head_oid_code(&project.code).unwrap().unwrap();
        assert_ne!(new_head, first.commit_sha, "HEAD moved");

        let again = persist_from_repo(&surreal, &storage, &repo, project_id, "amendment")
            .await
            .unwrap();
        assert!(!again.written, "identical bytes must not churn a version");
        assert_eq!(again.template.id, first.template.id, "same version row");
        assert_eq!(
            again.template.source_commit_sha.as_deref(),
            Some(first.commit_sha.as_str()),
            "provenance keeps the first commit that produced these bytes",
        );
    }
}
