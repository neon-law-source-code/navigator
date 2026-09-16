//! Public lead capture records.
//!
//! A lead is a request for contact, not an identity. The write path therefore
//! never creates or links a `person` row. Repeated requests for one mailbox on
//! one brand update the same row and increment `submissions`; the email index
//! is intentionally non-unique because it is a lookup aid, not an identity
//! constraint.

use chrono::{DateTime, Utc};
use surrealdb::types::{RecordId, SurrealValue};
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

const TABLE: &str = "lead";
const SELECT: &str = "id, email, email_lower, phone, brand_key, source_path, consent_version, \
                     consented_at, sms_consented_at, status, unsubscribed_at, person_id, \
                     submissions, inserted_at, updated_at";

/// One captured public contact request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lead {
    pub id: Uuid,
    pub email: String,
    pub email_lower: String,
    pub phone: Option<String>,
    pub brand_key: String,
    pub source_path: String,
    pub consent_version: String,
    pub consented_at: DateTime<Utc>,
    pub sms_consented_at: Option<DateTime<Utc>>,
    pub status: String,
    pub unsubscribed_at: Option<DateTime<Utc>>,
    pub person_id: Option<Uuid>,
    pub submissions: i64,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The values a public lead submission records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLead {
    pub email: String,
    pub phone: Option<String>,
    pub brand_key: String,
    pub source_path: String,
    pub consent_version: String,
    pub consented_at: DateTime<Utc>,
    pub sms_consented_at: Option<DateTime<Utc>>,
}

#[derive(SurrealValue)]
struct LeadRow {
    id: RecordId,
    email: String,
    email_lower: String,
    phone: Option<String>,
    brand_key: String,
    source_path: String,
    consent_version: String,
    consented_at: surrealdb::types::Datetime,
    sms_consented_at: Option<surrealdb::types::Datetime>,
    status: String,
    unsubscribed_at: Option<surrealdb::types::Datetime>,
    person_id: Option<RecordId>,
    submissions: i64,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl LeadRow {
    fn into_lead(self) -> Option<Lead> {
        Some(Lead {
            id: record_uuid(&self.id)?,
            email: self.email,
            email_lower: self.email_lower,
            phone: self.phone,
            brand_key: self.brand_key,
            source_path: self.source_path,
            consent_version: self.consent_version,
            consented_at: self.consented_at.into(),
            sms_consented_at: self.sms_consented_at.map(Into::into),
            status: self.status,
            unsubscribed_at: self.unsubscribed_at.map(Into::into),
            person_id: self.person_id.as_ref().and_then(record_uuid),
            submissions: self.submissions,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// Errors reading or writing public lead records.
#[derive(Debug, thiserror::Error)]
pub enum LeadError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error("writing a lead returned no usable row")]
    WriteReturnedNothing,
}

/// Insert a lead or update the existing `(email_lower, brand_key)` row.
///
/// The lookup and update are deliberately application-level rather than a
/// unique constraint: the schema permits several rows for the same mailbox so
/// the later admin queue can retain a brand-scoped request without making a
/// mailbox an identity key.
pub async fn record(db: &SurrealDb, new: &NewLead) -> Result<Lead, LeadError> {
    let email = new.email.trim().to_string();
    let email_lower = email.to_lowercase();
    let now = Utc::now();
    let existing = find_by_key(db, &email_lower, &new.brand_key).await?;

    let mut response = if let Some(existing) = existing {
        db.query(format!(
            "UPDATE $id SET email = $email, phone = $phone, source_path = $source_path, \
             consent_version = $consent_version, consented_at = $consented_at, \
             sms_consented_at = $sms_consented_at, submissions += 1, updated_at = $now \
             RETURN {SELECT}"
        ))
        .bind(("id", existing.id))
        .bind(("email", email.clone()))
        .bind(("phone", new.phone.clone()))
        .bind(("source_path", new.source_path.clone()))
        .bind(("consent_version", new.consent_version.clone()))
        .bind((
            "consented_at",
            surrealdb::types::Datetime::from(new.consented_at),
        ))
        .bind((
            "sms_consented_at",
            new.sms_consented_at.map(surrealdb::types::Datetime::from),
        ))
        .bind(("now", surrealdb::types::Datetime::from(now)))
        .await?
        .check()?
    } else {
        db.query(format!(
            "CREATE $id SET email = $email, email_lower = $email_lower, phone = $phone, \
             brand_key = $brand_key, source_path = $source_path, consent_version = $consent_version, \
             consented_at = $consented_at, sms_consented_at = $sms_consented_at, status = 'new', \
             unsubscribed_at = NONE, person_id = NONE, submissions = 1, inserted_at = $now, \
             updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, Uuid::now_v7())))
        .bind(("email", email))
        .bind(("email_lower", email_lower))
        .bind(("phone", new.phone.clone()))
        .bind(("brand_key", new.brand_key.clone()))
        .bind(("source_path", new.source_path.clone()))
        .bind(("consent_version", new.consent_version.clone()))
        .bind((
            "consented_at",
            surrealdb::types::Datetime::from(new.consented_at),
        ))
        .bind((
            "sms_consented_at",
            new.sms_consented_at.map(surrealdb::types::Datetime::from),
        ))
        .bind(("now", surrealdb::types::Datetime::from(now)))
        .await?
        .check()?
    };

    let row: Option<LeadRow> = response.take(0)?;
    row.and_then(LeadRow::into_lead)
        .ok_or(LeadError::WriteReturnedNothing)
}

/// List leads newest first for the later admin queue.
pub async fn list(db: &SurrealDb) -> Result<Vec<Lead>, LeadError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} ORDER BY updated_at DESC, id DESC"
        ))
        .await?
        .check()?;
    let rows: Vec<LeadRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(LeadRow::into_lead).collect())
}

async fn find_by_key(
    db: &SurrealDb,
    email_lower: &str,
    brand_key: &str,
) -> Result<Option<LeadRow>, LeadError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} WHERE email_lower = $email_lower \
             AND brand_key = $brand_key ORDER BY updated_at DESC LIMIT 1"
        ))
        .bind(("email_lower", email_lower.to_string()))
        .bind(("brand_key", brand_key.to_string()))
        .await?
        .check()?;
    response.take(0).map_err(LeadError::from)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{list, record, NewLead};
    use crate::test_support::mem_surreal;

    fn lead(email: &str, brand_key: &str) -> NewLead {
        NewLead {
            email: email.to_string(),
            phone: None,
            brand_key: brand_key.to_string(),
            source_path: "/contact".to_string(),
            consent_version: "By sending this, you agree.".to_string(),
            consented_at: Utc::now(),
            sms_consented_at: None,
        }
    }

    #[tokio::test]
    async fn records_a_lead_without_creating_a_person() {
        let db = mem_surreal().await;
        let written = record(&db, &lead("Visitor@example.com", "neon"))
            .await
            .unwrap();

        assert_eq!(written.email_lower, "visitor@example.com");
        assert_eq!(written.status, "new");
        assert_eq!(written.submissions, 1);
        assert!(crate::persons::find_by_email_ci(&db, "visitor@example.com")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn repeated_mailbox_and_brand_updates_one_row_and_increments_submissions() {
        let db = mem_surreal().await;
        record(&db, &lead("visitor@example.com", "neon"))
            .await
            .unwrap();
        let mut repeat = lead("VISITOR@example.com", "neon");
        repeat.phone = Some("+ ()".to_string());
        repeat.sms_consented_at = Some(Utc::now());
        let written = record(&db, &repeat).await.unwrap();

        assert_eq!(written.submissions, 2);
        assert_eq!(written.phone.as_deref(), Some("+ ()"));
        assert!(written.sms_consented_at.is_some());
        assert_eq!(list(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_same_mailbox_on_two_brands_is_two_leads() {
        let db = mem_surreal().await;
        record(&db, &lead("visitor@example.com", "neon"))
            .await
            .unwrap();
        record(&db, &lead("visitor@example.com", "delete-your-data"))
            .await
            .unwrap();

        assert_eq!(list(&db).await.unwrap().len(), 2);
    }
}
