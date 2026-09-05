//! `store::authorities` — the citation apparatus' write and read
//! commands (#890).
//!
//! The module exists to hold one distinction: an **Authority** is global
//! reference data, while a **matter's use** of it is scoped. Every
//! command here keeps that split, because collapsing it is what leaks one
//! matter's litigation posture into another matter's view of the same
//! case.
//!
//! Reads come in two lenses, and the client lens is not merely a subset —
//! it excludes dispositions that record firm reasoning. See
//! [`client_visible_uses`].
//!
//! # This table group lives in SurrealDB
//!
//! `authorities`, `authority_uses`, and `citations` moved with wave five
//! of #1093 (ENG-121), in the citation-apparatus slice. Matter scoping
//! flows through `authority_use.project_id` alone: `citation` carries no
//! `project_id` of its own, resolving its matter only by following
//! `authority_use_id`, and `authority` carries none at all, being global
//! reference data. Neither the Rust layer nor the Surreal schema
//! denormalizes a `project_id` onto either row — see
//! `store/src/schema/navigator.surql`'s comments on `citation` and
//! `authority`.
//!
//! "One posture per matter per authority" is a convention here rather than
//! an index: the Surreal schema's
//! `authority_use_project` index is not marked `UNIQUE`, so
//! [`cite_in_matter`] reads the existing row back before writing rather
//! than relying on the engine to refuse a duplicate — the same shape
//! `store::attestations::record` uses for the same reason.
//!
//! A matter delete reaches
//! `authority_uses → citations → verifications`, and
//! Surreal cascades none of it, so [`delete_for_project`] walks
//! the chain explicitly.

use rules::citation::{AuthorityClass, Disposition};
use serde_json::Value as Json;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

const AUTHORITY_TABLE: &str = "authority";
const AUTHORITY_USE_TABLE: &str = "authority_use";
const CITATION_TABLE: &str = "citation";

/// Why a citation-apparatus command refused.
#[derive(Debug, thiserror::Error)]
pub enum AuthorityError {
    /// The class is outside [`AuthorityClass`].
    #[error("`{0}` is not a recognized authority class")]
    UnknownClass(String),
    /// The disposition is outside [`Disposition`].
    #[error("`{0}` is not a recognized citation disposition")]
    UnknownDisposition(String),
    /// The position is outside the closed `ours` / `adverse` / `neutral`
    /// vocabulary.
    #[error("`{0}` is not a recognized position")]
    UnknownPosition(String),
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing to the citation apparatus returned no usable row")]
    WriteReturnedNothing,
}

fn is_authority_citation_conflict(error: &surrealdb::Error) -> bool {
    crate::surreal::retry::unique_violation(error) == Some("authority_citation")
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, surrealdb::Error>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await
}

// ---------------------------------------------------------------------
// authority — global reference data
// ---------------------------------------------------------------------

/// One globally-shared piece of authority: a case, a statute, a
/// regulation, an administrative order, or a secondary source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Authority {
    pub id: Uuid,
    pub class: String,
    pub citation: String,
    pub short_cite: Option<String>,
    pub title: String,
    pub publisher: Option<String>,
    pub issued_on: Option<String>,
    pub canonical_url: Option<String>,
    pub checked_on: Option<String>,
    pub archived_asset_id: Option<Uuid>,
    pub inserted_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SurrealValue)]
struct AuthorityRow {
    id: surrealdb::types::RecordId,
    class: String,
    citation: String,
    short_cite: Option<String>,
    title: String,
    publisher: Option<String>,
    issued_on: Option<String>,
    canonical_url: Option<String>,
    checked_on: Option<String>,
    archived_asset_id: Option<surrealdb::types::RecordId>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl AuthorityRow {
    /// `None` when a record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_authority(self) -> Option<Authority> {
        Some(Authority {
            id: record_uuid(&self.id)?,
            class: self.class,
            citation: self.citation,
            short_cite: self.short_cite,
            title: self.title,
            publisher: self.publisher,
            issued_on: self.issued_on,
            canonical_url: self.canonical_url,
            checked_on: self.checked_on,
            archived_asset_id: self.archived_asset_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const AUTHORITY_SELECT: &str = "id, class, citation, short_cite, title, publisher, issued_on, \
     canonical_url, checked_on, archived_asset_id, inserted_at, updated_at";

async fn by_citation(
    db: &SurrealDb,
    citation: &str,
) -> Result<Option<Authority>, surrealdb::Error> {
    let mut response = db
        .query(format!(
            "SELECT {AUTHORITY_SELECT} FROM {AUTHORITY_TABLE} WHERE citation = $citation LIMIT 1"
        ))
        .bind(("citation", citation.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<AuthorityRow> = response.take(0)?;
    Ok(row.and_then(AuthorityRow::into_authority))
}

/// What a new [`Authority`] needs.
#[derive(Debug, Clone)]
pub struct NewAuthority<'a> {
    pub class: AuthorityClass,
    pub citation: &'a str,
    pub short_cite: Option<&'a str>,
    pub title: &'a str,
    pub publisher: Option<&'a str>,
    pub issued_on: Option<&'a str>,
    pub canonical_url: Option<&'a str>,
    pub checked_on: Option<&'a str>,
    pub archived_asset_id: Option<Uuid>,
}

/// Find-or-create the global Authority for `new.citation`.
///
/// Find-or-create rather than insert, because the whole point of a global
/// table is that the second matter to cite a case reuses the first
/// matter's row. A blind insert would rebuild the five private lists this
/// table replaces, one unique-violation at a time. A race on `citation`
/// resolves by re-reading the winner's row, exactly like
/// `store::signatures::record_request`.
///
/// # Errors
/// [`AuthorityError`] on a database failure.
pub async fn record(db: &SurrealDb, new: &NewAuthority<'_>) -> Result<Authority, AuthorityError> {
    if let Some(existing) = by_citation(db, new.citation).await? {
        return Ok(existing);
    }

    let id = Uuid::now_v7();
    match db
        .query(format!(
            "CREATE $id SET \
             class = $class, citation = $citation, short_cite = $short_cite, title = $title, \
             publisher = $publisher, issued_on = $issued_on, canonical_url = $canonical_url, \
             checked_on = $checked_on, archived_asset_id = $archived_asset_id \
             RETURN {AUTHORITY_SELECT}"
        ))
        .bind(("id", record_id(AUTHORITY_TABLE, id)))
        .bind(("class", new.class.as_str().to_string()))
        .bind(("citation", new.citation.to_string()))
        .bind(("short_cite", new.short_cite.map(str::to_string)))
        .bind(("title", new.title.to_string()))
        .bind(("publisher", new.publisher.map(str::to_string)))
        .bind(("issued_on", new.issued_on.map(str::to_string)))
        .bind(("canonical_url", new.canonical_url.map(str::to_string)))
        .bind(("checked_on", new.checked_on.map(str::to_string)))
        .bind((
            "archived_asset_id",
            new.archived_asset_id.map(|id| record_id("asset", id)),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)
    {
        Ok(mut response) => {
            let row: Option<AuthorityRow> = response.take(0)?;
            row.and_then(AuthorityRow::into_authority)
                .ok_or(AuthorityError::WriteReturnedNothing)
        }
        Err(error) if is_authority_citation_conflict(&error) => by_citation(db, new.citation)
            .await?
            .ok_or(AuthorityError::WriteReturnedNothing),
        Err(error) => Err(AuthorityError::Db(error)),
    }
}

/// Parse a stored `authority.class`.
///
/// # Errors
/// [`AuthorityError::UnknownClass`] when the value is outside the closed
/// vocabulary — which means a row was written around the intended
/// taxonomy.
pub fn class_of(row: &Authority) -> Result<AuthorityClass, AuthorityError> {
    AuthorityClass::parse(&row.class).ok_or_else(|| AuthorityError::UnknownClass(row.class.clone()))
}

// ---------------------------------------------------------------------
// authority_use — a matter's scoped use of a global authority
// ---------------------------------------------------------------------

/// Which side of the matter relies on an authority.
pub const POSITIONS: &[&str] = &["ours", "adverse", "neutral"];

/// One matter's use of a global [`Authority`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthorityUse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub authority_id: Uuid,
    pub position: String,
    pub disposition: String,
    pub role: Option<String>,
    pub inserted_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SurrealValue)]
struct AuthorityUseRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    authority_id: surrealdb::types::RecordId,
    position: String,
    disposition: String,
    role: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl AuthorityUseRow {
    fn into_authority_use(self) -> Option<AuthorityUse> {
        Some(AuthorityUse {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            authority_id: record_uuid(&self.authority_id)?,
            position: self.position,
            disposition: self.disposition,
            role: self.role,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const AUTHORITY_USE_SELECT: &str =
    "id, project_id, authority_id, position, disposition, role, inserted_at, updated_at";

fn many_authority_uses(
    mut response: surrealdb::IndexedResults,
) -> Result<Vec<AuthorityUse>, surrealdb::Error> {
    let rows: Vec<AuthorityUseRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AuthorityUseRow::into_authority_use)
        .collect())
}

async fn find_use(
    db: &SurrealDb,
    project_id: Uuid,
    authority_id: Uuid,
) -> Result<Option<AuthorityUse>, surrealdb::Error> {
    let mut response = db
        .query(format!(
            "SELECT {AUTHORITY_USE_SELECT} FROM {AUTHORITY_USE_TABLE} \
             WHERE project_id = $project AND authority_id = $authority LIMIT 1"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("authority", record_id(AUTHORITY_TABLE, authority_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<AuthorityUseRow> = response.take(0)?;
    Ok(row.and_then(AuthorityUseRow::into_authority_use))
}

/// Record that `project_id` uses `authority_id`, taking `position` with
/// `disposition`.
///
/// Idempotent on `(project_id, authority_id)`: a matter takes one posture
/// on a given authority, so a second call updates that posture rather
/// than adding a contradictory second row. The Surreal schema carries no
/// unique index on the pair (see the module header), so this reads the
/// existing row back first and upserts it by id — the same shape
/// `store::attestations::record` uses.
///
/// # Errors
/// [`AuthorityError`] when the position is outside its closed vocabulary,
/// or on a database failure.
pub async fn cite_in_matter(
    db: &SurrealDb,
    project_id: Uuid,
    authority_id: Uuid,
    position: &str,
    disposition: Disposition,
    role: Option<&str>,
) -> Result<AuthorityUse, AuthorityError> {
    if !POSITIONS.contains(&position) {
        return Err(AuthorityError::UnknownPosition(position.to_string()));
    }

    let existing = find_use(db, project_id, authority_id).await?;
    let id = existing.as_ref().map_or_else(Uuid::now_v7, |u| u.id);
    let mut response = writing(|| {
        db.query(format!(
            "UPSERT $id SET \
             project_id = $project_id, authority_id = $authority_id, position = $position, \
             disposition = $disposition, role = $role, updated_at = time::now() \
             RETURN {AUTHORITY_USE_SELECT}"
        ))
        .bind(("id", record_id(AUTHORITY_USE_TABLE, id)))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("authority_id", record_id(AUTHORITY_TABLE, authority_id)))
        .bind(("position", position.to_string()))
        .bind(("disposition", disposition.as_str().to_string()))
        .bind(("role", role.map(str::to_string)))
    })
    .await?;
    let row: Option<AuthorityUseRow> = response.take(0)?;
    row.and_then(AuthorityUseRow::into_authority_use)
        .ok_or(AuthorityError::WriteReturnedNothing)
}

/// Every use recorded on `project_id`, newest first — the **lawyer lens**.
///
/// Scoped by `project_id` and nothing else, which is what makes one
/// matter's use of an authority invisible from another matter.
///
/// # Errors
/// Propagates any database error.
pub async fn uses_for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<AuthorityUse>, AuthorityError> {
    let response = db
        .query(format!(
            "SELECT {AUTHORITY_USE_SELECT} FROM {AUTHORITY_USE_TABLE} \
             WHERE project_id = $project ORDER BY id DESC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(many_authority_uses(response)?)
}

/// The uses on `project_id` a **client** may see.
///
/// Not merely a subset of [`uses_for_project`]. Several dispositions
/// record *firm reasoning* — what the firm considered and chose not to
/// rely on — and a client who sees "reviewed, not used" learns the firm's
/// strategic assessment of their own matter. That is a disclosure of work
/// product, a different and worse failure than an ordinary visibility
/// bug.
///
/// The allowlist is derived from
/// [`Disposition::is_firm_reasoning`](rules::citation::Disposition::is_firm_reasoning)
/// rather than written out here, so adding a disposition cannot leave a
/// stale literal behind.
///
/// # Errors
/// Propagates any database error.
pub async fn client_visible_uses(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<AuthorityUse>, AuthorityError> {
    let allowed: Vec<String> = Disposition::client_visible()
        .into_iter()
        .map(|d| d.as_str().to_string())
        .collect();
    let response = db
        .query(format!(
            "SELECT {AUTHORITY_USE_SELECT} FROM {AUTHORITY_USE_TABLE} \
             WHERE project_id = $project AND disposition IN $allowed ORDER BY id DESC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .bind(("allowed", allowed))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(many_authority_uses(response)?)
}

/// Parse a stored `authority_use.disposition`.
///
/// # Errors
/// [`AuthorityError::UnknownDisposition`] when the value is outside the
/// closed vocabulary.
pub fn disposition_of(row: &AuthorityUse) -> Result<Disposition, AuthorityError> {
    Disposition::parse(&row.disposition)
        .ok_or_else(|| AuthorityError::UnknownDisposition(row.disposition.clone()))
}

// ---------------------------------------------------------------------
// citation — the Locator
// ---------------------------------------------------------------------

/// One Locator: an assertion citing an authority at a specific place,
/// pinned to both the source and the firm's own draft.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Citation {
    pub id: Uuid,
    pub authority_use_id: Uuid,
    pub quote: String,
    pub why: String,
    pub source_pin: Option<Json>,
    pub draft_pin: Option<Json>,
    pub inserted_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(SurrealValue)]
struct CitationRow {
    id: surrealdb::types::RecordId,
    authority_use_id: surrealdb::types::RecordId,
    quote: String,
    why: String,
    source_pin: Option<Json>,
    draft_pin: Option<Json>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl CitationRow {
    fn into_citation(self) -> Option<Citation> {
        Some(Citation {
            id: record_uuid(&self.id)?,
            authority_use_id: record_uuid(&self.authority_use_id)?,
            quote: self.quote,
            why: self.why,
            source_pin: self.source_pin,
            draft_pin: self.draft_pin,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const CITATION_SELECT: &str =
    "id, authority_use_id, quote, why, source_pin, draft_pin, inserted_at, updated_at";

fn many_citations(
    mut response: surrealdb::IndexedResults,
) -> Result<Vec<Citation>, surrealdb::Error> {
    let rows: Vec<CitationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(CitationRow::into_citation)
        .collect())
}

/// What a new [`Citation`] needs.
#[derive(Debug, Clone)]
pub struct NewCitation<'a> {
    pub authority_use_id: Uuid,
    /// The exact relied-on text.
    pub quote: &'a str,
    /// Why the source supports the assertion. Required — see the module
    /// docs on [`Citation`].
    pub why: &'a str,
    /// Paragraph, page, or normalised rect within the source.
    pub source_pin: Option<Json>,
    /// Where in the firm's own draft the assertion sits.
    pub draft_pin: Option<Json>,
}

/// Record a Locator against a matter's use of an authority.
///
/// # Errors
/// Propagates any database error.
pub async fn cite(db: &SurrealDb, new: &NewCitation<'_>) -> Result<Citation, AuthorityError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             authority_use_id = $authority_use_id, quote = $quote, why = $why, \
             source_pin = $source_pin, draft_pin = $draft_pin \
             RETURN {CITATION_SELECT}"
        ))
        .bind(("id", record_id(CITATION_TABLE, id)))
        .bind((
            "authority_use_id",
            record_id(AUTHORITY_USE_TABLE, new.authority_use_id),
        ))
        .bind(("quote", new.quote.to_string()))
        .bind(("why", new.why.to_string()))
        .bind(("source_pin", new.source_pin.clone()))
        .bind(("draft_pin", new.draft_pin.clone()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<CitationRow> = response.take(0)?;
    row.and_then(CitationRow::into_citation)
        .ok_or(AuthorityError::WriteReturnedNothing)
}

/// Every Locator recorded against `authority_use_id`, newest first.
///
/// # Errors
/// Propagates any database error.
pub async fn citations_for_use(
    db: &SurrealDb,
    authority_use_id: Uuid,
) -> Result<Vec<Citation>, AuthorityError> {
    let response = db
        .query(format!(
            "SELECT {CITATION_SELECT} FROM {CITATION_TABLE} \
             WHERE authority_use_id = $use ORDER BY id DESC"
        ))
        .bind(("use", record_id(AUTHORITY_USE_TABLE, authority_use_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(many_citations(response)?)
}

/// Delete every `authority_use`, `citation`, and `verification` row that
/// belongs to `project_id`, deepest first.
///
/// The chain is `authority_uses → citations → verifications`, and
/// Surreal cascades none of it, so a matter delete walks it
/// explicitly here. The delete itself runs inside one `BEGIN`/`COMMIT` —
/// see `store::notation_clauses::move_clause` for why a multi-statement
/// Surreal query is not one transaction on its own.
///
/// # Errors
/// Propagates any database error.
pub async fn delete_for_project(db: &SurrealDb, project_id: Uuid) -> Result<(), AuthorityError> {
    let uses = uses_for_project(db, project_id).await?;
    if uses.is_empty() {
        return Ok(());
    }
    let use_ids: Vec<surrealdb::types::RecordId> = uses
        .iter()
        .map(|u| record_id(AUTHORITY_USE_TABLE, u.id))
        .collect();

    let mut citation_response = db
        .query(format!(
            "SELECT VALUE id FROM {CITATION_TABLE} WHERE authority_use_id IN $uses"
        ))
        .bind(("uses", use_ids.clone()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let citation_ids: Vec<surrealdb::types::RecordId> = citation_response.take(0)?;

    db.query(format!(
        "BEGIN; \
         DELETE verification WHERE citation_id IN $citations; \
         DELETE {CITATION_TABLE} WHERE id IN $citations; \
         DELETE {AUTHORITY_USE_TABLE} WHERE id IN $uses; \
         COMMIT;",
    ))
    .bind(("citations", citation_ids))
    .bind(("uses", use_ids))
    .await
    .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        citations_for_use, cite, cite_in_matter, class_of, client_visible_uses, delete_for_project,
        disposition_of, record, uses_for_project, NewAuthority, NewCitation,
    };
    use crate::surreal::test_support::mem;
    use crate::test_support::seed_project_surreal;
    use rules::citation::{AuthorityClass, Disposition};

    fn case(citation: &str, title: &str) -> NewAuthority<'static> {
        NewAuthority {
            class: AuthorityClass::CaseLaw,
            citation: Box::leak(citation.to_string().into_boxed_str()),
            short_cite: None,
            title: Box::leak(title.to_string().into_boxed_str()),
            publisher: None,
            issued_on: None,
            canonical_url: None,
            checked_on: None,
            archived_asset_id: None,
        }
    }

    /// The property #890 names: **one matter's use of an authority is
    /// invisible from another matter.** Two matters cite the same case
    /// from opposite sides; neither can see the other's posture.
    #[tokio::test]
    async fn a_matters_use_of_an_authority_is_invisible_from_another_matter() {
        let surreal = mem().await;
        let ours = seed_project_surreal(&surreal, "matter-a").await;
        let theirs = seed_project_surreal(&surreal, "matter-b").await;

        let shared = record(&surreal, &case("410 U.S. 113 (1973)", "Roe v. Wade"))
            .await
            .expect("record authority");

        cite_in_matter(
            &surreal,
            ours,
            shared.id,
            "ours",
            Disposition::ReliedOn,
            Some("controls the standard of review"),
        )
        .await
        .expect("our use");
        cite_in_matter(
            &surreal,
            theirs,
            shared.id,
            "adverse",
            Disposition::ReviewedNotUsed,
            Some("opponent leans on it; distinguishable on the facts"),
        )
        .await
        .expect("their use");

        let ours_view = uses_for_project(&surreal, ours).await.expect("ours");
        assert_eq!(ours_view.len(), 1, "each matter sees only its own use");
        assert_eq!(ours_view[0].position, "ours");
        assert_eq!(
            disposition_of(&ours_view[0]).expect("disposition"),
            Disposition::ReliedOn
        );

        let theirs_view = uses_for_project(&surreal, theirs).await.expect("theirs");
        assert_eq!(theirs_view.len(), 1);
        assert_eq!(theirs_view[0].position, "adverse");
        assert!(
            !ours_view.iter().any(|u| u.id == theirs_view[0].id),
            "one matter's posture must never appear in another matter's view"
        );
    }

    /// The load-bearing rule, at the store boundary. A client seeing
    /// "reviewed, not used" learns the firm's strategic assessment of
    /// their own matter — a disclosure of work product.
    #[tokio::test]
    async fn the_client_lens_never_returns_a_firm_reasoning_disposition() {
        let surreal = mem().await;
        let project_id = seed_project_surreal(&surreal, "lens").await;

        for (i, d) in Disposition::ALL.iter().enumerate() {
            let a = record(
                &surreal,
                &case(&format!("{i} U.S. 1"), &format!("Case {i}")),
            )
            .await
            .expect("record");
            cite_in_matter(&surreal, project_id, a.id, "ours", *d, None)
                .await
                .expect("use");
        }

        let lawyer = uses_for_project(&surreal, project_id)
            .await
            .expect("lawyer lens");
        assert_eq!(
            lawyer.len(),
            Disposition::ALL.len(),
            "lawyers see every disposition"
        );

        let client = client_visible_uses(&surreal, project_id)
            .await
            .expect("client lens");
        assert_eq!(
            client.len(),
            Disposition::client_visible().len(),
            "the client lens is the derived allowlist, not the whole set"
        );
        for use_row in &client {
            let d = disposition_of(use_row).expect("disposition");
            assert!(
                !d.is_firm_reasoning(),
                "{} is firm reasoning and reached the client lens",
                d.as_str()
            );
        }
        let visible: Vec<&str> = client.iter().map(|u| u.disposition.as_str()).collect();
        assert!(!visible.contains(&"reviewed-not-used"));
        assert!(!visible.contains(&"monitoring-not-relied-on"));
    }

    /// Authority is not case-shaped: a statute and an administrative
    /// proceeding are first-class authorities, not case records bent to
    /// fit.
    #[tokio::test]
    async fn a_statute_and_an_administrative_proceeding_are_first_class_authorities() {
        let surreal = mem().await;

        for (class, cite_str, title) in [
            (
                AuthorityClass::Statute,
                "NRS 86.201",
                "Nevada LLC formation",
            ),
            (
                AuthorityClass::Regulation,
                "12 C.F.R. 1022.42",
                "FCRA accuracy",
            ),
            (
                AuthorityClass::Administrative,
                "In re Example, 2026 WL 1",
                "Agency order",
            ),
            (
                AuthorityClass::Secondary,
                "Restatement (Second) of Contracts 90",
                "Promissory estoppel",
            ),
        ] {
            let mut new = case(cite_str, title);
            new.class = class;
            let row = record(&surreal, &new).await.expect("record");
            assert_eq!(class_of(&row).expect("class"), class);
        }
    }

    /// Find-or-create, not insert. The second matter to cite a case
    /// reuses the first matter's row; a blind insert would rebuild the
    /// private per-matter lists this table replaces.
    #[tokio::test]
    async fn recording_the_same_citation_twice_reuses_the_global_row() {
        let surreal = mem().await;

        let first = record(&surreal, &case("42 U.S. 1", "Example v. Example"))
            .await
            .expect("first");
        let second = record(&surreal, &case("42 U.S. 1", "Example v. Example"))
            .await
            .expect("second");

        assert_eq!(first.id, second.id, "the citation is the identity");
    }

    /// A matter takes one posture on a given authority. Re-citing updates
    /// it rather than adding a contradictory second row.
    #[tokio::test]
    async fn a_matter_holds_one_posture_per_authority() {
        let surreal = mem().await;
        let project_id = seed_project_surreal(&surreal, "posture").await;
        let a = record(&surreal, &case("7 U.S. 7", "Example"))
            .await
            .expect("record");

        cite_in_matter(
            &surreal,
            project_id,
            a.id,
            "ours",
            Disposition::OpenReview,
            None,
        )
        .await
        .expect("first");
        let updated = cite_in_matter(
            &surreal,
            project_id,
            a.id,
            "adverse",
            Disposition::ReviewedNotUsed,
            Some("distinguishable"),
        )
        .await
        .expect("second");

        let all = uses_for_project(&surreal, project_id).await.expect("all");
        assert_eq!(all.len(), 1, "one posture per matter per authority");
        assert_eq!(all[0].id, updated.id);
        assert_eq!(all[0].position, "adverse");
        assert_eq!(all[0].disposition, "reviewed-not-used");
    }

    /// The Locator pins **both** ends. Pinning only the source half is
    /// the common mistake in the surveyed corpus.
    #[tokio::test]
    async fn a_locator_pins_both_the_source_and_the_draft() {
        let surreal = mem().await;
        let project_id = seed_project_surreal(&surreal, "locator").await;
        let a = record(&surreal, &case("9 U.S. 9", "Example"))
            .await
            .expect("record");
        let use_row = cite_in_matter(
            &surreal,
            project_id,
            a.id,
            "ours",
            Disposition::ReliedOn,
            None,
        )
        .await
        .expect("use");

        let source_pin = serde_json::json!({"page": 4, "rect": [0.1, 0.2, 0.5, 0.3]});
        let draft_pin = serde_json::json!({"page": 2, "rect": [0.05, 0.6, 0.9, 0.1]});
        cite(
            &surreal,
            &NewCitation {
                authority_use_id: use_row.id,
                quote: "the standard is de novo",
                why: "states the standard of review this brief argues for",
                source_pin: Some(source_pin.clone()),
                draft_pin: Some(draft_pin.clone()),
            },
        )
        .await
        .expect("cite");

        let found = citations_for_use(&surreal, use_row.id)
            .await
            .expect("citations");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source_pin.as_ref(), Some(&source_pin));
        assert_eq!(
            found[0].draft_pin.as_ref(),
            Some(&draft_pin),
            "the draft end is pinned too"
        );
        assert!(
            !found[0].why.is_empty(),
            "why is what makes this a legal tool rather than a footnote widget"
        );
    }

    /// `position` is a closed vocabulary at the command, since the
    /// Surreal schema carries no `CHECK` of its own.
    #[tokio::test]
    async fn position_is_a_closed_vocabulary() {
        let surreal = mem().await;
        let project_id = seed_project_surreal(&surreal, "check").await;
        let a = record(&surreal, &case("11 U.S. 11", "Example"))
            .await
            .expect("record");

        assert!(
            cite_in_matter(
                &surreal,
                project_id,
                a.id,
                "sideways",
                Disposition::ReliedOn,
                None
            )
            .await
            .is_err(),
            "position is a closed vocabulary"
        );
    }

    /// Deleting a matter takes its citation apparatus with it — Surreal
    /// has no `ON DELETE CASCADE`, so [`delete_for_project`] must reach
    /// every level itself.
    #[tokio::test]
    async fn delete_for_project_removes_every_use_and_citation() {
        let surreal = mem().await;
        let project_id = seed_project_surreal(&surreal, "cascade").await;
        let a = record(&surreal, &case("13 U.S. 13", "Example"))
            .await
            .expect("record");
        let use_row = cite_in_matter(
            &surreal,
            project_id,
            a.id,
            "ours",
            Disposition::ReliedOn,
            None,
        )
        .await
        .expect("use");
        cite(
            &surreal,
            &NewCitation {
                authority_use_id: use_row.id,
                quote: "quote",
                why: "why",
                source_pin: None,
                draft_pin: None,
            },
        )
        .await
        .expect("cite");

        delete_for_project(&surreal, project_id)
            .await
            .expect("delete");

        assert!(uses_for_project(&surreal, project_id)
            .await
            .expect("uses")
            .is_empty());
        assert!(citations_for_use(&surreal, use_row.id)
            .await
            .expect("citations")
            .is_empty());

        // The global authority survives — only the matter's own use and
        // citations are scoped to the deleted project.
        let survivor = record(&surreal, &case("13 U.S. 13", "Example"))
            .await
            .expect("authority still exists");
        assert_eq!(survivor.id, a.id);
    }
}
