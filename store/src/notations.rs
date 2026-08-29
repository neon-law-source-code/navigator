//! One filled-in template — the notation core — and every query against
//! the table.
//!
//! # This table lives in SurrealDB
//!
//! `notations` moved with wave five of #1093 (ENG-121), the heaviest slice
//! of the notation group: `template_id`, `person_id`, `entity_id`, and
//! `project_id` are real `record<…>` links now that every table they name
//! is Surreal-resident, but the engine does not validate a link, so every
//! read-back a caller relied on the (already-dropped) foreign keys for
//! still has to happen in Rust — see [`find_by_id`] and its callers.
//!
//! One thing this module owns beyond CRUD: **rendered-document storage
//! keys** — the per-notation object-store convention every writer and
//! reader of the signing walk shares.
//!
//! # No delete — except undoing our own aborted create
//!
//! A real notation is never deleted, only state-transitioned. The one
//! exception is [`delete_uncommitted`]: a self-serve intake creates its
//! Notation before running the conflict check, and a blocking conflict
//! refuses the matter before the client has ever seen it — the row was
//! never a legal record to begin with, so the caller's own compensation
//! for its own aborted request is not the same operation as deleting a
//! filed notation, and callers must not reach for it for anything else.
//!
//! # No unique index, no write retry
//!
//! Unlike [`crate::templates`] or [`crate::mailrooms`], `notation` carries
//! no unique index — every row is addressed by its own fresh id, never a
//! natural key two writers could race on. [`create`] therefore does not
//! retry a transaction conflict, the same reasoning [`crate::answers`]
//! gives for its own append: there is no row for two writers to contend
//! over. The single-row updates below (`update_state`,
//! `update_questionnaire_snapshot`) act on an id the caller already
//! resolved, and a lost race there is a genuinely concurrent edit of the
//! same row — worth retrying, the way every other store module's `writing`
//! helper does.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value as Json;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
pub(crate) const TABLE: &str = "notation";
const PERSON_TABLE: &str = "person";
const ENTITY_TABLE: &str = "entity";
const TEMPLATE_TABLE: &str = "template";

/// Captive client recipient: signs embedded inside Neon Law Navigator (no
/// email). The historical retainer-walk default.
pub const DELIVERY_EMBEDDED: &str = "embedded";

/// Non-captive client recipient: DocuSign emails a signing link the client
/// opens from their own inbox. The matter-open default — a brand-new client
/// opened from the admin page is not in the room.
pub const DELIVERY_EMAILED: &str = "emailed";

/// One filled-in template, owned by a person (and optionally an entity)
/// inside exactly one Project.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`NotationRow`] is the seam that turns it into (and back out of) what
/// the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notation {
    pub id: Uuid,
    pub template_id: Uuid,
    pub person_id: Uuid,
    pub entity_id: Option<Uuid>,
    /// Every Notation belongs to exactly one Project — the glossary's
    /// load-bearing rule for the matter-audit-trail story.
    pub project_id: Uuid,
    /// `BEGIN`, `lawyer_review`, `sent_for_signature__pending`, …
    pub state: String,
    /// How the client receives this notation for signature:
    /// [`DELIVERY_EMBEDDED`] or [`DELIVERY_EMAILED`].
    pub delivery: String,
    /// The frozen questionnaire the Notation was opened against — written
    /// once at creation; render/step/fill resolve against it so a later
    /// template edit can't re-route this Notation. `None` for a Notation
    /// created with no snapshot.
    pub questionnaire_snapshot: Option<Json>,
    /// Commit SHA in the Project's repo holding this notation's rendered
    /// document. `None` until the durable commit step lands.
    pub git_commit_sha: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it.
#[derive(SurrealValue)]
struct NotationRow {
    id: surrealdb::types::RecordId,
    template_id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    entity_id: Option<surrealdb::types::RecordId>,
    project_id: surrealdb::types::RecordId,
    state: String,
    delivery: String,
    questionnaire_snapshot: Option<Json>,
    git_commit_sha: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl NotationRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_notation(self) -> Option<Notation> {
        Some(Notation {
            id: record_uuid(&self.id)?,
            template_id: record_uuid(&self.template_id)?,
            person_id: record_uuid(&self.person_id)?,
            entity_id: self.entity_id.as_ref().and_then(record_uuid),
            project_id: record_uuid(&self.project_id)?,
            state: self.state,
            delivery: self.delivery,
            questionnaire_snapshot: self.questionnaire_snapshot,
            git_commit_sha: self.git_commit_sha,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`NotationRow`] from only one query.
const SELECT: &str = "id, template_id, person_id, entity_id, project_id, state, delivery, \
     questionnaire_snapshot, git_commit_sha, inserted_at, updated_at";

/// Everything [`create`] needs to open a Notation.
#[derive(Debug, Clone)]
pub struct NewNotation {
    pub template_id: Uuid,
    pub person_id: Uuid,
    pub entity_id: Option<Uuid>,
    pub project_id: Uuid,
    pub state: String,
    pub delivery: String,
    pub questionnaire_snapshot: Option<Json>,
}

impl NewNotation {
    /// A bare Notation at `state`, embedded delivery, no entity and no
    /// snapshot — narrow it with the builders below.
    #[must_use]
    pub fn new(
        template_id: Uuid,
        person_id: Uuid,
        project_id: Uuid,
        state: impl Into<String>,
    ) -> Self {
        Self {
            template_id,
            person_id,
            entity_id: None,
            project_id,
            state: state.into(),
            delivery: DELIVERY_EMBEDDED.to_string(),
            questionnaire_snapshot: None,
        }
    }

    #[must_use]
    pub fn with_entity(mut self, entity_id: Uuid) -> Self {
        self.entity_id = Some(entity_id);
        self
    }

    #[must_use]
    pub fn with_delivery(mut self, delivery: impl Into<String>) -> Self {
        self.delivery = delivery.into();
        self
    }

    #[must_use]
    pub fn with_questionnaire_snapshot(mut self, snapshot: Json) -> Self {
        self.questionnaire_snapshot = Some(snapshot);
        self
    }
}

/// Errors reading or writing a notation.
#[derive(Debug, thiserror::Error)]
pub enum NotationError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("notation {0} not found")]
    NotFound(Uuid),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a notation returned no usable row")]
    WriteReturnedNothing,
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, NotationError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(NotationError::Db)
}

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Notation>, NotationError> {
    let row: Option<NotationRow> = response.take(0)?;
    Ok(row.and_then(NotationRow::into_notation))
}

fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Notation>, NotationError> {
    let rows: Vec<NotationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(NotationRow::into_notation)
        .collect())
}

/// Open a Notation. No unique index guards this insert — see the module
/// header for why no retry wraps it.
///
/// # Errors
///
/// [`NotationError::Db`] if the insert fails, or
/// [`NotationError::WriteReturnedNothing`] if it reports success but
/// returns no row.
pub async fn create(db: &SurrealDb, new: &NewNotation) -> Result<Notation, NotationError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             template_id = $template_id, \
             person_id = $person_id, \
             entity_id = $entity_id, \
             project_id = $project_id, \
             state = $state, \
             delivery = $delivery, \
             questionnaire_snapshot = $questionnaire_snapshot \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("template_id", record_id(TEMPLATE_TABLE, new.template_id)))
        .bind(("person_id", record_id(PERSON_TABLE, new.person_id)))
        .bind((
            "entity_id",
            new.entity_id.map(|e| record_id(ENTITY_TABLE, e)),
        ))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, new.project_id),
        ))
        .bind(("state", new.state.clone()))
        .bind(("delivery", new.delivery.clone()))
        .bind(("questionnaire_snapshot", new.questionnaire_snapshot.clone()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let row: Option<NotationRow> = response.take(0)?;
    row.and_then(NotationRow::into_notation)
        .ok_or(NotationError::WriteReturnedNothing)
}

/// Undo a Notation this same request just created and is aborting before
/// it was ever shown to anyone — see the module header's "No delete"
/// section. Not for removing a real notation; there is no other way to do
/// that, on purpose.
///
/// # Errors
///
/// [`NotationError::Db`] if the delete fails.
pub async fn delete_uncommitted(db: &SurrealDb, id: Uuid) -> Result<(), NotationError> {
    db.query("DELETE $id")
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

/// Resolve a notation by id.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Notation>, NotationError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Every notation on a Project, newest first.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn list_by_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<Notation>, NotationError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id = $project \
             ORDER BY inserted_at DESC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every notation across several Projects, newest first — the batched
/// resolver behind "which of these matters have an engagement/closing
/// notation" checks.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn list_by_projects(
    db: &SurrealDb,
    project_ids: &[Uuid],
) -> Result<Vec<Notation>, NotationError> {
    if project_ids.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<surrealdb::types::RecordId> = project_ids
        .iter()
        .map(|id| record_id(crate::projects::PROJECT_TABLE, *id))
        .collect();
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE project_id IN $projects \
             ORDER BY inserted_at DESC"
        ))
        .bind(("projects", keys))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every notation a Person is the respondent on, newest first.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn list_by_person(
    db: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<Notation>, NotationError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE person_id = $person \
             ORDER BY inserted_at DESC"
        ))
        .bind(("person", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every notation in the workspace, newest first — the lawyer "Notations"
/// directory listing and the data-lake export.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Notation>, NotationError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at DESC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// The one notation (if any) already open for this exact
/// `(project_id, template_id, person_id)` triple — the idempotent seed's
/// find-or-create probe.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn find_by_project_template_person(
    db: &SurrealDb,
    project_id: Uuid,
    template_id: Uuid,
    person_id: Uuid,
) -> Result<Option<Notation>, NotationError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE project_id = $project AND template_id = $template AND person_id = $person \
             LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("template", record_id(TEMPLATE_TABLE, template_id)))
        .bind(("person", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Whether any notation exists on this Project — the matter-delete guard.
///
/// # Errors
///
/// [`NotationError::Db`] if the lookup fails.
pub async fn exists_for_project(db: &SurrealDb, project_id: Uuid) -> Result<bool, NotationError> {
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

/// Transition a notation's workflow `state`. Returns the updated row.
///
/// # Errors
///
/// [`NotationError::NotFound`] if the notation does not exist, or
/// [`NotationError::Db`] for anything else.
pub async fn update_state(
    db: &SurrealDb,
    id: Uuid,
    new_state: &str,
) -> Result<Notation, NotationError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPDATE $id SET state = $state, updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("state", new_state.to_string()))
    })
    .await?;
    let row: Option<NotationRow> = response.take(0)?;
    row.and_then(NotationRow::into_notation)
        .ok_or(NotationError::NotFound(id))
}

/// Overwrite the frozen questionnaire snapshot. Production never calls
/// this after creation — it exists for the test that proves resolution
/// reads the snapshot, not the template.
///
/// # Errors
///
/// [`NotationError::NotFound`] if the notation does not exist, or
/// [`NotationError::Db`] for anything else.
pub async fn update_questionnaire_snapshot(
    db: &SurrealDb,
    id: Uuid,
    snapshot: Json,
) -> Result<Notation, NotationError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPDATE $id SET questionnaire_snapshot = $snapshot, updated_at = time::now() \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("snapshot", snapshot.clone()))
    })
    .await?;
    let row: Option<NotationRow> = response.take(0)?;
    row.and_then(NotationRow::into_notation)
        .ok_or(NotationError::NotFound(id))
}

/// Storage-key convention for the rendered document PDF of a given notation —
/// the version sent out for signature. Per-notation and template-agnostic (the
/// retainer, the trust, and any future signed template share this scheme). It
/// lives here, next to the notation reads, so every reader and writer of the
/// object store — the signing walk, the document-download route, and the
/// matter-detail existence probe — derives the same key from one place.
#[must_use]
pub fn document_pdf_storage_key(notation_id: Uuid) -> String {
    format!("notations/{notation_id}/document.pdf")
}

/// Storage key for the executed (signed) document PDF — the version the
/// provider returns once every party has signed.
#[must_use]
pub fn signed_document_storage_key(notation_id: Uuid) -> String {
    format!("notations/{notation_id}/signed-document.pdf")
}

/// Storage key for the Certificate of Completion — the ESIGN evidentiary record
/// archived alongside the signed retainer.
#[must_use]
pub fn certificate_of_completion_storage_key(notation_id: Uuid) -> String {
    format!("notations/{notation_id}/certificate-of-completion.pdf")
}

#[cfg(test)]
mod tests {
    use super::{
        certificate_of_completion_storage_key, create, document_pdf_storage_key, find_by_id,
        find_by_project_template_person, list_all, list_by_person, list_by_project,
        list_by_projects, signed_document_storage_key, update_questionnaire_snapshot, update_state,
        NewNotation, NotationError, DELIVERY_EMBEDDED,
    };
    use crate::surreal::test_support::mem;
    use uuid::Uuid;

    async fn a_template(surreal: &crate::surreal::SurrealDb) -> Uuid {
        crate::templates::save_version(
            surreal,
            None,
            &format!("sitting__transcript-{}", Uuid::now_v7()),
            crate::templates::Version {
                title: "Estate".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: None,
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model()
        .id
    }

    async fn a_person(surreal: &crate::surreal::SurrealDb, email: &str) -> Uuid {
        crate::persons::create(surreal, &crate::persons::NewPerson::new("Libra", email))
            .await
            .unwrap()
            .id
    }

    async fn open_notation(surreal: &crate::surreal::SurrealDb) -> Uuid {
        let project = crate::test_support::seed_project_surreal(surreal, "matter").await;
        let template_id = a_template(surreal).await;
        let person_id = a_person(surreal, &format!("libra-{}@example.com", Uuid::now_v7())).await;
        create(
            surreal,
            &NewNotation::new(template_id, person_id, project, "BEGIN"),
        )
        .await
        .unwrap()
        .id
    }

    #[test]
    fn notation_storage_keys_are_stable_per_notation_paths() {
        let id: Uuid = "00000000-0000-0000-0000-0000000000ab".parse().unwrap();
        assert_eq!(
            document_pdf_storage_key(id),
            "notations/00000000-0000-0000-0000-0000000000ab/document.pdf"
        );
        assert_eq!(
            signed_document_storage_key(id),
            "notations/00000000-0000-0000-0000-0000000000ab/signed-document.pdf"
        );
        assert_eq!(
            certificate_of_completion_storage_key(id),
            "notations/00000000-0000-0000-0000-0000000000ab/certificate-of-completion.pdf"
        );
    }

    #[tokio::test]
    async fn create_writes_a_notation_and_find_by_id_reads_it_back() {
        let surreal = mem().await;
        let project = crate::test_support::seed_project_surreal(&surreal, "matter").await;
        let template_id = a_template(&surreal).await;
        let person_id = a_person(&surreal, "libra@example.com").await;
        let created = create(
            &surreal,
            &NewNotation::new(template_id, person_id, project, "BEGIN"),
        )
        .await
        .unwrap();
        assert_eq!(created.delivery, DELIVERY_EMBEDDED);
        assert_eq!(created.state, "BEGIN");

        let found = find_by_id(&surreal, created.id).await.unwrap().unwrap();
        assert_eq!(found, created);
    }

    #[tokio::test]
    async fn a_missing_notation_reads_back_as_none() {
        let surreal = mem().await;
        assert!(find_by_id(&surreal, Uuid::now_v7())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn list_by_project_reads_only_that_projects_notations() {
        let surreal = mem().await;
        let project_a = crate::test_support::seed_project_surreal(&surreal, "matter-a").await;
        let project_b = crate::test_support::seed_project_surreal(&surreal, "matter-b").await;
        let template_id = a_template(&surreal).await;
        let person_id = a_person(&surreal, "libra@example.com").await;
        let on_a = create(
            &surreal,
            &NewNotation::new(template_id, person_id, project_a, "BEGIN"),
        )
        .await
        .unwrap()
        .id;
        create(
            &surreal,
            &NewNotation::new(template_id, person_id, project_b, "BEGIN"),
        )
        .await
        .unwrap();

        let rows = list_by_project(&surreal, project_a).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, on_a);
    }

    #[tokio::test]
    async fn list_by_projects_batches_across_several_projects() {
        let surreal = mem().await;
        let project_a = crate::test_support::seed_project_surreal(&surreal, "matter-a").await;
        let project_b = crate::test_support::seed_project_surreal(&surreal, "matter-b").await;
        let project_c = crate::test_support::seed_project_surreal(&surreal, "matter-c").await;
        let template_id = a_template(&surreal).await;
        let person_id = a_person(&surreal, "libra@example.com").await;
        create(
            &surreal,
            &NewNotation::new(template_id, person_id, project_a, "BEGIN"),
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewNotation::new(template_id, person_id, project_b, "BEGIN"),
        )
        .await
        .unwrap();

        let rows = list_by_projects(&surreal, &[project_a, project_b])
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(list_by_projects(&surreal, &[project_c])
            .await
            .unwrap()
            .is_empty());
        assert!(list_by_projects(&surreal, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn list_by_person_reads_only_that_persons_notations() {
        let surreal = mem().await;
        let project = crate::test_support::seed_project_surreal(&surreal, "matter").await;
        let template_id = a_template(&surreal).await;
        let libra = a_person(&surreal, "libra@example.com").await;
        let aries = a_person(&surreal, "aries@example.com").await;
        create(
            &surreal,
            &NewNotation::new(template_id, libra, project, "BEGIN"),
        )
        .await
        .unwrap();
        create(
            &surreal,
            &NewNotation::new(template_id, aries, project, "BEGIN"),
        )
        .await
        .unwrap();

        let rows = list_by_person(&surreal, libra).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].person_id, libra);
    }

    #[tokio::test]
    async fn list_all_reads_every_notation() {
        let surreal = mem().await;
        open_notation(&surreal).await;
        open_notation(&surreal).await;
        assert_eq!(list_all(&surreal).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn find_by_project_template_person_matches_the_exact_triple() {
        let surreal = mem().await;
        let project = crate::test_support::seed_project_surreal(&surreal, "matter").await;
        let template_id = a_template(&surreal).await;
        let person_id = a_person(&surreal, "libra@example.com").await;
        let created = create(
            &surreal,
            &NewNotation::new(template_id, person_id, project, "BEGIN"),
        )
        .await
        .unwrap();

        let found = find_by_project_template_person(&surreal, project, template_id, person_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, created.id);

        let other_project = crate::test_support::seed_project_surreal(&surreal, "other").await;
        assert!(
            find_by_project_template_person(&surreal, other_project, template_id, person_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn exists_for_project_reflects_whether_any_notation_is_open() {
        let surreal = mem().await;
        let project = crate::test_support::seed_project_surreal(&surreal, "matter").await;
        assert!(!super::exists_for_project(&surreal, project).await.unwrap());
        let template_id = a_template(&surreal).await;
        let person_id = a_person(&surreal, "libra@example.com").await;
        create(
            &surreal,
            &NewNotation::new(template_id, person_id, project, "BEGIN"),
        )
        .await
        .unwrap();
        assert!(super::exists_for_project(&surreal, project).await.unwrap());
    }

    #[tokio::test]
    async fn update_state_transitions_the_row() {
        let surreal = mem().await;
        let id = open_notation(&surreal).await;
        let updated = update_state(&surreal, id, "lawyer_review").await.unwrap();
        assert_eq!(updated.state, "lawyer_review");
        assert_eq!(
            find_by_id(&surreal, id).await.unwrap().unwrap().state,
            "lawyer_review"
        );
    }

    #[tokio::test]
    async fn update_state_on_a_missing_notation_reports_not_found() {
        let surreal = mem().await;
        let err = update_state(&surreal, Uuid::now_v7(), "lawyer_review")
            .await
            .unwrap_err();
        assert!(matches!(err, NotationError::NotFound(_)));
    }

    #[tokio::test]
    async fn update_questionnaire_snapshot_overwrites_the_frozen_copy() {
        let surreal = mem().await;
        let id = open_notation(&surreal).await;
        let alt = serde_json::json!({"states": {"replaced": true}});
        let updated = update_questionnaire_snapshot(&surreal, id, alt.clone())
            .await
            .unwrap();
        assert_eq!(updated.questionnaire_snapshot, Some(alt));
    }
}
