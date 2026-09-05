//! Aggregate public website visitor analytics.
//!
//! This module only stores bounded counters. It intentionally has no fields for
//! IP addresses, user agents, raw paths, raw query strings, session ids, person
//! ids, or full referrer URLs.
//!
//! # This table lives in SurrealDB
//!
//! `visitor_route_counts` moved with wave two of the flat-table ports
//! (#1093; ENG-20). It is a leaf counter table — nothing references it
//! and it references nothing — so the port could not cascade.
//!
//! `bucket_date` is a `datetime` at midnight UTC: Surreal has no
//! date-only type, so a day is an instant at its start. The daily and
//! monthly rollups format it rather than storing a derived column.

use chrono::{DateTime, Timelike, Utc};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "visitor_route_count";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitorVisit<'a> {
    pub country_code: &'a str,
    pub route_pattern: &'a str,
    pub source: &'a str,
    pub locale: &'a str,
    pub status_class: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeriodTotal {
    pub bucket: String,
    pub visits: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionTotal {
    pub label: String,
    pub visits: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VisitorAnalyticsSummary {
    pub total_visits: i64,
    pub daily: Vec<PeriodTotal>,
    pub monthly: Vec<PeriodTotal>,
    pub countries: Vec<DimensionTotal>,
    pub routes: Vec<DimensionTotal>,
    pub sources: Vec<DimensionTotal>,
}

/// Errors reading or writing visitor counters.
#[derive(Debug, thiserror::Error)]
pub enum VisitorAnalyticsError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The bucket could not be incremented or created within the bounded
    /// number of attempts — every create lost the unique index and every
    /// following update found nothing, which should not be reachable.
    #[error("recording a visit neither incremented nor created a bucket")]
    BucketVanished,
}

/// A row shape for the two-column aggregate reads. The period and
/// dimension rollups differ only in what the grouped column means, so
/// one struct serves both and the query's alias decides.
#[derive(SurrealValue)]
struct TotalRow {
    bucket: String,
    visits: i64,
}

/// Whether a create lost the `visitor_route_count_bucket` unique index —
/// a concurrent visit created the same bucket first. A unique violation
/// carries **no typed detail**, so the index name in the message is the
/// only discriminator, through the shared classifier in
/// [`crate::surreal::retry`].
fn is_bucket_taken(error: &surrealdb::Error) -> bool {
    crate::surreal::retry::unique_violation(error) == Some("visitor_route_count_bucket")
}

/// One increment, then one create if there was nothing to increment.
/// Bounded because each pass past the first is a genuine race loss, not
/// a retry of a bad statement: the loser's next `UPDATE` finds the
/// winner's row.
const RECORD_ATTEMPTS: usize = 4;

/// Increment the aggregate row for one public website visit.
///
/// The shape is increment-then-create, and the unique index is what keeps two
/// concurrent visits from splitting one bucket into two rows: the
/// creator that loses it re-runs the increment against the winner's row.
///
/// SurrealDB does have a closer analogue — `UPSERT <table> SET …` against
/// a *bare table* recovers from a unique-index collision and retries as
/// an update on the conflicting record, as does
/// `INSERT … ON DUPLICATE KEY UPDATE`. (Only `UPSERT table:id` keys on
/// the record id.) It is not used here because the counter has to be
/// incremented rather than assigned, and the create path would have to
/// supply all six bucket columns anyway — so the explicit two-statement
/// shape says what happens without relying on `+=` against an absent
/// field. The read-back-and-retry loop is the same either way.
///
/// # Errors
///
/// [`VisitorAnalyticsError::Db`] if a statement fails, and
/// [`VisitorAnalyticsError::BucketVanished`] if every attempt both
/// failed to increment and lost the create.
pub async fn record_visit(
    db: &SurrealDb,
    visit: &VisitorVisit<'_>,
) -> Result<(), VisitorAnalyticsError> {
    let bucket_date = midnight_utc(Utc::now());

    for _ in 0..RECORD_ATTEMPTS {
        // The bucket usually exists; try the cheap path first.
        let mut response = db
            .query(format!(
                "UPDATE {TABLE} SET visits += 1, updated_at = time::now() \
                 WHERE bucket_date = $bucket_date \
                 AND country_code = $country_code \
                 AND route_pattern = $route_pattern \
                 AND source = $source \
                 AND locale = $locale \
                 AND status_class = $status_class \
                 RETURN id"
            ))
            .bind(("bucket_date", surrealdb::types::Datetime::from(bucket_date)))
            .bind(("country_code", visit.country_code.to_string()))
            .bind(("route_pattern", visit.route_pattern.to_string()))
            .bind(("source", visit.source.to_string()))
            .bind(("locale", visit.locale.to_string()))
            .bind(("status_class", visit.status_class.to_string()))
            .await
            .and_then(surrealdb::IndexedResults::check)?;
        let touched: Vec<surrealdb::types::Value> = response.take(0)?;
        if !touched.is_empty() {
            return Ok(());
        }

        // Nothing to increment — this is the bucket's first visit.
        let created = db
            .query(
                "CREATE $id SET \
                 bucket_date = $bucket_date, \
                 country_code = $country_code, \
                 route_pattern = $route_pattern, \
                 source = $source, \
                 locale = $locale, \
                 status_class = $status_class, \
                 visits = 1",
            )
            .bind(("id", record_id(TABLE, Uuid::now_v7())))
            .bind(("bucket_date", surrealdb::types::Datetime::from(bucket_date)))
            .bind(("country_code", visit.country_code.to_string()))
            .bind(("route_pattern", visit.route_pattern.to_string()))
            .bind(("source", visit.source.to_string()))
            .bind(("locale", visit.locale.to_string()))
            .bind(("status_class", visit.status_class.to_string()))
            .await
            .and_then(surrealdb::IndexedResults::check);

        match created {
            Ok(_) => return Ok(()),
            // A concurrent visit created this bucket between the update
            // and the create. Loop: the next update finds its row.
            Err(error) if is_bucket_taken(&error) => {}
            Err(error) => return Err(VisitorAnalyticsError::Db(error)),
        }
    }
    Err(VisitorAnalyticsError::BucketVanished)
}

/// Midnight UTC on the day of `at` — day resolution, expressed in the
/// only instant type Surreal has.
fn midnight_utc(at: DateTime<Utc>) -> DateTime<Utc> {
    at.with_hour(0)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(at)
}

/// Summarize aggregate visit counts for the lawyer analytics page.
///
/// # Errors
///
/// [`VisitorAnalyticsError::Db`] if a lookup fails.
pub async fn summary(db: &SurrealDb) -> Result<VisitorAnalyticsSummary, VisitorAnalyticsError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE math::sum(visits) FROM {TABLE} GROUP ALL"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    // `SELECT VALUE math::sum(...)` yields a bare number, unlike
    // `count()` under `GROUP ALL`, which yields a `{ count: n }` object.
    let totals: Vec<i64> = response.take(0)?;
    let total_visits = totals.first().copied().unwrap_or(0);

    Ok(VisitorAnalyticsSummary {
        total_visits,
        daily: period_totals(db, "%Y-%m-%d", 30).await?,
        monthly: period_totals(db, "%Y-%m", 12).await?,
        countries: dimension_totals(db, "country_code").await?,
        routes: dimension_totals(db, "route_pattern").await?,
        sources: dimension_totals(db, "source").await?,
    })
}

/// Visits grouped by `bucket_date` rendered through `format`, newest
/// first. `%Y-%m-%d` gives the daily rollup and `%Y-%m` the monthly one:
/// the grouping *is* the formatted string, so the month rollup needs no
/// `date_trunc` equivalent. Both sort correctly as strings because the
/// formats are zero-padded and big-endian.
async fn period_totals(
    db: &SurrealDb,
    format: &str,
    limit: usize,
) -> Result<Vec<PeriodTotal>, VisitorAnalyticsError> {
    let mut response = db
        .query(format!(
            "SELECT time::format(bucket_date, $format) AS bucket, \
             math::sum(visits) AS visits \
             FROM {TABLE} GROUP BY bucket ORDER BY bucket DESC LIMIT $limit"
        ))
        .bind(("format", format.to_string()))
        .bind(("limit", limit))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<TotalRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|r| PeriodTotal {
            bucket: r.bucket,
            visits: r.visits,
        })
        .collect())
}

/// The top 20 values of one dimension by visits, ties broken by label so
/// the listing is deterministic.
async fn dimension_totals(
    db: &SurrealDb,
    column: &str,
) -> Result<Vec<DimensionTotal>, VisitorAnalyticsError> {
    // `column` is one of three literals chosen by `summary`, never
    // caller input — a bound parameter cannot name a field.
    let mut response = db
        .query(format!(
            "SELECT {column} AS bucket, math::sum(visits) AS visits \
             FROM {TABLE} GROUP BY bucket ORDER BY visits DESC, bucket ASC LIMIT 20"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<TotalRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|r| DimensionTotal {
            label: r.bucket,
            visits: r.visits,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{record_visit, summary, DimensionTotal, VisitorAnalyticsSummary, VisitorVisit};
    use crate::surreal::test_support::mem;

    fn visit(source: &'static str) -> VisitorVisit<'static> {
        VisitorVisit {
            country_code: "US",
            route_pattern: "/blog/{slug}",
            source,
            locale: "en",
            status_class: "2xx",
        }
    }

    #[tokio::test]
    async fn record_visit_upserts_same_aggregate_bucket() {
        let db = mem().await;

        record_visit(&db, &visit("linkedin")).await.unwrap();
        record_visit(&db, &visit("linkedin")).await.unwrap();
        record_visit(&db, &visit("google")).await.unwrap();

        let summary = summary(&db).await.unwrap();
        assert_eq!(summary.total_visits, 3);
        assert_eq!(
            summary.sources,
            vec![
                DimensionTotal {
                    label: "linkedin".into(),
                    visits: 2
                },
                DimensionTotal {
                    label: "google".into(),
                    visits: 1
                },
            ],
            "a different source gets its own bucket; the same one increments"
        );
    }

    #[tokio::test]
    async fn concurrent_visits_to_one_bucket_do_not_split_it() {
        let db = mem().await;
        // The unique index is what makes this safe: whichever creator
        // loses it re-runs the increment against the winner's row.
        let racers: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move { record_visit(&db, &visit("linkedin")).await })
            })
            .collect();
        for racer in racers {
            racer
                .await
                .expect("the task itself must not panic")
                .unwrap();
        }

        let summary = summary(&db).await.unwrap();
        assert_eq!(summary.total_visits, 8, "every visit is counted once");
        assert_eq!(
            summary.sources.len(),
            1,
            "and they all land in a single bucket"
        );
    }

    #[tokio::test]
    async fn summary_returns_daily_monthly_and_dimension_totals() {
        let db = mem().await;

        record_visit(&db, &visit("linkedin")).await.unwrap();
        record_visit(&db, &visit("linkedin")).await.unwrap();

        let summary = summary(&db).await.unwrap();
        assert_eq!(summary.total_visits, 2);
        assert_eq!(summary.daily[0].visits, 2);
        assert_eq!(summary.monthly[0].visits, 2);
        assert_eq!(summary.countries[0].label, "US");
        assert_eq!(summary.routes[0].label, "/blog/{slug}");
        assert_eq!(summary.sources[0].label, "linkedin");
    }

    #[tokio::test]
    async fn the_daily_bucket_is_a_date_and_the_monthly_one_its_prefix() {
        let db = mem().await;
        record_visit(&db, &visit("linkedin")).await.unwrap();

        let summary = summary(&db).await.unwrap();
        let day = &summary.daily[0].bucket;
        let month = &summary.monthly[0].bucket;
        assert_eq!(day.len(), 10, "YYYY-MM-DD, got {day}");
        assert_eq!(month.len(), 7, "YYYY-MM, got {month}");
        assert!(
            day.starts_with(month.as_str()),
            "the month rollup is the day's prefix: {day} vs {month}"
        );
    }

    #[tokio::test]
    async fn an_empty_table_summarizes_to_zero_rather_than_failing() {
        let db = mem().await;
        let summary = summary(&db).await.unwrap();
        assert_eq!(summary, VisitorAnalyticsSummary::default());
    }
}
