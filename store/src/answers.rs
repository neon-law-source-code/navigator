//! One respondent's answer to one question, and every query against the
//! table.
//!
//! # This table lives in SurrealDB
//!
//! `answers` moved with wave five of #1093 (ENG-121), together with
//! [`crate::questions`]: `answers.question_id` couples them, so porting one
//! without the other would leave the walker's hottest read spanning two
//! engines.
//!
//! # Append-only
//!
//! Re-asks (verification) and corrections are **new rows, never updates**,
//! and latest-per-`(notation_id, state_name)` wins on read. There is no
//! unique constraint, and this module exports no update and no delete —
//! [`record`] is the only writer. That is the guarantee's whole
//! enforcement: SurrealDB has no trigger equivalent in our usage, so the
//! invariant lives
//! in the command seam's shape, the same way `store::sent_emails` carries
//! it. `no_update_or_delete_is_exported` pins it against a future edit.
//!
//! # Who answered, and who typed it
//!
//! `person_id` is the **respondent** — whose answer it is.
//! [`NewAnswer::source`] and [`NewAnswer::authored_by_person_id`] record
//! *who supplied it*: lawyer filling it in on the client's behalf, or the
//! client themselves through the magic link. A two-sided intake can
//! interleave both authorships on one notation, and the data lake can tell
//! them apart.
//!
//! `notation_id` scopes the answer to the Notation that collected it, and
//! `state_name` carries the full `<type>__<role>` questionnaire state
//! (`entity__company`, `entity__subsidiary`) so two records of the same type
//! stay distinct — the bare `question_id` alone would collapse them.
//!
//! # The `value` column is the ported JSONB
//!
//! It carries three shapes: primitives as `{"value": …}` ([`primitive`]),
//! singular record answers mirroring the row they create or select, and
//! aggregates as an array of that shape. On a SCHEMAFULL table only
//! `TYPE any` accepts all three — see the field's comment in
//! `store/src/schema/navigator.surql` for the three near misses that do not,
//! and `value_round_trips_all_three_shapes` for the proof. Read a stored
//! value through [`primitive_str`] or [`display_value`] rather than
//! destructuring it at a call site.
//!
//! `TYPE any` also means the column's non-null requirement cannot be a schema
//! ASSERT (one does not fire on a NONE-valued `any` field). [`record`]
//! always sets `value` and nothing else writes this table, so the guarantee
//! is this seam's, like append-only above.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value as Json};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "answer";
/// The table [`Answer::question_id`] addresses.
const QUESTION_TABLE: &str = "question";
/// The table [`Answer::person_id`] addresses.
const PERSON_TABLE: &str = "person";
/// The table [`Answer::notation_id`] addresses.
const NOTATION_TABLE: &str = "notation";

/// `answers.source` — lawyer entered the answer on the client's behalf.
pub const SOURCE_LAWYER: &str = "lawyer";
/// `answers.source` — the client self-entered the answer (magic link).
pub const SOURCE_CLIENT: &str = "client";
/// `answers.source` — machine-extracted from a recorded sitting's
/// transcript (AIDA/Gemini), neither lawyer- nor client-typed. The distinct
/// value is the human-in-the-loop boundary: a machine-proposed answer is
/// visibly different from a confirmed one, so an attorney can see and
/// correct it before any draft is released to the client.
pub const SOURCE_EXTRACTED: &str = "extracted";

/// Wrap a scalar answer into the primitive JSON envelope `{"value": …}`.
#[must_use]
pub fn primitive(value: &str) -> Json {
    json!({ "value": value })
}

/// Read the inner string of a primitive envelope (`{"value": "…"}`),
/// `None` for a record/aggregate shape or a non-string `value`.
#[must_use]
pub fn primitive_str(value: &Json) -> Option<&str> {
    value.get("value").and_then(Json::as_str)
}

/// The string a template placeholder should render for this answer. A
/// primitive envelope unwraps to its inner scalar; a record/aggregate shape
/// (resolved by the evaluator) falls back to its compact JSON.
#[must_use]
pub fn display_value(value: &Json) -> String {
    match value.get("value") {
        Some(Json::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => value.to_string(),
    }
}

/// One answer.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`AnswerRow`] is the seam that turns it into (and back out of) what the
/// SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Answer {
    pub id: Uuid,
    pub question_id: Uuid,
    /// The **respondent** — whose answer this is.
    pub person_id: Uuid,
    /// The Notation that collected this answer. `None` for the
    /// person-scoped canonical-seed fixtures (`Answer.yaml`), which have no
    /// Notation behind them; every Notation-bound write site sets it.
    pub notation_id: Option<Uuid>,
    /// Full `<type>__<role>` questionnaire state this answer was given for
    /// (`entity__company`). `None` for bare/seed answers carrying no role.
    /// Render keys on this so two records of one type stay distinct.
    pub state_name: Option<String>,
    /// The ported JSONB payload — see the module header. Read it through
    /// [`primitive_str`] or [`display_value`].
    pub value: Json,
    /// `lawyer` | `client` | `extracted` — who supplied this answer. The
    /// schema ASSERTs the closed set.
    pub source: String,
    /// Who actually typed the answer. `None` for system answers.
    pub authored_by_person_id: Option<Uuid>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from [`Answer`]
/// because the SDK owns its own `RecordId` and `Datetime`, and the
/// conversion belongs at this seam rather than in every caller.
#[derive(SurrealValue)]
struct AnswerRow {
    id: surrealdb::types::RecordId,
    question_id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    notation_id: Option<surrealdb::types::RecordId>,
    state_name: Option<String>,
    value: Json,
    source: String,
    authored_by_person_id: Option<surrealdb::types::RecordId>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl AnswerRow {
    /// `None` when a record id is not a native UUID key — a row written by
    /// something that bypassed [`crate::surreal::record_id`].
    fn into_answer(self) -> Option<Answer> {
        Some(Answer {
            id: record_uuid(&self.id)?,
            question_id: record_uuid(&self.question_id)?,
            person_id: record_uuid(&self.person_id)?,
            notation_id: self.notation_id.as_ref().and_then(record_uuid),
            state_name: self.state_name,
            value: self.value,
            source: self.source,
            // A link this module could not read back is dropped to `None`
            // rather than dropping the whole answer: authorship is an
            // analytics dimension, and losing it must not lose the answer.
            authored_by_person_id: self.authored_by_person_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`AnswerRow`] from only one query.
const SELECT: &str = "id, question_id, person_id, notation_id, state_name, value, source, \
                      authored_by_person_id, inserted_at, updated_at";

/// Errors reading or writing an answer.
#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back — see [`AnswerRow::into_answer`].
    #[error("writing an answer returned no usable row")]
    WriteReturnedNothing,
}

/// Everything [`record`] needs to append one answer.
#[derive(Debug, Clone)]
pub struct NewAnswer {
    pub question_id: Uuid,
    /// The respondent — whose answer this is.
    pub person_id: Uuid,
    pub notation_id: Option<Uuid>,
    pub state_name: Option<String>,
    pub value: Json,
    /// `lawyer` | `client` | `extracted`; anything else is refused by the
    /// schema ASSERT. [`NewAnswer::new`] defaults it to [`SOURCE_LAWYER`], so
    /// a writer that names no author records one written by a lawyer.
    pub source: String,
    pub authored_by_person_id: Option<Uuid>,
}

impl NewAnswer {
    /// A lawyer-sourced answer with no notation scope — the bare shape the
    /// canonical seed's person-scoped fixtures use. Narrow it with the
    /// builders below.
    #[must_use]
    pub fn new(question_id: Uuid, person_id: Uuid, value: Json) -> Self {
        Self {
            question_id,
            person_id,
            notation_id: None,
            state_name: None,
            value,
            source: SOURCE_LAWYER.to_string(),
            authored_by_person_id: None,
        }
    }

    /// Scope this answer to the Notation that collected it, under the full
    /// `<type>__<role>` questionnaire state.
    #[must_use]
    pub fn in_notation(mut self, notation_id: Uuid, state_name: impl Into<String>) -> Self {
        self.notation_id = Some(notation_id);
        self.state_name = Some(state_name.into());
        self
    }

    /// Record who supplied the answer, as distinct from whose answer it is.
    #[must_use]
    pub fn authored_by(mut self, source: impl Into<String>, person_id: Option<Uuid>) -> Self {
        self.source = source.into();
        self.authored_by_person_id = person_id;
        self
    }
}

/// Read one answer out of a query response, dropping a row this module
/// could not have written.
fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Answer>, AnswerError> {
    let row: Option<AnswerRow> = response.take(0)?;
    Ok(row.and_then(AnswerRow::into_answer))
}

/// Read every answer out of a query response, in the order the engine
/// returned them.
fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Answer>, AnswerError> {
    let rows: Vec<AnswerRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AnswerRow::into_answer)
        .collect())
}

/// Append one answer.
///
/// The only writer. The record id is minted from a fresh v7 `Uuid`, so
/// `ORDER BY id` is a chronological sort — which is what makes
/// "latest wins" well-defined on a table with no unique key and no
/// meaningful `updated_at` (nothing ever updates a row).
///
/// Nothing here retries a transaction conflict. An append-only table with
/// no unique index has no row for two writers to contend over: each
/// [`record`] call creates its own key, so there is no race to lose.
///
/// # Errors
///
/// [`AnswerError::Db`] if the insert fails — including the schema ASSERT
/// refusing a `source` outside `lawyer`/`client`/`extracted`.
pub async fn record(db: &SurrealDb, new: &NewAnswer) -> Result<Answer, AnswerError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             question_id = $question_id, \
             person_id = $person_id, \
             notation_id = $notation_id, \
             state_name = $state_name, \
             value = $value, \
             source = $source, \
             authored_by_person_id = $authored_by_person_id \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("question_id", record_id(QUESTION_TABLE, new.question_id)))
        .bind(("person_id", record_id(PERSON_TABLE, new.person_id)))
        .bind((
            "notation_id",
            new.notation_id.map(|id| record_id(NOTATION_TABLE, id)),
        ))
        .bind(("state_name", new.state_name.clone()))
        .bind(("value", new.value.clone()))
        .bind(("source", new.source.clone()))
        .bind((
            "authored_by_person_id",
            new.authored_by_person_id
                .map(|p| record_id(PERSON_TABLE, p)),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let row: Option<AnswerRow> = response.take(0)?;
    row.and_then(AnswerRow::into_answer)
        .ok_or(AnswerError::WriteReturnedNothing)
}

/// The most recent answer one respondent gave for one question in one
/// questionnaire state of one Notation — the walker's per-step read.
///
/// "Most recent" is `ORDER BY id DESC LIMIT 1`: the ids are v7, so this is
/// the newest append, which is what latest-wins means on an append-only
/// table.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn latest_for_state(
    db: &SurrealDb,
    question_id: Uuid,
    state_name: &str,
    person_id: Uuid,
    notation_id: Uuid,
) -> Result<Option<Answer>, AnswerError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE question_id = $question_id \
               AND state_name = $state_name \
               AND person_id = $person_id \
               AND notation_id = $notation_id \
             ORDER BY id DESC LIMIT 1"
        ))
        .bind(("question_id", record_id(QUESTION_TABLE, question_id)))
        .bind(("state_name", state_name.to_string()))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("notation_id", record_id(NOTATION_TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Every answer collected by one Notation, oldest first — the transcript
/// read. Ascending because a later row supersedes an earlier one, so a
/// consumer folding this into a map ends holding the latest per key.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn for_notation(db: &SurrealDb, notation_id: Uuid) -> Result<Vec<Answer>, AnswerError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE notation_id = $notation_id ORDER BY id ASC"
        ))
        .bind(("notation_id", record_id(NOTATION_TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every answer one respondent gave in one Notation, oldest first — the
/// same fold as [`for_notation`], narrowed to one side of a two-sided
/// intake.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn for_person_in_notation(
    db: &SurrealDb,
    person_id: Uuid,
    notation_id: Uuid,
) -> Result<Vec<Answer>, AnswerError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE person_id = $person_id AND notation_id = $notation_id \
             ORDER BY id ASC"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("notation_id", record_id(NOTATION_TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Every answer one respondent gave to one question in one questionnaire
/// state, oldest first — the re-ask history behind a single step.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn for_question_and_person(
    db: &SurrealDb,
    question_id: Uuid,
    person_id: Uuid,
    state_name: Option<&str>,
) -> Result<Vec<Answer>, AnswerError> {
    let narrowed = if state_name.is_some() {
        " AND state_name = $state_name"
    } else {
        ""
    };
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE question_id = $question_id AND person_id = $person_id{narrowed} \
             ORDER BY id ASC"
        ))
        .bind(("question_id", record_id(QUESTION_TABLE, question_id)))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("state_name", state_name.map(ToString::to_string)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// The most recent answer one respondent gave, across every question and
/// notation. The read behind "what did they last say".
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn latest_for_person(
    db: &SurrealDb,
    person_id: Uuid,
) -> Result<Option<Answer>, AnswerError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE person_id = $person_id ORDER BY id DESC LIMIT 1"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Whether this Notation already carries an answer for this questionnaire
/// state — the seed's idempotence check, which must not re-append on a
/// second boot.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn exists_for_state(
    db: &SurrealDb,
    notation_id: Uuid,
    state_name: &str,
) -> Result<bool, AnswerError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE notation_id = $notation_id AND state_name = $state_name LIMIT 1"
        ))
        .bind(("notation_id", record_id(NOTATION_TABLE, notation_id)))
        .bind(("state_name", state_name.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(one(response)?.is_some())
}

/// Whether this respondent already gave exactly this value to this
/// question — the person-scoped seed fixture's idempotence check, which has
/// no Notation to key on.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn exists_with_value(
    db: &SurrealDb,
    question_id: Uuid,
    person_id: Uuid,
    value: &Json,
) -> Result<bool, AnswerError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE question_id = $question_id \
               AND person_id = $person_id \
               AND value = $value LIMIT 1"
        ))
        .bind(("question_id", record_id(QUESTION_TABLE, question_id)))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("value", value.clone()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(one(response)?.is_some())
}

/// Every answer, oldest first — the `/app/lawyer/answers` transparency listing.
///
/// # Errors
///
/// [`AnswerError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Answer>, AnswerError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY id ASC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

#[cfg(test)]
mod tests {
    use super::{
        display_value, exists_for_state, exists_with_value, for_notation, for_person_in_notation,
        for_question_and_person, latest_for_person, latest_for_state, list_all, primitive,
        primitive_str, record, NewAnswer, SOURCE_CLIENT, SOURCE_EXTRACTED, SOURCE_LAWYER,
    };
    use crate::questions::{self, NewQuestion};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;
    use serde_json::{json, Value as Json};
    use uuid::Uuid;

    /// A question and a respondent to hang answers off. `person_id` is a
    /// bare id: a `record<person>` link is a type constraint, not a foreign
    /// key, and nothing here needs the person resolved.
    async fn a_question(db: &SurrealDb, code: &str) -> Uuid {
        questions::create(db, &NewQuestion::new(code, "?", "string"))
            .await
            .unwrap()
            .id
    }

    #[test]
    fn the_primitive_envelope_wraps_and_unwraps() {
        let wrapped = primitive("Libra Jones");
        assert_eq!(wrapped, json!({ "value": "Libra Jones" }));
        assert_eq!(primitive_str(&wrapped), Some("Libra Jones"));
        assert_eq!(display_value(&wrapped), "Libra Jones");
    }

    #[test]
    fn display_value_falls_back_for_record_and_aggregate_shapes() {
        // A non-string inner value renders as its compact JSON.
        assert_eq!(display_value(&json!({ "value": 42 })), "42");
        // A record shape unwraps to its display string.
        let record_shape = json!({ "value": "Acme LLC", "name": "Acme LLC" });
        assert_eq!(display_value(&record_shape), "Acme LLC");
        // An aggregate has no `value` key at all, so the whole array renders.
        let aggregate = json!([{ "value": "A" }, { "value": "B" }]);
        assert_eq!(
            display_value(&aggregate),
            r#"[{"value":"A"},{"value":"B"}]"#
        );
        assert_eq!(primitive_str(&aggregate), None);
    }

    /// The headline risk of the port. `answers.value` is the ported JSONB
    /// column and carries three distinct shapes; on a SCHEMAFULL table most
    /// of the plausible field types accept some and reject others, and none
    /// of them fail at DEFINE time. This proves all three survive a real
    /// write and read, byte for byte — not just the primitive case.
    #[tokio::test]
    async fn value_round_trips_all_three_shapes() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();

        let shapes: [(&str, Json); 3] = [
            ("primitive", primitive("Libra Jones")),
            (
                "record",
                json!({ "value": "Acme LLC", "name": "Acme LLC", "id": Uuid::now_v7() }),
            ),
            (
                "aggregate",
                json!([
                    { "value": "Libra Jones", "name": "Libra Jones" },
                    { "value": "Virgo Stone", "name": "Virgo Stone" },
                ]),
            ),
        ];

        for (shape, value) in shapes {
            let written = record(&db, &NewAnswer::new(question_id, person_id, value.clone()))
                .await
                .unwrap_or_else(|e| panic!("the {shape} shape was refused: {e}"));
            assert_eq!(written.value, value, "the {shape} shape drifted on write");

            let read_back = latest_for_person(&db, person_id)
                .await
                .unwrap()
                .expect("the answer just written");
            assert_eq!(
                read_back.value, value,
                "the {shape} shape drifted on read-back"
            );
        }
    }

    /// Deep nesting is the case `TYPE object` and every FLEXIBLE union
    /// variant reject — an undefined nested key on a SCHEMAFULL table.
    #[tokio::test]
    async fn value_carries_arbitrary_nesting() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();
        let deep = json!({
            "value": "Acme",
            "nested": { "deeper": { "deepest": [1, 2, { "leaf": true }] } },
        });

        let written = record(&db, &NewAnswer::new(question_id, person_id, deep.clone()))
            .await
            .unwrap();
        assert_eq!(written.value, deep);
    }

    #[tokio::test]
    async fn a_recorded_answer_reads_back_whole() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();
        let notation_id = Uuid::now_v7();
        let typist = Uuid::now_v7();

        let written = record(
            &db,
            &NewAnswer::new(question_id, person_id, primitive("Libra Jones"))
                .in_notation(notation_id, "person__client")
                .authored_by(SOURCE_CLIENT, Some(typist)),
        )
        .await
        .unwrap();

        assert_eq!(written.question_id, question_id);
        assert_eq!(written.person_id, person_id);
        assert_eq!(written.notation_id, Some(notation_id));
        assert_eq!(written.state_name.as_deref(), Some("person__client"));
        assert_eq!(written.source, SOURCE_CLIENT);
        assert_eq!(written.authored_by_person_id, Some(typist));
        assert_eq!(list_all(&db).await.unwrap(), vec![written]);
    }

    /// The seed's person-scoped fixtures have no Notation behind them.
    #[tokio::test]
    async fn an_answer_may_carry_no_notation_and_no_typist() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();

        let written = record(
            &db,
            &NewAnswer::new(question_id, person_id, primitive("Libra Jones")),
        )
        .await
        .unwrap();
        assert_eq!(written.notation_id, None);
        assert_eq!(written.state_name, None);
        assert_eq!(written.authored_by_person_id, None);
        assert_eq!(
            written.source, SOURCE_LAWYER,
            "an answer defaults to lawyer-supplied"
        );
    }

    /// Append-only in practice: a correction is a new row, and the walker
    /// reads the newest one.
    #[tokio::test]
    async fn a_correction_appends_and_the_latest_wins() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();
        let notation_id = Uuid::now_v7();

        for value in ["Libra Jones", "Libra Prime", "Libra Stone"] {
            record(
                &db,
                &NewAnswer::new(question_id, person_id, primitive(value))
                    .in_notation(notation_id, "person__client"),
            )
            .await
            .unwrap();
        }

        let latest = latest_for_state(&db, question_id, "person__client", person_id, notation_id)
            .await
            .unwrap()
            .expect("three answers were appended");
        assert_eq!(display_value(&latest.value), "Libra Stone");
        assert_eq!(
            for_notation(&db, notation_id).await.unwrap().len(),
            3,
            "every correction is kept — nothing is overwritten"
        );
    }

    /// The state is what keeps two records of one type distinct; without
    /// it, `entity__company` and `entity__subsidiary` would collapse.
    #[tokio::test]
    async fn two_states_of_one_question_stay_distinct() {
        let db = mem().await;
        let question_id = a_question(&db, "entity").await;
        let person_id = Uuid::now_v7();
        let notation_id = Uuid::now_v7();

        for (state, value) in [("entity__company", "Acme"), ("entity__subsidiary", "Beta")] {
            record(
                &db,
                &NewAnswer::new(question_id, person_id, primitive(value))
                    .in_notation(notation_id, state),
            )
            .await
            .unwrap();
        }

        for (state, expected) in [("entity__company", "Acme"), ("entity__subsidiary", "Beta")] {
            let found = latest_for_state(&db, question_id, state, person_id, notation_id)
                .await
                .unwrap()
                .expect("each state holds its own answer");
            assert_eq!(display_value(&found.value), expected);
        }
    }

    #[tokio::test]
    async fn a_notation_transcript_reads_oldest_first() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();
        let notation_id = Uuid::now_v7();
        let other_notation = Uuid::now_v7();

        for value in ["first", "second", "third"] {
            record(
                &db,
                &NewAnswer::new(question_id, person_id, primitive(value))
                    .in_notation(notation_id, "person__client"),
            )
            .await
            .unwrap();
        }
        record(
            &db,
            &NewAnswer::new(question_id, person_id, primitive("elsewhere"))
                .in_notation(other_notation, "person__client"),
        )
        .await
        .unwrap();

        let rendered: Vec<String> = for_notation(&db, notation_id)
            .await
            .unwrap()
            .iter()
            .map(|a| display_value(&a.value))
            .collect();
        assert_eq!(rendered, ["first", "second", "third"]);
    }

    /// A two-sided intake interleaves both authorships on one notation, and
    /// the read has to be able to separate them again.
    #[tokio::test]
    async fn a_two_sided_intake_separates_its_respondents() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let notation_id = Uuid::now_v7();
        let client = Uuid::now_v7();
        let lawyer = Uuid::now_v7();

        record(
            &db,
            &NewAnswer::new(question_id, client, primitive("from the client"))
                .in_notation(notation_id, "person__client")
                .authored_by(SOURCE_CLIENT, Some(client)),
        )
        .await
        .unwrap();
        record(
            &db,
            &NewAnswer::new(question_id, lawyer, primitive("from lawyer"))
                .in_notation(notation_id, "person__lawyer")
                .authored_by(SOURCE_LAWYER, Some(lawyer)),
        )
        .await
        .unwrap();

        assert_eq!(for_notation(&db, notation_id).await.unwrap().len(), 2);
        let just_client = for_person_in_notation(&db, client, notation_id)
            .await
            .unwrap();
        assert_eq!(just_client.len(), 1);
        assert_eq!(display_value(&just_client[0].value), "from the client");
        assert_eq!(just_client[0].source, SOURCE_CLIENT);
    }

    #[tokio::test]
    async fn an_extracted_answer_is_visibly_machine_proposed() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();

        let written = record(
            &db,
            &NewAnswer::new(question_id, person_id, primitive("heard on the call"))
                .authored_by(SOURCE_EXTRACTED, None),
        )
        .await
        .unwrap();
        assert_eq!(written.source, SOURCE_EXTRACTED);
        assert_eq!(
            written.authored_by_person_id, None,
            "no human typed a machine-extracted answer"
        );
    }

    #[tokio::test]
    async fn a_source_outside_the_closed_set_is_refused_by_the_schema() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let refused = record(
            &db,
            &NewAnswer::new(question_id, Uuid::now_v7(), primitive("x")).authored_by("robot", None),
        )
        .await;
        assert!(refused.is_err(), "the engine accepted source `robot`");
    }

    #[tokio::test]
    async fn the_re_ask_history_of_one_step_reads_in_order() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();
        let notation_id = Uuid::now_v7();

        for value in ["first", "corrected"] {
            record(
                &db,
                &NewAnswer::new(question_id, person_id, primitive(value))
                    .in_notation(notation_id, "person__client"),
            )
            .await
            .unwrap();
        }
        record(
            &db,
            &NewAnswer::new(question_id, person_id, primitive("other state"))
                .in_notation(notation_id, "person__other"),
        )
        .await
        .unwrap();

        let scoped = for_question_and_person(&db, question_id, person_id, Some("person__client"))
            .await
            .unwrap();
        let rendered: Vec<String> = scoped.iter().map(|a| display_value(&a.value)).collect();
        assert_eq!(rendered, ["first", "corrected"]);

        let unscoped = for_question_and_person(&db, question_id, person_id, None)
            .await
            .unwrap();
        assert_eq!(unscoped.len(), 3, "no state filter reads every state");
    }

    /// The seed runs on every boot, so both idempotence checks have to say
    /// "already there" on the second pass.
    #[tokio::test]
    async fn the_seed_idempotence_checks_see_an_existing_answer() {
        let db = mem().await;
        let question_id = a_question(&db, "client_name").await;
        let person_id = Uuid::now_v7();
        let notation_id = Uuid::now_v7();
        let value = primitive("Libra Jones");

        assert!(!exists_for_state(&db, notation_id, "person__client")
            .await
            .unwrap());
        assert!(!exists_with_value(&db, question_id, person_id, &value)
            .await
            .unwrap());

        record(
            &db,
            &NewAnswer::new(question_id, person_id, value.clone())
                .in_notation(notation_id, "person__client"),
        )
        .await
        .unwrap();

        assert!(exists_for_state(&db, notation_id, "person__client")
            .await
            .unwrap());
        assert!(exists_with_value(&db, question_id, person_id, &value)
            .await
            .unwrap());
        assert!(
            !exists_with_value(&db, question_id, person_id, &primitive("something else"))
                .await
                .unwrap(),
            "the value check compares the whole envelope, not just the question"
        );
    }

    /// Append-only is carried by this module's *shape*, not by the engine —
    /// SurrealDB has no trigger to raise on an UPDATE. So the guarantee is
    /// that no update and no delete is reachable through the seam, and this
    /// is what fails if one is added.
    #[test]
    fn no_update_or_delete_is_exported() {
        // Only the half above `mod tests` is the seam. Scanning the whole
        // file would match the needles in this test's own literals.
        let source = include_str!("answers.rs");
        let seam = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(above, _)| above);

        for verb in ["update", "delete", "remove", "set_"] {
            let forbidden = format!("pub async fn {verb}");
            assert!(
                !seam.contains(&forbidden),
                "`{forbidden}` would break the append-only guarantee this table \
                 carries at the command seam; see the module header"
            );
        }
    }
}
