//! The questionnaire's question catalog — one prompt presented to a
//! respondent during template traversal — and every query against it.
//!
//! # This table lives in SurrealDB
//!
//! `questions` moved with wave five of #1093 (ENG-121), together with
//! [`crate::answers`], because `answers.question_id` couples them: porting
//! one without the other would leave the walker's hottest read spanning
//! two engines.
//!
//! Rows are written by `cli import` out of template frontmatter and by the
//! canonical seed, and read by every questionnaire surface — the lawyer
//! walker, the client magic link, and the `/app/admin/questions` transparency
//! listing.
//!
//! # Engine facts this module is shaped around
//!
//! **A unique violation carries no typed detail.** It arrives as an
//! internal error with the index name in the message, so [`classify_write`]
//! discriminates on `question_code` — a `DEFINE INDEX` identifier this
//! workspace chose in `store/src/schema/navigator.surql`, not prose.
//!
//! **The key-value layer is optimistic, so a write can lose a race.**
//! [`writing`] re-runs the statement under the crate's one retry policy,
//! [`crate::surreal::retry`]. That matters more here than it looks:
//! `cli import` and the seed both run [`find_or_create`] on every boot, and
//! the cucumber suite shares one engine across concurrent scenarios, so a
//! writer that assumed exclusivity would flake rather than fail.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "question";

/// `questions.audience` — only lawyers see this question.
pub const AUDIENCE_LAWYER: &str = "lawyer";
/// `questions.audience` — only the client sees this question (magic link).
pub const AUDIENCE_CLIENT: &str = "client";
/// `questions.audience` — both sides may answer this question.
pub const AUDIENCE_BOTH: &str = "both";

/// Whether a question with `audience` is shown to the client on the
/// self-serve magic-link surface (`client` or `both`).
///
/// Total rather than fallible on purpose: the schema ASSERT keeps the
/// stored set closed, and a value from outside it — a hand-written row, a
/// future audience this build predates — must not be shown to the client
/// by accident. Unknown means lawyer-only.
#[must_use]
pub fn is_client_facing(audience: &str) -> bool {
    audience == AUDIENCE_CLIENT || audience == AUDIENCE_BOTH
}

/// One question.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`QuestionRow`] is the seam that turns it into (and back out of) what
/// the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Question {
    pub id: Uuid,
    /// Canonical identifier from template frontmatter (`client_name`).
    /// Unique.
    pub code: String,
    /// The prompt as the respondent reads it.
    pub prompt: String,
    /// `string`, `int`, `bool`, `choice`, … — the widget to render.
    pub answer_type: String,
    /// `lawyer` | `client` | `both` — which side of the intake sees this
    /// question. The schema ASSERTs the closed set. Read it through
    /// [`is_client_facing`] rather than comparing by hand.
    pub audience: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from [`Question`]
/// because the SDK owns its own `RecordId` and `Datetime`, and the
/// conversion belongs at this seam rather than in every caller.
#[derive(SurrealValue)]
struct QuestionRow {
    id: surrealdb::types::RecordId,
    code: String,
    prompt: String,
    answer_type: String,
    audience: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl QuestionRow {
    /// `None` when the record id is not a native UUID key — a row written
    /// by something that bypassed [`crate::surreal::record_id`].
    fn into_question(self) -> Option<Question> {
        Some(Question {
            id: record_uuid(&self.id)?,
            code: self.code,
            prompt: self.prompt,
            answer_type: self.answer_type,
            audience: self.audience,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`QuestionRow`] from only one query.
const SELECT: &str = "id, code, prompt, answer_type, audience, inserted_at, updated_at";

/// Errors reading or writing a question.
#[derive(Debug, thiserror::Error)]
pub enum QuestionError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `question_code` — another row already holds
    /// this code.
    #[error("that question code is already in use")]
    CodeTaken,
    /// A write reported success but returned no row, or returned one this
    /// module could not read back — see [`QuestionRow::into_question`].
    #[error("writing a question returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names, or
/// leave it as a database fault. A unique violation carries **no typed
/// detail** — the index name in the message is the only discriminator (see
/// `store::persons::classify_write` for the full account).
fn classify_write(error: surrealdb::Error) -> QuestionError {
    if error.to_string().contains("question_code") {
        QuestionError::CodeTaken
    } else {
        QuestionError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, QuestionError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// The fields a new question row carries.
#[derive(Debug, Clone)]
pub struct NewQuestion {
    pub code: String,
    pub prompt: String,
    pub answer_type: String,
    /// `lawyer` | `client` | `both`; anything else is refused by the schema
    /// ASSERT. [`NewQuestion::new`] defaults it to [`AUDIENCE_BOTH`], which
    /// is what the column's `'both'` default gave every row that
    /// predates an author narrowing it.
    pub audience: String,
}

impl NewQuestion {
    #[must_use]
    pub fn new(
        code: impl Into<String>,
        prompt: impl Into<String>,
        answer_type: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            prompt: prompt.into(),
            answer_type: answer_type.into(),
            audience: AUDIENCE_BOTH.to_string(),
        }
    }

    /// Narrow this question to one side of the intake.
    #[must_use]
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = audience.into();
        self
    }
}

/// Read one question out of a query response, dropping a row this module
/// could not have written.
fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Question>, QuestionError> {
    let row: Option<QuestionRow> = response.take(0)?;
    Ok(row.and_then(QuestionRow::into_question))
}

/// Read every question out of a query response, in the order the engine
/// returned them.
fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Question>, QuestionError> {
    let rows: Vec<QuestionRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(QuestionRow::into_question)
        .collect())
}

/// Resolve a question by id.
///
/// # Errors
///
/// [`QuestionError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Question>, QuestionError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve a question by its canonical `code`. Exact match — codes are
/// template-frontmatter identifiers, never user-typed prose.
///
/// # Errors
///
/// [`QuestionError::Db`] if the lookup fails.
pub async fn find_by_code(db: &SurrealDb, code: &str) -> Result<Option<Question>, QuestionError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE code = $code LIMIT 1"
        ))
        .bind(("code", code.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve many questions by code in one round trip — what a walker does
/// once per template rather than once per step. Codes that match nothing
/// are simply absent from the result, so the caller keys on `code` rather
/// than on position.
///
/// # Errors
///
/// [`QuestionError::Db`] if the lookup fails.
pub async fn find_by_codes(
    db: &SurrealDb,
    codes: &[String],
) -> Result<Vec<Question>, QuestionError> {
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE code IN $codes ORDER BY code ASC"
        ))
        .bind(("codes", codes.to_vec()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Resolve many questions by id in one round trip — what a listing does
/// after reading a page of answers, rather than one lookup per row.
///
/// # Errors
///
/// [`QuestionError::Db`] if the lookup fails.
pub async fn find_by_ids(db: &SurrealDb, ids: &[Uuid]) -> Result<Vec<Question>, QuestionError> {
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

/// Every question, ordered by code — the lawyer transparency listing and
/// the importer's "what is already catalogued" read.
///
/// # Errors
///
/// [`QuestionError::Db`] if the lookup fails.
pub async fn list_all(db: &SurrealDb) -> Result<Vec<Question>, QuestionError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM {TABLE} ORDER BY code ASC"))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Write a new question row. The record id is minted from a fresh v7
/// `Uuid` through [`crate::surreal::record_id`], so the key stays the
/// native UUID spelling `answers.question_id` addresses.
///
/// # Errors
///
/// [`QuestionError::CodeTaken`] when another row already holds this code,
/// and [`QuestionError::Db`] for anything else — including the schema
/// ASSERT refusing an `audience` outside `lawyer`/`client`/`both`.
pub async fn create(db: &SurrealDb, input: &NewQuestion) -> Result<Question, QuestionError> {
    let id = Uuid::now_v7();
    let mut response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             code = $code, \
             prompt = $prompt, \
             answer_type = $answer_type, \
             audience = $audience \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("code", input.code.trim().to_string()))
        .bind(("prompt", input.prompt.clone()))
        .bind(("answer_type", input.answer_type.clone()))
        .bind(("audience", input.audience.clone()))
    })
    .await?;

    let row: Option<QuestionRow> = response.take(0)?;
    row.and_then(QuestionRow::into_question)
        .ok_or(QuestionError::WriteReturnedNothing)
}

/// Find the question holding `input.code`, creating it if absent.
///
/// Race-safe without a lock: a concurrent creator that wins the
/// `question_code` unique index turns this call's insert into
/// [`QuestionError::CodeTaken`], which is re-read as the winner's row.
/// `cli import` and the seed both run this on every boot, and the cucumber
/// suite runs concurrent scenarios against one engine, so idempotence under
/// contention is the contract rather than a nicety.
///
/// The existing row is returned **unchanged**: a question's prompt and
/// answer type are template-authored, and silently rewriting them here
/// would make an import reorder the catalog rather than extend it.
///
/// # Errors
///
/// [`QuestionError::Db`] if the lookup or the write fails.
pub async fn find_or_create(
    db: &SurrealDb,
    input: &NewQuestion,
) -> Result<Question, QuestionError> {
    if let Some(found) = find_by_code(db, &input.code).await? {
        return Ok(found);
    }
    match create(db, input).await {
        Ok(created) => Ok(created),
        Err(QuestionError::CodeTaken) => find_by_code(db, &input.code)
            .await?
            .ok_or(QuestionError::WriteReturnedNothing),
        Err(other) => Err(other),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create, find_by_code, find_by_codes, find_by_id, find_by_ids, find_or_create,
        is_client_facing, list_all, NewQuestion, QuestionError, AUDIENCE_BOTH, AUDIENCE_CLIENT,
        AUDIENCE_LAWYER,
    };
    use crate::surreal::test_support::mem;

    #[test]
    fn audience_filters_the_client_visible_set() {
        assert!(is_client_facing(AUDIENCE_CLIENT));
        assert!(is_client_facing(AUDIENCE_BOTH));
        assert!(!is_client_facing(AUDIENCE_LAWYER));
        // An unknown/garbage audience is not shown to the client.
        assert!(!is_client_facing("nonsense"));
    }

    #[tokio::test]
    async fn a_created_question_reads_back_whole() {
        let db = mem().await;
        let written = create(
            &db,
            &NewQuestion::new("client_name", "Your name?", "string"),
        )
        .await
        .unwrap();

        assert_eq!(written.code, "client_name");
        assert_eq!(written.prompt, "Your name?");
        assert_eq!(written.answer_type, "string");
        assert_eq!(
            written.audience, AUDIENCE_BOTH,
            "a question defaults to both sides of the intake"
        );
        assert_eq!(find_by_id(&db, written.id).await.unwrap(), Some(written));
    }

    #[tokio::test]
    async fn a_question_is_found_by_its_code() {
        let db = mem().await;
        let written = create(&db, &NewQuestion::new("trustee_name", "Who?", "string"))
            .await
            .unwrap();

        assert_eq!(
            find_by_code(&db, "trustee_name").await.unwrap(),
            Some(written)
        );
        assert_eq!(find_by_code(&db, "no_such_code").await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_duplicate_code_is_refused_by_the_index() {
        let db = mem().await;
        create(
            &db,
            &NewQuestion::new("client_name", "Your name?", "string"),
        )
        .await
        .unwrap();

        let again = create(&db, &NewQuestion::new("client_name", "Again?", "string")).await;
        assert!(
            matches!(again, Err(QuestionError::CodeTaken)),
            "a second row under one code must be refused; got {again:?}"
        );
    }

    /// The seed and `cli import` both run on every boot, so a second pass
    /// must return the row rather than a conflict — and must not rewrite
    /// the template-authored prompt.
    #[tokio::test]
    async fn find_or_create_is_idempotent_and_leaves_the_existing_row_alone() {
        let db = mem().await;
        let first = find_or_create(
            &db,
            &NewQuestion::new("client_name", "Your name?", "string"),
        )
        .await
        .unwrap();
        let second = find_or_create(
            &db,
            &NewQuestion::new("client_name", "A DIFFERENT PROMPT", "int"),
        )
        .await
        .unwrap();

        assert_eq!(first, second, "the second pass returns the existing row");
        assert_eq!(second.prompt, "Your name?");
        assert_eq!(list_all(&db).await.unwrap().len(), 1);
    }

    /// The cucumber suite runs concurrent scenarios against one engine, so
    /// a seeder that assumed exclusivity would flake rather than fail.
    #[tokio::test]
    async fn concurrent_find_or_create_on_one_code_settles_on_one_row() {
        let db = mem().await;
        let racers: Vec<_> = (0..6)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move {
                    find_or_create(
                        &db,
                        &NewQuestion::new("client_name", "Your name?", "string"),
                    )
                    .await
                })
            })
            .collect();

        let mut ids = Vec::new();
        for racer in racers {
            ids.push(
                racer
                    .await
                    .expect("task must not panic")
                    .expect("every racer resolves to a row")
                    .id,
            );
        }
        assert!(
            ids.windows(2).all(|w| w[0] == w[1]),
            "the racers disagreed about which row won: {ids:?}"
        );
        assert_eq!(
            list_all(&db).await.unwrap().len(),
            1,
            "one question, not six"
        );
    }

    #[tokio::test]
    async fn an_audience_outside_the_closed_set_is_refused_by_the_schema() {
        let db = mem().await;
        // A typo'd audience would silently hide the question from the
        // client rather than fail, which is why the schema ASSERTs it.
        let refused = create(
            &db,
            &NewQuestion::new("client_name", "Your name?", "string").with_audience("clientt"),
        )
        .await;
        assert!(refused.is_err(), "the engine accepted audience `clientt`");
    }

    #[tokio::test]
    async fn a_client_only_question_keeps_its_audience() {
        let db = mem().await;
        let written = create(
            &db,
            &NewQuestion::new("client_name", "Your name?", "string").with_audience(AUDIENCE_CLIENT),
        )
        .await
        .unwrap();
        assert!(is_client_facing(&written.audience));

        let lawyer_only = create(
            &db,
            &NewQuestion::new("internal_note", "Note", "string").with_audience(AUDIENCE_LAWYER),
        )
        .await
        .unwrap();
        assert!(!is_client_facing(&lawyer_only.audience));
    }

    #[tokio::test]
    async fn batched_lookups_resolve_by_code_and_by_id() {
        let db = mem().await;
        let a = create(&db, &NewQuestion::new("aaa", "A?", "string"))
            .await
            .unwrap();
        let b = create(&db, &NewQuestion::new("bbb", "B?", "string"))
            .await
            .unwrap();
        create(&db, &NewQuestion::new("ccc", "C?", "string"))
            .await
            .unwrap();

        let by_code = find_by_codes(&db, &["bbb".into(), "aaa".into(), "nope".into()])
            .await
            .unwrap();
        assert_eq!(
            by_code,
            vec![a.clone(), b.clone()],
            "an unmatched code is simply absent"
        );

        let by_id = find_by_ids(&db, &[b.id, a.id]).await.unwrap();
        assert_eq!(by_id, vec![a, b]);
    }

    #[tokio::test]
    async fn empty_batches_do_not_reach_the_engine() {
        let db = crate::surreal::test_support::unreachable();
        assert!(find_by_codes(&db, &[]).await.unwrap().is_empty());
        assert!(find_by_ids(&db, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_catalog_lists_in_code_order() {
        let db = mem().await;
        for code in ["ccc", "aaa", "bbb"] {
            create(&db, &NewQuestion::new(code, "?", "string"))
                .await
                .unwrap();
        }
        let codes: Vec<String> = list_all(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|q| q.code)
            .collect();
        assert_eq!(codes, ["aaa", "bbb", "ccc"]);
    }
}
