//! The person directory: the [`Role`] authorization tier and every query
//! that reads or writes a `person` row.
//!
//! # This table lives in SurrealDB
//!
//! Sign-in resolves against this engine: the OIDC callback looks up the
//! row by `oidc_subject` or by email and reads `role` from it.
//!
//! [`Role`] is the system-wide tier every authorization gate evaluates
//! against. It is read from the database row at callback time, never
//! trusted from the OIDC token: the Rauthy (or Google) id_token carries
//! only `sub` and `email`. Authorization stays *above* the database
//! (#1145) — the table keeps `PERMISSIONS NONE` and this module is the
//! only thing that reads or writes it. See
//! [`docs/access-model.md`](../../../docs/access-model.md).
//!
//! # Five engine facts this module is shaped around
//!
//! **An index cannot be defined over an expression.** `DEFINE INDEX …
//! FIELDS string::lowercase(email)` is refused as the statement runs —
//! the engine evaluates the expression at define time and reports
//! `string::lowercase()` receiving `NONE`. So one email per person is
//! enforced by the stored `email_lower` field
//! (`VALUE string::lowercase(email)`) with a plain UNIQUE index on it.
//! Every case-insensitive email match therefore filters that stored
//! field rather than lowercasing `email` in the predicate, so the lookup
//! and the constraint agree by construction.
//!
//! **A unique violation carries no typed detail.** It arrives as
//! [`surrealdb::types::ErrorDetails::Internal`] with the index name in
//! the message and nothing structured to match on, so
//! [`classify_write`] discriminates on the index name — the one part of
//! the text the schema pins — and
//! [`a_duplicate_email_is_reported_as_the_email_being_taken`] holds it
//! against a real engine.
//!
//! **A UNIQUE index does not enforce across concurrent transactions,
//! and reading the value first does not help.** The optimistic layer
//! conflicts on *record* keys; index entries are not part of conflict
//! detection, so racers writing distinct `person` ids both commit the
//! same `email_lower`. Probing first does not close it either — a
//! `WHERE email_lower = $v` predicate is a scan, and a scan is not
//! tracked in the transaction's read set the way a direct record read
//! is. So one person per mailbox is enforced by the `person_mailbox`
//! claim table, whose record id *is* `email_lower`, taken as its own
//! committed statement before the person row is written — see [`CLAIM`]
//! and [`find_or_create`]. The UNIQUE index remains the backstop for a
//! fork that is not a race.
//!
//! //! **`IF … THEN … ELSE … END` does not parse inside `ORDER BY`.** The
//! authority ladder is therefore ranked in Rust via
//! [`Role::authority_rank`] rather than written a second time in
//! SurrealQL — see [`default_firm_dri`].
//!
//! **The key-value layer is optimistic, so a write can lose a race.**
//! Two writers touching one record conflict, the loser is rolled back,
//! and the engine reports `QueryError::TransactionConflict` — this one
//! typed, unlike the unique violation above. Nothing was wrong with the
//! statement, so [`writing`] re-runs it rather than letting a
//! simultaneous save read as a database fault. How long it re-runs for is
//! not this module's decision — [`crate::surreal::retry`] holds that
//! policy for the whole crate, and `person` is only its most contended
//! caller.
//!
//! # A link is not validated
//!
//! `person_project_role`, `notation`, and the rest reference a person
//! through a `record<person>` link, and the engine accepts one naming a
//! row that was never written. A caller that needs the person behind a
//! link resolves it here, which is what [`find_by_id`] and
//! [`find_by_ids`] exist for.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::{AlreadyExistsError, ErrorDetails, SurrealValue};
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "person";

/// System-wide authorization tier for a [`Person`]. Stored as a `string`
/// with an `ASSERT $value IN [...]` on the field, which is what closes
/// the vocabulary. Anonymous callers have no row at all.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The accountable principal who owns the Navigator application. Owner
    /// inherits every Admin and Lawyer capability and alone may assign the
    /// Owner role.
    Owner,
    /// A licensed lawyer with system-administration authority. Bypasses
    /// project-scoping entirely and is a member of the lawyer tier.
    Admin,
    /// A licensed lawyer authorized to perform legal work through Navigator.
    /// The lawyer tier may perform work only on assigned projects and supervise
    /// Clerk work where a future Clerk-specific capability permits it. This is
    /// not an employment, email-domain, or source-forge membership grant.
    Lawyer,
    /// A supervised non-lawyer firm worker. This role is intentionally
    /// outside the lawyer tier and receives no `/app/lawyer`, MCP, Git, or
    /// legal-work authority merely by existing. Narrow Clerk project work
    /// must name its own route and supervision boundary.
    Clerk,
    /// A person the firm represents on at least one matter. Sees
    /// only projects with a matching `person_project_roles` row.
    ///
    /// The default, for both seeded rows and freshly-created ones:
    /// promotion above Client is always opt-in.
    #[default]
    Client,
}

impl Role {
    /// String form used in embedded Rego policy inputs, the URL-encoded
    /// admin form, and the stored `role` field.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Lawyer => "lawyer",
            Self::Clerk => "clerk",
            Self::Client => "client",
        }
    }

    /// The role named by its stored spelling, or `None` for anything
    /// else. The inverse of [`Role::as_str`], and the only way a stored
    /// `role` becomes a [`Role`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "owner" => Some(Self::Owner),
            "admin" => Some(Self::Admin),
            "lawyer" => Some(Self::Lawyer),
            "clerk" => Some(Self::Clerk),
            "client" => Some(Self::Client),
            _ => None,
        }
    }

    /// The role's place in the application authority ladder.
    ///
    /// Higher roles may govern lower roles. Participation remains a separate
    /// matter-scope fact and is not represented here.
    #[must_use]
    pub const fn authority_rank(self) -> u8 {
        match self {
            Self::Owner => 4,
            Self::Admin => 3,
            Self::Lawyer => 2,
            Self::Clerk => 1,
            Self::Client => 0,
        }
    }

    /// `true` for Owner and Admin — the system-wide tiers that gate
    /// `/admin/*` and bypass project scoping.
    #[must_use]
    pub fn is_admin_tier(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// `true` only for the accountable Owner tier.
    #[must_use]
    pub fn is_owner(self) -> bool {
        matches!(self, Self::Owner)
    }

    /// `true` for `Lawyer`, `Admin`, and `Owner` — the lawyer tiers that gate
    /// `/app/lawyer/*` legal work. Clerk is deliberately excluded: its
    /// non-lawyer work must be granted by a separate, supervised capability.
    #[must_use]
    pub fn is_lawyer_tier(self) -> bool {
        matches!(self, Self::Owner | Self::Admin | Self::Lawyer)
    }

    /// `true` when this role is the explicit non-lawyer Clerk tier.
    #[must_use]
    pub fn is_clerk(self) -> bool {
        matches!(self, Self::Clerk)
    }
}

/// One person in the directory.
///
/// The application-facing shape: plain Rust types, no engine handles.
/// [`PersonRow`] is the seam that turns it into (and back out of) what
/// the SDK reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Person {
    pub id: Uuid,
    /// The display name. The structured legal-name parts below are what
    /// a filing that must split the name reads.
    pub name: String,
    /// The person's given (first) name. `None` until a matter needs the
    /// legal name split into parts.
    pub given_name: Option<String>,
    /// The person's family (last) name. `None` until set.
    pub family_name: Option<String>,
    /// The person's middle name, if any. `None` until set, and `None`
    /// for a person with no middle name.
    pub middle_name: Option<String>,
    /// The mailbox, as supplied. Matching is case-insensitive through
    /// the stored `email_lower` field — see [`find_by_email_ci`].
    pub email: String,
    /// OIDC `sub` claim — stable identifier from the IdP (Rauthy,
    /// Google, etc.). `None` for seeded persons not yet linked.
    pub oidc_subject: Option<String>,
    /// System-wide tier.
    pub role: Role,
    /// The contact's role at their organization (e.g. "Executive
    /// Director"). Free text; `None` until set by the importer or an
    /// admin edit.
    pub title: Option<String>,
    /// The contact's direct phone line. `None` until set.
    pub phone: Option<String>,
    /// Xero `ContactID` (GUID) once this person has been mirrored to
    /// Xero Contacts via the billing seam (one-way, Neon Law Navigator →
    /// Xero). `None` until first synced.
    pub xero_contact_id: Option<String>,
    /// Optional public profile image URL. Used only on consented public
    /// attribution surfaces such as testimonials.
    pub profile_image_url: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads it. Separate from [`Person`] because the
/// SDK owns its own `RecordId` and `Datetime`, and `role` arrives as the
/// stored string; the conversion belongs at this seam rather than in
/// every caller.
#[derive(SurrealValue)]
struct PersonRow {
    id: surrealdb::types::RecordId,
    name: String,
    given_name: Option<String>,
    family_name: Option<String>,
    middle_name: Option<String>,
    email: String,
    oidc_subject: Option<String>,
    role: String,
    title: Option<String>,
    phone: Option<String>,
    xero_contact_id: Option<String>,
    profile_image_url: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl PersonRow {
    /// `None` when the record id is not a native UUID key (see
    /// [`crate::surreal`] for why the two key spellings differ) or when
    /// `role` is not one the ladder names. Both are rows this workspace
    /// could not have written; reporting them as a [`Person`] would
    /// invent an id or an authority tier.
    fn into_person(self) -> Option<Person> {
        Some(Person {
            id: record_uuid(&self.id)?,
            name: self.name,
            given_name: self.given_name,
            family_name: self.family_name,
            middle_name: self.middle_name,
            email: self.email,
            oidc_subject: self.oidc_subject,
            role: Role::parse(&self.role)?,
            title: self.title,
            phone: self.phone,
            xero_contact_id: self.xero_contact_id,
            profile_image_url: self.profile_image_url,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`PersonRow`] from only one query.
/// `email_lower` is deliberately absent: it is a stored derivation of
/// `email` that exists for the unique index, not a fact a caller needs.
const SELECT: &str = "id, name, given_name, family_name, middle_name, email, oidc_subject, \
                      role, title, phone, xero_contact_id, profile_image_url, \
                      inserted_at, updated_at";

/// Errors reading or writing a person.
#[derive(Debug, thiserror::Error)]
pub enum PersonError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The write collided with `person_email_lower` — another row
    /// already holds this mailbox, case-insensitively.
    #[error("that email is already in use")]
    EmailTaken,
    /// The write collided with `person_oidc_subject` — another row is
    /// already linked to this IdP identity.
    #[error("that IdP identity is already linked to another person")]
    OidcSubjectTaken,
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see [`PersonRow::into_person`].
    #[error("writing a person returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault.
///
/// A unique violation carries **no typed detail** — the engine reports
/// it as `ErrorDetails::Internal`, so there is nothing structured to
/// match on and the index name in the message is the only discriminator
/// available. That is not pattern-matching on prose: the names are
/// `DEFINE INDEX` identifiers this workspace chose in
/// `store/src/schema/navigator.surql`, and
/// [`a_duplicate_email_is_reported_as_the_email_being_taken`] pins each
/// one against a real engine so a rename cannot silently reclassify a
/// conflict as a server fault.
fn classify_write(error: surrealdb::Error) -> PersonError {
    let message = error.to_string();
    if message.contains("person_email_lower") {
        PersonError::EmailTaken
    } else if message.contains("person_oidc_subject") {
        PersonError::OidcSubjectTaken
    } else {
        PersonError::Db(error)
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), classifying whatever finally comes back
/// as a person-shaped error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate — see
/// [`a_write_that_loses_an_optimistic_race_is_retried_not_surfaced`].
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, PersonError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(classify_write)
}

/// The table whose **record id is the mailbox** — `email_lower`, the same
/// stored, lowercased spelling `person_email_lower` indexes. Writing it is
/// what serializes two racers claiming one mailbox: they collide on one
/// identical record key, which the engine enforces, rather than on a
/// UNIQUE index entry, which under concurrency it does not. See the
/// `person_mailbox` block in `store/src/schema/navigator.surql`.
const CLAIM_TABLE: &str = "person_mailbox";

/// Claim `$email_lower` for `$id`, refusing when any row already holds it.
///
/// `CREATE` rather than `UPSERT` on purpose: `UPSERT` would take the claim
/// from its current holder, which is the fork this exists to refuse.
///
/// **It runs as its own statement, never inside a wider transaction.**
/// That is the whole mechanism, and it is easy to "tidy" away by folding
/// it back in with the person write. An explicit `BEGIN … COMMIT` reads
/// the snapshot taken at `BEGIN`, so two racers each see the mailbox free
/// and each `CREATE` succeeds against its own snapshot — the fork comes
/// straight back, and the wider the transaction the likelier it is.
/// Committing the claim on its own is what makes the second racer read the
/// first one's row.
const CLAIM: &str = "CREATE type::record('person_mailbox', $email_lower) SET person_id = $id;";

/// Give back one specific mailbox claim, and only when `$id` is what
/// holds it. Conditional so a release on a failure path cannot take a
/// claim that has since become somebody else's.
const RELEASE_MAILBOX: &str =
    "DELETE type::record('person_mailbox', $email_lower) WHERE person_id = $id;";

/// Give back every claim `$id` holds. A no-op when it holds none, so it
/// costs nothing to run on a path that may not have claimed.
const RELEASE_PERSON: &str = "DELETE person_mailbox WHERE person_id = $id;";

/// The first window [`find_or_create`] waits before re-reading a claim
/// whose person row has not appeared yet. Same shape as the shared write
/// backoff: every loser is refused at the same instant, so looking again
/// immediately just re-reads the same un-committed row.
const SETTLE_FIRST_BACKOFF: std::time::Duration = std::time::Duration::from_millis(2);

/// The largest window that poll will wait, so a long wait inside the
/// write budget is not spent in one sleep.
const SETTLE_BACKOFF_CEILING: std::time::Duration = std::time::Duration::from_millis(64);

/// Whether `error` is the claim record refusing a second holder.
///
/// The claim collision is **typed**: `CREATE` onto a taken record id
/// reports [`AlreadyExistsError::Record`] carrying that id, so the
/// discriminator is a structured value rather than prose — unlike the
/// UNIQUE-index violation [`classify_write`] has to read the message for.
fn claims_a_mailbox(error: &surrealdb::Error) -> bool {
    matches!(
        error.details(),
        ErrorDetails::AlreadyExists(Some(AlreadyExistsError::Record { id }))
            if id.starts_with(CLAIM_TABLE)
    )
}

/// Take the claim on `email_lower` for `id`, reporting whether this call
/// is the one now holding it.
///
/// `Ok(false)` is not a failure — it is another row holding the mailbox,
/// which every caller here answers differently: [`create`] and [`edit`]
/// turn it into [`PersonError::EmailTaken`], while [`find_or_create`]
/// settles on the holder instead.
async fn take_mailbox(db: &SurrealDb, email_lower: &str, id: Uuid) -> Result<bool, PersonError> {
    match retry::writing(|| {
        db.query(CLAIM)
            .bind(("id", record_id(TABLE, id)))
            .bind(("email_lower", email_lower.to_string()))
    })
    .await
    {
        Ok(_) => Ok(true),
        Err(error) if claims_a_mailbox(&error) => Ok(false),
        Err(error) => Err(classify_write(error)),
    }
}

/// The person currently holding `email_lower`, or `None` when the mailbox
/// is free.
///
/// A **direct record read**, deliberately not a scan. That is not a
/// micro-optimisation: a `WHERE email_lower = $v` scan is what the forked
/// shape did, and a scan does not enter the transaction's read set, so it
/// cannot make one racer observe another's write.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn mailbox_holder(
    db: &SurrealDb,
    email_lower: &str,
) -> Result<Option<Uuid>, PersonError> {
    let mut response = db
        .query(format!(
            "SELECT VALUE person_id FROM ONLY type::record('{CLAIM_TABLE}', $email_lower)"
        ))
        .bind(("email_lower", email_lower.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let holder: Option<surrealdb::types::RecordId> = response.take(0)?;
    Ok(holder.as_ref().and_then(record_uuid))
}

/// Give back a claim taken for a write that then did not land, so a
/// failure does not leave a mailbox claimed by a row that never became a
/// person — which nothing could then create.
///
/// Best effort on purpose: it runs on a path that is already reporting
/// something else, and the caller's own outcome is the one worth
/// surfacing. A claim stranded here is visible as a `person_mailbox` row
/// whose `person_id` names a row that does not exist, and
/// [`find_or_create`] waits such a claim out rather than trusting it
/// forever.
async fn release_mailbox(db: &SurrealDb, email_lower: &str, id: Uuid) {
    let _ = retry::writing(|| {
        db.query(RELEASE_MAILBOX)
            .bind(("id", record_id(TABLE, id)))
            .bind(("email_lower", email_lower.to_string()))
    })
    .await;
}

/// The fields a new person row carries. Everything but `name` and
/// `email` has a sensible empty default, so a caller that only knows
/// those two writes them and takes the rest.
#[derive(Debug, Clone, Default)]
pub struct NewPerson {
    pub name: String,
    pub email: String,
    /// Defaults to [`Role::Client`]: promotion is always opt-in.
    pub role: Role,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub middle_name: Option<String>,
    pub oidc_subject: Option<String>,
    pub title: Option<String>,
    pub phone: Option<String>,
    pub profile_image_url: Option<String>,
}

impl NewPerson {
    /// The common case: a display name and a mailbox, everything else
    /// defaulted.
    #[must_use]
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            ..Self::default()
        }
    }

    /// The same, at an explicit authority tier.
    #[must_use]
    pub fn with_role(name: impl Into<String>, email: impl Into<String>, role: Role) -> Self {
        Self {
            role,
            ..Self::new(name, email)
        }
    }
}

/// The fields an edit may change, in the shape the People command
/// boundary needs: a `None` leaves the column alone, and the doubled
/// option on a structured name part keeps "clear it" distinct from
/// "don't touch it" — the PATCH distinction a single option collapses.
#[derive(Debug, Clone, Default)]
// The doubled option is the whole point — see the doc comment.
#[allow(clippy::option_option)]
pub struct PersonEdit {
    pub name: Option<String>,
    pub email: Option<String>,
    pub role: Option<Role>,
    pub given_name: Option<Option<String>>,
    pub family_name: Option<Option<String>>,
    pub middle_name: Option<Option<String>>,
    /// A seed reconciliation may replace the optional directory image while
    /// leaving identity and authority untouched. Browser-facing commands do
    /// not populate this field, so their existing PATCH contract is unchanged.
    pub profile_image_url: Option<Option<String>>,
}

/// The contact facts a directory import owns: the display name and the
/// two free-text contact columns. Separate from [`PersonEdit`] because
/// an import must never touch `role` or `email` — the row it found is
/// the row it updates, and a person promoted to lawyer stays promoted.
#[derive(Debug, Clone, Default)]
pub struct ContactUpdate {
    pub name: String,
    pub title: Option<String>,
    pub phone: Option<String>,
}

/// Read one person out of a query response, dropping a row this module
/// could not have written.
fn one(mut response: surrealdb::IndexedResults) -> Result<Option<Person>, PersonError> {
    let row: Option<PersonRow> = response.take(0)?;
    Ok(row.and_then(PersonRow::into_person))
}

/// Read every person out of a query response, in the order the engine
/// returned them.
fn many(mut response: surrealdb::IndexedResults) -> Result<Vec<Person>, PersonError> {
    let rows: Vec<PersonRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(PersonRow::into_person)
        .collect())
}

/// Resolve a person by id.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn find_by_id(db: &SurrealDb, id: Uuid) -> Result<Option<Person>, PersonError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Resolve several people at once, for a caller holding a batch of
/// `person_id`s from a table that has not moved yet. Ids with no row are
/// simply absent from the result, so the caller sees exactly who exists.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn find_by_ids(db: &SurrealDb, ids: &[Uuid]) -> Result<Vec<Person>, PersonError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let records: Vec<surrealdb::types::RecordId> =
        ids.iter().map(|id| record_id(TABLE, *id)).collect();
    let response = db
        .query(format!("SELECT {SELECT} FROM person WHERE id IN $ids"))
        .bind(("ids", records))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Resolve a person by `email`, matched case-insensitively.
///
/// Email is a case-insensitive identifier: `Attorney@Example.com` and
/// `attorney@example.com` are the same mailbox, and an IdP may present a
/// casing that differs from the stored row. Every lookup keyed on email
/// goes through here, filtering the stored `email_lower` field so it
/// agrees with the `person_email_lower` unique index by construction —
/// the engine rejects an expression index, so lowercasing in the
/// predicate instead would leave the lookup and the constraint able to
/// disagree.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn find_by_email_ci(db: &SurrealDb, email: &str) -> Result<Option<Person>, PersonError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY person WHERE email_lower = $email LIMIT 1"
        ))
        .bind(("email", email.trim().to_lowercase()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// Every person currently holding `role`, in no particular order. The
/// bootstrap-Owner reconciliation uses this to find every `Owner` besides
/// the one the configured `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` names, so a
/// stale identity from a previous env-var value can be demoted rather than
/// left at `Owner` indefinitely.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn find_by_role(db: &SurrealDb, role: Role) -> Result<Vec<Person>, PersonError> {
    let response = db
        .query(format!("SELECT {SELECT} FROM person WHERE role = $role"))
        .bind(("role", role.as_str().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Resolve a person by their IdP `sub` claim — the first thing the OIDC
/// callback asks, before falling back to the email.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn find_by_oidc_subject(
    db: &SurrealDb,
    subject: &str,
) -> Result<Option<Person>, PersonError> {
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY person WHERE oidc_subject = $subject LIMIT 1"
        ))
        .bind(("subject", subject.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    one(response)
}

/// The firm-side person to designate as a matter's lawyer DRI when a create
/// path has no explicit opener — the self-serve intake (no lawyer in the
/// room), the CLI, and AIDA's tool calls. Returns the lowest-id `owner`,
/// else the lowest-id `admin`, else the lowest-id `lawyer` — i.e. the firm
/// principal in a seeded install, resolved by **role**, not a hard-coded
/// email, so a white-label fork gets its own principal with no code
/// change. `None` only on a database with no firm-side person at all
/// (which the caller treats as an error).
///
/// The ladder is ranked here rather than in the query: `IF … THEN … ELSE
/// … END` does not parse inside `ORDER BY`, and the alternative — three
/// ordered queries, or the ladder spelled a second time in SurrealQL —
/// would let [`Role::authority_rank`] and the resolver drift apart.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn default_firm_dri(db: &SurrealDb) -> Result<Option<Uuid>, PersonError> {
    let firm_side = [Role::Owner, Role::Admin, Role::Lawyer].map(|role| role.as_str().to_string());
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM person WHERE role IN $roles ORDER BY id ASC"
        ))
        .bind(("roles", firm_side.to_vec()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    // `ORDER BY id ASC` settles the tie inside a tier, and `min_by_key`
    // returns the FIRST minimum — so ranking by the reversed authority
    // gives the lowest-id row of the highest tier. (`max_by_key` returns
    // the *last* maximum, which would pick the highest id in the tier.)
    Ok(many(response)?
        .into_iter()
        .min_by_key(|person| std::cmp::Reverse(person.role.authority_rank()))
        .map(|person| person.id))
}

/// The person directory the lawyer people page renders, filtered and sorted by
/// the JSON:API `?sort=` / `filter[...]` query parameters. `filter_name` /
/// `filter_email` are case-insensitive substring matches (empty = no filter);
/// `sort` is a list of `(key, descending)` pairs where `key` is `"name"` or
/// `"email"` (any other key is ignored — the caller validates and 400s on an
/// unadvertised field before reaching here). With no sort, rows come back
/// ordered by display name. The shared base query so the Dioxus people
/// component (issue #641 / #355) and the existing admin handler agree.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn list_directory(
    db: &SurrealDb,
    filter_name: &str,
    filter_email: &str,
    sort: &[(String, bool)],
) -> Result<Vec<Person>, PersonError> {
    let mut order = Vec::new();
    for (key, descending) in sort {
        // The column list is a whitelist, never the caller's string —
        // an unadvertised key is ignored here and 400s upstream.
        let column = match key.as_str() {
            "name" => "name",
            "email" => "email",
            _ => continue,
        };
        order.push(format!(
            "{column} {}",
            if *descending { "DESC" } else { "ASC" }
        ));
    }
    if order.is_empty() {
        order.push("name ASC".to_string());
    }

    let response = db
        .query(format!(
            "SELECT {SELECT} FROM person \
             WHERE ($name = '' OR string::contains(string::lowercase(name), $name)) \
               AND ($email = '' OR string::contains(email_lower, $email)) \
             ORDER BY {}",
            order.join(", ")
        ))
        .bind(("name", filter_name.to_lowercase()))
        .bind(("email", filter_email.to_lowercase()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Fuzzy-find people by an optional name and/or email substring. Both
/// needles are matched case-insensitively as substrings and ANDed when
/// both are supplied; the caller is responsible for rejecting the
/// all-`None` case (a blank query would return the whole directory).
/// Results are ordered by name and capped at `limit`.
///
/// This is the read half of the People command boundary: the AIDA
/// `aida_show_person` tool and any web lookup share this one query
/// instead of re-implementing the predicate.
///
/// # Errors
///
/// [`PersonError::Db`] if the lookup fails.
pub async fn search(
    db: &SurrealDb,
    name: Option<&str>,
    email: Option<&str>,
    limit: u64,
) -> Result<Vec<Person>, PersonError> {
    let name = name.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
    let email = email.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("");
    let response = db
        .query(format!(
            "SELECT {SELECT} FROM person \
             WHERE ($name = '' OR string::contains(string::lowercase(name), $name)) \
               AND ($email = '' OR string::contains(email_lower, $email)) \
             ORDER BY name ASC LIMIT $limit"
        ))
        .bind(("name", name.to_lowercase()))
        .bind(("email", email.to_lowercase()))
        .bind(("limit", i64::try_from(limit).unwrap_or(i64::MAX)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    many(response)
}

/// Write a new person row.
///
/// The record id is minted from a fresh v7 `Uuid` through
/// [`crate::surreal::record_id`], so the key stays the native UUID
/// spelling every cross-engine `person_id` still addresses.
///
/// Minting its own key is also why this takes the mailbox claim first.
/// Two concurrent `create` calls for one mailbox collide on no shared
/// record key, so the UNIQUE `person_email_lower` index alone admits both
/// — the same fork [`find_or_create`] guards against, differing only in
/// that a loser here wants the refusal rather than the winner's row. See
/// [`CLAIM`].
///
/// # Errors
///
/// [`PersonError::EmailTaken`] when another row already holds this
/// mailbox case-insensitively, [`PersonError::OidcSubjectTaken`] when
/// another row is already linked to this IdP identity, and
/// [`PersonError::Db`] for anything else.
pub async fn create(db: &SurrealDb, input: &NewPerson) -> Result<Person, PersonError> {
    let id = Uuid::now_v7();
    let email_lower = input.email.trim().to_lowercase();
    if !take_mailbox(db, &email_lower, id).await? {
        return Err(PersonError::EmailTaken);
    }
    match write_row(db, id, input).await {
        Ok(person) => Ok(person),
        Err(error) => {
            // The claim outliving a row that never landed would lock this
            // mailbox out for good — including on the `OidcSubjectTaken`
            // path, where nothing was wrong with the mailbox at all.
            release_mailbox(db, &email_lower, id).await;
            Err(error)
        }
    }
}

/// Write the person row itself under `id`. The mailbox claim is the
/// caller's business — every caller has already taken it.
async fn write_row(db: &SurrealDb, id: Uuid, input: &NewPerson) -> Result<Person, PersonError> {
    let response = writing(|| {
        db.query(format!(
            "CREATE $id SET \
             name = $name, \
             email = $email, \
             role = $role, \
             given_name = $given_name, \
             family_name = $family_name, \
             middle_name = $middle_name, \
             oidc_subject = $oidc_subject, \
             title = $title, \
             phone = $phone, \
             profile_image_url = $profile_image_url \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("name", input.name.trim().to_string()))
        .bind(("email", input.email.trim().to_string()))
        .bind(("role", input.role.as_str().to_string()))
        .bind(("given_name", input.given_name.clone()))
        .bind(("family_name", input.family_name.clone()))
        .bind(("middle_name", input.middle_name.clone()))
        .bind(("oidc_subject", input.oidc_subject.clone()))
        .bind(("title", input.title.clone()))
        .bind(("phone", input.phone.clone()))
        .bind(("profile_image_url", input.profile_image_url.clone()))
    })
    .await?;

    one(response)?.ok_or(PersonError::WriteReturnedNothing)
}

/// The person holding this mailbox, creating them if nobody does.
///
/// The canonical seed runs on every boot and every `navigator db list`,
/// so two processes can start together — and `person.role` is the
/// authorization root, so a mailbox that forks into two rows is one human
/// carrying two roles.
///
/// Identity is settled by **claiming the mailbox as its own committed
/// statement** before the person row is written. Nothing here probes for
/// the mailbox first, because that is the shape that forked: every racer
/// minted its own record id and read `email_lower` through a scan, so no
/// two racers touched a key the optimistic layer could conflict on and
/// each committed a row of its own. The claim gives them one identical
/// record key to collide on. See [`CLAIM`] for why it may not be folded
/// back into a transaction with the person write, and
/// `store/tests/person_mailbox_race.rs` for the fork reproduced against
/// the old shape.
///
/// A loser is not refused: it reads the claim's holder by direct record
/// read and returns that person.
///
/// Only the mailbox is matched. A caller that also needs the name or role
/// to be right brings them up to date itself — this settles identity, not
/// content.
///
/// # Errors
///
/// [`PersonError::OidcSubjectTaken`] when `input` carries an IdP identity
/// another person already holds — a real conflict, not a race —
/// [`PersonError::WriteReturnedNothing`] when a claim never resolves into
/// a readable person inside the write budget, and [`PersonError::Db`] for
/// anything else.
pub async fn find_or_create(db: &SurrealDb, input: &NewPerson) -> Result<Person, PersonError> {
    let email_lower = input.email.trim().to_lowercase();
    let deadline = tokio::time::Instant::now() + retry::WRITE_BUDGET;
    let mut backoff = SETTLE_FIRST_BACKOFF;

    loop {
        let id = Uuid::now_v7();
        if take_mailbox(db, &email_lower, id).await? {
            return match write_row(db, id, input).await {
                Ok(person) => Ok(person),
                Err(error) => {
                    release_mailbox(db, &email_lower, id).await;
                    // The UNIQUE index refused a row that holds this
                    // mailbox without holding its claim — one written
                    // before the claim table existed. The backstop caught
                    // it, and the row it protected is the answer.
                    if matches!(error, PersonError::EmailTaken) {
                        find_by_email_ci(db, &input.email)
                            .await?
                            .ok_or(PersonError::WriteReturnedNothing)
                    } else {
                        Err(error)
                    }
                }
            };
        }

        // Another row holds the claim. Read the holder by record id —
        // never by scanning `email_lower`, which is what forked.
        if let Some(holder) = mailbox_holder(db, &email_lower).await? {
            if let Some(person) = find_by_id(db, holder).await? {
                return Ok(person);
            }
        }

        // Either the claim was released between the refusal and the read,
        // or the winner's person row has not committed yet — the claim
        // commits first, on its own, so that window is inherent to the
        // mechanism rather than a fault. Both resolve on their own, so
        // wait and look again. Bounded by the same wall-clock budget every
        // contended write in this crate runs under, so a claim genuinely
        // stranded by a crash surfaces as a slow error rather than a hang.
        if tokio::time::Instant::now() >= deadline {
            return Err(PersonError::WriteReturnedNothing);
        }
        tokio::time::sleep(rand::random_range(std::time::Duration::ZERO..=backoff)).await;
        backoff = (backoff * 2).min(SETTLE_BACKOFF_CEILING);
    }
}

/// Apply an update statement to one person, returning the row as it now
/// stands or `None` when no such person exists.
///
/// `UPDATE` never creates: the engine checks the record exists before it
/// touches anything, so a stale cross-engine `person_id` updates nothing
/// and reads back as `None`. That is `UPSERT`'s job, and nothing in this
/// module reaches for it — a person is created only through [`create`],
/// which mints its own key. See
/// [`an_update_never_creates_the_person_upsert_would_have`].
async fn update_one(
    db: &SurrealDb,
    id: Uuid,
    assignments: &str,
    bindings: Vec<(&'static str, surrealdb::types::Value)>,
) -> Result<Option<Person>, PersonError> {
    let response = writing(|| {
        let mut query = db
            .query(format!(
                "UPDATE person SET {assignments}, updated_at = time::now() \
                 WHERE id = $id RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)));
        // Rebound each attempt: awaiting a `Query` consumes it, so a
        // retry builds a fresh one rather than reusing a spent handle.
        for binding in bindings.iter().cloned() {
            query = query.bind(binding);
        }
        query
    })
    .await?;
    one(response)
}

/// A bound value, in the shape [`update_one`] collects them.
fn bind<T: SurrealValue>(name: &'static str, value: T) -> (&'static str, surrealdb::types::Value) {
    (name, value.into_value())
}

/// Edit a person's directory fields. `None` on a field leaves the column
/// alone; a present-but-`None` structured name part clears it. Returns
/// `None` when the person no longer exists.
///
/// This is the persistence half of the People command boundary — the
/// bootstrap-owner guard, the authority-ladder check, and the validation
/// live in [`crate::people_commands`], which calls this once it has
/// decided the edit is allowed.
///
/// `email_lower` is a computed `VALUE string::lowercase(email)` field, so
/// changing the email changes which mailbox this row holds — and the
/// claim that says so has to move with it. A claim left on the old
/// mailbox would lock it out for good, and one never taken on the new
/// mailbox would leave that mailbox forkable. Losing the race for the new
/// mailbox is [`PersonError::EmailTaken`], the same answer the UNIQUE
/// index gives, rather than a database fault.
///
/// # Errors
///
/// [`PersonError::EmailTaken`] when the new email belongs to another
/// row, and [`PersonError::Db`] for anything else.
pub async fn edit(
    db: &SurrealDb,
    id: Uuid,
    input: &PersonEdit,
) -> Result<Option<Person>, PersonError> {
    let mut assignments: Vec<&str> = Vec::new();
    if input.name.is_some() {
        assignments.push("name = $name");
    }
    if input.email.is_some() {
        assignments.push("email = $email");
    }
    if input.role.is_some() {
        assignments.push("role = $role");
    }
    if input.given_name.is_some() {
        assignments.push("given_name = $given_name");
    }
    if input.family_name.is_some() {
        assignments.push("family_name = $family_name");
    }
    if input.middle_name.is_some() {
        assignments.push("middle_name = $middle_name");
    }
    if input.profile_image_url.is_some() {
        assignments.push("profile_image_url = $profile_image_url");
    }
    if assignments.is_empty() {
        return find_by_id(db, id).await;
    }

    // The mailbox this edit moves away from, and the one it moves to —
    // `None` when the edit does not touch the email, or names the mailbox
    // this row already holds.
    let moving = match input
        .email
        .as_ref()
        .map(|email| email.trim().to_lowercase())
    {
        Some(next) => {
            let Some(current) = find_by_id(db, id).await? else {
                return Ok(None);
            };
            let previous = current.email.to_lowercase();
            if previous == next {
                None
            } else {
                Some((previous, next))
            }
        }
        None => None,
    };

    if let Some((_, next)) = &moving {
        if !take_mailbox(db, next, id).await? {
            return Err(PersonError::EmailTaken);
        }
    }

    let edited = update_one(
        db,
        id,
        &assignments.join(", "),
        vec![
            bind("name", input.name.as_ref().map(|v| v.trim().to_string())),
            bind("email", input.email.as_ref().map(|v| v.trim().to_string())),
            bind("role", input.role.map(|role| role.as_str().to_string())),
            bind("given_name", input.given_name.clone().unwrap_or_default()),
            bind("family_name", input.family_name.clone().unwrap_or_default()),
            bind("middle_name", input.middle_name.clone().unwrap_or_default()),
            bind(
                "profile_image_url",
                input.profile_image_url.clone().unwrap_or_default(),
            ),
        ],
    )
    .await;

    if let Some((previous, next)) = &moving {
        // The claim follows the row: the old mailbox is freed only once
        // the row has actually moved off it, and the new one is given back
        // if it did not.
        if matches!(edited, Ok(Some(_))) {
            release_mailbox(db, previous, id).await;
        } else {
            release_mailbox(db, next, id).await;
        }
    }

    edited
}

/// Set a person's authority tier. Returns `None` when the person no
/// longer exists.
///
/// # Errors
///
/// [`PersonError::Db`] if the write fails.
pub async fn set_role(db: &SurrealDb, id: Uuid, role: Role) -> Result<Option<Person>, PersonError> {
    update_one(
        db,
        id,
        "role = $role",
        vec![bind("role", role.as_str().to_string())],
    )
    .await
}

/// Cache the Xero `ContactID` on a person. No-op (`Ok(None)`) when the
/// person row no longer exists. Idempotent: re-setting the same id just
/// bumps `updated_at`.
///
/// # Errors
///
/// [`PersonError::Db`] if the write fails.
pub async fn set_xero_contact_id(
    db: &SurrealDb,
    id: Uuid,
    xero_contact_id: &str,
) -> Result<Option<Person>, PersonError> {
    update_one(
        db,
        id,
        "xero_contact_id = $xero",
        vec![bind("xero", xero_contact_id.to_string())],
    )
    .await
}

/// Link a person to the IdP identity that just authenticated as them.
/// Returns `None` when the person no longer exists.
///
/// # Errors
///
/// [`PersonError::OidcSubjectTaken`] when another row already holds this
/// `sub`, and [`PersonError::Db`] for anything else.
pub async fn link_oidc_subject(
    db: &SurrealDb,
    id: Uuid,
    subject: &str,
) -> Result<Option<Person>, PersonError> {
    update_one(
        db,
        id,
        "oidc_subject = $subject",
        vec![bind("subject", subject.to_string())],
    )
    .await
}

/// Apply a directory import's contact facts: the display name and the
/// two free-text contact columns. Deliberately cannot reach `email` or
/// `role` — a re-import is authoritative for how to reach someone, never
/// for who they are or what they may do.
///
/// # Errors
///
/// [`PersonError::Db`] if the write fails.
pub async fn update_contact(
    db: &SurrealDb,
    id: Uuid,
    input: &ContactUpdate,
) -> Result<Option<Person>, PersonError> {
    update_one(
        db,
        id,
        "name = $name, title = $title, phone = $phone",
        vec![
            bind("name", input.name.trim().to_string()),
            bind("title", input.title.clone()),
            bind("phone", input.phone.clone()),
        ],
    )
    .await
}

/// Remove a person. Idempotent: deleting one that is not there is a
/// no-op.
///
/// Whether a person *may* be deleted — only clients, never the bootstrap
/// Owner — is [`crate::people_commands::delete_person`]'s question, asked
/// before this is called.
///
/// Deleting the row releases its mailbox claims, in that order. A claim
/// that outlived its person would lock that mailbox out of ever being
/// used again — nothing could create the next person to hold it, and
/// [`find_or_create`] would wait out the write budget on a row that is
/// never coming. The release is not best-effort for that reason: a
/// failure to free the mailbox is the caller's to see.
///
/// # Errors
///
/// [`PersonError::Db`] if the delete or the release fails.
pub async fn delete(db: &SurrealDb, id: Uuid) -> Result<(), PersonError> {
    writing(|| db.query("DELETE $id").bind(("id", record_id(TABLE, id)))).await?;
    writing(|| db.query(RELEASE_PERSON).bind(("id", record_id(TABLE, id)))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        create, default_firm_dri, delete, edit, find_by_email_ci, find_by_id, find_by_ids,
        find_by_oidc_subject, find_or_create, link_oidc_subject, list_directory, retry, search,
        set_role, set_xero_contact_id, update_contact, ContactUpdate, NewPerson, PersonEdit,
        PersonError, Role,
    };
    use crate::surreal::test_support::mem;
    use crate::surreal::{record_id, SurrealDb};
    use uuid::Uuid;

    /// How many writers the contention assertions race for one record.
    ///
    /// Not a round number for its own sake: it is the herd size at which
    /// a counted five-attempt budget gives up on about 21% of individual
    /// writes, so at least one racer is refused on essentially every
    /// run. Smaller herds fail only intermittently, which is why a
    /// budget that does not scale with contention reads as a flake
    /// rather than as the policy defect it is.
    const CONTENDED_WRITERS: usize = 32;

    async fn person(db: &SurrealDb, name: &str, email: &str) -> super::Person {
        create(db, &NewPerson::new(name, email)).await.unwrap()
    }

    async fn person_at(db: &SurrealDb, name: &str, email: &str, role: Role) -> super::Person {
        create(db, &NewPerson::with_role(name, email, role))
            .await
            .unwrap()
    }

    #[test]
    fn lawyer_and_clerk_tiers_are_disjoint() {
        assert!(Role::Owner.is_lawyer_tier());
        assert!(Role::Owner.is_admin_tier());
        assert!(Role::Owner.is_owner());
        assert_eq!(Role::Owner.authority_rank(), 4);
        assert!(Role::Admin.is_admin_tier());
        assert!(!Role::Admin.is_owner());
        assert!(Role::Lawyer.is_lawyer_tier());
        assert!(Role::Admin.is_lawyer_tier());
        assert!(!Role::Clerk.is_lawyer_tier());
        assert!(Role::Clerk.is_clerk());
        assert!(!Role::Client.is_clerk());
    }

    #[test]
    fn every_role_round_trips_through_its_stored_spelling() {
        // The stored spelling is what the schema's `ASSERT $value IN
        // [...]` names and what `PersonRow` reads back, so a role that
        // did not round-trip would be a row this module wrote and then
        // could not load.
        for role in [
            Role::Owner,
            Role::Admin,
            Role::Lawyer,
            Role::Clerk,
            Role::Client,
        ] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }
        assert_eq!(Role::parse("wizard"), None);
        assert_eq!(Role::default(), Role::Client);
    }

    #[tokio::test]
    async fn a_created_person_reads_back_by_id_and_by_email() {
        let db = mem().await;
        let created = create(
            &db,
            &NewPerson {
                given_name: Some("Libra".into()),
                family_name: Some("Scales".into()),
                title: Some("Executive Director".into()),
                phone: Some("+1-555-0100".into()),
                ..NewPerson::with_role("Libra Scales", "libra@example.com", Role::Lawyer)
            },
        )
        .await
        .unwrap();

        assert_eq!(created.role, Role::Lawyer);
        assert_eq!(created.given_name.as_deref(), Some("Libra"));
        assert_eq!(created.title.as_deref(), Some("Executive Director"));
        assert!(created.oidc_subject.is_none());
        assert!(created.xero_contact_id.is_none());

        assert_eq!(find_by_id(&db, created.id).await.unwrap(), Some(created));
    }

    #[tokio::test]
    async fn create_trims_the_name_and_email() {
        let db = mem().await;
        let row = person(&db, "  Libra ", "  libra@example.com ").await;
        assert_eq!(row.name, "Libra");
        assert_eq!(row.email, "libra@example.com");
    }

    #[tokio::test]
    async fn create_defaults_the_role_to_client() {
        let db = mem().await;
        assert_eq!(
            person(&db, "Libra", "libra@example.com").await.role,
            Role::Client
        );
    }

    #[tokio::test]
    async fn find_by_id_is_none_for_a_person_who_never_existed() {
        let db = mem().await;
        assert!(find_by_id(&db, Uuid::now_v7()).await.unwrap().is_none());
    }

    /// The auth path's whole contract: an IdP may present a casing the
    /// stored row does not have, and it must still resolve.
    #[tokio::test]
    async fn email_matches_case_insensitively_in_both_directions() {
        let db = mem().await;
        let stored = person(&db, "Attorney", "Attorney@Example.com").await;

        for probe in [
            "attorney@example.com",
            "ATTORNEY@EXAMPLE.COM",
            "Attorney@Example.com",
            "  attorney@example.com  ",
        ] {
            assert_eq!(
                find_by_email_ci(&db, probe).await.unwrap().map(|p| p.id),
                Some(stored.id),
                "{probe} did not resolve to the stored row"
            );
        }
        assert!(find_by_email_ci(&db, "someone@example.com")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn a_duplicate_email_is_reported_as_the_email_being_taken() {
        let db = mem().await;
        person(&db, "Libra", "dup@example.com").await;

        // Byte-identical, and the case variant the `email_lower` index
        // exists to catch. Both are the same mailbox.
        for duplicate in ["dup@example.com", "DUP@Example.com"] {
            let refused = create(&db, &NewPerson::new("Other", duplicate)).await;
            assert!(
                matches!(refused, Err(PersonError::EmailTaken)),
                "{duplicate} was not classified as a taken email: {refused:?}"
            );
        }
    }

    /// A write that loses an optimistic-concurrency race is retried, not
    /// surfaced.
    ///
    /// SurrealDB's key-value layer is optimistic: concurrent
    /// read-modify-write passes over one record race, the loser is rolled
    /// back, and the engine says so — with a message that ends "This
    /// transaction can be retried". Nothing about the statement was
    /// wrong, so surfacing it would make two simultaneous saves look like
    /// a database fault to whoever lost. That is what the cucumber suite
    /// hit, since its scenarios share one engine.
    ///
    /// Contention on ONE record, not many: separate records touch
    /// separate keys and do not conflict. That is what makes this a test
    /// of the retry rather than of concurrency in general.
    ///
    /// [`CONTENDED_WRITERS`] is why it is a test of the retry *policy*
    /// as well. A counted five-attempt budget drains a herd of eight
    /// about 98% of the time, so at that size the property holds on most
    /// runs and the rest reads as noise. At the herd size below such a
    /// budget fails essentially every run, which is what a property is
    /// supposed to look like when it does not hold.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_write_that_loses_an_optimistic_race_is_retried_not_surfaced() {
        let db = mem().await;
        let subject = person(&db, "Contended", "contended@example.com").await.id;
        let roles = [
            Role::Owner,
            Role::Admin,
            Role::Lawyer,
            Role::Clerk,
            Role::Client,
        ];

        let writes: Vec<_> = (0..CONTENDED_WRITERS)
            .map(|n| {
                let db = db.clone();
                let role = roles[n % roles.len()];
                tokio::spawn(async move { set_role(&db, subject, role).await })
            })
            .collect();

        for (n, write) in writes.into_iter().enumerate() {
            let result = write.await.expect("write task");
            assert!(
                result.is_ok(),
                "concurrent write {n} was surfaced instead of retried: {result:?}",
            );
        }
    }

    /// [`find_or_create`] settles on one row when callers race for the
    /// same mailbox — the property the canonical seed depends on, since
    /// it runs on every boot and two processes can start together.
    ///
    /// Every racer must return the canonical row. What makes them agree
    /// is the `person_mailbox` claim: they collide on one identical
    /// *record* key, which the optimistic layer enforces, and the loser
    /// reads the winner's row back through that claim. Neither the UNIQUE
    /// `person_email_lower` index nor a shared transaction around the
    /// email probe does this — an index entry is not part of conflict
    /// detection, and a `WHERE email_lower = $v` probe is a scan, which
    /// does not enter the read set. That shape is what forked, and
    /// `store/tests/person_mailbox_race.rs` still races it as the
    /// control.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_find_or_create_for_one_mailbox_settles_on_one_row() {
        let db = mem().await;

        let racers: Vec<_> = (0..8)
            .map(|_| {
                let db = db.clone();
                tokio::spawn(async move {
                    find_or_create(&db, &NewPerson::new("Contested", "contested@example.com")).await
                })
            })
            .collect();

        let mut ids = std::collections::BTreeSet::new();
        for (n, racer) in racers.into_iter().enumerate() {
            let person = racer
                .await
                .expect("racer task")
                .unwrap_or_else(|e| panic!("racer {n} was refused instead of settling: {e:?}"));
            ids.insert(person.id);
        }

        assert_eq!(ids.len(), 1, "the racers disagreed about which row won");
        assert_eq!(
            list_directory(&db, "", "", &[]).await.unwrap().len(),
            1,
            "a race must not leave a second row behind",
        );
    }

    /// Deleting a person frees their mailbox, so the next person may hold
    /// it. The claim is what makes this a lifecycle rather than a
    /// one-way door: a claim left behind by a deleted row would lock that
    /// mailbox out of ever being created again.
    #[tokio::test]
    async fn deleting_a_person_frees_their_mailbox_for_the_next_one() {
        let db = mem().await;
        let outgoing = person(&db, "Outgoing", "reused@example.com").await;

        delete(&db, outgoing.id).await.expect("delete");
        assert_eq!(
            super::mailbox_holder(&db, "reused@example.com")
                .await
                .unwrap(),
            None,
            "the claim goes with the person",
        );

        let incoming = create(&db, &NewPerson::new("Incoming", "reused@example.com"))
            .await
            .expect("the freed mailbox may be claimed again");
        assert_ne!(incoming.id, outgoing.id);
        assert_eq!(
            super::mailbox_holder(&db, "reused@example.com")
                .await
                .unwrap(),
            Some(incoming.id),
        );
    }

    /// Editing the email moves the claim: the new mailbox becomes this
    /// row's, and the old one is freed for somebody else.
    ///
    /// `email_lower` is computed from `email`, so the mailbox this row
    /// holds changes the moment the edit lands — a claim that did not
    /// follow would either strand the old mailbox or leave the new one
    /// unguarded.
    #[tokio::test]
    async fn editing_the_email_moves_the_mailbox_claim() {
        let db = mem().await;
        let subject = person(&db, "Mover", "before@example.com").await;

        let moved = edit(
            &db,
            subject.id,
            &PersonEdit {
                email: Some("After@example.com".into()),
                ..PersonEdit::default()
            },
        )
        .await
        .expect("the edit is allowed")
        .expect("the row is still there");
        assert_eq!(moved.email, "After@example.com");

        assert_eq!(
            super::mailbox_holder(&db, "after@example.com")
                .await
                .unwrap(),
            Some(subject.id),
            "the claim followed the row, keyed on the lowercased mailbox",
        );
        assert_eq!(
            super::mailbox_holder(&db, "before@example.com")
                .await
                .unwrap(),
            None,
            "the mailbox it left is free",
        );

        let successor = create(&db, &NewPerson::new("Successor", "before@example.com"))
            .await
            .expect("the vacated mailbox may be claimed");
        assert_eq!(
            super::mailbox_holder(&db, "before@example.com")
                .await
                .unwrap(),
            Some(successor.id),
        );
    }

    /// An edit onto a mailbox another row holds is refused as
    /// [`PersonError::EmailTaken`] — the answer the UNIQUE index gave —
    /// not as a database fault, and it leaves both rows alone.
    #[tokio::test]
    async fn editing_onto_a_held_mailbox_is_refused_as_the_email_being_taken() {
        let db = mem().await;
        let holder = person(&db, "Holder", "held@example.com").await;
        let mover = person(&db, "Mover", "free@example.com").await;

        let refused = edit(
            &db,
            mover.id,
            &PersonEdit {
                email: Some("HELD@example.com".into()),
                ..PersonEdit::default()
            },
        )
        .await;
        assert!(
            matches!(refused, Err(PersonError::EmailTaken)),
            "a case variant of a held mailbox is the same mailbox, got {refused:?}",
        );
        assert_eq!(
            super::mailbox_holder(&db, "held@example.com")
                .await
                .unwrap(),
            Some(holder.id),
            "the original holder keeps its claim",
        );
        assert_eq!(
            find_by_id(&db, mover.id).await.unwrap().unwrap().email,
            "free@example.com",
            "the refused edit changed nothing",
        );
    }

    /// A failed create gives the mailbox back. Claiming before writing
    /// means a write that then fails for an unrelated reason — here a
    /// duplicate IdP identity — must not leave the mailbox claimed by a
    /// person who never existed.
    #[tokio::test]
    async fn a_create_that_fails_gives_the_mailbox_back() {
        let db = mem().await;
        create(
            &db,
            &NewPerson {
                oidc_subject: Some("sub-held".into()),
                ..NewPerson::new("First", "first@example.com")
            },
        )
        .await
        .expect("the first person links the identity");

        let refused = create(
            &db,
            &NewPerson {
                oidc_subject: Some("sub-held".into()),
                ..NewPerson::new("Second", "second@example.com")
            },
        )
        .await;
        assert!(
            matches!(refused, Err(PersonError::OidcSubjectTaken)),
            "the IdP identity is what collided, got {refused:?}",
        );
        assert_eq!(
            super::mailbox_holder(&db, "second@example.com")
                .await
                .unwrap(),
            None,
            "the mailbox the failed row claimed is free again",
        );
        create(&db, &NewPerson::new("Second", "second@example.com"))
            .await
            .expect("the mailbox is usable after the failed create");
    }

    /// A row that holds a mailbox without holding its claim — written
    /// before the claim table existed — is still found rather than
    /// refused. The UNIQUE index is the backstop that catches it, and the
    /// row it protected is the answer.
    #[tokio::test]
    async fn find_or_create_settles_on_a_row_that_predates_the_claim() {
        let db = mem().await;
        let unclaimed = Uuid::now_v7();
        db.query("CREATE $id SET name = 'Legacy', email = 'legacy@example.com', role = 'client'")
            .bind(("id", record_id(super::TABLE, unclaimed)))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect("seed a row with no claim");
        assert_eq!(
            super::mailbox_holder(&db, "legacy@example.com")
                .await
                .unwrap(),
            None,
            "the seeded row holds no claim",
        );

        let found = find_or_create(&db, &NewPerson::new("Legacy", "legacy@example.com"))
            .await
            .expect("the unclaimed row is the answer, not a refusal");
        assert_eq!(found.id, unclaimed);
        assert_eq!(
            list_directory(&db, "", "", &[]).await.unwrap().len(),
            1,
            "no second row was written",
        );
    }

    /// A pre-existing row is returned as it stands. `find_or_create`
    /// settles identity, not content: it must not quietly rewrite a name
    /// or demote a role to match what the caller happened to pass.
    #[tokio::test]
    async fn find_or_create_returns_the_existing_row_untouched() {
        let db = mem().await;
        let seeded = person_at(&db, "Original", "held@example.com", Role::Admin).await;

        let found = find_or_create(
            &db,
            &NewPerson::with_role("Different", "HELD@example.com", Role::Client),
        )
        .await
        .unwrap();

        assert_eq!(found.id, seeded.id, "the case variant is the same mailbox");
        assert_eq!(found.name, "Original");
        assert_eq!(
            found.role,
            Role::Admin,
            "an existing role must not be lowered"
        );
    }

    /// A timeout is not a lost race. The shared policy owns the
    /// predicate now, but this crate's most contended table is where a
    /// widened retryable set would do the most damage, so the negative
    /// case is pinned from here too.
    #[test]
    fn a_timeout_is_not_treated_as_a_lost_race() {
        let timed_out = surrealdb::Error::query(
            "Query timed out".to_string(),
            Some(surrealdb::types::QueryError::TimedOut {
                duration: std::time::Duration::from_secs(30),
            }),
        );
        assert!(
            !retry::is_retryable(&timed_out),
            "a timeout is not a lost race and must not be re-run",
        );
    }

    /// Why [`classify_write`] reads the message rather than the typed
    /// detail, held against the engine so a future SDK that *does* type
    /// this fails here and the workaround can go.
    ///
    /// `surrealdb_types::ErrorDetails` has an `AlreadyExists` variant, so
    /// "match structurally" looks available — but the engine raises the
    /// unique violation as `surrealdb_core::err::Error::IndexExists`
    /// through `bail!`, and nothing maps that variant onto the public
    /// detail. It arrives as the `Internal` catch-all. What survives is
    /// the message, whose `Database index \`{index}\`` prefix carries a
    /// name this workspace chose in `navigator.surql`.
    #[tokio::test]
    async fn a_unique_violation_carries_no_typed_detail_only_the_index_name() {
        use surrealdb::types::ErrorDetails;

        let db = mem().await;
        person(&db, "Libra", "dup@example.com").await;

        let raw = db
            .query("CREATE $id SET name = 'Other', email = 'dup@example.com', role = 'client'")
            .bind(("id", record_id(super::TABLE, Uuid::now_v7())))
            .await
            .and_then(surrealdb::IndexedResults::check)
            .expect_err("the second write must collide");

        assert!(
            matches!(raw.details(), ErrorDetails::Internal),
            "a typed detail is now available — classify_write should match on it \
             instead of the message; got {:?}",
            raw.details()
        );
        assert!(
            raw.to_string().contains("person_email_lower"),
            "the index name is the only discriminator, and it is gone: {raw}"
        );
    }

    /// The line every write in this module sits on: `UPDATE` never
    /// creates, `UPSERT` does.
    ///
    /// A stale cross-engine `person_id` reaching an update must change
    /// nothing rather than conjure a person, and what guarantees that is
    /// the statement chosen — `Document::update` checks the record exists
    /// before touching anything. Pinned as a pair so the difference is a
    /// tested fact rather than a remembered one: swap one keyword and a
    /// dangling reference silently becomes a row.
    #[tokio::test]
    async fn an_update_never_creates_the_person_upsert_would_have() {
        let db = mem().await;
        let ghost = Uuid::now_v7();

        db.query("UPDATE $id SET name = 'Conjured', email = 'conjured@example.com'")
            .bind(("id", record_id(super::TABLE, ghost)))
            .await
            .unwrap()
            .check()
            .expect("an update against a missing record is not an error");
        assert!(
            find_by_id(&db, ghost).await.unwrap().is_none(),
            "UPDATE must never create the person it was pointed at"
        );

        // The same statement as an UPSERT does create it — which is why
        // no write here is spelled that way.
        db.query("UPSERT $id SET name = 'Conjured', email = 'conjured@example.com'")
            .bind(("id", record_id(super::TABLE, ghost)))
            .await
            .unwrap()
            .check()
            .unwrap();
        assert!(
            find_by_id(&db, ghost).await.unwrap().is_some(),
            "UPSERT is the statement that creates — the distinction this module rests on"
        );
    }

    #[tokio::test]
    async fn a_duplicate_oidc_subject_is_reported_as_that_identity_being_linked() {
        let db = mem().await;
        create(
            &db,
            &NewPerson {
                oidc_subject: Some("sub-1".into()),
                ..NewPerson::new("Libra", "libra@example.com")
            },
        )
        .await
        .unwrap();

        let refused = create(
            &db,
            &NewPerson {
                oidc_subject: Some("sub-1".into()),
                ..NewPerson::new("Aries", "aries@example.com")
            },
        )
        .await;
        assert!(
            matches!(refused, Err(PersonError::OidcSubjectTaken)),
            "{refused:?}"
        );
    }

    /// The index is nullable-unique: seeded people not yet linked to an
    /// IdP must coexist.
    #[tokio::test]
    async fn several_people_may_be_unlinked_at_once() {
        let db = mem().await;
        person(&db, "Libra", "libra@example.com").await;
        person(&db, "Aries", "aries@example.com").await;
        person(&db, "Virgo", "virgo@example.com").await;

        assert_eq!(list_directory(&db, "", "", &[]).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_linked_subject_resolves_and_an_unknown_one_does_not() {
        let db = mem().await;
        let row = person(&db, "Libra", "libra@example.com").await;

        assert!(find_by_oidc_subject(&db, "sub-1").await.unwrap().is_none());
        let linked = link_oidc_subject(&db, row.id, "sub-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(linked.oidc_subject.as_deref(), Some("sub-1"));
        assert_eq!(
            find_by_oidc_subject(&db, "sub-1")
                .await
                .unwrap()
                .map(|p| p.id),
            Some(row.id)
        );
    }

    #[tokio::test]
    async fn linking_a_subject_another_person_holds_is_refused() {
        let db = mem().await;
        let first = person(&db, "Libra", "libra@example.com").await;
        let second = person(&db, "Aries", "aries@example.com").await;
        link_oidc_subject(&db, first.id, "sub-1").await.unwrap();

        let refused = link_oidc_subject(&db, second.id, "sub-1").await;
        assert!(
            matches!(refused, Err(PersonError::OidcSubjectTaken)),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn find_by_ids_returns_only_the_people_who_exist() {
        let db = mem().await;
        let a = person(&db, "Aries", "aries@example.com").await;
        let b = person(&db, "Libra", "libra@example.com").await;
        person(&db, "Virgo", "virgo@example.com").await;

        let found = find_by_ids(&db, &[a.id, Uuid::now_v7(), b.id])
            .await
            .unwrap();
        let mut ids: Vec<Uuid> = found.into_iter().map(|p| p.id).collect();
        ids.sort();
        let mut expected = vec![a.id, b.id];
        expected.sort();
        assert_eq!(ids, expected);

        assert!(find_by_ids(&db, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_default_firm_dri_walks_down_the_authority_ladder() {
        let db = mem().await;
        // Nobody firm-side at all.
        person(&db, "Client", "client@example.com").await;
        assert!(default_firm_dri(&db).await.unwrap().is_none());

        let lawyer = person_at(&db, "Stella", "stella@neonlaw.com", Role::Lawyer).await;
        assert_eq!(default_firm_dri(&db).await.unwrap(), Some(lawyer.id));

        let admin = person_at(&db, "Ada", "ada@neonlaw.com", Role::Admin).await;
        assert_eq!(default_firm_dri(&db).await.unwrap(), Some(admin.id));

        let owner = person_at(&db, "Ozzy", "ozzy@neonlaw.com", Role::Owner).await;
        assert_eq!(default_firm_dri(&db).await.unwrap(), Some(owner.id));

        // A Clerk is not firm-side for this purpose: it is outside the
        // lawyer tier and cannot be a matter's lawyer DRI.
        person_at(&db, "Clio", "clio@neonlaw.com", Role::Clerk).await;
        assert_eq!(default_firm_dri(&db).await.unwrap(), Some(owner.id));
    }

    #[tokio::test]
    async fn the_default_firm_dri_takes_the_lowest_id_within_a_tier() {
        let db = mem().await;
        let first = person_at(&db, "First", "first@neonlaw.com", Role::Lawyer).await;
        let second = person_at(&db, "Second", "second@neonlaw.com", Role::Lawyer).await;
        assert!(first.id < second.id, "v7 ids are ordered by creation");
        assert_eq!(default_firm_dri(&db).await.unwrap(), Some(first.id));
    }

    #[tokio::test]
    async fn the_directory_sorts_by_name_and_filters_case_insensitively() {
        let db = mem().await;
        person(&db, "Sagittarius", "sagittarius@example.com").await;
        person(&db, "Aquarius", "aquarius@neonlaw.com").await;
        person(&db, "Aries", "aries@example.com").await;

        let names = |rows: Vec<super::Person>| -> Vec<String> {
            rows.into_iter().map(|p| p.name).collect()
        };

        assert_eq!(
            names(list_directory(&db, "", "", &[]).await.unwrap()),
            vec!["Aquarius", "Aries", "Sagittarius"]
        );
        assert_eq!(
            names(list_directory(&db, "ARI", "", &[]).await.unwrap()),
            vec!["Aquarius", "Aries", "Sagittarius"]
        );
        assert_eq!(
            names(list_directory(&db, "", "NEONLAW", &[]).await.unwrap()),
            vec!["Aquarius"]
        );
    }

    #[tokio::test]
    async fn the_directory_honours_a_sort_key_and_ignores_an_unknown_one() {
        let db = mem().await;
        person(&db, "Aquarius", "zeta@example.com").await;
        person(&db, "Sagittarius", "alpha@example.com").await;

        let sorted = |rows: Vec<super::Person>| -> Vec<String> {
            rows.into_iter().map(|p| p.email).collect()
        };

        assert_eq!(
            sorted(
                list_directory(&db, "", "", &[("email".into(), false)])
                    .await
                    .unwrap()
            ),
            vec!["alpha@example.com", "zeta@example.com"]
        );
        assert_eq!(
            sorted(
                list_directory(&db, "", "", &[("name".into(), true)])
                    .await
                    .unwrap()
            ),
            vec!["alpha@example.com", "zeta@example.com"]
        );
        // An unadvertised key falls back to the default name order.
        assert_eq!(
            sorted(
                list_directory(&db, "", "", &[("shoe_size".into(), true)])
                    .await
                    .unwrap()
            ),
            vec!["zeta@example.com", "alpha@example.com"]
        );
    }

    #[tokio::test]
    async fn search_matches_substrings_ands_both_needles_and_respects_the_limit() {
        let db = mem().await;
        person(&db, "Aquarius", "aquarius@neonlaw.com").await;
        person(&db, "Aries", "aries@example.com").await;
        person(&db, "Sagittarius", "sagittarius@neonlaw.com").await;

        let names = |rows: Vec<super::Person>| -> Vec<String> {
            rows.into_iter().map(|p| p.name).collect()
        };

        assert_eq!(
            names(search(&db, Some("ARI"), None, 50).await.unwrap()),
            vec!["Aquarius", "Aries", "Sagittarius"]
        );
        assert_eq!(
            names(search(&db, Some("ari"), Some("neonlaw"), 50).await.unwrap()),
            vec!["Aquarius", "Sagittarius"]
        );
        assert_eq!(search(&db, Some("a"), None, 1).await.unwrap().len(), 1);
        assert!(search(&db, Some("ghost"), None, 50)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn an_edit_touches_only_the_fields_it_names() {
        let db = mem().await;
        let row = create(
            &db,
            &NewPerson {
                given_name: Some("Gemma".into()),
                family_name: Some("Twin".into()),
                title: Some("Director".into()),
                ..NewPerson::with_role("Gem", "gem@example.com", Role::Lawyer)
            },
        )
        .await
        .unwrap();

        let renamed = edit(
            &db,
            row.id,
            &PersonEdit {
                name: Some("Gemini".into()),
                ..PersonEdit::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(renamed.name, "Gemini");
        assert_eq!(renamed.email, "gem@example.com");
        assert_eq!(renamed.role, Role::Lawyer, "an unnamed field is preserved");
        assert_eq!(renamed.given_name.as_deref(), Some("Gemma"));
        assert_eq!(renamed.title.as_deref(), Some("Director"));
    }

    #[tokio::test]
    async fn an_edit_clears_a_present_but_empty_name_part() {
        let db = mem().await;
        let row = create(
            &db,
            &NewPerson {
                given_name: Some("Gemma".into()),
                family_name: Some("Twin".into()),
                ..NewPerson::new("Gem", "gem@example.com")
            },
        )
        .await
        .unwrap();

        let cleared = edit(
            &db,
            row.id,
            &PersonEdit {
                given_name: Some(None),
                ..PersonEdit::default()
            },
        )
        .await
        .unwrap()
        .unwrap();

        assert!(cleared.given_name.is_none(), "a present None clears it");
        assert_eq!(
            cleared.family_name.as_deref(),
            Some("Twin"),
            "an omitted part is untouched"
        );
    }

    #[tokio::test]
    async fn an_edit_onto_another_persons_email_is_refused() {
        let db = mem().await;
        person(&db, "Libra", "libra@example.com").await;
        let aries = person(&db, "Aries", "aries@example.com").await;

        let refused = edit(
            &db,
            aries.id,
            &PersonEdit {
                email: Some("LIBRA@example.com".into()),
                ..PersonEdit::default()
            },
        )
        .await;
        assert!(
            matches!(refused, Err(PersonError::EmailTaken)),
            "{refused:?}"
        );
    }

    /// Every write filters by id rather than addressing the record, so a
    /// stale reference updates nothing instead of conjuring a person.
    /// `UPDATE person:<id>` would have created one.
    #[tokio::test]
    async fn a_write_against_a_missing_person_is_a_no_op() {
        let db = mem().await;
        let ghost = Uuid::now_v7();

        assert!(edit(
            &db,
            ghost,
            &PersonEdit {
                name: Some("Ghost".into()),
                ..PersonEdit::default()
            }
        )
        .await
        .unwrap()
        .is_none());
        assert!(set_role(&db, ghost, Role::Owner).await.unwrap().is_none());
        assert!(set_xero_contact_id(&db, ghost, "x")
            .await
            .unwrap()
            .is_none());
        assert!(link_oidc_subject(&db, ghost, "sub")
            .await
            .unwrap()
            .is_none());
        assert!(update_contact(&db, ghost, &ContactUpdate::default())
            .await
            .unwrap()
            .is_none());

        assert!(
            list_directory(&db, "", "", &[]).await.unwrap().is_empty(),
            "a no-op write must not have created a row"
        );
    }

    #[tokio::test]
    async fn setting_a_role_moves_the_person_up_the_ladder() {
        let db = mem().await;
        let row = person(&db, "Stella", "stella@neonlaw.com").await;
        assert_eq!(row.role, Role::Client);

        let promoted = set_role(&db, row.id, Role::Lawyer).await.unwrap().unwrap();
        assert_eq!(promoted.role, Role::Lawyer);
        assert_eq!(
            find_by_id(&db, row.id).await.unwrap().unwrap().role,
            Role::Lawyer
        );
    }

    #[tokio::test]
    async fn set_xero_contact_id_caches_then_is_idempotent() {
        let db = mem().await;
        let row = person(&db, "Capricorn", "capricorn@example.com").await;
        assert!(row.xero_contact_id.is_none());

        let updated = set_xero_contact_id(&db, row.id, "xero-contact-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.xero_contact_id.as_deref(), Some("xero-contact-1"));

        // Re-set the same id — still one value, no error.
        let again = set_xero_contact_id(&db, row.id, "xero-contact-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.xero_contact_id.as_deref(), Some("xero-contact-1"));
    }

    #[tokio::test]
    async fn update_contact_replaces_the_contact_facts_and_nothing_else() {
        let db = mem().await;
        let row = person_at(&db, "Libra", "libra@example.com", Role::Lawyer).await;

        let updated = update_contact(
            &db,
            row.id,
            &ContactUpdate {
                name: "Libra Scales".into(),
                title: Some("Executive Director".into()),
                phone: Some("+1-555-0100".into()),
            },
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(updated.name, "Libra Scales");
        assert_eq!(updated.title.as_deref(), Some("Executive Director"));
        assert_eq!(updated.phone.as_deref(), Some("+1-555-0100"));
        assert_eq!(
            updated.email, "libra@example.com",
            "an import cannot move the mailbox"
        );
        assert_eq!(
            updated.role,
            Role::Lawyer,
            "an import cannot change authority"
        );
    }

    #[tokio::test]
    async fn deleting_removes_the_person_and_is_idempotent() {
        let db = mem().await;
        let row = person(&db, "Cleo", "cleo@example.com").await;

        delete(&db, row.id).await.unwrap();
        assert!(find_by_id(&db, row.id).await.unwrap().is_none());
        assert!(find_by_email_ci(&db, "cleo@example.com")
            .await
            .unwrap()
            .is_none());

        // Deleting again, and deleting one that never existed, are no-ops.
        delete(&db, row.id).await.unwrap();
        delete(&db, Uuid::now_v7()).await.unwrap();
    }

    /// A deleted mailbox is free again — the unique index must not keep
    /// it reserved after the row is gone.
    #[tokio::test]
    async fn a_deleted_email_can_be_reused() {
        let db = mem().await;
        let first = person(&db, "Cleo", "cleo@example.com").await;
        delete(&db, first.id).await.unwrap();

        let second = person(&db, "Cleo Again", "cleo@example.com").await;
        assert_ne!(second.id, first.id);
    }
}
