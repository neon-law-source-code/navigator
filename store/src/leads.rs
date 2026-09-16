//! Public lead capture records.
//!
//! A lead is a request for contact, not an identity. Capture never creates a
//! `person` row. Conversion and linking write through [`crate::persons`] and
//! set `lead.person_id`, so the Person table stays the human directory.
//! Repeated requests for one mailbox on one brand update the same row and
//! increment `submissions`; the email index is intentionally non-unique
//! because it is a lookup aid, not an identity constraint.

use chrono::{DateTime, Utc};
use surrealdb::types::{RecordId, SurrealValue};
use uuid::Uuid;

use crate::persons::{self, ContactUpdate, NewPerson, PersonError};
use crate::surreal::{record_id, record_uuid, SurrealDb};

const TABLE: &str = "lead";
const PERSON_TABLE: &str = "person";
const SELECT: &str = "id, email, email_lower, phone, brand_key, source_path, consent_version, \
                     consented_at, sms_consented_at, status, unsubscribed_at, person_id, \
                     submissions, inserted_at, updated_at";

/// Closed status set stored on `lead.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeadStatus {
    New,
    Contacted,
    Converted,
    Declined,
    Unsubscribed,
}

impl LeadStatus {
    /// Stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Contacted => "contacted",
            Self::Converted => "converted",
            Self::Declined => "declined",
            Self::Unsubscribed => "unsubscribed",
        }
    }

    /// Parse a stored or submitted status word.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "new" => Some(Self::New),
            "contacted" => Some(Self::Contacted),
            "converted" => Some(Self::Converted),
            "declined" => Some(Self::Declined),
            "unsubscribed" => Some(Self::Unsubscribed),
            _ => None,
        }
    }
}

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
    #[error("unknown lead")]
    NotFound,
    #[error("status is not one of new, contacted, converted, declined, unsubscribed")]
    InvalidStatus,
    #[error("a person already holds this mailbox")]
    EmailTaken { person_id: Uuid },
    #[error("no person holds this mailbox")]
    NoMatchingPerson,
    #[error(transparent)]
    Person(#[from] PersonError),
}

/// Last four digits of a phone, or an em dash when none were recorded.
#[must_use]
pub fn mask_phone(phone: Option<&str>) -> String {
    let digits: String = phone
        .unwrap_or("")
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    match digits.len() {
        0 => "—".to_string(),
        n if n <= 4 => format!("…{digits}"),
        n => format!("…{}", &digits[n - 4..]),
    }
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

/// One lead by id.
pub async fn find(db: &SurrealDb, id: Uuid) -> Result<Option<Lead>, LeadError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id"))
        .bind(("id", record_id(TABLE, id)))
        .await?
        .check()?;
    let row: Option<LeadRow> = response.take(0)?;
    Ok(row.and_then(LeadRow::into_lead))
}

/// Persist a status word. `unsubscribed_at` is written the first time the
/// row becomes unsubscribed and left alone afterward.
pub async fn set_status(db: &SurrealDb, id: Uuid, status: LeadStatus) -> Result<Lead, LeadError> {
    let existing = find(db, id).await?.ok_or(LeadError::NotFound)?;
    apply_status(db, existing, status).await
}

/// Create a Client Person from the lead mailbox and mark the row converted.
///
/// Refuses when that mailbox already belongs to a Person, naming the existing
/// row so the admin surface can offer a link instead.
pub async fn convert(db: &SurrealDb, id: Uuid) -> Result<Lead, LeadError> {
    let existing = find(db, id).await?.ok_or(LeadError::NotFound)?;
    if let Some(person) = persons::find_by_email_ci(db, &existing.email).await? {
        return Err(LeadError::EmailTaken {
            person_id: person.id,
        });
    }
    let mut input = NewPerson::new(existing.email.clone(), existing.email.clone());
    input.phone = existing.phone.clone();
    let person = persons::create(db, &input).await?;
    link_converted(db, existing.id, person.id).await
}

/// Point the lead at the Person who already holds its mailbox and mark it
/// converted.
///
/// When that Person has no phone and the lead recorded one, the phone is
/// copied onto the Person row so later contact reads the directory, not the
/// queue.
pub async fn link_person(db: &SurrealDb, id: Uuid) -> Result<Lead, LeadError> {
    let existing = find(db, id).await?.ok_or(LeadError::NotFound)?;
    let Some(person) = persons::find_by_email_ci(db, &existing.email).await? else {
        return Err(LeadError::NoMatchingPerson);
    };
    if person.phone.is_none() {
        if let Some(phone) = existing.phone.clone() {
            persons::update_contact(
                db,
                person.id,
                &ContactUpdate {
                    name: person.name.clone(),
                    title: person.title.clone(),
                    phone: Some(phone),
                },
            )
            .await?;
        }
    }
    link_converted(db, existing.id, person.id).await
}

async fn apply_status(
    db: &SurrealDb,
    existing: Lead,
    status: LeadStatus,
) -> Result<Lead, LeadError> {
    let now = Utc::now();
    let unsubscribed_at = if status == LeadStatus::Unsubscribed {
        Some(existing.unsubscribed_at.unwrap_or(now))
    } else {
        existing.unsubscribed_at
    };
    let mut response = db
        .query(format!(
            "UPDATE $id SET status = $status, unsubscribed_at = $unsubscribed_at, \
             updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, existing.id)))
        .bind(("status", status.as_str().to_string()))
        .bind((
            "unsubscribed_at",
            unsubscribed_at.map(surrealdb::types::Datetime::from),
        ))
        .bind(("now", surrealdb::types::Datetime::from(now)))
        .await?
        .check()?;
    let row: Option<LeadRow> = response.take(0)?;
    row.and_then(LeadRow::into_lead)
        .ok_or(LeadError::WriteReturnedNothing)
}

async fn link_converted(db: &SurrealDb, lead_id: Uuid, person_id: Uuid) -> Result<Lead, LeadError> {
    let now = Utc::now();
    let mut response = db
        .query(format!(
            "UPDATE $id SET status = 'converted', person_id = $person_id, \
             updated_at = $now RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, lead_id)))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("now", surrealdb::types::Datetime::from(now)))
        .await?
        .check()?;
    let row: Option<LeadRow> = response.take(0)?;
    row.and_then(LeadRow::into_lead)
        .ok_or(LeadError::WriteReturnedNothing)
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

    use super::{
        convert, find, link_person, list, mask_phone, record, set_status, LeadError, LeadStatus,
        NewLead,
    };
    use crate::persons::{self, NewPerson, Role};
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

    #[tokio::test]
    async fn status_transitions_persist_and_unsubscribe_stamps_once() {
        let db = mem_surreal().await;
        let lead = record(&db, &lead("visitor@example.com", "neon"))
            .await
            .unwrap();

        let contacted = set_status(&db, lead.id, LeadStatus::Contacted)
            .await
            .unwrap();
        assert_eq!(contacted.status, "contacted");
        assert!(contacted.unsubscribed_at.is_none());

        let first = set_status(&db, lead.id, LeadStatus::Unsubscribed)
            .await
            .unwrap();
        assert_eq!(first.status, "unsubscribed");
        let stamped = first.unsubscribed_at.expect("unsubscribe stamps the row");

        let again = set_status(&db, lead.id, LeadStatus::Unsubscribed)
            .await
            .unwrap();
        assert_eq!(again.unsubscribed_at, Some(stamped));

        let declined = set_status(&db, lead.id, LeadStatus::Declined)
            .await
            .unwrap();
        assert_eq!(declined.status, "declined");
        assert_eq!(declined.unsubscribed_at, Some(stamped));
        assert_eq!(
            find(&db, lead.id).await.unwrap().unwrap().unsubscribed_at,
            Some(stamped)
        );
    }

    #[tokio::test]
    async fn conversion_writes_person_id_and_refuses_a_duplicate_email() {
        let db = mem_surreal().await;
        let mut incoming = lead("visitor@example.com", "neon");
        incoming.phone = Some("+1 (555) 010-9876".to_string());
        let written = record(&db, &incoming).await.unwrap();

        let converted = convert(&db, written.id).await.unwrap();
        let person_id = converted.person_id.expect("conversion links a person");
        assert_eq!(converted.status, "converted");
        let person = persons::find_by_id(&db, person_id).await.unwrap().unwrap();
        assert_eq!(person.email, "visitor@example.com");
        assert_eq!(person.role, Role::Client);
        assert_eq!(person.phone.as_deref(), Some("+1 (555) 010-9876"));

        let duplicate = record(&db, &lead("visitor@example.com", "delete-your-data"))
            .await
            .unwrap();
        match convert(&db, duplicate.id).await {
            Err(LeadError::EmailTaken {
                person_id: existing,
            }) => {
                assert_eq!(existing, person_id);
            }
            other => panic!("expected EmailTaken, got {other:?}"),
        }

        let linked = link_person(&db, duplicate.id).await.unwrap();
        assert_eq!(linked.status, "converted");
        assert_eq!(linked.person_id, Some(person_id));
    }

    #[tokio::test]
    async fn link_copies_a_missing_phone_onto_the_person_and_leaves_an_existing_number() {
        let db = mem_surreal().await;
        let without_phone = persons::create(&db, &NewPerson::new("Existing", "link@example.com"))
            .await
            .unwrap();
        let mut incoming = lead("link@example.com", "neon");
        incoming.phone = Some("+1 (555) 010-1111".to_string());
        let written = record(&db, &incoming).await.unwrap();

        link_person(&db, written.id).await.unwrap();
        let person = persons::find_by_id(&db, without_phone.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(person.phone.as_deref(), Some("+1 (555) 010-1111"));

        let mut other = NewPerson::new("Kept", "kept@example.com");
        other.phone = Some("+1 (555) 010-2222".to_string());
        let kept = persons::create(&db, &other).await.unwrap();
        let mut second = lead("kept@example.com", "neon");
        second.phone = Some("+1 (555) 010-3333".to_string());
        let second_lead = record(&db, &second).await.unwrap();
        link_person(&db, second_lead.id).await.unwrap();
        let person = persons::find_by_id(&db, kept.id).await.unwrap().unwrap();
        assert_eq!(person.phone.as_deref(), Some("+1 (555) 010-2222"));
    }

    #[tokio::test]
    async fn convert_refuses_an_already_seeded_mailbox() {
        let db = mem_surreal().await;
        persons::create(&db, &NewPerson::new("Existing", "already@example.com"))
            .await
            .unwrap();
        let written = record(&db, &lead("already@example.com", "neon"))
            .await
            .unwrap();
        assert!(matches!(
            convert(&db, written.id).await,
            Err(LeadError::EmailTaken { .. })
        ));
    }

    #[test]
    fn mask_phone_keeps_only_the_last_four_digits() {
        assert_eq!(mask_phone(Some("+1 (555) 010-9876")), "…9876");
        assert_eq!(mask_phone(None), "—");
        assert_eq!(mask_phone(Some("12")), "…12");
    }
}
