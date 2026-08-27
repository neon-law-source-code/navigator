//! Template resolution and body access.
//!
//! # This table lives in SurrealDB
//!
//! `templates` moved with wave five of #1093 (ENG-121), together with
//! [`crate::assets`]. They had to move as one slice: [`save_version`] folds
//! `asset_id` into the tuple that decides whether a version changed, so
//! split across engines the lane would hold its version identity in one and
//! the bytes it names in the other.
//!
//! Two responsibilities this module owns (see `docs/notation.md`):
//!
//! - **Body access.** A Template's markdown body is a content-addressed
//!   [`crate::assets`] asset referenced by `templates.asset_id`; [`body`]
//!   fetches it from storage. `fk_templates_asset` is gone
//!   (`m20260911_drop_notation_group_foreign_keys`), and what replaced it
//!   is [`body`]'s read-back of the asset before it reaches storage — not
//!   the link's type. The engine does not validate a `record<asset>` link.
//! - **Project scoping.** A Template is either workspace-shared
//!   (`project_id` unset) or scoped to one Project. [`resolve`] looks
//!   a code up preferring the caller's Project, falling back to the
//!   shared row — so a Project can override a shared `code` or define
//!   its own without colliding with another Project's.
//!
//! # One current row per code, without a partial index
//!
//! The rule is one current row per shared `code` and one per
//! `(project_id, code)`. Surreal has no partial
//! index, so [`current_key`] computes a key that exists only while a row is
//! current, and the schema's UNIQUE index refuses the second one. Multiple
//! NONEs do not collide there, so retired versions never take part. That
//! matters more than it looks: [`save_version`] retires the old row and
//! inserts the new one in two statements, and a multi-statement Surreal
//! query is not one transaction — the index is what makes the pair safe
//! under a race rather than merely usually-right.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use cloud::StorageService;
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "template";

/// One template version.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`TemplateRow`] is the seam that turns it into (and back out of) what
/// the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Template {
    pub id: Uuid,
    /// Stable code. Unique among *current* shared templates; unique per
    /// Project among current project-scoped ones.
    pub code: String,
    pub title: String,
    /// `entity`, `person`, or `person_and_entity`.
    pub respondent_type: String,
    /// The Project this template is scoped to; `None` for the
    /// workspace-shared public catalog.
    pub project_id: Option<Uuid>,
    /// Whether this row is the live version of its `code`.
    pub is_current: bool,
    /// The [`crate::assets`] row holding the markdown body (with
    /// `{{question_code}}` placeholders). `None` only transiently before
    /// the body is ingested. Read via [`body`].
    pub asset_id: Option<Uuid>,
    /// forms-registry code of the government form this template fills
    /// (e.g. `nv__llc_formation`), from the `form:` frontmatter key;
    /// `None` for Typst-rendered templates.
    pub form_code: Option<String>,
    /// The declared notation `kind` from the template's `kind:`
    /// frontmatter — one of the values in `rules::kind::Kind`. Lets
    /// callers gate on kind (e.g. "the first notation on a Project must be
    /// a `retainer`") without re-parsing the body.
    pub kind: Option<String>,
    /// Commit SHA of `refs/heads/main` the version was cut from when a
    /// Project repo fed it through [`crate::template_source`]. `None` for
    /// the workspace catalog, which is seeded from bundled files rather
    /// than a repo. Provenance, not identity — not part of the version
    /// tuple.
    pub source_commit_sha: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct TemplateRow {
    id: surrealdb::types::RecordId,
    code: String,
    title: String,
    respondent_type: String,
    project_id: Option<surrealdb::types::RecordId>,
    is_current: bool,
    asset_id: Option<surrealdb::types::RecordId>,
    form_code: Option<String>,
    kind: Option<String>,
    source_commit_sha: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl TemplateRow {
    /// `None` when the record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_template(self) -> Option<Template> {
        Some(Template {
            id: record_uuid(&self.id)?,
            code: self.code,
            title: self.title,
            respondent_type: self.respondent_type,
            project_id: self.project_id.as_ref().and_then(record_uuid),
            is_current: self.is_current,
            asset_id: self.asset_id.as_ref().and_then(record_uuid),
            form_code: self.form_code,
            kind: self.kind,
            source_commit_sha: self.source_commit_sha,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new field cannot reach [`TemplateRow`] from only one query.
const SELECT: &str = "id, code, title, respondent_type, project_id, is_current, asset_id, \
     form_code, kind, source_commit_sha, inserted_at, updated_at";

/// Errors from [`body`].
#[derive(Debug, thiserror::Error)]
pub enum TemplateBodyError {
    #[error("template `{0}` has no stored body (asset_id is null)")]
    MissingBody(Uuid),
    #[error("template `{0}` names an asset that does not exist")]
    DanglingAsset(Uuid),
    #[error("asset: {0}")]
    Asset(#[from] crate::assets::AssetError),
    #[error("template body is not valid UTF-8")]
    NotUtf8,
}

/// Errors reading or writing a template.
#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Another writer already holds the current row for this
    /// `(project_id, code)` — the `template_current_key` index refused a
    /// second one.
    #[error("that template code already has a current version")]
    CurrentVersionTaken,
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a template returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names, or
/// leave it as a database fault. A unique violation carries **no typed
/// detail** — the index name in the message is the only discriminator, so
/// this matches on `template_current_key`, a `DEFINE INDEX` identifier this
/// workspace chose in `store/src/schema/navigator.surql`, not prose.
fn classify_write(error: surrealdb::Error) -> TemplateError {
    if error.to_string().contains("template_current_key") {
        TemplateError::CurrentVersionTaken
    } else {
        TemplateError::Db(error)
    }
}

/// The value stored in `current_key` for a row that is current.
///
/// The unit separator (`U+001F`) joins the scope to the code because it
/// cannot occur in either: a Project id is a UUID and a template code is a
/// frontmatter identifier. A plain separator like `:` could be produced by
/// a code containing it, which would let two distinct `(project, code)`
/// pairs collide on one key and silently retire each other's version.
///
/// The shared line uses an empty scope, so a shared `amendment` and a
/// project-scoped `amendment` get different keys and coexist.
fn current_key(project_id: Option<Uuid>, code: &str) -> String {
    let scope = project_id.map(|id| id.to_string()).unwrap_or_default();
    format!("{scope}\u{1f}{code}")
}

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Template>, TemplateError> {
    let row: Option<TemplateRow> = response.take(0)?;
    Ok(row.and_then(TemplateRow::into_template))
}

fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Template>, TemplateError> {
    let rows: Vec<TemplateRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(TemplateRow::into_template)
        .collect())
}

/// Resolve a template by id — what a Notation does with the
/// `template_id` it pinned.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Template>, TemplateError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve many templates by id in one round trip — what a listing does
/// after reading a page of notations, rather than one lookup per row.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn find_by_ids(db: &SurrealDb, ids: &[Uuid]) -> Result<Vec<Template>, TemplateError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<surrealdb::types::RecordId> =
        ids.iter().map(|id| record_id(TABLE, *id)).collect();
    let response = db
        .query(format!("SELECT {SELECT} FROM $ids ORDER BY code ASC"))
        .bind(("ids", keys))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Resolve a template by `code` for a given Project context. Prefers a
/// Project-scoped row (`project_id = project_id`), then falls back to
/// the workspace-shared row (`project_id` unset). Returns `None` when
/// neither exists.
///
/// Pass `project_id = None` to look up only the shared row (the public
/// catalog).
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn resolve(
    db: &SurrealDb,
    project_id: Option<Uuid>,
    code: &str,
) -> Result<Option<Template>, TemplateError> {
    if project_id.is_some() {
        if let Some(scoped) = resolve_exact(db, project_id, code).await? {
            return Ok(Some(scoped));
        }
    }
    resolve_exact(db, None, code).await
}

/// The current row for an exact `(project_id, code)` — no shared
/// fallback, unlike [`resolve`]. [`save_version`]'s pointer flip must act
/// on the same scope it writes to.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn resolve_exact(
    db: &SurrealDb,
    project_id: Option<Uuid>,
    code: &str,
) -> Result<Option<Template>, TemplateError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE current_key = $key LIMIT 1"
        ))
        .bind(("key", current_key(project_id, code)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Every version ever written for `(project_id, code)`, newest first —
/// the current row plus every retired one.
///
/// The immutability property made observable: a spec change appends a row
/// and flips `is_current`, so the retired versions are still here for the
/// Notations that pinned them through `notation.template_id`.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn versions_of(
    db: &SurrealDb,
    project_id: Option<Uuid>,
    code: &str,
) -> Result<Vec<Template>, TemplateError> {
    // Matched on `(code, project_id)` rather than `current_key`, which only
    // a current row carries — that is the whole point of this read.
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE code = $code AND project_id = $project \
             ORDER BY id DESC"
        ))
        .bind(("code", code.to_string()))
        .bind((
            "project",
            project_id.map(|p| record_id(crate::projects::PROJECT_TABLE, p)),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every current template in the workspace-shared catalog, ordered by
/// code — the lawyer listing and the importer's "what is already
/// catalogued" read.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn list_current(db: &SurrealDb) -> Result<Vec<Template>, TemplateError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE is_current = true ORDER BY code ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every current template scoped to one Project, ordered by code.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn list_for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<Template>, TemplateError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE is_current = true AND project_id = $project ORDER BY code ASC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Whether any template — current or superseded — is scoped to this
/// Project. The matter-delete guard.
///
/// Deliberately unfiltered by `is_current`, unlike [`list_for_project`]: a
/// superseded version still carries the matter's `project_id`, so deleting
/// the matter out from under it would orphan the whole version line rather
/// than just the row a reader sees.
///
/// # Errors
///
/// [`TemplateError::Db`] if the lookup fails.
pub async fn exists_for_project(db: &SurrealDb, project_id: Uuid) -> Result<bool, TemplateError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} WHERE project_id = $project LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let ids: Vec<surrealdb::types::RecordId> = response.take(0)?;
    Ok(!ids.is_empty())
}

/// The spec fields that make up one Template version. Identity for
/// "did this change?" is the tuple `(title, respondent_type, asset_id,
/// form_code, kind)` — `code` and `project_id` locate the version line.
pub struct Version {
    pub title: String,
    pub respondent_type: String,
    pub asset_id: Option<Uuid>,
    pub form_code: Option<String>,
    /// Declared notation kind — one of the notation-family `kind:` values
    /// (`letter`, `filing`, `will`, `trust`, `directive`, `agreement`,
    /// `onboarding`, `offboarding`, `memo`) from the template's frontmatter;
    /// `None` for a template stored before it declared one. The
    /// engagement-first matter gate keys off the kinds that open a matter
    /// (`rules::kind::Kind::opens_a_matter`).
    pub kind: Option<String>,
    /// Git commit SHA the version was cut from (`crate::template_source`),
    /// or `None` for a non-repo source (the seeded workspace catalog).
    /// Recorded on write as **provenance** — deliberately absent from the
    /// change-detection tuple above, so a byte-identical re-read at a
    /// newer commit is still [`Saved::Unchanged`] and keeps the first
    /// commit that produced these bytes.
    pub source_commit_sha: Option<String>,
}

/// Outcome of [`save_version`].
pub enum Saved {
    /// The current row already matched the spec; nothing was written.
    Unchanged(Template),
    /// A new current row was written — the first version of this code, or
    /// a change that retired the prior version.
    Written(Template),
}

impl Saved {
    /// The now-current Template row, either way.
    #[must_use]
    pub fn into_model(self) -> Template {
        match self {
            Saved::Unchanged(m) | Saved::Written(m) => m,
        }
    }

    /// Whether this call wrote a new row.
    #[must_use]
    pub fn was_written(&self) -> bool {
        matches!(self, Saved::Written(_))
    }
}

/// Make `version` the current Template for `(project_id, code)`, appending
/// it as a new row and retiring any existing current row.
///
/// Immutable by policy: a spec change never rewrites a row a Notation
/// pinned via `notation.template_id`. When `version` matches the existing
/// current row, this is a no-op and returns [`Saved::Unchanged`] — so a
/// re-seed of an unchanged template does not churn versions.
///
/// Race-safe without a lock. `cli import` and the canonical seed both run
/// this on every boot, and the cucumber suite runs concurrent scenarios
/// against one engine, so idempotence under contention is the contract
/// rather than a nicety. A concurrent writer that wins the
/// `template_current_key` index turns this call's insert into
/// [`TemplateError::CurrentVersionTaken`], which is re-read once: if the
/// winner wrote the same spec the answer is [`Saved::Unchanged`], and only
/// a genuinely different spec that still lost surfaces the conflict.
///
/// # Errors
///
/// [`TemplateError::CurrentVersionTaken`] when a concurrent writer holds
/// the current row for a *different* spec, and [`TemplateError::Db`] for
/// anything else.
pub async fn save_version(
    db: &SurrealDb,
    project_id: Option<Uuid>,
    code: &str,
    version: Version,
) -> Result<Saved, TemplateError> {
    let mut backoff = WRITE_BACKOFF;
    for remaining in (0..WRITE_ATTEMPTS).rev() {
        match attempt_save(db, project_id, code, &version).await {
            Err(TemplateError::CurrentVersionTaken) => {
                // Someone else holds the slot. If they wrote what we were
                // about to write, this call is simply a no-op — the shape a
                // concurrent re-seed takes.
                if let Some(winner) = resolve_exact(db, project_id, code).await? {
                    if matches_spec(&winner, &version) {
                        return Ok(Saved::Unchanged(winner));
                    }
                }
                // Otherwise we may have read mid-flip. `attempt_save` retires
                // the incumbent and inserts its replacement in two statements,
                // so between them NO row carries `current_key` — a reader in
                // that window sees no current version at all and must not
                // conclude anything from it. Back off and look again rather
                // than deciding from a half-applied pointer flip.
                if remaining == 0 {
                    return Err(TemplateError::CurrentVersionTaken);
                }
                tokio::time::sleep(rand::random_range(std::time::Duration::ZERO..=backoff)).await;
                backoff *= 2;
            }
            other => return other,
        }
    }
    unreachable!("the last attempt returns rather than falling out of the loop")
}

/// How many times [`save_version`] re-reads after losing the
/// `template_current_key` slot, and the first backoff window.
///
/// Not the transaction-conflict retry: that is one policy for the whole
/// crate, in [`crate::surreal::retry`], and the statements this module
/// issues reach it through [`crate::assets::writing`]. This bound is
/// around a *unique index* violation, where the point of another pass is
/// to re-read the winner rather than to re-run the write.
const WRITE_ATTEMPTS: usize = 5;
const WRITE_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);

/// Whether `row` already carries exactly the spec `version` describes.
/// `code` and `project_id` locate the version line; `source_commit_sha` is
/// provenance and deliberately absent, so byte-identical bytes re-read at a
/// newer commit stay [`Saved::Unchanged`].
fn matches_spec(row: &Template, version: &Version) -> bool {
    row.title == version.title
        && row.respondent_type == version.respondent_type
        && row.asset_id == version.asset_id
        && row.form_code == version.form_code
        && row.kind == normalized_kind(version.kind.clone())
}

/// The rules layer (S103) validates the *trimmed* scalar, so store it
/// trimmed too — otherwise a padded frontmatter value like
/// `kind: "retainer "` passes validation but is stored untrimmed and then
/// fails to parse at the engagement-first gate. Empty-after-trim collapses
/// to "no kind declared".
fn normalized_kind(kind: Option<String>) -> Option<String> {
    kind.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

async fn attempt_save(
    db: &SurrealDb,
    project_id: Option<Uuid>,
    code: &str,
    spec: &Version,
) -> Result<Saved, TemplateError> {
    let mut version = Version {
        title: spec.title.clone(),
        respondent_type: spec.respondent_type.clone(),
        asset_id: spec.asset_id,
        form_code: spec.form_code.clone(),
        kind: spec.kind.clone(),
        source_commit_sha: spec.source_commit_sha.clone(),
    };
    version.kind = normalized_kind(version.kind);

    let current = resolve_exact(db, project_id, code).await?;
    if let Some(existing) = &current {
        if matches_spec(existing, &version) {
            return Ok(Saved::Unchanged(existing.clone()));
        }
    }

    // Retire the incumbent first: it holds `current_key`, and the UNIQUE
    // index would refuse the new row while it still does. Clearing the key
    // is what makes the slot available, so this is the pointer flip, not
    // bookkeeping after one.
    //
    // Both writes go through the shared conflict retry. The key-value layer
    // is optimistic, so a write that merely *raced* another one comes back
    // as a retryable `TransactionConflict` — nothing about the statement was
    // wrong. The cucumber suite shares one engine across concurrent
    // scenarios, so a writer that assumed exclusivity flakes rather than
    // fails.
    if let Some(existing) = &current {
        crate::assets::writing(|| {
            db.query(
                "UPDATE $id SET is_current = false, current_key = NONE, updated_at = time::now()",
            )
            .bind(("id", record_id(TABLE, existing.id)))
        })
        .await
        .map_err(classify_write)?;
    }

    let id = Uuid::now_v7();
    let mut response = crate::assets::writing(|| {
        db.query(format!(
            "CREATE $id SET \
             code = $code, \
             title = $title, \
             respondent_type = $respondent_type, \
             project_id = $project_id, \
             is_current = true, \
             asset_id = $asset_id, \
             form_code = $form_code, \
             kind = $kind, \
             source_commit_sha = $source_commit_sha, \
             current_key = $current_key \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("code", code.to_string()))
        .bind(("title", version.title.clone()))
        .bind(("respondent_type", version.respondent_type.clone()))
        .bind((
            "project_id",
            project_id.map(|p| record_id(crate::projects::PROJECT_TABLE, p)),
        ))
        .bind((
            "asset_id",
            version.asset_id.map(|a| record_id(crate::assets::TABLE, a)),
        ))
        .bind(("form_code", version.form_code.clone()))
        .bind(("kind", version.kind.clone()))
        .bind(("source_commit_sha", version.source_commit_sha.clone()))
        .bind(("current_key", current_key(project_id, code)))
    })
    .await
    .map_err(classify_write)?;

    let row: Option<TemplateRow> = response.take(0)?;
    row.and_then(TemplateRow::into_template)
        .map(Saved::Written)
        .ok_or(TemplateError::WriteReturnedNothing)
}

/// Fetch a Template's markdown body from object storage.
///
/// Reads the asset row back before reaching storage. That read-back is
/// what replaced `fk_templates_asset` when the constraint was dropped: the
/// engine does not validate a `record<asset>` link, so a template naming
/// an asset that was never written is accepted at write time and has to be
/// caught here.
///
/// # Errors
///
/// [`TemplateBodyError`] when the template has no body, names an asset
/// that does not exist, storage fails, or the bytes are not UTF-8.
pub async fn body(
    db: &SurrealDb,
    storage: &Arc<dyn StorageService>,
    template: &Template,
) -> Result<String, TemplateBodyError> {
    let asset_id = template
        .asset_id
        .ok_or(TemplateBodyError::MissingBody(template.id))?;
    let asset = crate::assets::find_by_id(db, asset_id)
        .await?
        .ok_or(TemplateBodyError::DanglingAsset(template.id))?;
    let bytes = storage
        .get(&asset.storage_key)
        .await
        .map_err(|e| TemplateBodyError::Asset(crate::assets::AssetError::Storage(e)))?
        .bytes;
    String::from_utf8(bytes).map_err(|_| TemplateBodyError::NotUtf8)
}

#[cfg(test)]
mod tests {
    use super::{body, resolve, resolve_exact, save_version, Template, TemplateBodyError};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    async fn fs_storage() -> std::sync::Arc<dyn cloud::StorageService> {
        let dir = std::env::temp_dir().join(format!("navigator-templates-{}", Uuid::now_v7()));
        std::sync::Arc::new(cloud::FsStorage::new(dir).await.unwrap())
    }

    fn version(title: &str, asset: Option<Uuid>) -> super::Version {
        super::Version {
            title: title.into(),
            respondent_type: "entity".into(),
            asset_id: asset,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        }
    }

    async fn insert_template(db: &SurrealDb, code: &str, project_id: Option<Uuid>) -> Uuid {
        save_version(db, project_id, code, version(code, None))
            .await
            .unwrap()
            .into_model()
            .id
    }

    #[tokio::test]
    async fn resolve_prefers_project_scoped_then_falls_back_to_shared() {
        let db = mem().await;
        let p = crate::test_support::seed_project_surreal(&db, "matter").await;
        let shared = insert_template(&db, "amendment", None).await;
        let scoped = insert_template(&db, "amendment", Some(p)).await;

        // From the project: the scoped row wins.
        assert_eq!(
            resolve(&db, Some(p), "amendment")
                .await
                .unwrap()
                .unwrap()
                .id,
            scoped
        );
        // No project context: the shared row.
        assert_eq!(
            resolve(&db, None, "amendment").await.unwrap().unwrap().id,
            shared
        );
        // A different project with no scoped row falls back to shared.
        let other = crate::test_support::seed_project_surreal(&db, "other").await;
        assert_eq!(
            resolve(&db, Some(other), "amendment")
                .await
                .unwrap()
                .unwrap()
                .id,
            shared
        );
    }

    /// A shared `code` and a project-scoped one coexist, which is what
    /// `current_key`'s scope carries: empty for the shared line, the
    /// Project id otherwise.
    #[tokio::test]
    async fn shared_and_project_scoped_codes_coexist() {
        let db = mem().await;
        let p = crate::test_support::seed_project_surreal(&db, "matter").await;
        let shared = insert_template(&db, "consent", None).await;
        let scoped = insert_template(&db, "consent", Some(p)).await;

        assert_ne!(shared, scoped);
        assert_eq!(
            resolve_exact(&db, None, "consent")
                .await
                .unwrap()
                .unwrap()
                .id,
            shared
        );
        assert_eq!(
            resolve_exact(&db, Some(p), "consent")
                .await
                .unwrap()
                .unwrap()
                .id,
            scoped
        );
    }

    #[tokio::test]
    async fn save_version_writes_first_version_and_resolve_reads_it() {
        let db = mem().await;
        let saved = save_version(&db, None, "amendment", version("Amendment", None))
            .await
            .unwrap();
        assert!(saved.was_written());
        let current = resolve(&db, None, "amendment").await.unwrap().unwrap();
        assert_eq!(current.id, saved.into_model().id);
        assert!(current.is_current);
    }

    #[tokio::test]
    async fn save_version_persists_and_reads_back_the_kind() {
        let db = mem().await;
        let mut v = version("Retainer", None);
        v.kind = Some("retainer".into());
        save_version(&db, None, "onboarding__retainer", v)
            .await
            .unwrap();
        let current = resolve(&db, None, "onboarding__retainer")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.kind.as_deref(), Some("retainer"));
    }

    #[tokio::test]
    async fn a_padded_kind_is_stored_trimmed_so_the_retainer_gate_matches() {
        let db = mem().await;
        let mut v = version("Retainer", None);
        v.kind = Some("  retainer  ".into());
        save_version(&db, None, "onboarding__retainer", v)
            .await
            .unwrap();
        // Stored trimmed → an exact `== "retainer"` gate matches.
        assert_eq!(
            resolve(&db, None, "onboarding__retainer")
                .await
                .unwrap()
                .unwrap()
                .kind
                .as_deref(),
            Some("retainer"),
        );
    }

    #[tokio::test]
    async fn a_whitespace_only_kind_collapses_to_none() {
        let db = mem().await;
        let mut v = version("T", None);
        v.kind = Some("   ".into());
        save_version(&db, None, "amendment", v).await.unwrap();
        assert_eq!(
            resolve(&db, None, "amendment").await.unwrap().unwrap().kind,
            None,
        );
    }

    #[tokio::test]
    async fn changing_only_the_kind_appends_a_new_version() {
        let db = mem().await;
        save_version(&db, None, "closing__letter", version("Closing", None))
            .await
            .unwrap();
        let mut with_kind = version("Closing", None);
        with_kind.kind = Some("letter".into());
        let again = save_version(&db, None, "closing__letter", with_kind)
            .await
            .unwrap();
        assert!(
            again.was_written(),
            "declaring a kind on an existing template must cut a new version"
        );
        assert_eq!(
            resolve(&db, None, "closing__letter")
                .await
                .unwrap()
                .unwrap()
                .kind
                .as_deref(),
            Some("letter"),
        );
    }

    /// `asset_id` is in the change-detection tuple, which is the whole
    /// reason `templates` and `assets` moved in one slice: a new body
    /// means a new version, and that comparison has to see both sides.
    #[tokio::test]
    async fn changing_only_the_asset_appends_a_new_version() {
        let db = mem().await;
        let storage = fs_storage().await;
        let first_body = crate::assets::ingest_content(&db, &storage, b"# v1", "text/markdown")
            .await
            .unwrap();
        let second_body = crate::assets::ingest_content(&db, &storage, b"# v2", "text/markdown")
            .await
            .unwrap();

        let v1 = save_version(&db, None, "deed", version("Deed", Some(first_body)))
            .await
            .unwrap();
        assert!(v1.was_written());
        let v2 = save_version(&db, None, "deed", version("Deed", Some(second_body)))
            .await
            .unwrap();
        assert!(
            v2.was_written(),
            "a new body must cut a new version even when every other field matches"
        );
        assert_eq!(
            resolve(&db, None, "deed").await.unwrap().unwrap().asset_id,
            Some(second_body)
        );
    }

    #[tokio::test]
    async fn unchanged_re_save_is_a_no_op() {
        let db = mem().await;
        let first = save_version(&db, None, "amendment", version("Amendment", None))
            .await
            .unwrap()
            .into_model();
        let again = save_version(&db, None, "amendment", version("Amendment", None))
            .await
            .unwrap();
        assert!(
            !again.was_written(),
            "an identical spec must not churn versions"
        );
        assert_eq!(again.into_model().id, first.id);
    }

    #[tokio::test]
    async fn changing_a_template_appends_a_version_and_pins_the_old_one() {
        let db = mem().await;
        let v1 = save_version(&db, None, "amendment", version("Amendment", None))
            .await
            .unwrap()
            .into_model();
        let v2 = save_version(&db, None, "amendment", version("Amendment v2", None))
            .await
            .unwrap();
        assert!(v2.was_written());
        let v2 = v2.into_model();
        assert_ne!(v1.id, v2.id, "a change appends a new row, never rewrites");

        // resolve returns the new current version.
        assert_eq!(
            resolve(&db, None, "amendment").await.unwrap().unwrap().id,
            v2.id
        );
        // The old row survives, retired — a Notation that pinned
        // `template_id = v1` still finds its exact bytes.
        let pinned = super::find_by_id(&db, v1.id).await.unwrap().unwrap();
        assert!(!pinned.is_current);
        assert_eq!(pinned.title, "Amendment");
    }

    /// The shape the seed and `cli import` actually produce: several
    /// writers racing to save the *same* spec on every boot. Every one of
    /// them must succeed and agree on the row — a loser that surfaced
    /// `CurrentVersionTaken` here would fail a boot over a no-op.
    #[tokio::test]
    async fn concurrent_re_seed_of_one_spec_settles_on_one_row_without_erroring() {
        let db = mem().await;
        let racers: Vec<_> = (0..6)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move {
                    save_version(&db, None, "amendment", version("Amendment", None)).await
                })
            })
            .collect();

        let mut ids = Vec::new();
        for racer in racers {
            ids.push(
                racer
                    .await
                    .expect("task must not panic")
                    .expect("an identical re-seed must never surface a conflict")
                    .into_model()
                    .id,
            );
        }
        assert!(
            ids.windows(2).all(|w| w[0] == w[1]),
            "the racers disagreed about which row won: {ids:?}"
        );
        assert_eq!(
            super::versions_of(&db, None, "amendment")
                .await
                .unwrap()
                .len(),
            1,
            "one version, not six"
        );
    }

    /// The seed and `cli import` both run on every boot, and the cucumber
    /// suite runs concurrent scenarios against one engine, so racing
    /// writers must settle on one current row rather than forking the
    /// version line. The `template_current_key` UNIQUE index is what makes
    /// that true — `save_version` retires and inserts in two statements,
    /// and a multi-statement Surreal query is not one transaction.
    #[tokio::test]
    async fn concurrent_save_version_leaves_exactly_one_current_row() {
        let db = mem().await;
        let racers: Vec<_> = (0..6)
            .map(|n| {
                let db = db.clone();
                tokio::spawn(async move {
                    save_version(&db, None, "amendment", version(&format!("v{n}"), None)).await
                })
            })
            .collect();

        for racer in racers {
            // A loser is refused by the index rather than forking the line;
            // either outcome is acceptable, a panic is not.
            let _ = racer.await.expect("task must not panic");
        }

        let current: Vec<Template> = super::list_current(&db).await.unwrap();
        assert_eq!(
            current.iter().filter(|t| t.code == "amendment").count(),
            1,
            "the racers forked the version line: {current:?}"
        );
    }

    #[tokio::test]
    async fn body_reads_the_markdown_back_from_object_storage() {
        let db = mem().await;
        let storage = fs_storage().await;
        let asset_id =
            crate::assets::ingest_content(&db, &storage, b"# Deed\n\n{{buyer}}", "text/markdown")
                .await
                .unwrap();
        let tmpl = save_version(&db, None, "deed", version("Deed", Some(asset_id)))
            .await
            .unwrap()
            .into_model();
        let text = body(&db, &storage, &tmpl).await.unwrap();
        assert_eq!(text, "# Deed\n\n{{buyer}}");
    }

    #[tokio::test]
    async fn body_reports_a_template_with_no_stored_body() {
        let db = mem().await;
        let storage = fs_storage().await;
        let tmpl = save_version(&db, None, "deed", version("Deed", None))
            .await
            .unwrap()
            .into_model();
        assert!(matches!(
            body(&db, &storage, &tmpl).await,
            Err(TemplateBodyError::MissingBody(_))
        ));
    }

    /// `fk_templates_asset` is dropped, and the engine does not validate a
    /// `record<asset>` link — so a template naming an asset that was never
    /// written is accepted at write time. This read-back is what replaced
    /// the constraint.
    #[tokio::test]
    async fn body_reports_a_template_naming_an_asset_that_does_not_exist() {
        let db = mem().await;
        let storage = fs_storage().await;
        let tmpl = save_version(&db, None, "deed", version("Deed", Some(Uuid::now_v7())))
            .await
            .unwrap()
            .into_model();
        assert!(matches!(
            body(&db, &storage, &tmpl).await,
            Err(TemplateBodyError::DanglingAsset(_))
        ));
    }

    #[tokio::test]
    async fn a_missing_template_reads_back_as_none() {
        let db = mem().await;
        assert!(super::find_by_id(&db, Uuid::now_v7())
            .await
            .unwrap()
            .is_none());
        assert!(super::find_by_ids(&db, &[]).await.unwrap().is_empty());
    }
}
