//! The `entities` table — a legal organization (LLC, trust,
//! corporation) or the `Human` entity standing for a solo natural
//! person — and every query that reads or writes an `entity` row.
//!
//! # This table lives in SurrealDB
//!
//! One table carries both the full row and the node the conflict graph
//! traverses.
//!
//! `entity_type_id` and `jurisdiction_id` are real `record<>` links. The
//! engine does **not** validate a link, so `entity_commands` reads both
//! rows back before any write that stores one, exactly as
//! `store::credentials` does.
//!
//! # The firm anchor is a schema guarantee, not a lock
//!
//! `firm_anchor_key` is the one field no caller sets from user input.
//! The firm's own Entity may not be forked, renamed, or deleted.
//! SurrealDB has no advisory lock, and a multi-statement query is not one
//! transaction, so the invariant lives
//! in the schema: `entity_commands` computes this key — the lowercased
//! name, and only for a row it judges to be an anchor — and every write
//! that mints it first claims [`CLAIM_TABLE`], a table whose *record id
//! is that key*.
//!
//! The claim is what the guarantee rests on, and it is **strong but not
//! absolute**. The UNIQUE `entity_firm_anchor` index reads like the guard
//! and is not one under concurrency: racers writing distinct entity rows
//! never collide on an index entry, so eight of them mint two anchors
//! (`store/tests/firm_anchor_race.rs`). Writing one identical *record*
//! key is far stronger, and is the claim's whole job.
//!
//! It is not total, though, and ENG-312 measured where it gives out: two
//! concurrent `CREATE`s of the *same* record id can **both** report
//! success while the `firm_anchor` table is still **empty**. Observed
//! directly — both racers reading the claim as free, both returning
//! `CLAIMED`, no retry involved — at roughly one double-commit per
//! 160,000 eight-way raced creates against an empty claim table. With one
//! claim row already present, 480,000 raced creates produced no fork on
//! either the embedded or the server engine.
//!
//! So the window is the first mint on a fresh database, and it closes as
//! soon as any claim row exists. `store/tests/firm_anchor_race.rs` builds
//! a fresh engine per round, which puts it inside that window on every
//! round — it is not a proxy for the steady state a deployment runs in.
//! `.github/workflows/firm-anchor-soak.yml` is the lane that watches this.
//!
//! Deciding *what counts as* an anchor stays in `entity_commands` with
//! `is_firm_anchor`, because it reads configuration. This module writes
//! what it is told and reports the collision as [`EntityError::FirmAnchorTaken`].

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::{AlreadyExistsError, ErrorDetails, SurrealValue};
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "entity";

/// One legal organization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entity {
    pub id: Uuid,
    /// Display name. Deliberately **not** unique — namesakes are real —
    /// except for the firm anchor, which [`Entity::is_firm_anchor`]
    /// reports and the `entity_firm_anchor` index enforces.
    pub name: String,
    /// The kind of organization. A `record<entity_type>` link in the
    /// engine, surfaced here as the id every caller already held.
    pub entity_type_id: Uuid,
    /// Where the organization is domiciled. A `record<jurisdiction>`
    /// link in the engine.
    pub jurisdiction_id: Uuid,
    /// Main phone line. `None` until set by the bulk-contact importer
    /// or an admin edit.
    pub phone: Option<String>,
    /// Canonical website URL (https), canonicalized by the importer.
    pub url: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The lowercased name when this row is the firm anchor, `None`
    /// otherwise. Read-only to callers: `entity_commands` computes it.
    pub firm_anchor_key: Option<String>,
}

impl Entity {
    /// Whether this row carries the firm-anchor key — the protected
    /// row that may not be forked, renamed, or deleted.
    #[must_use]
    pub fn is_firm_anchor(&self) -> bool {
        self.firm_anchor_key.is_some()
    }
}

/// The row as the engine reads and writes it. Separate from [`Entity`]
/// because the SDK owns its own `RecordId` and `Datetime`, and the
/// conversion belongs at this seam rather than in every caller.
#[derive(SurrealValue)]
struct EntityRow {
    id: surrealdb::types::RecordId,
    name: String,
    entity_type_id: surrealdb::types::RecordId,
    jurisdiction_id: surrealdb::types::RecordId,
    phone: Option<String>,
    url: Option<String>,
    firm_anchor_key: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl EntityRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`], or a
    /// link that names one.
    fn into_entity(self) -> Option<Entity> {
        Some(Entity {
            id: record_uuid(&self.id)?,
            name: self.name,
            entity_type_id: record_uuid(&self.entity_type_id)?,
            jurisdiction_id: record_uuid(&self.jurisdiction_id)?,
            phone: self.phone,
            url: self.url,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
            firm_anchor_key: self.firm_anchor_key,
        })
    }
}

/// The projection every read shares, so one field list describes the
/// row and a new column cannot reach [`EntityRow`] from only one query.
const SELECT: &str = "id, name, entity_type_id, jurisdiction_id, phone, url, firm_anchor_key, \
                      inserted_at, updated_at";

/// What a write stores. `firm_anchor_key` is computed by
/// `entity_commands`, never supplied by a request body.
#[derive(Debug, Clone)]
pub struct NewEntity {
    pub name: String,
    pub entity_type_id: Uuid,
    pub jurisdiction_id: Uuid,
    pub phone: Option<String>,
    pub url: Option<String>,
    pub firm_anchor_key: Option<String>,
}

/// Errors reading or writing an entity.
#[derive(Debug, thiserror::Error)]
pub enum EntityError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `entity_firm_anchor` — a row already
    /// holds the firm anchor's identity, and a second would be born
    /// protected. See this module's header.
    #[error("the firm entity already exists")]
    FirmAnchorTaken,
    /// A write reported success but returned no row, or one this
    /// module could not read back.
    #[error("writing an entity returned no usable row")]
    WriteReturnedNothing,
}

/// The table whose **record id is the firm-anchor key**. Writing it is
/// what serializes two racers minting the anchor: they collide on one
/// identical record key, which the engine enforces far more reliably than
/// a UNIQUE index entry — which under concurrency it does not enforce at
/// all. Not absolutely, though: see this module's header for the measured
/// empty-table window. See also the `firm_anchor` block in
/// `store/src/schema/navigator.surql`.
const CLAIM_TABLE: &str = "firm_anchor";

/// Claim the anchor for `$id`, refusing when any row already holds it.
///
/// `CREATE` rather than `UPSERT` on purpose: `UPSERT` would take the claim
/// from its current holder, which is the fork this exists to refuse.
///
/// **It runs as its own statement, never inside a wider transaction.**
/// That is the whole mechanism, and it is easy to "tidy" away. An
/// explicit `BEGIN … COMMIT` reads the snapshot taken at `BEGIN`, so two
/// racers each see the claim free and each `CREATE` succeeds against its
/// own snapshot — the fork comes straight back, and the wider the
/// transaction the likelier it is. Committing the claim on its own is
/// what makes the second racer read the first one's row.
const CLAIM: &str = "CREATE type::record('firm_anchor', $firm_anchor_key) SET entity_id = $id;";

/// Release whatever claim `$id` holds. A no-op when it holds none, so it
/// costs nothing to run on a path that may not have claimed.
const RELEASE: &str = "DELETE firm_anchor WHERE entity_id = $id;";

/// Turn a write failure into the conflict it names, or leave it a
/// database fault.
///
/// The claim collision is **typed**: `CREATE` onto a taken record id
/// reports [`AlreadyExistsError::Record`] carrying that id, so the
/// discriminator is a structured value rather than prose — the same
/// standard [`crate::surreal::retry::is_retryable`] holds itself to, and
/// the shared classifier in [`crate::surreal::retry`] could not meet.
///
/// The UNIQUE `entity_firm_anchor` index is still a backstop for a fork
/// that is not a race, and it *is* untyped, so its arm still reads the
/// message. That arm is pinned by
/// [`tests::a_unique_violation_carries_no_typed_detail_only_the_index_name`]
/// against a real engine, so an upstream reword fails there rather than
/// silently turning a refusal into a server fault.
fn classify_write(error: surrealdb::Error) -> EntityError {
    if claims_the_firm_anchor(&error)
        || crate::surreal::retry::unique_violation(&error) == Some("entity_firm_anchor")
    {
        EntityError::FirmAnchorTaken
    } else {
        EntityError::Db(error)
    }
}

/// Whether `error` is the claim record refusing a second holder.
fn claims_the_firm_anchor(error: &surrealdb::Error) -> bool {
    matches!(
        error.details(),
        ErrorDetails::AlreadyExists(Some(AlreadyExistsError::Record { id }))
            if id.starts_with(CLAIM_TABLE)
    )
}

/// Take the anchor claim for `id` when it does not already hold it,
/// reporting whether this call is the one now holding it.
///
/// A write that mints nothing claims nothing, and a holder re-writing its
/// own row claims nothing either — which is what makes a re-run seed and
/// an in-place edit of the anchor idempotent rather than self-refusing.
/// A holder that is a *different* entity does try, and is refused, which
/// is the right answer.
async fn take_claim(db: &SurrealDb, id: Uuid, key: Option<&str>) -> Result<bool, EntityError> {
    let Some(key) = key else { return Ok(false) };
    let holder = firm_anchor_holder(db, key).await?;
    if holder == Some(id) {
        anchor_trace(&format!(
            "id={id} holder={holder:?} exit=skip-holder-is-self"
        ));
        return Ok(false);
    }
    let outcome = writing(|| {
        db.query(CLAIM)
            .bind(("id", record_id(TABLE, id)))
            .bind(("firm_anchor_key", key.to_string()))
    })
    .await;
    match &outcome {
        Ok(_) => anchor_trace(&format!("id={id} holder={holder:?} exit=CLAIMED")),
        Err(error) => {
            anchor_trace(&format!("id={id} holder={holder:?} exit=refused {error:?}"));
        }
    }
    outcome?;
    Ok(true)
}

/// ENG-312 diagnostic. Off unless `NAV_ANCHOR_TRACE` is set, so ordinary
/// runs are untouched; the soak lane sets it to catch which branch each
/// racer left `take_claim` through when the anchor forks.
fn anchor_trace(message: &str) {
    if std::env::var_os("NAV_ANCHOR_TRACE").is_some() {
        eprintln!("ANCHOR {message}");
    }
}

/// Give back a claim taken for a write that then did not land, so a
/// failure does not leave the anchor claimed by a row that never became
/// it — which nothing could mint afterwards.
///
/// Best effort on purpose: it runs on a path that is already reporting
/// something else, and the caller's own outcome is the one worth
/// surfacing. A claim stranded here is visible as a `firm_anchor` row
/// whose `entity_id` names a row that does not carry the key.
async fn surrender_claim(db: &SurrealDb, id: Uuid) {
    let _ = writing(|| db.query(RELEASE).bind(("id", record_id(TABLE, id)))).await;
}

/// The entity currently holding `key`, or `None` when the anchor is free.
///
/// This reads the claim rather than `entity.firm_anchor_key`, so it
/// answers "who owns the claim" rather than "which row looks like the
/// anchor" — and it is a direct record read, not the scan
/// [`firm_anchor_exists`] performs.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn firm_anchor_holder(db: &SurrealDb, key: &str) -> Result<Option<Uuid>, EntityError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE entity_id FROM ONLY type::record('{CLAIM_TABLE}', $key)"
        ))
        .bind(("key", key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let holder: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(holder.as_ref().and_then(record_uuid))
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, EntityError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Entity>, EntityError> {
    let row: Option<EntityRow> = response.take(0)?;
    Ok(row.and_then(EntityRow::into_entity))
}

fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Entity>, EntityError> {
    let rows: Vec<EntityRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(EntityRow::into_entity)
        .collect())
}

/// Resolve one entity by id.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Entity>, EntityError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve many entities by id in one round trip — the bulk read a
/// conflict finding's labels need, so a report does not issue one query
/// per reached node.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn find_by_ids(db: &SurrealDb, ids: &[Uuid]) -> Result<Vec<Entity>, EntityError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let records: Vec<surrealdb::types::RecordId> =
        ids.iter().map(|id| record_id(TABLE, *id)).collect();
    let response = db
        .query(format!("SELECT {SELECT} FROM $ids"))
        .bind(("ids", records))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Resolve an entity by exact `name`.
///
/// Names are deliberately non-unique, so this returns the first match
/// the engine reports and is only meaningful for names the caller knows
/// to be singular — the firm anchor (`store::seed`) or a fixture.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn find_by_name(db: &SurrealDb, name: &str) -> Result<Option<Entity>, EntityError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE name = $name LIMIT 1"
        ))
        .bind(("name", name.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve an entity by the `(name, entity_type_id)` pair the canonical
/// seed keys on.
///
/// Names alone are not unique, and the seed's fixtures are distinguished
/// by their type — a `Human` and an `LLC` may legitimately share a name.
/// This is the seed's find half of find-or-create, not a general lookup.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn find_by_name_and_type(
    db: &SurrealDb,
    name: &str,
    entity_type_id: Uuid,
) -> Result<Option<Entity>, EntityError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE name = $name AND entity_type_id = $entity_type_id LIMIT 1"
        ))
        .bind(("name", name.trim().to_string()))
        .bind(("entity_type_id", record_id("entity_type", entity_type_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve an entity by the full `(name, entity_type_id,
/// jurisdiction_id)` triple.
///
/// The bulk-contact importer's identity: two organizations may share a
/// name, and even a name and a type, but the same name as the same kind
/// of entity in the same jurisdiction is one organization. Distinct from
/// [`find_by_name_and_type`], which the seed uses precisely *because* it
/// ignores the jurisdiction — it exists to repair one that drifted.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn find_by_identity(
    db: &SurrealDb,
    name: &str,
    entity_type_id: Uuid,
    jurisdiction_id: Uuid,
) -> Result<Option<Entity>, EntityError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE name = $name AND entity_type_id = $entity_type_id \
               AND jurisdiction_id = $jurisdiction_id LIMIT 1"
        ))
        .bind(("name", name.trim().to_string()))
        .bind(("entity_type_id", record_id("entity_type", entity_type_id)))
        .bind((
            "jurisdiction_id",
            record_id("jurisdiction", jurisdiction_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Point an existing entity at a different jurisdiction, leaving every
/// other field alone.
///
/// The canonical seed's repair path: a persisted database can outlive a
/// memory-backed local engine whose re-seeded jurisdictions carry fresh
/// ids, and a link the engine never validated then dangles. Narrow on
/// purpose — a full [`update`] here would need the caller to restate
/// fields it has no opinion about.
///
/// # Errors
///
/// [`EntityError::Db`] if the write fails.
pub async fn repoint_jurisdiction(
    db: &SurrealDb,
    id: Uuid,
    jurisdiction_id: Uuid,
) -> Result<Option<Entity>, EntityError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPDATE $id SET jurisdiction_id = $jurisdiction_id, updated_at = time::now() \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind((
            "jurisdiction_id",
            record_id("jurisdiction", jurisdiction_id),
        ))
    })
    .await?;
    let row: Option<EntityRow> = response.take(0)?;
    Ok(row.and_then(EntityRow::into_entity))
}

/// Move `firm_anchor_key` onto — or off — the entity carrying `id`,
/// returning the saved row.
///
/// Every other door computes the key from the name at the instant it
/// writes the row, which is right for a create and for an operator
/// edit. It is not enough for a *rename of the configured anchor
/// itself*: the rows already exist, the seed skips them, and the delete
/// guard reads this column rather than the name — so an anchor moved in
/// `seed::FIRM_ENTITY_NAME` would leave the outgoing firm protected and
/// the incoming one deletable. This is the seam the seed reconciles
/// through, and it is deliberately narrow: it writes one column and
/// takes the key already computed by
/// [`crate::entity_commands::firm_anchor_key`] rather than a name it
/// would have to re-derive.
///
/// # Errors
///
/// [`EntityError::FirmAnchorTaken`] when another row already holds
/// `key`, and [`EntityError::Db`] for anything else.
pub async fn set_firm_anchor_key(
    db: &SurrealDb,
    id: Uuid,
    key: Option<String>,
) -> Result<Option<Entity>, EntityError> {
    let write = format!(
        "UPDATE $id SET firm_anchor_key = $firm_anchor_key, updated_at = time::now() \
         RETURN {SELECT}"
    );
    // Surrendering the key must surrender the claim with it, or the
    // anchor could never be minted again — the claim, not the column, is
    // what a later mint collides with.
    let claimed = take_claim(db, id, key.as_deref()).await?;
    // Surrendering the key surrenders the claim with it, or the anchor
    // could never be minted again — the claim, not the column, is what a
    // later mint collides with. The column is cleared first, so a failure
    // between the two leaves the anchor claimed rather than free while a
    // row still reads as the anchor.
    let statement = if key.is_none() {
        format!("{write}; {RELEASE}")
    } else {
        write
    };
    let written = async {
        let mut response = writing(|| {
            db.query(&statement)
                .bind(("id", record_id(TABLE, id)))
                .bind(("firm_anchor_key", key.clone()))
        })
        .await?;
        let row: Option<EntityRow> = response.take(0)?;
        Ok::<_, EntityError>(row.and_then(EntityRow::into_entity))
    }
    .await;
    if claimed && !matches!(written, Ok(Some(_))) {
        surrender_claim(db, id).await;
    }
    written
}

/// Whether any row already carries `key` as its firm-anchor key.
///
/// The existence half of the guard `entity_commands` runs before a
/// create. It is **not** what makes the guard safe — the claim
/// [`take_claim`] writes is — but it turns the common case into a clean
/// validation error rather than a write that has to fail first.
///
/// It reads `entity.firm_anchor_key`, so it answers "does a row look like
/// the anchor". [`firm_anchor_holder`] answers the different question of
/// who holds the claim, and that is the one a race turns on.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn firm_anchor_exists(db: &SurrealDb, key: &str) -> Result<bool, EntityError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE id FROM {TABLE} WHERE firm_anchor_key = $key LIMIT 1"
        ))
        .bind(("key", key.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let ids: Vec<surrealdb::types::RecordId> = response.take(0)?;
    Ok(!ids.is_empty())
}

/// Every entity, ordered by name so a listing is stably ordered before
/// any caller applies its own sort.
///
/// # Errors
///
/// [`EntityError::Db`] if the lookup fails.
pub async fn all(db: &SurrealDb) -> Result<Vec<Entity>, EntityError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY name ASC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// The `SET` clause every write shares, so a new column cannot be
/// stored by only one of them.
const WRITE_FIELDS: &str = "name = $name, entity_type_id = $entity_type_id, \
                            jurisdiction_id = $jurisdiction_id, phone = $phone, url = $url, \
                            firm_anchor_key = $firm_anchor_key, updated_at = time::now()";

/// Write a new entity row under a fresh v7 id.
///
/// # Errors
///
/// [`EntityError::FirmAnchorTaken`] when the row would fork the firm
/// anchor, and [`EntityError::Db`] for anything else.
pub async fn create(db: &SurrealDb, input: &NewEntity) -> Result<Entity, EntityError> {
    upsert_with_id(db, Uuid::now_v7(), input).await
}

/// Create or update the entity carrying `id`.
///
/// [`create`] owns id generation, which is right for a real create. A
/// seed or a fixture that must exist under a known id cannot use it.
/// Idempotent, so re-running a seed reconciles the row instead of
/// duplicating it.
///
/// # Errors
///
/// [`EntityError::FirmAnchorTaken`] when the row would fork the firm
/// anchor, and [`EntityError::Db`] for anything else.
pub async fn upsert_with_id(
    db: &SurrealDb,
    id: Uuid,
    input: &NewEntity,
) -> Result<Entity, EntityError> {
    let claimed = take_claim(db, id, input.firm_anchor_key.as_deref()).await?;
    let written = upsert_row(db, id, input).await;
    if claimed && written.is_err() {
        surrender_claim(db, id).await;
    }
    written
}

/// The `UPSERT` half of [`upsert_with_id`], after the claim is settled.
async fn upsert_row(db: &SurrealDb, id: Uuid, input: &NewEntity) -> Result<Entity, EntityError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPSERT $id SET {WRITE_FIELDS}, \
             inserted_at = IF inserted_at THEN inserted_at ELSE time::now() END \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("name", input.name.trim().to_string()))
        .bind((
            "entity_type_id",
            record_id("entity_type", input.entity_type_id),
        ))
        .bind((
            "jurisdiction_id",
            record_id("jurisdiction", input.jurisdiction_id),
        ))
        .bind(("phone", input.phone.clone()))
        .bind(("url", input.url.clone()))
        .bind(("firm_anchor_key", input.firm_anchor_key.clone()))
    })
    .await?;
    let row: Option<EntityRow> = response.take(0)?;
    row.and_then(EntityRow::into_entity)
        .ok_or(EntityError::WriteReturnedNothing)
}

/// Replace every field of the entity carrying `id`, returning the saved
/// row — or `None` when no such row exists **or the write would rename
/// the firm anchor**.
///
/// The `WHERE` is the rename half of the guard [`delete_unless_firm_anchor`]
/// carries for deletes, and it exists for the same reason: a concurrent
/// write can mint the anchor on this row after the caller read it as
/// ordinary, and a rename that decided on that earlier read would move
/// the freshly-protected firm away. The condition therefore travels with
/// the write rather than being checked before it.
///
/// It admits exactly two cases: the name is unchanged (so the anchor
/// stays editable — an operator can still correct its entity type), or
/// the row is not the anchor. A caller that needs to tell "refused" from
/// "no such row" re-reads; this seam reports both as `None` because from
/// its side they are the same outcome — nothing was written.
///
/// # Errors
///
/// [`EntityError::FirmAnchorTaken`] when the write would fork the firm
/// anchor, and [`EntityError::Db`] for anything else.
pub async fn update(
    db: &SurrealDb,
    id: Uuid,
    input: &NewEntity,
) -> Result<Option<Entity>, EntityError> {
    let claimed = take_claim(db, id, input.firm_anchor_key.as_deref()).await?;
    let written = update_row(db, id, input).await;
    // `Ok(None)` is the refused rename, not a write — a claim taken for it
    // has to go back, or one refused edit would strand the anchor.
    if claimed && !matches!(written, Ok(Some(_))) {
        surrender_claim(db, id).await;
    }
    written
}

/// The `UPDATE` half of [`update`], after the claim is settled.
async fn update_row(
    db: &SurrealDb,
    id: Uuid,
    input: &NewEntity,
) -> Result<Option<Entity>, EntityError> {
    let mut response = writing(|| {
        db.query(format!(
            "UPDATE $id SET {WRITE_FIELDS} \
             WHERE name = $name OR firm_anchor_key IS NONE \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("name", input.name.trim().to_string()))
        .bind((
            "entity_type_id",
            record_id("entity_type", input.entity_type_id),
        ))
        .bind((
            "jurisdiction_id",
            record_id("jurisdiction", input.jurisdiction_id),
        ))
        .bind(("phone", input.phone.clone()))
        .bind(("url", input.url.clone()))
        .bind(("firm_anchor_key", input.firm_anchor_key.clone()))
    })
    .await?;
    let rows: Vec<EntityRow> = response.take(0)?;
    Ok(rows.into_iter().next().and_then(EntityRow::into_entity))
}

/// Delete the entity carrying `id` **unless it is the firm anchor**,
/// returning the removed row — or `None` when nothing was removed,
/// because the row was already gone or because it was protected.
///
/// There is deliberately no unguarded delete. The protection has to be
/// part of the delete statement rather than a check the caller ran
/// first: a concurrent rename can mint the anchor in the window between
/// the two, and a delete that decided on the earlier read would then
/// remove the freshly-protected firm. The `WHERE`
/// closes that window by making the refusal a property of the row at the instant
/// of the write.
///
/// `firm_anchor_key` is the discriminator rather than the name, because
/// it is what every door that mints an anchor sets — and what the
/// `entity_firm_anchor` index already trusts.
///
/// Callers must still check [`dependents`] first: nothing in the engine
/// refuses a delete that strands a reference, because a `record<>` link
/// is not a foreign key.
///
/// # Errors
///
/// [`EntityError::Db`] if the delete fails.
pub async fn delete_unless_firm_anchor(
    db: &SurrealDb,
    id: Uuid,
) -> Result<Option<Entity>, EntityError> {
    let mut response = writing(|| {
        db.query("DELETE $id WHERE firm_anchor_key IS NONE RETURN BEFORE")
            .bind(("id", record_id(TABLE, id)))
    })
    .await?;
    let rows: Vec<EntityRow> = response.take(0)?;
    Ok(rows.into_iter().next().and_then(EntityRow::into_entity))
}

/// The tables that reference `id`, with how many rows each holds. Empty
/// when the entity is free to delete.
///
/// Nothing in the engine refuses a delete that strands a reference, so
/// the check is an explicit read *before* the write. The counts are
/// reported rather than a bare boolean so the operator sees which rows to
/// detach.
///
/// # Errors
///
/// [`EntityError::Db`] if any count fails.
pub async fn dependents(db: &SurrealDb, id: Uuid) -> Result<Vec<Dependent>, EntityError> {
    let mut response = db
        .query(
            "SELECT VALUE count() FROM project WHERE entity_id = $id GROUP ALL;
             SELECT VALUE count() FROM address WHERE entity_id = $id GROUP ALL;
             SELECT VALUE count() FROM entity_role WHERE out = $id GROUP ALL;
             SELECT VALUE count() FROM relationship WHERE in = $id OR out = $id GROUP ALL;",
        )
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    // `GROUP ALL` over an empty table yields no row at all, not a zero,
    // so the absent case is the common one and must read as none.
    let mut found = Vec::new();
    for (index, (singular, plural)) in DEPENDENT_TABLES.iter().enumerate() {
        let counted: Option<i64> = response.take(index)?;
        let count = usize::try_from(counted.unwrap_or(0)).unwrap_or(0);
        if count > 0 {
            found.push(Dependent {
                singular,
                plural,
                count,
            });
        }
    }
    Ok(found)
}

/// One table still pointing at an entity, and how many of its rows do.
///
/// Both spellings travel because the refusal is shown to a person: "1
/// address" and "2 addresses" are the same fact, and deriving one from
/// the other by trimming an `s` gets "1 addresse".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependent {
    pub singular: &'static str,
    pub plural: &'static str,
    pub count: usize,
}

impl Dependent {
    /// The noun this count should be printed with.
    #[must_use]
    pub fn noun(&self) -> &'static str {
        if self.count == 1 {
            self.singular
        } else {
            self.plural
        }
    }
}

/// The Surreal-resident tables that reference `entity`, in the order
/// [`dependents`] counts them — which is the order its statements run,
/// so the two cannot drift apart silently.
const DEPENDENT_TABLES: [(&str, &str); 4] = [
    ("project", "projects"),
    ("address", "addresses"),
    ("entity role", "entity roles"),
    ("relationship", "relationships"),
];

#[cfg(test)]
mod tests {
    use super::classify_write;
    use super::{
        all, create, delete_unless_firm_anchor, dependents, find_by_id, find_by_ids, find_by_name,
        firm_anchor_exists, update, AlreadyExistsError, EntityError, ErrorDetails, NewEntity,
        CLAIM, CLAIM_TABLE, RELEASE, TABLE, WRITE_FIELDS,
    };
    use crate::surreal::test_support::mem;
    use crate::surreal::{record_id, retry, SurrealDb};
    use uuid::Uuid;

    fn input(name: &str) -> NewEntity {
        NewEntity {
            name: name.into(),
            entity_type_id: Uuid::now_v7(),
            jurisdiction_id: Uuid::now_v7(),
            phone: None,
            url: None,
            firm_anchor_key: None,
        }
    }

    fn anchor(name: &str) -> NewEntity {
        NewEntity {
            firm_anchor_key: Some(name.to_lowercase()),
            ..input(name)
        }
    }

    #[tokio::test]
    async fn a_created_entity_reads_back_by_id_and_name() {
        let db = mem().await;
        let created = create(&db, &input("  Beta LLC  ")).await.unwrap();

        assert_eq!(
            created.name, "Beta LLC",
            "the name is trimmed on the way in"
        );
        assert_eq!(
            find_by_id(&db, created.id).await.unwrap(),
            Some(created.clone())
        );
        assert_eq!(find_by_name(&db, "Beta LLC").await.unwrap(), Some(created));
        assert_eq!(find_by_name(&db, "Nothing Co").await.unwrap(), None);
    }

    /// The reference ids are `record<>` links in the engine but plain
    /// UUIDs to a caller, and the round trip is the only thing proving
    /// the two spellings agree — `entity_type:u'…'` and `entity_type:⟨…⟩`
    /// both parse and are different records.
    #[tokio::test]
    async fn the_reference_links_round_trip_as_the_ids_the_caller_supplied() {
        let db = mem().await;
        let wanted = input("Gamma Trust");
        let created = create(&db, &wanted).await.unwrap();

        assert_eq!(created.entity_type_id, wanted.entity_type_id);
        assert_eq!(created.jurisdiction_id, wanted.jurisdiction_id);
        let read_back = find_by_id(&db, created.id).await.unwrap().unwrap();
        assert_eq!(read_back.entity_type_id, wanted.entity_type_id);
        assert_eq!(read_back.jurisdiction_id, wanted.jurisdiction_id);
    }

    /// The `firm_anchor` claim is what refuses the fork, so this passes
    /// without any read-before-write on the part of the caller.
    #[tokio::test]
    async fn a_second_row_under_the_firm_anchor_key_is_refused() {
        let db = mem().await;
        create(&db, &anchor("Neon Law")).await.unwrap();

        let forked = create(&db, &anchor("Neon Law")).await;
        assert!(
            matches!(forked, Err(EntityError::FirmAnchorTaken)),
            "the unique `entity_firm_anchor` index is the gate, got {forked:?}"
        );
        assert!(firm_anchor_exists(&db, "neon law").await.unwrap());
    }

    /// Concurrency is the whole reason the guard is an index rather than
    /// a read followed by a write: two racers both see a free name, and
    /// only the index can stop them both writing.
    #[tokio::test]
    async fn concurrent_creates_of_the_anchor_yield_exactly_one_row() {
        let db = mem().await;
        let racer = anchor("Neon Law");
        let other = anchor("Neon Law");
        let (first, second) = tokio::join!(create(&db, &racer), create(&db, &other));

        let outcomes = [first, second];
        assert_eq!(
            outcomes.iter().filter(|r| r.is_ok()).count(),
            1,
            "exactly one racer may mint the anchor: {outcomes:?}"
        );
        assert_eq!(all(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ordinary_namesakes_still_coexist() {
        // Only the anchor takes the key; two unrelated Betas both land,
        // because multiple NONEs do not collide on a unique index.
        let db = mem().await;
        create(&db, &input("Beta LLC")).await.unwrap();
        create(&db, &input("Beta LLC")).await.unwrap();
        assert_eq!(all(&db).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn update_replaces_every_field_and_reports_a_missing_row() {
        let db = mem().await;
        let row = create(&db, &input("Beta LLC")).await.unwrap();
        let mut edit = input("Beta Holdings LLC");
        edit.phone = Some("+1 702 555 0100".into());
        edit.url = Some("https://example.com".into());

        let updated = update(&db, row.id, &edit).await.unwrap().unwrap();
        assert_eq!(updated.id, row.id);
        assert_eq!(updated.name, "Beta Holdings LLC");
        assert_eq!(updated.entity_type_id, edit.entity_type_id);
        assert_eq!(updated.phone.as_deref(), Some("+1 702 555 0100"));
        assert_eq!(updated.url.as_deref(), Some("https://example.com"));

        assert_eq!(update(&db, Uuid::now_v7(), &edit).await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_returns_the_removed_row_once_and_then_nothing() {
        let db = mem().await;
        let row = create(&db, &input("Beta LLC")).await.unwrap();

        assert_eq!(
            delete_unless_firm_anchor(&db, row.id)
                .await
                .unwrap()
                .map(|e| e.id),
            Some(row.id)
        );
        assert_eq!(find_by_id(&db, row.id).await.unwrap(), None);
        assert_eq!(delete_unless_firm_anchor(&db, row.id).await.unwrap(), None);
    }

    /// The window the delete's own `WHERE` closes: a rename mints the
    /// anchor *after* a delete has read its target and judged it
    /// ordinary. A delete that decided on that earlier read would remove
    /// the freshly-protected firm, so the refusal lives in the delete.
    #[tokio::test]
    async fn a_row_that_became_the_anchor_after_the_read_is_not_deleted() {
        let db = mem().await;
        let ordinary = create(&db, &input("Acme LLC")).await.unwrap();

        // What a caller would have read: an ordinary, deletable row.
        assert!(!find_by_id(&db, ordinary.id)
            .await
            .unwrap()
            .unwrap()
            .is_firm_anchor());

        // …and then a concurrent rename mints the anchor on it.
        update(
            &db,
            ordinary.id,
            &NewEntity {
                firm_anchor_key: Some("shook law pllc".into()),
                ..input("Neon Law")
            },
        )
        .await
        .unwrap();

        assert_eq!(
            delete_unless_firm_anchor(&db, ordinary.id).await.unwrap(),
            None,
            "the delete must refuse a row that became the anchor"
        );
        assert!(
            find_by_id(&db, ordinary.id).await.unwrap().is_some(),
            "the freshly-protected firm must survive the racing delete"
        );
    }

    #[tokio::test]
    async fn find_by_ids_reads_many_and_ignores_ids_with_no_row() {
        let db = mem().await;
        let one = create(&db, &input("One LLC")).await.unwrap();
        let two = create(&db, &input("Two LLC")).await.unwrap();

        let found = find_by_ids(&db, &[one.id, two.id, Uuid::now_v7()])
            .await
            .unwrap();
        let mut names: Vec<String> = found.into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["One LLC", "Two LLC"]);
        assert!(find_by_ids(&db, &[]).await.unwrap().is_empty());
    }

    /// The read that replaced the foreign-key violation. Nothing in the
    /// engine refuses a delete that strands a reference, so this is the
    /// only thing between the lawyer delete button and a matter pointing
    /// at an entity that is gone.
    #[tokio::test]
    async fn dependents_counts_the_rows_a_delete_would_strand() {
        let db = mem().await;
        let entity = create(&db, &input("Referenced LLC")).await.unwrap();
        assert!(
            dependents(&db, entity.id).await.unwrap().is_empty(),
            "a fresh entity is free to delete"
        );

        crate::projects::create(
            &db,
            &crate::projects::NewProject {
                code: format!("dep-{}", Uuid::now_v7()),
                name: "Dependent matter".into(),
                status: "open".into(),
                entity_id: entity.id,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let found = dependents(&db, entity.id).await.unwrap();
        assert_eq!(
            found
                .iter()
                .map(|d| (d.noun(), d.count))
                .collect::<Vec<_>>(),
            vec![("project", 1)],
            "the matter opened against this entity has to be named"
        );
    }

    /// A dangling link is invisible to the engine, so a count keyed on
    /// the wrong record-id spelling would silently read zero and let a
    /// referenced entity be deleted. Pinning a *non*-zero count on a
    /// second table is what catches that.
    #[tokio::test]
    async fn dependents_sees_a_graph_edge_as_well_as_a_matter() {
        let db = mem().await;
        let entity = create(&db, &input("Edged LLC")).await.unwrap();
        let person = crate::persons::create(
            &db,
            &crate::persons::NewPerson::new(
                "Edge Holder",
                format!("{}@example.com", Uuid::now_v7()),
            ),
        )
        .await
        .unwrap();
        crate::entity_roles::grant(&db, person.id, entity.id, "manages")
            .await
            .unwrap();

        let found = dependents(&db, entity.id).await.unwrap();
        assert_eq!(
            found
                .iter()
                .map(|d| (d.noun(), d.count))
                .collect::<Vec<_>>(),
            vec![("entity role", 1)]
        );
    }

    /// `mem()` applies the deployment schema, so this also proves the
    /// handle the rest of the module is tested against is the real one.
    #[tokio::test]
    async fn listing_orders_by_name() {
        let db: SurrealDb = mem().await;
        for name in ["Zeta LLC", "Alpha LLC", "Mu LLC"] {
            create(&db, &input(name)).await.unwrap();
        }
        let names: Vec<String> = all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["Alpha LLC", "Mu LLC", "Zeta LLC"]);
    }

    /// The claim collision — the arm that actually runs — is typed, and
    /// this is what says so. `classify_write` reads
    /// [`AlreadyExistsError::Record`] rather than the message, so an
    /// upstream reword of the prose cannot turn a refusal into a `Db`
    /// fault. The day that detail stops arriving, this fails.
    ///
    /// It must also not be retryable: a refusal that
    /// [`crate::surreal::retry::is_retryable`] accepted would be re-run
    /// until the write budget expired and then reported as a conflict, so
    /// the caller would wait seconds for the wrong error.
    #[tokio::test]
    async fn the_claim_collision_is_typed_and_final() {
        let db = mem().await;
        create(&db, &anchor("Neon Law")).await.unwrap();

        let raw = db
            .query("CREATE type::record('firm_anchor', $key) SET entity_id = $id")
            .bind(("key", "neon law".to_string()))
            .bind(("id", record_id(TABLE, Uuid::now_v7())))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("a second claim must collide");

        assert!(
            matches!(
                raw.details(),
                ErrorDetails::AlreadyExists(Some(AlreadyExistsError::Record { id }))
                    if id.starts_with(CLAIM_TABLE)
            ),
            "the claim's typed detail is gone — classify_write would fall back \
             to the message; got {:?}",
            raw.details(),
        );
        assert!(
            !retry::is_retryable(&raw),
            "a refused claim is not a lost race and must not be re-run",
        );
        assert!(matches!(classify_write(raw), EntityError::FirmAnchorTaken));
    }

    /// The backstop arm, held against the engine the way
    /// `store::persons` holds its own.
    ///
    /// The UNIQUE index cannot serialize racers, but it still refuses a
    /// fork that is not a race — and that refusal *is* untyped: the engine
    /// raises it as the `Internal` catch-all, so the index name in the
    /// message is the only discriminator left. It is a `DEFINE INDEX`
    /// identifier this workspace chose, and this pins it, so a silent
    /// reword fails here rather than in production.
    #[tokio::test]
    async fn a_unique_violation_carries_no_typed_detail_only_the_index_name() {
        let db = mem().await;
        let holder = create(&db, &anchor("Neon Law")).await.unwrap();
        // Around the claim on purpose: this exercises the index alone,
        // which is what a non-concurrent fork would meet.
        let raw = db
            .query(format!(
                "UPSERT $id SET {WRITE_FIELDS}, inserted_at = time::now()"
            ))
            .bind(("id", record_id(TABLE, Uuid::now_v7())))
            .bind(("name", "Neon Law".to_string()))
            .bind((
                "entity_type_id",
                record_id("entity_type", holder.entity_type_id),
            ))
            .bind((
                "jurisdiction_id",
                record_id("jurisdiction", holder.jurisdiction_id),
            ))
            .bind(("phone", None::<String>))
            .bind(("url", None::<String>))
            .bind(("firm_anchor_key", Some("neon law".to_string())))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("the index must refuse the second row");

        assert!(
            matches!(raw.details(), ErrorDetails::Internal),
            "a typed detail is now available — classify_write should match on it \
             instead of the message; got {:?}",
            raw.details(),
        );
        assert!(
            raw.to_string().contains("entity_firm_anchor"),
            "the index name is the only discriminator, and it is gone: {raw}",
        );
        assert!(matches!(classify_write(raw), EntityError::FirmAnchorTaken));
    }

    /// An error that is neither refusal stays a `Db` fault. Widening
    /// either arm into a catch-all would make a server fault read as a
    /// clean refusal, which is exactly how a broken guard looks green.
    #[tokio::test]
    async fn an_unrelated_failure_is_not_mistaken_for_a_refusal() {
        let db = mem().await;
        let raw = db
            .query("SELECT * FROM entity WHERE")
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("a malformed statement must fail");

        assert!(matches!(classify_write(raw), EntityError::Db(_)));
    }

    /// [`CLAIM`] and [`RELEASE`] name the table in SurrealQL, where a
    /// `const` cannot interpolate [`CLAIM_TABLE`]. Renaming the constant
    /// without rewriting the statements would leave writes claiming one
    /// table while [`claims_the_firm_anchor`] classified another — a
    /// refusal silently demoted to a server fault.
    #[test]
    fn the_claim_statements_name_the_claim_table() {
        assert!(CLAIM.contains(CLAIM_TABLE), "{CLAIM}");
        assert!(RELEASE.contains(CLAIM_TABLE), "{RELEASE}");
    }
}
