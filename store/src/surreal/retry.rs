//! The one retry policy every contended write in `store` runs under.
//!
//! # Why a policy, and why only one
//!
//! SurrealDB's key-value layer is optimistic. Two writers that touch the
//! same keys race, the loser's transaction is rolled back, and the
//! engine says so with a typed
//! [`QueryError::TransactionConflict`] whose own message ends "This
//! transaction can be retried". Nothing about the losing statement was
//! wrong. Re-running the loser is the application's job, so every write
//! in this crate needs the same wrapper.
//!
//! It lives here once because it was previously copied into more than
//! twenty query modules, and the copies had already drifted: most
//! retried only [`QueryError::TransactionConflict`] while four also
//! retried [`QueryError::NotExecuted`], so the same engine condition was
//! a retry in `store::projects` and a caller-visible fault in
//! `store::persons`. A policy that differs per table is not a policy.
//!
//! # Why a deadline rather than a count
//!
//! The copies each allowed five attempts with a 2ms base backoff — about
//! 15ms of total patience, fixed, regardless of how many writers were
//! competing. That cannot work: the number of times a writer must
//! re-run before it wins grows with the size of the herd it is in, and a
//! fixed count knows nothing about that. Measured against an embedded
//! engine, writers racing for one record exhausted a five-attempt budget
//! 0.2% of the time at eight-way contention, 3.9% at sixteen, and 21% at
//! thirty-two — so the five-attempt loop did not fail because the
//! contention was pathological, it failed because the budget was
//! expressed in the wrong unit.
//!
//! [`WRITE_BUDGET`] is that budget in the unit that actually matters.
//! The concern the counted loop was protecting — that a hot record must
//! fail fast rather than hang — is a statement about wall-clock time,
//! and bounding wall-clock time directly bounds it while letting the
//! herd drain however many attempts that takes.

use std::future::IntoFuture;
use std::time::Duration;

use surrealdb::types::{ErrorDetails, QueryError};
use tokio::time::Instant;

/// How long a write keeps re-running while the engine reports a
/// conflict, before the conflict becomes the caller's error.
///
/// Two seconds is chosen from the measured drain time of the worst
/// contention this workspace can produce, with an order of magnitude
/// over it. Thirty-two writers racing for a single record against an
/// embedded engine — far past anything a request path generates, since
/// real writes spread over many records — all settle inside 112–211ms
/// across repeated runs of `store::persons`'s
/// `a_write_that_loses_an_optimistic_race_is_retried_not_surfaced`. At
/// roughly ten times the slowest of those, the herd drains and the
/// deadline is reached only by a record that is genuinely wedged.
///
/// It is also small enough to keep the original promise. A caller that
/// waits out the whole budget still fails in two seconds, well inside
/// any request timeout above it, so a pathological record surfaces as a
/// slow error rather than a hang.
pub const WRITE_BUDGET: Duration = Duration::from_secs(2);

/// The first backoff window. Every loser fails at the same instant, so
/// re-running immediately just re-stages the same collision.
const FIRST_BACKOFF: Duration = Duration::from_millis(2);

/// The largest backoff window. The window doubles each attempt until it
/// reaches this, which keeps a long wait inside [`WRITE_BUDGET`] from
/// being spent in one sleep: without a ceiling the tenth window alone
/// would exceed the whole budget, so a writer would get far fewer
/// attempts than the budget nominally buys it.
const BACKOFF_CEILING: Duration = Duration::from_millis(64);

/// Is this the engine asking the caller to try again?
///
/// Both variants mean "your transaction did not run", never "your
/// statement was wrong":
///
/// - [`QueryError::TransactionConflict`] is the optimistic layer
///   reporting a lost write race.
/// - [`QueryError::NotExecuted`] is a statement the engine skipped
///   because the transaction around it had already failed.
///
/// Retrying the second cannot hide a real fault.
/// [`surrealdb::IndexedResults::check`] surfaces the *first* failing
/// statement, so a statement that failed on its own merits is the error
/// the caller sees; `NotExecuted` reaches here only when the transaction
/// itself is what went wrong. Even then the worst case is that a
/// deterministic failure is re-run until the budget expires and is then
/// reported unchanged.
///
/// Matched on the typed detail, never on message text.
#[must_use]
pub fn is_retryable(error: &surrealdb::Error) -> bool {
    matches!(
        error.details(),
        ErrorDetails::Query(Some(
            QueryError::TransactionConflict | QueryError::NotExecuted
        ))
    )
}

/// The unique indexes whose names SurrealDB includes in an untyped
/// uniqueness violation. Keep this list beside the one message match so an
/// index rename is a test failure rather than a silent change from a caller
/// conflict to a database fault.
const UNIQUE_INDEX_NAMES: &[&str] = &[
    "project_code",
    "xero_invoice_project",
    "person_project_role_pair",
    "person_firm_role_pair",
    "project_module_pair",
    "statutory_deadline_replay",
    "git_access_token_hash",
    "testimonial_replay",
    "person_external_identity_account",
    "person_external_identity_person_system",
    "credential_person_jurisdiction",
    "person_email_lower",
    "person_oidc_subject",
    "jurisdiction_code",
    "entity_type_name",
    "entity_firm_anchor",
    "firm_entity",
    "firm_brand_pair",
    "firm_brand_key",
    "entity_role_tie",
    "git_repository_remote_hash",
    "glossary_term_slug",
    "email_token_hash",
    "mailroom_name",
    "question_code",
    "template_current_key",
    "review_document_notation_kind",
    "signature_provider_request",
    "notarization_provider_request",
    "authority_citation",
    "playbook_entity_name",
    "email_conversation_token",
    "communication_channel_source_ref",
    "visitor_route_count_bucket",
];

/// Return the schema index named by SurrealDB's untyped uniqueness error.
///
/// SurrealDB currently reports a duplicate index entry as
/// [`ErrorDetails::Internal`] and puts the index name in its display text.
/// The match is intentionally isolated here: callers receive a stable,
/// structured discriminator and never need to inspect or render the engine
/// message. When the engine gains a typed uniqueness detail, this is the one
/// place that should change.
#[must_use]
pub fn unique_violation(error: &surrealdb::Error) -> Option<&'static str> {
    if !matches!(error.details(), ErrorDetails::Internal) {
        return None;
    }
    let message = error.to_string();
    UNIQUE_INDEX_NAMES
        .iter()
        .copied()
        .find(|index| message.contains(index))
}

/// Run `attempt`, re-running it while the engine reports a conflict, for
/// up to [`WRITE_BUDGET`]. Any other error returns immediately.
///
/// The closure is called afresh each time because awaiting a
/// `surrealdb::Query` consumes it. There is nothing to undo between
/// attempts: a conflicting transaction is rolled back before its error
/// is returned.
///
/// # Errors
///
/// The engine's own error — a conflict that outlived the budget, or the
/// first non-retryable failure.
pub async fn retrying<F, Fut, T>(mut attempt: F) -> Result<T, surrealdb::Error>
where
    F: FnMut() -> Fut,
    Fut: IntoFuture<Output = Result<T, surrealdb::Error>>,
{
    let deadline = Instant::now() + WRITE_BUDGET;
    let mut backoff = FIRST_BACKOFF;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(error) if is_retryable(&error) && Instant::now() < deadline => {
                // Uniform inside the window rather than at its edge:
                // backing off by the same interval keeps the herd in
                // lockstep and collides it again one tick later.
                tokio::time::sleep(rand::random_range(Duration::ZERO..=backoff)).await;
                backoff = (backoff * 2).min(BACKOFF_CEILING);
            }
            Err(error) => return Err(error),
        }
    }
}

/// [`retrying`] for a query: runs the statement and checks the response,
/// so a per-statement failure is retried or surfaced like any other.
///
/// Query modules wrap this with their own error mapping — the retry
/// policy is shared, but what a failure *means* stays with the table
/// that knows its indexes.
///
/// # Errors
///
/// The engine's own error, as [`retrying`] returns it.
pub async fn writing<F, Q>(mut attempt: F) -> Result<surrealdb::IndexedResults, surrealdb::Error>
where
    F: FnMut() -> Q,
    Q: IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retrying(|| {
        let query = attempt();
        async move { query.await.and_then(surrealdb::IndexedResults::check) }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::{is_retryable, retrying, unique_violation, UNIQUE_INDEX_NAMES, WRITE_BUDGET};
    use std::cell::Cell;
    use std::time::Duration;
    use surrealdb::types::QueryError;

    fn conflict() -> surrealdb::Error {
        surrealdb::Error::query(
            "Transaction conflict: Write conflict, retry the transaction".to_string(),
            QueryError::TransactionConflict,
        )
    }

    fn not_executed() -> surrealdb::Error {
        surrealdb::Error::query(
            "The query was not executed due to a failed transaction".to_string(),
            QueryError::NotExecuted,
        )
    }

    fn cancelled() -> surrealdb::Error {
        surrealdb::Error::query("Query was cancelled".to_string(), QueryError::Cancelled)
    }

    #[tokio::test]
    async fn a_retryable_conflict_is_retried_until_it_succeeds() {
        let attempts = Cell::new(0);
        let result: Result<&str, _> = retrying(|| {
            attempts.set(attempts.get() + 1);
            let failing = attempts.get() < 3;
            async move {
                if failing {
                    Err(conflict())
                } else {
                    Ok("committed")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "committed");
        assert_eq!(attempts.get(), 3);
    }

    /// A statement the engine skipped because its transaction failed is
    /// the same "did not run" answer as a conflict, and four query
    /// modules already treated it that way. One policy, so all of them
    /// do.
    #[tokio::test]
    async fn a_statement_the_transaction_never_ran_is_retried_too() {
        let attempts = Cell::new(0);
        let result: Result<&str, _> = retrying(|| {
            attempts.set(attempts.get() + 1);
            let failing = attempts.get() < 2;
            async move {
                if failing {
                    Err(not_executed())
                } else {
                    Ok("committed")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "committed");
        assert_eq!(attempts.get(), 2);
    }

    /// The budget is patience, not a headcount. A writer deep in a large
    /// herd loses many more than the five races the previous per-module
    /// loops allowed, and must still be carried through to its commit.
    #[tokio::test]
    async fn far_more_conflicts_than_the_old_five_attempt_loop_allowed_are_absorbed() {
        let attempts = Cell::new(0);
        let result: Result<&str, _> = retrying(|| {
            attempts.set(attempts.get() + 1);
            let failing = attempts.get() < 25;
            async move {
                if failing {
                    Err(conflict())
                } else {
                    Ok("committed")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "committed");
        assert_eq!(attempts.get(), 25);
    }

    /// Bounded, and bounded in time: a conflict that never clears
    /// surfaces rather than hanging the caller, and it surfaces on
    /// roughly the schedule [`WRITE_BUDGET`] advertises.
    #[tokio::test]
    async fn a_conflict_that_never_clears_gives_up_when_the_budget_expires() {
        let started = tokio::time::Instant::now();
        let result: Result<(), _> = retrying(|| async { Err(conflict()) }).await;
        let waited = started.elapsed();

        assert!(is_retryable(&result.unwrap_err()));
        assert!(
            waited >= WRITE_BUDGET,
            "gave up after {waited:?}, before the budget was spent",
        );
        assert!(
            waited < WRITE_BUDGET + Duration::from_secs(2),
            "took {waited:?}, far past the budget it promises",
        );
    }

    /// Only what the engine marks retryable is retried — anything else
    /// returns on the first attempt, so a real failure is not hidden
    /// behind two seconds of pointless round trips.
    #[tokio::test]
    async fn a_non_retryable_error_is_returned_immediately() {
        let attempts = Cell::new(0);
        let result: Result<(), _> = retrying(|| {
            attempts.set(attempts.get() + 1);
            async { Err(cancelled()) }
        })
        .await;

        assert!(!is_retryable(&result.unwrap_err()));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn a_unique_violation_returns_its_schema_index_name() {
        let error = surrealdb::Error::internal(
            "Database index `person_email_lower` already contains this value".to_string(),
        );

        assert_eq!(unique_violation(&error), Some("person_email_lower"));
    }

    #[test]
    fn only_internal_errors_are_classified_as_unique_violations() {
        let error = surrealdb::Error::query(
            "Database index `person_email_lower` already contains this value".to_string(),
            QueryError::Cancelled,
        );

        assert_eq!(unique_violation(&error), None);
    }

    #[test]
    fn every_classifier_index_name_is_defined_in_the_schema() {
        let schema = include_str!("../schema/navigator.surql");

        for index in UNIQUE_INDEX_NAMES {
            assert!(
                schema.contains(&format!("DEFINE INDEX OVERWRITE {index}")),
                "unique violation classifier names an index absent from the schema: {index}"
            );
        }
    }
}
