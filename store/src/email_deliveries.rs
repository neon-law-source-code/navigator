//! Durable Slack delivery state for inbound email summaries.

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

const DELIVERY_TABLE: &str = "email_delivery";
const ATTEMPT_TABLE: &str = "email_delivery_attempt";

pub const NOT_ATTEMPTED: &str = "not_attempted";
pub const SENDING: &str = "sending";
pub const UNKNOWN: &str = "unknown";
pub const CONFIRMED: &str = "confirmed";
pub const FAILED: &str = "failed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailDelivery {
    pub id: Uuid,
    pub receipt_id: Uuid,
    pub channel_id: String,
    pub state: String,
    pub current_correlation_id: Option<String>,
    pub slack_timestamp: Option<String>,
    pub last_error: Option<String>,
    pub reconciled_by: Option<String>,
    pub reconciliation: Option<String>,
    pub reconciled_at: Option<DateTime<Utc>>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailDeliveryAttempt {
    pub id: Uuid,
    pub delivery_id: Uuid,
    pub correlation_id: String,
    pub state: String,
    pub slack_timestamp: Option<String>,
    pub error: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryAdmission {
    pub delivery: EmailDelivery,
    pub attempt_id: Option<Uuid>,
}

struct DeliveryUpdate<'a> {
    state: &'a str,
    channel_id: Option<&'a str>,
    timestamp: Option<&'a str>,
    error: Option<&'a str>,
    actor: Option<&'a str>,
    reconciliation: Option<&'a str>,
}

#[derive(Debug, thiserror::Error)]
pub enum EmailDeliveryError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("delivery record was not found")]
    NotFound,
    #[error("writing an email delivery returned no usable row")]
    WriteReturnedNothing,
}

#[derive(SurrealValue)]
struct DeliveryRow {
    id: surrealdb::types::RecordId,
    receipt_id: surrealdb::types::RecordId,
    channel_id: String,
    state: String,
    current_correlation_id: Option<String>,
    slack_timestamp: Option<String>,
    last_error: Option<String>,
    reconciled_by: Option<String>,
    reconciliation: Option<String>,
    reconciled_at: Option<surrealdb::types::Datetime>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

#[derive(SurrealValue)]
struct AttemptRow {
    id: surrealdb::types::RecordId,
    delivery_id: surrealdb::types::RecordId,
    correlation_id: String,
    state: String,
    slack_timestamp: Option<String>,
    error: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

const DELIVERY_SELECT: &str = "id, receipt_id, channel_id, state, current_correlation_id, \
    slack_timestamp, last_error, reconciled_by, reconciliation, reconciled_at, inserted_at, updated_at";
const ATTEMPT_SELECT: &str =
    "id, delivery_id, correlation_id, state, slack_timestamp, error, inserted_at, updated_at";

impl DeliveryRow {
    fn into_delivery(self) -> Option<EmailDelivery> {
        Some(EmailDelivery {
            id: record_uuid(&self.id)?,
            receipt_id: record_uuid(&self.receipt_id)?,
            channel_id: self.channel_id,
            state: self.state,
            current_correlation_id: self.current_correlation_id,
            slack_timestamp: self.slack_timestamp,
            last_error: self.last_error,
            reconciled_by: self.reconciled_by,
            reconciliation: self.reconciliation,
            reconciled_at: self.reconciled_at.map(Into::into),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

impl AttemptRow {
    fn into_attempt(self) -> Option<EmailDeliveryAttempt> {
        Some(EmailDeliveryAttempt {
            id: record_uuid(&self.id)?,
            delivery_id: record_uuid(&self.delivery_id)?,
            correlation_id: self.correlation_id,
            state: self.state,
            slack_timestamp: self.slack_timestamp,
            error: self.error,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

pub async fn ensure(
    db: &SurrealDb,
    receipt_id: Uuid,
    channel_id: &str,
) -> Result<EmailDelivery, EmailDeliveryError> {
    if let Some(delivery) = find(db, receipt_id).await? {
        return Ok(delivery);
    }
    let id = Uuid::now_v7();
    let created = db
        .query(format!(
            "CREATE $id SET receipt_id = $receipt_id, channel_id = $channel_id, \
             state = $state RETURN {DELIVERY_SELECT}"
        ))
        .bind(("id", record_id(DELIVERY_TABLE, id)))
        .bind((
            "receipt_id",
            record_id(crate::email_receipts::TABLE, receipt_id),
        ))
        .bind(("channel_id", channel_id.to_string()))
        .bind(("state", NOT_ATTEMPTED.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check);
    let mut response = match created {
        Ok(response) => response,
        Err(error)
            if crate::surreal::retry::unique_violation(&error)
                == Some("email_delivery_receipt") =>
        {
            return find(db, receipt_id)
                .await?
                .ok_or(EmailDeliveryError::Db(error));
        }
        Err(error) => return Err(EmailDeliveryError::Db(error)),
    };
    let row: Option<DeliveryRow> = response.take(0)?;
    row.and_then(DeliveryRow::into_delivery)
        .ok_or(EmailDeliveryError::WriteReturnedNothing)
}

pub async fn find(
    db: &SurrealDb,
    receipt_id: Uuid,
) -> Result<Option<EmailDelivery>, EmailDeliveryError> {
    let mut response = db
        .query(format!(
            "SELECT {DELIVERY_SELECT} FROM {DELIVERY_TABLE} \
             WHERE receipt_id = $receipt_id LIMIT 1"
        ))
        .bind((
            "receipt_id",
            record_id(crate::email_receipts::TABLE, receipt_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DeliveryRow> = response.take(0)?;
    Ok(row.and_then(DeliveryRow::into_delivery))
}

/// Atomically reserve a send. A confirmed or unknown delivery is never
/// retried implicitly; the caller must reconcile an unknown state first.
pub async fn admit_attempt(
    db: &SurrealDb,
    receipt_id: Uuid,
    correlation_id: &str,
) -> Result<DeliveryAdmission, EmailDeliveryError> {
    let delivery = find(db, receipt_id)
        .await?
        .ok_or(EmailDeliveryError::NotFound)?;
    if delivery.state == CONFIRMED || delivery.state == UNKNOWN || delivery.state == SENDING {
        return Ok(DeliveryAdmission {
            delivery,
            attempt_id: None,
        });
    }
    let attempt_id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "UPDATE $id SET state = $state, current_correlation_id = $correlation_id, \
             last_error = NONE, updated_at = time::now() \
             WHERE state IN ['{NOT_ATTEMPTED}', '{FAILED}'] RETURN AFTER"
        ))
        .bind(("id", record_id(DELIVERY_TABLE, delivery.id)))
        .bind(("state", SENDING.to_string()))
        .bind(("correlation_id", correlation_id.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let changed: Option<DeliveryRow> = response.take(0)?;
    let Some(changed) = changed.and_then(DeliveryRow::into_delivery) else {
        return Ok(DeliveryAdmission {
            delivery: find(db, receipt_id)
                .await?
                .ok_or(EmailDeliveryError::NotFound)?,
            attempt_id: None,
        });
    };
    db.query(
        "CREATE $id SET delivery_id = $delivery_id, correlation_id = $correlation_id, \
         state = $state",
    )
    .bind(("id", record_id(ATTEMPT_TABLE, attempt_id)))
    .bind(("delivery_id", record_id(DELIVERY_TABLE, changed.id)))
    .bind(("correlation_id", correlation_id.to_string()))
    .bind(("state", SENDING.to_string()))
    .await
    .and_then(surrealdb::IndexedResults::check)?;
    Ok(DeliveryAdmission {
        delivery: changed,
        attempt_id: Some(attempt_id),
    })
}

pub async fn mark_confirmed(
    db: &SurrealDb,
    receipt_id: Uuid,
    channel_id: &str,
    timestamp: &str,
) -> Result<EmailDelivery, EmailDeliveryError> {
    update_delivery(
        db,
        receipt_id,
        DeliveryUpdate {
            state: CONFIRMED,
            channel_id: Some(channel_id),
            timestamp: Some(timestamp),
            error: None,
            actor: None,
            reconciliation: None,
        },
    )
    .await
}

pub async fn mark_unknown(
    db: &SurrealDb,
    receipt_id: Uuid,
    error: &str,
) -> Result<EmailDelivery, EmailDeliveryError> {
    update_delivery(
        db,
        receipt_id,
        DeliveryUpdate {
            state: UNKNOWN,
            channel_id: None,
            timestamp: None,
            error: Some(error),
            actor: None,
            reconciliation: None,
        },
    )
    .await
}

pub async fn mark_failed(
    db: &SurrealDb,
    receipt_id: Uuid,
    error: &str,
) -> Result<EmailDelivery, EmailDeliveryError> {
    update_delivery(
        db,
        receipt_id,
        DeliveryUpdate {
            state: FAILED,
            channel_id: None,
            timestamp: None,
            error: Some(error),
            actor: None,
            reconciliation: None,
        },
    )
    .await
}

pub async fn reconcile_confirmed(
    db: &SurrealDb,
    receipt_id: Uuid,
    actor: &str,
    channel_id: &str,
    timestamp: &str,
) -> Result<EmailDelivery, EmailDeliveryError> {
    update_delivery(
        db,
        receipt_id,
        DeliveryUpdate {
            state: CONFIRMED,
            channel_id: Some(channel_id),
            timestamp: Some(timestamp),
            error: None,
            actor: Some(actor),
            reconciliation: Some("operator_confirmed"),
        },
    )
    .await
}

pub async fn authorize_resend(
    db: &SurrealDb,
    receipt_id: Uuid,
    actor: &str,
) -> Result<EmailDelivery, EmailDeliveryError> {
    update_delivery(
        db,
        receipt_id,
        DeliveryUpdate {
            state: NOT_ATTEMPTED,
            channel_id: None,
            timestamp: None,
            error: None,
            actor: Some(actor),
            reconciliation: Some("operator_authorized_resend"),
        },
    )
    .await
}

async fn update_delivery(
    db: &SurrealDb,
    receipt_id: Uuid,
    update: DeliveryUpdate<'_>,
) -> Result<EmailDelivery, EmailDeliveryError> {
    let delivery = find(db, receipt_id)
        .await?
        .ok_or(EmailDeliveryError::NotFound)?;
    let mut response = db
        .query(
            "UPDATE $id SET state = $state, \
             channel_id = IF $channel_id IS NONE THEN channel_id ELSE $channel_id END, \
             current_correlation_id = IF $state = $not_attempted THEN NONE ELSE current_correlation_id END, \
             slack_timestamp = $timestamp, last_error = $error, \
             reconciled_by = $actor, reconciliation = $reconciliation, \
             reconciled_at = IF $actor IS NONE THEN reconciled_at ELSE time::now() END, \
             updated_at = time::now() RETURN AFTER",
        )
        .bind(("id", record_id(DELIVERY_TABLE, delivery.id)))
        .bind(("state", update.state.to_string()))
        .bind(("not_attempted", NOT_ATTEMPTED.to_string()))
        .bind(("channel_id", update.channel_id.map(str::to_string)))
        .bind(("timestamp", update.timestamp.map(str::to_string)))
        .bind(("error", update.error.map(str::to_string)))
        .bind(("actor", update.actor.map(str::to_string)))
        .bind(("reconciliation", update.reconciliation.map(str::to_string)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DeliveryRow> = response.take(0)?;
    let updated = row
        .and_then(DeliveryRow::into_delivery)
        .ok_or(EmailDeliveryError::WriteReturnedNothing)?;
    if update.state != NOT_ATTEMPTED {
        if let Some(correlation_id) = updated.current_correlation_id.as_deref() {
            db.query(format!(
                "UPDATE {ATTEMPT_TABLE} SET state = $state, slack_timestamp = $timestamp, \
             error = $error, updated_at = time::now() WHERE delivery_id = $delivery_id \
             AND correlation_id = $correlation_id"
            ))
            .bind(("state", update.state.to_string()))
            .bind(("timestamp", update.timestamp.map(str::to_string)))
            .bind(("error", update.error.map(str::to_string)))
            .bind(("delivery_id", record_id(DELIVERY_TABLE, updated.id)))
            .bind(("correlation_id", correlation_id.to_string()))
            .await
            .and_then(surrealdb::IndexedResults::check)?;
        }
    }
    Ok(updated)
}

pub async fn attempts(
    db: &SurrealDb,
    receipt_id: Uuid,
) -> Result<Vec<EmailDeliveryAttempt>, EmailDeliveryError> {
    let delivery = find(db, receipt_id)
        .await?
        .ok_or(EmailDeliveryError::NotFound)?;
    let mut response = db
        .query(format!(
            "SELECT {ATTEMPT_SELECT} FROM {ATTEMPT_TABLE} \
             WHERE delivery_id = $delivery_id ORDER BY inserted_at"
        ))
        .bind(("delivery_id", record_id(DELIVERY_TABLE, delivery.id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<AttemptRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(AttemptRow::into_attempt)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (SurrealDb, Uuid) {
        let db = crate::surreal::test_support::mem().await;
        let receipt = Uuid::now_v7();
        crate::email_receipts::ensure(
            &db,
            &crate::email_receipts::NewEmailReceipt {
                receiving_mailbox: "support@example.com",
                deployment: "staging",
                raw_digest: &format!("{receipt}"),
                source_message_id: None,
                archive_key: "inbound/example.eml",
                letter_id: Uuid::now_v7(),
            },
        )
        .await
        .expect("receipt setup");
        let mut response = db
            .query("SELECT id FROM email_receipt LIMIT 1")
            .await
            .expect("receipt query")
            .check()
            .expect("receipt query check");
        let row: Option<surrealdb::types::RecordId> = response.take("id").expect("receipt id");
        let receipt_id = record_uuid(&row.expect("receipt row")).expect("receipt uuid");
        (db, receipt_id)
    }

    #[tokio::test]
    async fn concurrent_admission_allows_one_sender() {
        let (db, receipt_id) = setup().await;
        ensure(&db, receipt_id, "C123")
            .await
            .expect("delivery setup");
        let (first, second) = tokio::join!(
            admit_attempt(&db, receipt_id, "corr-a"),
            admit_attempt(&db, receipt_id, "corr-b")
        );
        let first = first.expect("first admission");
        let second = second.expect("second admission");
        assert_ne!(first.attempt_id.is_some(), second.attempt_id.is_some());
        assert_eq!(attempts(&db, receipt_id).await.expect("attempts").len(), 1);
    }

    #[tokio::test]
    async fn confirmed_delivery_replays_without_a_new_attempt() {
        let (db, receipt_id) = setup().await;
        ensure(&db, receipt_id, "C123")
            .await
            .expect("delivery setup");
        let started = admit_attempt(&db, receipt_id, "corr-a")
            .await
            .expect("admission");
        assert!(started.attempt_id.is_some());
        mark_confirmed(&db, receipt_id, "C123", "1700000000.000001")
            .await
            .expect("confirmation");
        let replay = admit_attempt(&db, receipt_id, "corr-b")
            .await
            .expect("replay");
        assert!(replay.attempt_id.is_none());
        assert_eq!(replay.delivery.state, CONFIRMED);
        assert_eq!(attempts(&db, receipt_id).await.expect("attempts").len(), 1);
    }

    #[tokio::test]
    async fn unknown_delivery_requires_operator_authorization_before_resend() {
        let (db, receipt_id) = setup().await;
        ensure(&db, receipt_id, "C123")
            .await
            .expect("delivery setup");
        admit_attempt(&db, receipt_id, "corr-a")
            .await
            .expect("admission");
        mark_unknown(&db, receipt_id, "response lost after dispatch")
            .await
            .expect("unknown");
        let blocked = admit_attempt(&db, receipt_id, "corr-b")
            .await
            .expect("blocked replay");
        assert!(blocked.attempt_id.is_none());
        authorize_resend(&db, receipt_id, "operator@example.com")
            .await
            .expect("authorize");
        let resumed = admit_attempt(&db, receipt_id, "corr-c")
            .await
            .expect("resend admission");
        assert!(resumed.attempt_id.is_some());
    }

    #[tokio::test]
    async fn reconciliation_records_operator_decision_and_updates_attempt() {
        let (db, receipt_id) = setup().await;
        ensure(&db, receipt_id, "C123")
            .await
            .expect("delivery setup");
        admit_attempt(&db, receipt_id, "corr-a")
            .await
            .expect("admission");
        mark_unknown(&db, receipt_id, "response lost after dispatch")
            .await
            .expect("unknown");

        let confirmed = reconcile_confirmed(
            &db,
            receipt_id,
            "operator@example.com",
            "C123",
            "1700000000.000001",
        )
        .await
        .expect("reconciliation");
        assert_eq!(confirmed.state, CONFIRMED);
        assert_eq!(
            confirmed.reconciled_by.as_deref(),
            Some("operator@example.com")
        );
        assert_eq!(
            confirmed.reconciliation.as_deref(),
            Some("operator_confirmed")
        );
        assert_eq!(
            confirmed.slack_timestamp.as_deref(),
            Some("1700000000.000001")
        );
        let attempt = &attempts(&db, receipt_id).await.expect("attempts")[0];
        assert_eq!(attempt.state, CONFIRMED);
        assert_eq!(
            attempt.slack_timestamp.as_deref(),
            Some("1700000000.000001")
        );
        assert_eq!(attempt.error, None);

        authorize_resend(&db, receipt_id, "operator@example.com")
            .await
            .expect("authorize resend");
        admit_attempt(&db, receipt_id, "corr-b")
            .await
            .expect("resend admission");
        let failed = mark_failed(&db, receipt_id, "permanent Slack rejection")
            .await
            .expect("failed delivery");
        assert_eq!(failed.state, FAILED);
        assert_eq!(
            failed.last_error.as_deref(),
            Some("permanent Slack rejection")
        );
        let attempt = &attempts(&db, receipt_id).await.expect("attempts")[1];
        assert_eq!(attempt.state, FAILED);
        assert_eq!(attempt.error.as_deref(), Some("permanent Slack rejection"));
    }
}
