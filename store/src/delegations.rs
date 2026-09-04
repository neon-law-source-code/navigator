//! Recorded delegations: the fact that one client helps another with
//! their matters.
//!
//! A mailbox is how we reach you. It is not who you are, and it is not
//! permission to be someone else. `person.email` is required and
//! `person_email_lower` is UNIQUE, so today the address on a Person row
//! doubles as that Person's login identifier — which means a client with
//! no email address cannot be reached by the portal, and sharing a
//! relative's address would make one credential resolve to whichever row
//! it happened to match. This module is the deliberate alternative: the
//! link is written by a named firm member, on a stated basis, and it can
//! be revoked and audited.
//!
//! # This module grants nothing
//!
//! A [`Delegation`] records a relationship. It confers no read, no write,
//! and no session. No authorization path reads this table, the same
//! posture [`crate::external_identities`] holds, and
//! `store/tests/person_delegate_is_inert.rs` pins that against the
//! authorization surfaces by name.
//!
//! What a delegate may *do* is a separate decision that has not been
//! made. [`crate::access::MatterViewer::ClientDri`] carries plan
//! approval, so a session acting through a delegation must never reach it
//! without its own explicit, separately recorded grant. That is why there
//! is no `may_act` column: an unread permission flag is what gets wired
//! up carelessly later. [`Basis`] is the legal shape of the
//! relationship, not a capability.
//!
//! # Both sides must be clients
//!
//! A delegation is confined to [`Role::Client`] on both sides. Firm-side
//! people get exactly one profile: themselves. Without that confinement
//! the mechanism would let a clerk act as an admin, or one lawyer act as
//! another — privilege escalation across the firm dressed up as a
//! convenience for clients.
//!
//! The confinement is enforced twice, and the second time is the one that
//! matters. [`grant`] refuses a non-client on either side. But
//! `person.role` is written through three doors —
//! [`crate::persons::set_role`], [`crate::persons::edit`], and
//! [`crate::persons::create`] — so a cascade hooked to any one of them
//! would look like the control without being it. Instead
//! [`live_for_delegate`] and [`live_for_subject`] re-read both roles at
//! *read* time. That cannot be bypassed by a role-change door which does
//! not exist yet, nor by a direct database write.
//!
//! A grant whose parties are no longer both clients is **dormant**, which
//! is deliberately distinct from **revoked**: [`Delegation::state`]
//! reports which, so a surface can say "dormant because this person is
//! now firm-side" rather than silently hiding the row. A dormant grant
//! becomes live again if both parties are clients again; ending a
//! delegation for good is [`revoke`], which is a recorded act with a named
//! actor.
//!
//! A client's helper who works at the firm therefore cannot be linked.
//! That is correct rather than a limitation — it is a conflicts question
//! for a lawyer, not a portal feature.
//!
//! # The same defect exists at the other membership door
//!
//! ENG-492 records the mirror image of what [`grant`] refuses:
//! [`crate::firms::add_membership`] writes a `person_firm_role` row
//! without ever reading `person.role`, so a `client` can be given
//! firm-side visibility. Both are the same class — `person.role` going
//! unchecked at a membership write path — approached from opposite sides.
//!
//! This module deliberately does **not** fix that; it is tracked
//! separately. The refusal here is shaped to match, so the two can later
//! be folded into one shared "may this person hold this kind of
//! membership" predicate without either having to change its contract: a
//! typed error variant that names the person and the role that
//! disqualified them, refused at the write path before any row is
//! created.

use serde::Serialize;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::Role;
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

const TABLE: &str = "person_delegate";
const PERSON_TABLE: &str = "person";
const SELECT: &str = "id, delegate_person_id, subject_person_id, basis, \
                      instrument_reference, granted_by_person_id, granted_at, \
                      subject_notified_at, subject_notified_channel, \
                      revoked_at, revoked_by_person_id, inserted_at, updated_at";

/// The legal shape of a delegation — why one person may help another.
///
/// Not a capability. Each variant carries a different revocation trigger
/// in the world (a power of attorney ends on its own terms, a guardianship
/// on a court order, an assistant designation on a phone call), so a
/// boolean cannot hold them. Matches the `ASSERT` on
/// `person_delegate.basis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Basis {
    /// A helper with no legal instrument behind them — the spouse who
    /// reads the portal on behalf of a client who has no email address of
    /// their own. The narrowest basis, and the only one that asserts
    /// nothing about a document.
    Assistant,
    /// A power of attorney. Asserts a valid instrument exists, so
    /// [`NewDelegation::instrument_reference`] should name it.
    PowerOfAttorney,
    /// A court-appointed guardian.
    Guardian,
    /// The personal representative of an estate.
    Executor,
    /// A trustee acting for a trust's beneficiary.
    Trustee,
    /// A parent or legal guardian of a minor client.
    ParentOfMinor,
}

impl Basis {
    /// The stored spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::PowerOfAttorney => "power_of_attorney",
            Self::Guardian => "guardian",
            Self::Executor => "executor",
            Self::Trustee => "trustee",
            Self::ParentOfMinor => "parent_of_minor",
        }
    }

    /// The basis named by its stored spelling, or `None` for anything
    /// else. The inverse of [`Basis::as_str`].
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "assistant" => Some(Self::Assistant),
            "power_of_attorney" => Some(Self::PowerOfAttorney),
            "guardian" => Some(Self::Guardian),
            "executor" => Some(Self::Executor),
            "trustee" => Some(Self::Trustee),
            "parent_of_minor" => Some(Self::ParentOfMinor),
            _ => None,
        }
    }

    /// Whether this basis claims a legal instrument stands behind it.
    ///
    /// [`Basis::Assistant`] does not; every other variant does, and a row
    /// carrying one without an
    /// [`instrument_reference`](NewDelegation::instrument_reference) is
    /// asserting a legal fact about a client that the firm has not
    /// recorded reading. [`grant`] refuses that.
    #[must_use]
    pub fn claims_an_instrument(self) -> bool {
        !matches!(self, Self::Assistant)
    }
}

/// How the subject was told their delegation exists.
///
/// Recorded rather than assumed, because a subject with no mailbox cannot
/// be notified by email, and a grant whose only notice channel is the
/// *grantee's* mailbox leaves the subject structurally unable to learn
/// about their own grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum NoticeChannel {
    InPerson,
    Telephone,
    Post,
    Email,
}

impl NoticeChannel {
    /// The stored spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InPerson => "in_person",
            Self::Telephone => "telephone",
            Self::Post => "post",
            Self::Email => "email",
        }
    }

    /// The channel named by its stored spelling, or `None` for anything
    /// else.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "in_person" => Some(Self::InPerson),
            "telephone" => Some(Self::Telephone),
            "post" => Some(Self::Post),
            "email" => Some(Self::Email),
            _ => None,
        }
    }
}

/// Whether a recorded delegation is currently in force.
///
/// Three states rather than two, because "withdrawn by the firm" and "not
/// currently permissible" are different facts and a surface should be able
/// to say which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DelegationState {
    /// In force: not revoked, and both parties are still clients.
    Live,
    /// Withdrawn on a recorded date by a recorded actor. Terminal — a
    /// revoked row is never re-activated; a new [`grant`] writes a new row.
    Revoked,
    /// Not revoked, but at least one party is no longer
    /// [`Role::Client`], so the delegation may not be acted on. Becomes
    /// [`DelegationState::Live`] again if both are clients again.
    Dormant,
}

/// One recorded delegation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Delegation {
    pub id: Uuid,
    /// The client who may help.
    pub delegate_person_id: Uuid,
    /// The client being helped.
    pub subject_person_id: Uuid,
    pub basis: Basis,
    /// The document a non-[`Basis::Assistant`] basis rests on, as the
    /// attorney recorded it.
    pub instrument_reference: Option<String>,
    /// The firm member who wrote this row.
    pub granted_by_person_id: Uuid,
    pub granted_at: String,
    /// When the subject was told, if they have been.
    pub subject_notified_at: Option<String>,
    pub subject_notified_channel: Option<NoticeChannel>,
    pub revoked_at: Option<String>,
    pub revoked_by_person_id: Option<Uuid>,
    pub inserted_at: String,
    pub updated_at: String,
}

impl Delegation {
    /// Whether this row has been withdrawn.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Whether the subject has been told this delegation exists.
    ///
    /// A live delegation the subject has not been told about is not a bug
    /// in this module — the firm may write the row before the telephone
    /// call — but it is a thing a surface should be able to surface.
    #[must_use]
    pub fn subject_was_notified(&self) -> bool {
        self.subject_notified_at.is_some()
    }

    /// This row's state given both parties' current roles.
    ///
    /// Revocation wins over dormancy: a withdrawn delegation stays
    /// withdrawn whatever the roles later become.
    #[must_use]
    pub fn state(&self, delegate_role: Role, subject_role: Role) -> DelegationState {
        if self.is_revoked() {
            DelegationState::Revoked
        } else if delegate_role == Role::Client && subject_role == Role::Client {
            DelegationState::Live
        } else {
            DelegationState::Dormant
        }
    }
}

/// The fields a new delegation needs.
#[derive(Debug, Clone)]
pub struct NewDelegation {
    /// The client who may help. Must be [`Role::Client`].
    pub delegate_person_id: Uuid,
    /// The client being helped. Must be [`Role::Client`], and must not be
    /// the delegate.
    pub subject_person_id: Uuid,
    pub basis: Basis,
    /// Required for every basis except [`Basis::Assistant`]: a row
    /// claiming a power of attorney asserts a legal fact about a client,
    /// and the firm should name what it read before asserting it.
    pub instrument_reference: Option<String>,
    /// The firm member recording this. Not required to be a client — this
    /// is the one person id on the row that is expected to be firm-side.
    pub granted_by_person_id: Uuid,
    /// When and how the subject was told, if they have been already.
    pub subject_notified_at: Option<String>,
    pub subject_notified_channel: Option<NoticeChannel>,
}

/// Errors from the delegation seam.
#[derive(Debug, thiserror::Error)]
pub enum DelegationError {
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    #[error(transparent)]
    Person(#[from] crate::persons::PersonError),
    #[error("writing a delegation returned no usable row")]
    WriteReturnedNothing,
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    #[error("no delegation {0}")]
    NoSuchDelegation(Uuid),
    /// The delegate is not a client. Firm-side people get exactly one
    /// profile: themselves.
    #[error("person {0} is a {1} and only a client may be a delegate")]
    DelegateNotClient(Uuid, &'static str),
    /// The subject is not a client. Same rule, other side — a delegation
    /// must never be a route into a firm-side account.
    #[error("person {0} is a {1} and only a client may be a delegation subject")]
    SubjectNotClient(Uuid, &'static str),
    #[error("a person cannot be their own delegate")]
    SelfDelegation,
    #[error("basis {0} asserts a legal instrument, so instrument_reference is required")]
    InstrumentReferenceRequired(&'static str),
    #[error("that pair already has a live delegation")]
    AlreadyGranted,
    #[error("delegation {0} is already revoked")]
    AlreadyRevoked(Uuid),
}

#[derive(SurrealValue)]
struct DelegationRow {
    id: surrealdb::types::RecordId,
    delegate_person_id: surrealdb::types::RecordId,
    subject_person_id: surrealdb::types::RecordId,
    basis: String,
    instrument_reference: Option<String>,
    granted_by_person_id: surrealdb::types::RecordId,
    granted_at: String,
    subject_notified_at: Option<String>,
    subject_notified_channel: Option<String>,
    revoked_at: Option<String>,
    revoked_by_person_id: Option<surrealdb::types::RecordId>,
    inserted_at: String,
    updated_at: String,
}

impl DelegationRow {
    /// `None` when a stored row cannot be read as a [`Delegation`] — a
    /// non-UUID record key, or a `basis` outside the closed set. Both mean
    /// a row written around this module rather than through it, and the
    /// safe reading of an unreadable delegation is that there isn't one.
    fn into_delegation(self) -> Option<Delegation> {
        Some(Delegation {
            id: record_uuid(&self.id)?,
            delegate_person_id: record_uuid(&self.delegate_person_id)?,
            subject_person_id: record_uuid(&self.subject_person_id)?,
            basis: Basis::parse(&self.basis)?,
            instrument_reference: self.instrument_reference,
            granted_by_person_id: record_uuid(&self.granted_by_person_id)?,
            granted_at: self.granted_at,
            subject_notified_at: self.subject_notified_at,
            subject_notified_channel: self
                .subject_notified_channel
                .as_deref()
                .and_then(NoticeChannel::parse),
            revoked_at: self.revoked_at,
            revoked_by_person_id: self.revoked_by_person_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at,
            updated_at: self.updated_at,
        })
    }
}

/// Retry a write the same way every other store module does.
///
/// Unlike [`crate::firms`] there is no index violation to classify: the
/// pair index is deliberately not `UNIQUE` (a revoked row stays on the
/// table), so the duplicate-grant refusal is a read-back in [`grant`]
/// rather than an engine error to translate.
async fn writing<F, Q>(attempt: F) -> Result<surrealdb::IndexedResults, DelegationError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt).await.map_err(DelegationError::Db)
}

/// The role of a person who must exist, or [`DelegationError::NoSuchPerson`].
async fn role_of(surreal: &SurrealDb, id: Uuid) -> Result<Role, DelegationError> {
    crate::persons::find_by_id(surreal, id)
        .await?
        .map(|person| person.role)
        .ok_or(DelegationError::NoSuchPerson(id))
}

/// Record that one client may help another.
///
/// Refuses a non-client on either side, self-delegation, a basis that
/// claims an instrument without naming one, and a second live delegation
/// for a pair that already has one.
///
/// The pair index is deliberately not `UNIQUE` — a revoked row stays on
/// the table so a pair may hold several rows over time — so the live-grant
/// refusal is a read-back rather than an index violation, the same
/// convention `authority_use` documents. Two concurrent grants for one
/// pair can therefore both pass the check and land two live rows. That is
/// a duplicate of the same permission rather than an escalation, and
/// [`live_for_delegate`] returns both; it is not worth a lock.
///
/// # Errors
///
/// See [`DelegationError`]. Notably [`DelegationError::DelegateNotClient`]
/// and [`DelegationError::SubjectNotClient`] — the write-path half of the
/// client-only confinement, whose read-path half is in
/// [`live_for_delegate`].
pub async fn grant(
    surreal: &SurrealDb,
    input: &NewDelegation,
) -> Result<Delegation, DelegationError> {
    if input.delegate_person_id == input.subject_person_id {
        return Err(DelegationError::SelfDelegation);
    }
    if input.basis.claims_an_instrument()
        && input
            .instrument_reference
            .as_ref()
            .is_none_or(|reference| reference.trim().is_empty())
    {
        return Err(DelegationError::InstrumentReferenceRequired(
            input.basis.as_str(),
        ));
    }

    let delegate_role = role_of(surreal, input.delegate_person_id).await?;
    if delegate_role != Role::Client {
        return Err(DelegationError::DelegateNotClient(
            input.delegate_person_id,
            delegate_role.as_str(),
        ));
    }
    let subject_role = role_of(surreal, input.subject_person_id).await?;
    if subject_role != Role::Client {
        return Err(DelegationError::SubjectNotClient(
            input.subject_person_id,
            subject_role.as_str(),
        ));
    }
    if crate::persons::find_by_id(surreal, input.granted_by_person_id)
        .await?
        .is_none()
    {
        return Err(DelegationError::NoSuchPerson(input.granted_by_person_id));
    }
    if !find_live_for_pair(surreal, input.delegate_person_id, input.subject_person_id)
        .await?
        .is_empty()
    {
        return Err(DelegationError::AlreadyGranted);
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "CREATE $id SET delegate_person_id = $delegate, \
                 subject_person_id = $subject, basis = $basis, \
                 instrument_reference = $instrument, \
                 granted_by_person_id = $granted_by, granted_at = $now, \
                 subject_notified_at = $notified_at, \
                 subject_notified_channel = $notified_channel, \
                 revoked_at = NONE, revoked_by_person_id = NONE, \
                 inserted_at = $now, updated_at = $now RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, Uuid::now_v7())))
            .bind((
                "delegate",
                record_id(PERSON_TABLE, input.delegate_person_id),
            ))
            .bind(("subject", record_id(PERSON_TABLE, input.subject_person_id)))
            .bind(("basis", input.basis.as_str().to_string()))
            .bind(("instrument", input.instrument_reference.clone()))
            .bind((
                "granted_by",
                record_id(PERSON_TABLE, input.granted_by_person_id),
            ))
            .bind(("notified_at", input.subject_notified_at.clone()))
            .bind((
                "notified_channel",
                input
                    .subject_notified_channel
                    .map(|channel| channel.as_str().to_string()),
            ))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<DelegationRow> = response.take(0)?;
    row.and_then(DelegationRow::into_delegation)
        .ok_or(DelegationError::WriteReturnedNothing)
}

/// Withdraw a delegation, naming who withdrew it and when.
///
/// Never a delete: "this link existed and was withdrawn on this date" is
/// the fact a later inquiry needs, and a deleted row answers nothing —
/// the same choice `person.is_admitted` makes. Revocation is terminal;
/// re-granting writes a new row.
///
/// `revoked_by_person_id` is not required to be a client: withdrawing is
/// expected to be a firm-side act, and — per the Legal Council — must not
/// require the credential holder.
///
/// # Errors
///
/// [`DelegationError::NoSuchDelegation`] if the row is gone,
/// [`DelegationError::AlreadyRevoked`] if it is already withdrawn.
pub async fn revoke(
    surreal: &SurrealDb,
    id: Uuid,
    revoked_by_person_id: Uuid,
) -> Result<Delegation, DelegationError> {
    let existing = find_by_id(surreal, id)
        .await?
        .ok_or(DelegationError::NoSuchDelegation(id))?;
    if existing.is_revoked() {
        return Err(DelegationError::AlreadyRevoked(id));
    }
    if crate::persons::find_by_id(surreal, revoked_by_person_id)
        .await?
        .is_none()
    {
        return Err(DelegationError::NoSuchPerson(revoked_by_person_id));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET revoked_at = $now, \
                 revoked_by_person_id = $revoked_by, updated_at = $now \
                 RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)))
            .bind(("revoked_by", record_id(PERSON_TABLE, revoked_by_person_id)))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<DelegationRow> = response.take(0)?;
    row.and_then(DelegationRow::into_delegation)
        .ok_or(DelegationError::WriteReturnedNothing)
}

/// Record that the subject was told about a delegation, and how.
///
/// Separate from [`grant`] because the telephone call may happen after the
/// row is written, and separate from consent because being told and
/// agreeing are different facts.
///
/// # Errors
///
/// [`DelegationError::NoSuchDelegation`] if the row is gone.
pub async fn record_subject_notice(
    surreal: &SurrealDb,
    id: Uuid,
    channel: NoticeChannel,
) -> Result<Delegation, DelegationError> {
    if find_by_id(surreal, id).await?.is_none() {
        return Err(DelegationError::NoSuchDelegation(id));
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut response = writing(|| {
        surreal
            .query(format!(
                "UPDATE $id SET subject_notified_at = $now, \
                 subject_notified_channel = $channel, updated_at = $now \
                 RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, id)))
            .bind(("channel", channel.as_str().to_string()))
            .bind(("now", now.clone()))
    })
    .await?;
    let row: Option<DelegationRow> = response.take(0)?;
    row.and_then(DelegationRow::into_delegation)
        .ok_or(DelegationError::WriteReturnedNothing)
}

/// One delegation by id, whatever its state.
///
/// # Errors
///
/// [`DelegationError::Db`] if the read fails.
pub async fn find_by_id(
    surreal: &SurrealDb,
    id: Uuid,
) -> Result<Option<Delegation>, DelegationError> {
    let mut response = surreal
        .query(format!("SELECT {SELECT} FROM ONLY $id LIMIT 1"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DelegationRow> = response.take(0)?;
    Ok(row.and_then(DelegationRow::into_delegation))
}

/// Every delegation row naming this person as the delegate, revoked and
/// dormant ones included. The administrative view.
///
/// # Errors
///
/// [`DelegationError::Db`] if the read fails.
pub async fn all_for_delegate(
    surreal: &SurrealDb,
    delegate_person_id: Uuid,
) -> Result<Vec<Delegation>, DelegationError> {
    rows_where(surreal, "delegate_person_id = $person", delegate_person_id).await
}

/// Every delegation row naming this person as the subject, revoked and
/// dormant ones included.
///
/// This is the answer to "who can act as me?", and it must stay
/// answerable without the subject's own credential — a client with no
/// mailbox cannot self-serve, so the firm reads this on their behalf.
///
/// # Errors
///
/// [`DelegationError::Db`] if the read fails.
pub async fn all_for_subject(
    surreal: &SurrealDb,
    subject_person_id: Uuid,
) -> Result<Vec<Delegation>, DelegationError> {
    rows_where(surreal, "subject_person_id = $person", subject_person_id).await
}

async fn rows_where(
    surreal: &SurrealDb,
    predicate: &str,
    person_id: Uuid,
) -> Result<Vec<Delegation>, DelegationError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE {predicate} ORDER BY granted_at"
        ))
        .bind(("person", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DelegationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(DelegationRow::into_delegation)
        .collect())
}

/// The delegations this person may currently act through.
///
/// This is the read-path half of the client-only confinement, and the half
/// that actually holds: it drops any row whose delegate or subject is no
/// longer [`Role::Client`], whatever wrote that role change. Because
/// `person.role` is written through [`crate::persons::set_role`],
/// [`crate::persons::edit`] *and* [`crate::persons::create`], a cascade
/// hooked to one of them would cover a third of the doors while looking
/// like the control. Re-reading the roles here cannot be bypassed by a
/// door that does not exist yet, nor by a direct database write.
///
/// Revoked rows are dropped too. Use [`all_for_delegate`] for the
/// administrative view that keeps them.
///
/// # Errors
///
/// [`DelegationError::Db`] if a read fails.
pub async fn live_for_delegate(
    surreal: &SurrealDb,
    delegate_person_id: Uuid,
) -> Result<Vec<Delegation>, DelegationError> {
    let delegate_role = role_of(surreal, delegate_person_id).await?;
    if delegate_role != Role::Client {
        return Ok(Vec::new());
    }
    retain_live(
        surreal,
        all_for_delegate(surreal, delegate_person_id).await?,
    )
    .await
}

/// The delegations currently in force over this person — who may act as
/// them right now.
///
/// The same read-time re-validation [`live_for_delegate`] performs.
///
/// # Errors
///
/// [`DelegationError::Db`] if a read fails.
pub async fn live_for_subject(
    surreal: &SurrealDb,
    subject_person_id: Uuid,
) -> Result<Vec<Delegation>, DelegationError> {
    let subject_role = role_of(surreal, subject_person_id).await?;
    if subject_role != Role::Client {
        return Ok(Vec::new());
    }
    retain_live(surreal, all_for_subject(surreal, subject_person_id).await?).await
}

/// Drop every row that is revoked or whose counterparties are not both
/// clients. Roles are read per row rather than cached: the set is small,
/// and a stale role here is the exact hole this function exists to close.
///
/// A counterparty who no longer exists makes the row **not live** rather
/// than making the whole read fail. `record<person>` is not validated
/// against a live row by the engine — the schema says so of every such
/// link — and [`crate::persons::delete`] can leave one dangling. Erroring
/// here would let one deleted counterparty make an unrelated client's
/// entire delegation list unreadable, which is an availability bug wearing
/// a safety costume: the safe reading of "may this missing person act?" is
/// no.
async fn retain_live(
    surreal: &SurrealDb,
    rows: Vec<Delegation>,
) -> Result<Vec<Delegation>, DelegationError> {
    let mut live = Vec::new();
    for row in rows {
        if row.is_revoked() {
            continue;
        }
        let (Some(delegate_role), Some(subject_role)) = (
            optional_role_of(surreal, row.delegate_person_id).await?,
            optional_role_of(surreal, row.subject_person_id).await?,
        ) else {
            continue;
        };
        if row.state(delegate_role, subject_role) == DelegationState::Live {
            live.push(row);
        }
    }
    Ok(live)
}

/// The role of a person who may or may not still exist.
async fn optional_role_of(surreal: &SurrealDb, id: Uuid) -> Result<Option<Role>, DelegationError> {
    Ok(crate::persons::find_by_id(surreal, id)
        .await?
        .map(|person| person.role))
}

/// The live delegations for one exact pair. Used by [`grant`] to refuse a
/// duplicate, and normally holding zero or one row.
///
/// # Errors
///
/// [`DelegationError::Db`] if a read fails.
pub async fn find_live_for_pair(
    surreal: &SurrealDb,
    delegate_person_id: Uuid,
    subject_person_id: Uuid,
) -> Result<Vec<Delegation>, DelegationError> {
    let mut response = surreal
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE delegate_person_id = $delegate \
             AND subject_person_id = $subject AND revoked_at IS NONE \
             ORDER BY granted_at"
        ))
        .bind(("delegate", record_id(PERSON_TABLE, delegate_person_id)))
        .bind(("subject", record_id(PERSON_TABLE, subject_person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DelegationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(DelegationRow::into_delegation)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persons::{self, NewPerson};
    use crate::test_support::mem_surreal;

    /// A client, the ordinary case — `NewPerson` defaults to [`Role::Client`].
    async fn client(db: &SurrealDb, name: &str, email: &str) -> Uuid {
        persons::create(db, &NewPerson::new(name, email))
            .await
            .unwrap()
            .id
    }

    async fn person_at(db: &SurrealDb, name: &str, email: &str, role: Role) -> Uuid {
        persons::create(db, &NewPerson::with_role(name, email, role))
            .await
            .unwrap()
            .id
    }

    /// The motivating shape: two clients on one matter, one of whom cannot
    /// receive email, and a firm member recording the arrangement.
    struct Household {
        delegate: Uuid,
        subject: Uuid,
        firm: Uuid,
    }

    async fn household(db: &SurrealDb) -> Household {
        Household {
            delegate: client(db, "Delegate Client", "delegate@example.com").await,
            subject: client(db, "Subject Client", "subject@example.com").await,
            firm: person_at(db, "Firm Lawyer", "lawyer@example.com", Role::Lawyer).await,
        }
    }

    /// Move a person to another tier through [`persons::set_role`], the
    /// first of the three doors onto the role column. Asserts the row
    /// survived, so the returned `Option` is consumed rather than dropped.
    async fn set_role_to(db: &SurrealDb, id: Uuid, role: Role) {
        persons::set_role(db, id, role)
            .await
            .unwrap()
            .expect("the person must still exist after a role change");
    }

    /// The same move through [`persons::edit`], the second door.
    async fn edit_role_to(db: &SurrealDb, id: Uuid, role: Role) {
        persons::edit(
            db,
            id,
            &persons::PersonEdit {
                role: Some(role),
                ..persons::PersonEdit::default()
            },
        )
        .await
        .unwrap()
        .expect("the person must still exist after an edit");
    }

    fn assistant(h: &Household) -> NewDelegation {
        NewDelegation {
            delegate_person_id: h.delegate,
            subject_person_id: h.subject,
            basis: Basis::Assistant,
            instrument_reference: None,
            granted_by_person_id: h.firm,
            subject_notified_at: None,
            subject_notified_channel: None,
        }
    }

    #[tokio::test]
    async fn grant_round_trips_and_names_who_recorded_it() {
        let db = mem_surreal().await;
        let h = household(&db).await;

        let granted = grant(&db, &assistant(&h)).await.unwrap();

        assert_eq!(granted.delegate_person_id, h.delegate);
        assert_eq!(granted.subject_person_id, h.subject);
        assert_eq!(granted.basis, Basis::Assistant);
        assert_eq!(
            granted.granted_by_person_id, h.firm,
            "the row must name the firm member who wrote it",
        );
        assert!(!granted.is_revoked());
        assert!(
            !granted.subject_was_notified(),
            "a grant does not itself notify the subject",
        );

        let reloaded = find_by_id(&db, granted.id).await.unwrap().unwrap();
        assert_eq!(reloaded, granted);
    }

    // --- Nick's constraint: client on both sides, refused at the write path.

    #[tokio::test]
    async fn a_non_client_delegate_is_refused_for_every_firm_tier() {
        for role in [Role::Owner, Role::Admin, Role::Lawyer, Role::Clerk] {
            let db = mem_surreal().await;
            let h = household(&db).await;
            let firm_side = person_at(&db, "Firm Side", "firm-side@example.com", role).await;

            let error = grant(
                &db,
                &NewDelegation {
                    delegate_person_id: firm_side,
                    ..assistant(&h)
                },
            )
            .await
            .expect_err("a firm-side person must not be a delegate");

            assert!(
                matches!(error, DelegationError::DelegateNotClient(id, named)
                    if id == firm_side && named == role.as_str()),
                "expected DelegateNotClient for {}, got {error:?}",
                role.as_str(),
            );
        }
    }

    #[tokio::test]
    async fn a_non_client_subject_is_refused_for_every_firm_tier() {
        for role in [Role::Owner, Role::Admin, Role::Lawyer, Role::Clerk] {
            let db = mem_surreal().await;
            let h = household(&db).await;
            let firm_side = person_at(&db, "Firm Side", "firm-side@example.com", role).await;

            let error = grant(
                &db,
                &NewDelegation {
                    subject_person_id: firm_side,
                    ..assistant(&h)
                },
            )
            .await
            .expect_err("a firm-side person must not be a delegation subject");

            assert!(
                matches!(error, DelegationError::SubjectNotClient(id, named)
                    if id == firm_side && named == role.as_str()),
                "expected SubjectNotClient for {}, got {error:?}",
                role.as_str(),
            );
        }
    }

    #[tokio::test]
    async fn the_firm_member_recording_a_grant_need_not_be_a_client() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        // `h.firm` is a Lawyer, and it is the `granted_by` on every grant in
        // these tests — so this passing is the assertion that the client-only
        // rule binds the two linked sides and not the recorder.
        grant(&db, &assistant(&h))
            .await
            .expect("a lawyer must be able to record a delegation between clients");
    }

    // --- Role changes. The behaviour the design chose deliberately.

    #[tokio::test]
    async fn promoting_a_party_makes_the_grant_dormant_not_live() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();

        assert_eq!(
            live_for_delegate(&db, h.delegate).await.unwrap(),
            vec![granted.clone()],
        );

        // The hole Nick's constraint closes: a linked client is hired by the
        // firm. No cascade runs — `set_role` knows nothing about this table —
        // so the read path is what must refuse.
        set_role_to(&db, h.delegate, Role::Clerk).await;

        assert!(
            live_for_delegate(&db, h.delegate).await.unwrap().is_empty(),
            "a clerk must not act through a delegation",
        );
        assert!(
            live_for_subject(&db, h.subject).await.unwrap().is_empty(),
            "and the subject side must agree",
        );

        // Dormant, not revoked — the distinction a surface needs in order to
        // say *why* the row is not in force.
        let reloaded = find_by_id(&db, granted.id).await.unwrap().unwrap();
        assert!(!reloaded.is_revoked(), "dormancy is not revocation");
        assert_eq!(
            reloaded.state(Role::Clerk, Role::Client),
            DelegationState::Dormant,
        );
        assert_eq!(
            all_for_delegate(&db, h.delegate).await.unwrap(),
            vec![reloaded],
            "the administrative view still shows a dormant row",
        );
    }

    #[tokio::test]
    async fn promoting_the_subject_alone_also_makes_the_grant_dormant() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        grant(&db, &assistant(&h)).await.unwrap();

        set_role_to(&db, h.subject, Role::Lawyer).await;

        assert!(
            live_for_delegate(&db, h.delegate).await.unwrap().is_empty(),
            "a delegation must never be a route into a firm-side account",
        );
    }

    /// The deliberate, debatable half of the role-change decision: because
    /// liveness is recomputed rather than cascaded, returning a party to
    /// `client` returns the grant to force without anyone re-authorizing it.
    ///
    /// Pinned so the choice is visible. A reviewer who wants re-authorization
    /// instead changes this test and adds an explicit revoke on role change.
    #[tokio::test]
    async fn returning_a_party_to_client_makes_a_dormant_grant_live_again() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();

        set_role_to(&db, h.delegate, Role::Clerk).await;
        assert!(live_for_delegate(&db, h.delegate).await.unwrap().is_empty());

        set_role_to(&db, h.delegate, Role::Client).await;
        assert_eq!(
            live_for_delegate(&db, h.delegate).await.unwrap(),
            vec![granted],
            "dormancy tracks the current roles; ending a delegation for good \
             is `revoke`, which is a recorded act",
        );
    }

    /// `persons::edit` is a second door onto the role column, and
    /// `persons::create` is a third. A cascade hooked to `set_role` would
    /// cover one of the three; read-time re-validation covers all of them.
    #[tokio::test]
    async fn a_role_change_through_the_edit_door_is_also_caught() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        grant(&db, &assistant(&h)).await.unwrap();

        edit_role_to(&db, h.delegate, Role::Admin).await;

        assert!(
            live_for_delegate(&db, h.delegate).await.unwrap().is_empty(),
            "the read path must not care which door changed the role",
        );
    }

    // --- Revocation.

    #[tokio::test]
    async fn revoke_records_who_and_when_and_never_deletes() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();

        let revoked = revoke(&db, granted.id, h.firm).await.unwrap();

        assert!(revoked.is_revoked());
        assert_eq!(revoked.revoked_by_person_id, Some(h.firm));
        assert!(revoked.revoked_at.is_some());
        assert!(
            live_for_delegate(&db, h.delegate).await.unwrap().is_empty(),
            "a revoked delegation is not in force",
        );
        assert!(
            find_by_id(&db, granted.id).await.unwrap().is_some(),
            "the row survives revocation, because `this link existed and was \
             withdrawn on this date` is the fact an inquiry needs",
        );
    }

    #[tokio::test]
    async fn revocation_does_not_require_the_credential_holder() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();

        // `h.firm` is neither the delegate nor the subject. The firm can end a
        // delegation without the person holding the login cooperating, which
        // is the control that protects a client whose credential is held by
        // someone else.
        revoke(&db, granted.id, h.firm)
            .await
            .expect("the firm must be able to revoke unilaterally");
    }

    #[tokio::test]
    async fn revoking_twice_is_refused_rather_than_silently_restamped() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();
        revoke(&db, granted.id, h.firm).await.unwrap();

        let error = revoke(&db, granted.id, h.firm)
            .await
            .expect_err("a second revocation must not move the recorded date");
        assert!(matches!(error, DelegationError::AlreadyRevoked(id) if id == granted.id));
    }

    #[tokio::test]
    async fn a_revoked_grant_stays_revoked_whatever_the_roles_become() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();
        let revoked = revoke(&db, granted.id, h.firm).await.unwrap();

        assert_eq!(
            revoked.state(Role::Client, Role::Client),
            DelegationState::Revoked,
            "revocation wins over dormancy",
        );
    }

    #[tokio::test]
    async fn a_pair_may_be_granted_again_after_a_revocation() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let first = grant(&db, &assistant(&h)).await.unwrap();
        revoke(&db, first.id, h.firm).await.unwrap();

        let second = grant(&db, &assistant(&h))
            .await
            .expect("a withdrawn delegation must not lock the pair out forever");

        assert_ne!(second.id, first.id, "re-granting writes a new row");
        assert_eq!(
            all_for_delegate(&db, h.delegate).await.unwrap().len(),
            2,
            "both the withdrawn row and the new one remain on the table",
        );
        assert_eq!(
            live_for_delegate(&db, h.delegate).await.unwrap(),
            vec![second],
        );
    }

    #[tokio::test]
    async fn a_second_live_grant_for_one_pair_is_refused() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        grant(&db, &assistant(&h)).await.unwrap();

        let error = grant(&db, &assistant(&h))
            .await
            .expect_err("one live delegation per pair");
        assert!(matches!(error, DelegationError::AlreadyGranted));
    }

    // --- The remaining write-path refusals.

    #[tokio::test]
    async fn self_delegation_is_refused() {
        let db = mem_surreal().await;
        let h = household(&db).await;

        let error = grant(
            &db,
            &NewDelegation {
                subject_person_id: h.delegate,
                ..assistant(&h)
            },
        )
        .await
        .expect_err("a person cannot be their own delegate");
        assert!(matches!(error, DelegationError::SelfDelegation));
    }

    #[tokio::test]
    async fn a_basis_claiming_an_instrument_must_name_one() {
        let db = mem_surreal().await;
        let h = household(&db).await;

        for basis in [
            Basis::PowerOfAttorney,
            Basis::Guardian,
            Basis::Executor,
            Basis::Trustee,
            Basis::ParentOfMinor,
        ] {
            assert!(basis.claims_an_instrument(), "{}", basis.as_str());

            let error = grant(
                &db,
                &NewDelegation {
                    basis,
                    instrument_reference: None,
                    ..assistant(&h)
                },
            )
            .await
            .expect_err("a legal-instrument basis must cite its instrument");
            assert!(
                matches!(error, DelegationError::InstrumentReferenceRequired(named)
                    if named == basis.as_str()),
                "expected InstrumentReferenceRequired for {}, got {error:?}",
                basis.as_str(),
            );

            // Whitespace is not a citation.
            let error = grant(
                &db,
                &NewDelegation {
                    basis,
                    instrument_reference: Some("   ".to_string()),
                    ..assistant(&h)
                },
            )
            .await
            .expect_err("blank is not a citation");
            assert!(matches!(
                error,
                DelegationError::InstrumentReferenceRequired(_)
            ));
        }
    }

    #[tokio::test]
    async fn an_assistant_needs_no_instrument_because_it_asserts_none() {
        let db = mem_surreal().await;
        let h = household(&db).await;

        assert!(!Basis::Assistant.claims_an_instrument());
        let granted = grant(&db, &assistant(&h)).await.unwrap();
        assert_eq!(granted.instrument_reference, None);
    }

    #[tokio::test]
    async fn a_cited_instrument_round_trips() {
        let db = mem_surreal().await;
        let h = household(&db).await;

        let granted = grant(
            &db,
            &NewDelegation {
                basis: Basis::PowerOfAttorney,
                instrument_reference: Some("Durable POA dated 2026-03-14".to_string()),
                ..assistant(&h)
            },
        )
        .await
        .unwrap();

        assert_eq!(granted.basis, Basis::PowerOfAttorney);
        assert_eq!(
            granted.instrument_reference.as_deref(),
            Some("Durable POA dated 2026-03-14"),
        );
    }

    #[tokio::test]
    async fn a_missing_person_on_any_leg_is_refused() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let ghost = Uuid::now_v7();

        for input in [
            NewDelegation {
                delegate_person_id: ghost,
                ..assistant(&h)
            },
            NewDelegation {
                subject_person_id: ghost,
                ..assistant(&h)
            },
            NewDelegation {
                granted_by_person_id: ghost,
                ..assistant(&h)
            },
        ] {
            let error = grant(&db, &input)
                .await
                .expect_err("every person on the row must exist");
            assert!(
                matches!(error, DelegationError::NoSuchPerson(id) if id == ghost),
                "got {error:?}",
            );
        }
    }

    // --- Notice to the subject.

    #[tokio::test]
    async fn subject_notice_is_recorded_with_the_channel_it_travelled_on() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let granted = grant(&db, &assistant(&h)).await.unwrap();

        // The motivating case: the subject has no mailbox, so the firm
        // telephones them and records that it did.
        let notified = record_subject_notice(&db, granted.id, NoticeChannel::Telephone)
            .await
            .unwrap();

        assert!(notified.subject_was_notified());
        assert_eq!(
            notified.subject_notified_channel,
            Some(NoticeChannel::Telephone),
        );
        assert!(notified.subject_notified_at.is_some());
    }

    #[tokio::test]
    async fn notice_can_be_recorded_at_grant_time_too() {
        let db = mem_surreal().await;
        let h = household(&db).await;

        let granted = grant(
            &db,
            &NewDelegation {
                subject_notified_at: Some("2026-03-14T17:00:00Z".to_string()),
                subject_notified_channel: Some(NoticeChannel::InPerson),
                ..assistant(&h)
            },
        )
        .await
        .unwrap();

        assert!(granted.subject_was_notified());
        assert_eq!(
            granted.subject_notified_channel,
            Some(NoticeChannel::InPerson),
        );
    }

    #[tokio::test]
    async fn recording_notice_on_a_missing_delegation_is_refused() {
        let db = mem_surreal().await;
        let ghost = Uuid::now_v7();

        let error = record_subject_notice(&db, ghost, NoticeChannel::Telephone)
            .await
            .expect_err("no such delegation");
        assert!(matches!(error, DelegationError::NoSuchDelegation(id) if id == ghost));
    }

    // --- Reads.

    #[tokio::test]
    async fn the_subject_side_answers_who_can_act_as_me() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let other = client(&db, "Second Delegate", "second@example.com").await;
        let first = grant(&db, &assistant(&h)).await.unwrap();
        let second = grant(
            &db,
            &NewDelegation {
                delegate_person_id: other,
                ..assistant(&h)
            },
        )
        .await
        .unwrap();

        let mut live = live_for_subject(&db, h.subject).await.unwrap();
        live.sort_by_key(|row| row.id);
        let mut expected = vec![first, second];
        expected.sort_by_key(|row| row.id);
        assert_eq!(
            live, expected,
            "a client must be able to learn who may act as them without \
             holding a credential themselves",
        );
    }

    #[tokio::test]
    async fn a_person_with_no_delegations_reads_empty_rather_than_erroring() {
        let db = mem_surreal().await;
        let lonely = client(&db, "Unlinked", "unlinked@example.com").await;

        assert!(all_for_delegate(&db, lonely).await.unwrap().is_empty());
        assert!(all_for_subject(&db, lonely).await.unwrap().is_empty());
        assert!(live_for_delegate(&db, lonely).await.unwrap().is_empty());
        assert!(live_for_subject(&db, lonely).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reading_a_missing_person_is_an_error_not_an_empty_list() {
        let db = mem_surreal().await;
        let ghost = Uuid::now_v7();

        let error = live_for_delegate(&db, ghost).await.expect_err(
            "a delegate who does not exist is not the same as one with no \
             delegations",
        );
        assert!(matches!(error, DelegationError::NoSuchPerson(id) if id == ghost));
    }

    /// `record<person>` is not validated against a live row, and
    /// [`persons::delete`] can leave one dangling. A deleted counterparty
    /// must make that row not-live without making an unrelated client's
    /// whole list unreadable.
    #[tokio::test]
    async fn a_deleted_counterparty_drops_its_row_rather_than_failing_the_read() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let other = client(&db, "Second Subject", "second@example.com").await;
        grant(&db, &assistant(&h)).await.unwrap();
        let survivor = grant(
            &db,
            &NewDelegation {
                subject_person_id: other,
                ..assistant(&h)
            },
        )
        .await
        .unwrap();

        persons::delete(&db, h.subject).await.unwrap();

        assert_eq!(
            live_for_delegate(&db, h.delegate).await.unwrap(),
            vec![survivor],
            "the dangling row drops out and the other one still reads",
        );
    }

    #[tokio::test]
    async fn find_live_for_pair_sees_only_the_live_row() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        let first = grant(&db, &assistant(&h)).await.unwrap();

        assert_eq!(
            find_live_for_pair(&db, h.delegate, h.subject)
                .await
                .unwrap(),
            vec![first.clone()],
        );

        revoke(&db, first.id, h.firm).await.unwrap();
        assert!(find_live_for_pair(&db, h.delegate, h.subject)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn a_delegation_is_directional() {
        let db = mem_surreal().await;
        let h = household(&db).await;
        grant(&db, &assistant(&h)).await.unwrap();

        assert!(
            find_live_for_pair(&db, h.subject, h.delegate)
                .await
                .unwrap()
                .is_empty(),
            "helping someone does not entitle them to help you back",
        );
        assert!(
            live_for_delegate(&db, h.subject).await.unwrap().is_empty(),
            "the subject gains nothing as a delegate",
        );
    }

    // --- Enum round-trips, so a stored spelling and its type cannot drift.

    #[test]
    fn every_basis_round_trips_through_its_stored_spelling() {
        for basis in [
            Basis::Assistant,
            Basis::PowerOfAttorney,
            Basis::Guardian,
            Basis::Executor,
            Basis::Trustee,
            Basis::ParentOfMinor,
        ] {
            assert_eq!(Basis::parse(basis.as_str()), Some(basis));
        }
        assert_eq!(Basis::parse("attorney_in_fact"), None);
        assert_eq!(Basis::parse(""), None);
        assert_eq!(
            Basis::parse("  assistant  "),
            Some(Basis::Assistant),
            "stored spellings are trimmed, as `Role::parse` does",
        );
    }

    #[test]
    fn every_notice_channel_round_trips_through_its_stored_spelling() {
        for channel in [
            NoticeChannel::InPerson,
            NoticeChannel::Telephone,
            NoticeChannel::Post,
            NoticeChannel::Email,
        ] {
            assert_eq!(NoticeChannel::parse(channel.as_str()), Some(channel));
        }
        assert_eq!(NoticeChannel::parse("carrier_pigeon"), None);
    }

    /// The `ASSERT` on `person_delegate.basis` and [`Basis`] must name the
    /// same closed set, or a value one accepts the other cannot read back.
    #[test]
    fn the_basis_assert_and_the_enum_name_the_same_set() {
        // The schema module keeps `DEFINITIONS` private, so include the file
        // directly — same bytes, same build.
        let definitions = include_str!("schema/navigator.surql");
        let assert_line = definitions
            .lines()
            .find(|line| line.contains("ASSERT $value IN") && line.contains("power_of_attorney"))
            .expect("the basis ASSERT must be in navigator.surql");

        for basis in [
            Basis::Assistant,
            Basis::PowerOfAttorney,
            Basis::Guardian,
            Basis::Executor,
            Basis::Trustee,
            Basis::ParentOfMinor,
        ] {
            assert!(
                assert_line.contains(&format!("'{}'", basis.as_str())),
                "`{}` is a Basis variant but not in the schema ASSERT",
                basis.as_str(),
            );
        }

        let quoted = assert_line.matches('\'').count() / 2;
        assert_eq!(
            quoted, 6,
            "the ASSERT lists {quoted} bases but Basis has 6; one side has \
             drifted: {assert_line}",
        );
    }
}
