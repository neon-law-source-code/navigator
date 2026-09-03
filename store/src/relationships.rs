//! The `relationship` relation — the supplemental typed graph edge the
//! pre-matter conflict check traverses alongside `entity_role`.
//!
//! Adversity, related-party ties, and the edges an LLM later parses out
//! of a Relationship Log's free-form detail all land here. Each carries
//! its own confidence and its own provenance, because a finding must
//! let lawyers judge an edge rather than trust it.
//!
//! # Each end is a link, not a kind string beside a UUID
//!
//! Each end is a native `record<person|entity>` link and the engine
//! enforces the endpoint kinds, so a `from_type: "persn"` typo cannot be
//! written at all and no reader has to re-check one.
//!
//! As with [`crate::entity_roles`], the edge carries no surrogate `id`:
//! nothing addresses a single edge, and `source_id` points *out* of this
//! table rather than into it.
//!
//! # `confidence_pct` is the engine's, the floors are Rust's
//!
//! The 0–100 range is a schema ASSERT, so an out-of-range edge is
//! refused at write time. `REVIEW_FLOOR_PCT` and `BLOCK_FLOOR_PCT` stay
//! in `store::conflicts`: they decide which paths are worth following,
//! and that is a judgment about conflicts rather than about graphs.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

const TABLE: &str = "relationship";
const DISCLOSURE_TABLE: &str = "disclosure";
const RELATIONSHIP_LOG_TABLE: &str = "relationship_log";

/// Relationship kind: one node is legally adverse to the other. The
/// strongest conflict signal — a confident `adverse_to` edge between
/// the proposed matter and an existing client blocks the open.
pub const KIND_ADVERSE_TO: &str = "adverse_to";
/// Relationship kind: the two nodes are related parties (family,
/// commonly-controlled entities, insiders) — a softer signal that
/// warrants lawyer review rather than a hard block.
pub const KIND_RELATED_PARTY: &str = "related_party";

/// Provenance: a human asserted this edge directly.
pub const SOURCE_MANUAL: &str = "manual";
/// Provenance: derived from a `disclosures` row.
pub const SOURCE_DISCLOSURE: &str = "disclosure";
/// Provenance: parsed from a Relationship Log entry.
pub const SOURCE_RELATIONSHIP_LOG: &str = "relationship_log";
/// Provenance: extracted from unstructured text by an LLM. These land
/// at lower `confidence_pct` and are always shown as such in findings.
pub const SOURCE_LLM: &str = "llm";

/// Which table an endpoint names. The engine enforces this through the
/// relation's `FROM person|entity TO person|entity`, so an unknown kind
/// is a write-time error rather than a row to skip on read.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
pub enum Endpoint {
    Person,
    Entity,
}

impl Endpoint {
    /// The Surreal table this endpoint kind lives in.
    #[must_use]
    pub const fn table(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Entity => "entity",
        }
    }

    /// Read an endpoint kind back from a table name, or `None` for a
    /// table the graph does not model.
    #[must_use]
    pub fn from_table(table: &str) -> Option<Self> {
        match table {
            "person" => Some(Self::Person),
            "entity" => Some(Self::Entity),
            _ => None,
        }
    }
}

/// One supplemental typed edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relationship {
    pub from: Endpoint,
    pub from_id: Uuid,
    pub to: Endpoint,
    pub to_id: Uuid,
    pub kind: String,
    /// Confidence this edge is real, 0–100.
    pub confidence_pct: i32,
    pub source_kind: String,
    /// The originating row — a Relationship Log for an LLM-parsed edge,
    /// a Disclosure for a derived one. The schema stores it as a typed
    /// union link while the application-facing shape remains a UUID.
    pub source_id: Option<Uuid>,
    pub detail: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What a write stores.
#[derive(Debug, Clone)]
pub struct NewRelationship {
    pub from: Endpoint,
    pub from_id: Uuid,
    pub to: Endpoint,
    pub to_id: Uuid,
    pub kind: String,
    pub confidence_pct: i32,
    pub source_kind: String,
    pub source_id: Option<Uuid>,
    pub detail: Option<String>,
}

#[derive(SurrealValue)]
struct RelationshipRow {
    r#in: surrealdb::types::RecordId,
    out: surrealdb::types::RecordId,
    kind: String,
    confidence_pct: i32,
    source_kind: String,
    source_id: Option<surrealdb::types::RecordId>,
    detail: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl RelationshipRow {
    fn into_relationship(self) -> Option<Relationship> {
        Some(Relationship {
            from: Endpoint::from_table(self.r#in.table.as_str())?,
            from_id: record_uuid(&self.r#in)?,
            to: Endpoint::from_table(self.out.table.as_str())?,
            to_id: record_uuid(&self.out)?,
            kind: self.kind,
            confidence_pct: self.confidence_pct,
            source_kind: self.source_kind,
            source_id: self.source_id.as_ref().and_then(record_uuid),
            detail: self.detail,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const SELECT: &str =
    "in, out, kind, confidence_pct, source_kind, source_id, detail, inserted_at, updated_at";

#[derive(Debug, thiserror::Error)]
pub enum RelationshipError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or one this module
    /// could not read back.
    #[error("writing a relationship returned no usable row")]
    WriteReturnedNothing,
}

/// Record one typed edge.
///
/// # Errors
///
/// [`RelationshipError::Db`] if the write fails — including a
/// `confidence_pct` outside 0–100, which the schema ASSERT refuses, and
/// an endpoint kind the relation does not admit.
pub async fn record(
    db: &SurrealDb,
    input: &NewRelationship,
) -> Result<Relationship, RelationshipError> {
    let mut response = retry::writing(|| {
        db.query(format!(
            "RELATE $from->{TABLE}->$to \
             SET kind = $kind, confidence_pct = $confidence_pct, \
                 source_kind = $source_kind, source_id = $source_id, detail = $detail, \
                 inserted_at = time::now(), updated_at = time::now() \
             RETURN {SELECT}"
        ))
        .bind(("from", record_id(input.from.table(), input.from_id)))
        .bind(("to", record_id(input.to.table(), input.to_id)))
        .bind(("kind", input.kind.clone()))
        .bind(("confidence_pct", input.confidence_pct))
        .bind(("source_kind", input.source_kind.clone()))
        .bind((
            "source_id",
            input.source_id.map(|id| {
                let table = if input.source_kind == SOURCE_DISCLOSURE {
                    DISCLOSURE_TABLE
                } else {
                    RELATIONSHIP_LOG_TABLE
                };
                record_id(table, id)
            }),
        ))
        .bind(("detail", input.detail.clone()))
    })
    .await?;

    let rows: Vec<RelationshipRow> = response.take(0)?;
    rows.into_iter()
        .next()
        .and_then(RelationshipRow::into_relationship)
        .ok_or(RelationshipError::WriteReturnedNothing)
}

/// Every typed edge, oldest first.
///
/// # Errors
///
/// [`RelationshipError::Db`] if the lookup fails.
pub async fn all(db: &SurrealDb) -> Result<Vec<Relationship>, RelationshipError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY inserted_at ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<RelationshipRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(RelationshipRow::into_relationship)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        all, record, Endpoint, NewRelationship, KIND_ADVERSE_TO, SOURCE_LLM, SOURCE_MANUAL,
    };
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use uuid::Uuid;

    async fn person(db: &SurrealDb, name: &str) -> Uuid {
        crate::persons::create(
            db,
            &crate::persons::NewPerson::new(name, format!("{}@example.com", Uuid::now_v7())),
        )
        .await
        .unwrap()
        .id
    }

    async fn entity(db: &SurrealDb, name: &str) -> Uuid {
        crate::entities::create(
            db,
            &crate::entities::NewEntity {
                name: name.into(),
                entity_type_id: Uuid::now_v7(),
                jurisdiction_id: Uuid::now_v7(),
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    fn edge(from: Endpoint, from_id: Uuid, to: Endpoint, to_id: Uuid) -> NewRelationship {
        NewRelationship {
            from,
            from_id,
            to,
            to_id,
            kind: KIND_ADVERSE_TO.into(),
            confidence_pct: 100,
            source_kind: SOURCE_MANUAL.into(),
            source_id: None,
            detail: None,
        }
    }

    #[tokio::test]
    async fn a_recorded_edge_reads_back_with_both_endpoints_typed() {
        let db = mem().await;
        let alice = person(&db, "Alice").await;
        let acme = entity(&db, "Acme LLC").await;

        let written = record(&db, &edge(Endpoint::Person, alice, Endpoint::Entity, acme))
            .await
            .unwrap();

        assert_eq!(written.from, Endpoint::Person);
        assert_eq!(written.from_id, alice);
        assert_eq!(written.to, Endpoint::Entity);
        assert_eq!(written.to_id, acme);
        assert_eq!(written.kind, KIND_ADVERSE_TO);
        assert_eq!(all(&db).await.unwrap(), vec![written]);
    }

    #[tokio::test]
    async fn provenance_and_detail_travel_with_the_edge() {
        let db = mem().await;
        let alice = person(&db, "Parsed Alice").await;
        let bob = person(&db, "Parsed Bob").await;
        let source = Uuid::now_v7();

        let written = record(
            &db,
            &NewRelationship {
                confidence_pct: 40,
                source_kind: SOURCE_LLM.into(),
                source_id: Some(source),
                detail: Some("parsed from a log entry".into()),
                ..edge(Endpoint::Person, alice, Endpoint::Person, bob)
            },
        )
        .await
        .unwrap();

        assert_eq!(written.confidence_pct, 40);
        assert_eq!(written.source_kind, SOURCE_LLM);
        assert_eq!(written.source_id, Some(source));
        assert_eq!(written.detail.as_deref(), Some("parsed from a log entry"));
    }

    /// A schema ASSERT bounds the confidence, so a bad one is refused by
    /// the engine rather than by whichever caller remembers to look.
    #[tokio::test]
    async fn an_out_of_range_confidence_is_refused() {
        let db = mem().await;
        let alice = person(&db, "Over Confident").await;
        let acme = entity(&db, "Over LLC").await;

        let rejected = record(
            &db,
            &NewRelationship {
                confidence_pct: 101,
                ..edge(Endpoint::Person, alice, Endpoint::Entity, acme)
            },
        )
        .await;
        assert!(rejected.is_err(), "the engine accepted confidence_pct 101");
    }

    /// The ASSERT closes the provenance set, so a typo fails the write
    /// instead of silently landing an edge no finding can explain.
    #[tokio::test]
    async fn an_unknown_source_kind_is_refused() {
        let db = mem().await;
        let alice = person(&db, "Bad Source").await;
        let acme = entity(&db, "Bad Source LLC").await;

        let rejected = record(
            &db,
            &NewRelationship {
                source_kind: "guesswork".into(),
                ..edge(Endpoint::Person, alice, Endpoint::Entity, acme)
            },
        )
        .await;
        assert!(
            rejected.is_err(),
            "the engine accepted an unknown source_kind"
        );
    }
}
