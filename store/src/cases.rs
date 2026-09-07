//! `store::cases` — the litigation module's commands (#686).
//!
//! Two layers, and the split is the design:
//!
//! 1. A **generic spine**, [`DocketEntry`], modelled on how a court's own
//!    docket works — a numbered list of typed entries. A new niche
//!    instrument type is one [`EntryKind`] value, never a schema change.
//! 2. **Structured tables where structure is earned.** A device lives on
//!    the spine alone until it needs internal structure, and then
//!    graduates. Written discovery is the first to graduate.
//!
//! # These tables live in SurrealDB
//!
//! `cases`, `case_docket_entries`, `discovery_requests`, and
//! `discovery_items` moved with wave six of #1093 (ENG-160), in the
//! litigation slice.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashSet;
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

const CASE_TABLE: &str = "case";
const DOCKET_ENTRY_TABLE: &str = "case_docket_entry";
const DISCOVERY_REQUEST_TABLE: &str = "discovery_request";
const DISCOVERY_ITEM_TABLE: &str = "discovery_item";

/// What a docket entry is.
///
/// Closed but **code-extended**: adding a niche instrument is a variant
/// here plus one entry in the schema's `kind` ASSERT, not a schema change
/// to the spine. Deliberately spans the stages of litigation rather than
/// enumerating every device up front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Pleading,
    Motion,
    Opposition,
    Reply,
    Order,
    Notice,
    Stipulation,
    DiscoveryRequest,
    DiscoveryResponse,
    Subpoena,
    ExpertDisclosure,
    Hearing,
    Trial,
    Appeal,
    Settlement,
    /// An entry that is genuinely none of the above. Present so the spine
    /// never blocks recording something a court did.
    Other,
}

impl EntryKind {
    pub const ALL: &'static [EntryKind] = &[
        EntryKind::Pleading,
        EntryKind::Motion,
        EntryKind::Opposition,
        EntryKind::Reply,
        EntryKind::Order,
        EntryKind::Notice,
        EntryKind::Stipulation,
        EntryKind::DiscoveryRequest,
        EntryKind::DiscoveryResponse,
        EntryKind::Subpoena,
        EntryKind::ExpertDisclosure,
        EntryKind::Hearing,
        EntryKind::Trial,
        EntryKind::Appeal,
        EntryKind::Settlement,
        EntryKind::Other,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Pleading => "pleading",
            EntryKind::Motion => "motion",
            EntryKind::Opposition => "opposition",
            EntryKind::Reply => "reply",
            EntryKind::Order => "order",
            EntryKind::Notice => "notice",
            EntryKind::Stipulation => "stipulation",
            EntryKind::DiscoveryRequest => "discovery_request",
            EntryKind::DiscoveryResponse => "discovery_response",
            EntryKind::Subpoena => "subpoena",
            EntryKind::ExpertDisclosure => "expert_disclosure",
            EntryKind::Hearing => "hearing",
            EntryKind::Trial => "trial",
            EntryKind::Appeal => "appeal",
            EntryKind::Settlement => "settlement",
            EntryKind::Other => "other",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<EntryKind> {
        Self::ALL.iter().copied().find(|k| k.as_str() == value)
    }

    /// Hearings and trials are appearances: they carry `scheduled_on`.
    #[must_use]
    pub fn is_appearance(self) -> bool {
        matches!(self, Self::Hearing | Self::Trial)
    }
}

/// A written-discovery device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// Numbered written questions. The formal device the glossary
    /// reserves *Inquiry* as the generic noun for.
    Interrogatories,
    RequestsForProduction,
    RequestsForAdmission,
}

impl Device {
    pub const ALL: &'static [Device] = &[
        Device::Interrogatories,
        Device::RequestsForProduction,
        Device::RequestsForAdmission,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Device::Interrogatories => "interrogatories",
            Device::RequestsForProduction => "requests_for_production",
            Device::RequestsForAdmission => "requests_for_admission",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Device> {
        Self::ALL.iter().copied().find(|d| d.as_str() == value)
    }
}

/// Errors from the litigation commands.
#[derive(Debug, thiserror::Error)]
pub enum CaseError {
    #[error("`{0}` is not a recognized docket entry kind")]
    UnknownEntryKind(String),
    #[error("`{0}` is not a recognized discovery device")]
    UnknownDevice(String),
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// A write reported success but returned no row, or returned one this
    /// module could not read back.
    #[error("writing a {0} returned no usable row")]
    WriteReturnedNothing(&'static str),
    /// The item named by [`answer_item`] does not exist.
    #[error("no discovery item with id {0}")]
    ItemNotFound(Uuid),
}

/// One litigation matter before one forum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Case {
    pub id: Uuid,
    /// The matter this case belongs to.
    pub project_id: Uuid,
    /// The case caption as filed.
    pub caption: String,
    /// The court or tribunal.
    pub forum: Option<String>,
    pub jurisdiction: Option<String>,
    /// The forum's own docket number.
    pub docket_number: Option<String>,
    /// The presiding judge — part of the case masthead alongside caption,
    /// court, and docket number.
    pub judge: Option<String>,
    /// Which side the client is on.
    pub posture: String,
    /// `open`, `stayed`, or `closed`.
    pub status: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct CaseRow {
    id: surrealdb::types::RecordId,
    project_id: surrealdb::types::RecordId,
    caption: String,
    forum: Option<String>,
    jurisdiction: Option<String>,
    docket_number: Option<String>,
    judge: Option<String>,
    posture: String,
    status: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl CaseRow {
    fn into_case(self) -> Option<Case> {
        Some(Case {
            id: record_uuid(&self.id)?,
            project_id: record_uuid(&self.project_id)?,
            caption: self.caption,
            forum: self.forum,
            jurisdiction: self.jurisdiction,
            docket_number: self.docket_number,
            judge: self.judge,
            posture: self.posture,
            status: self.status,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const CASE_SELECT: &str = "id, project_id, caption, forum, jurisdiction, docket_number, judge, \
                           posture, status, inserted_at, updated_at";

/// One entry on a case's docket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DocketEntry {
    pub id: Uuid,
    pub case_id: Uuid,
    /// Per-case ordinal as text, so `29-1` round-trips exactly.
    pub entry_number: String,
    /// The closed, code-extended entry type.
    pub kind: String,
    pub title: String,
    /// Who filed or served it.
    pub party: Option<String>,
    /// RFC 3339 date filed or served.
    pub filed_or_served_on: Option<String>,
    /// When the court set a hearing or trial. Required for those kinds,
    /// absent for every other. Historical rows that predate the field stay
    /// `None` until an operator records the date; they are never backfilled.
    pub scheduled_on: Option<DateTime<Utc>>,
    /// The prior appearance this continuance replaces. The superseded
    /// entry keeps its date; the calendar follows the chain to the entry
    /// no later entry points at.
    pub supersedes: Option<Uuid>,
    /// `None` means *source pending*.
    pub document_asset_id: Option<Uuid>,
    /// Set when the firm drafted it.
    pub notation_id: Option<Uuid>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct DocketEntryRow {
    id: surrealdb::types::RecordId,
    case_id: surrealdb::types::RecordId,
    entry_number: String,
    kind: String,
    title: String,
    party: Option<String>,
    filed_or_served_on: Option<String>,
    scheduled_on: Option<surrealdb::types::Datetime>,
    supersedes: Option<surrealdb::types::RecordId>,
    document_asset_id: Option<surrealdb::types::RecordId>,
    notation_id: Option<surrealdb::types::RecordId>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl DocketEntryRow {
    fn into_entry(self) -> Option<DocketEntry> {
        Some(DocketEntry {
            id: record_uuid(&self.id)?,
            case_id: record_uuid(&self.case_id)?,
            entry_number: self.entry_number,
            kind: self.kind,
            title: self.title,
            party: self.party,
            filed_or_served_on: self.filed_or_served_on,
            scheduled_on: self.scheduled_on.map(Into::into),
            supersedes: self.supersedes.as_ref().and_then(record_uuid),
            document_asset_id: self.document_asset_id.as_ref().and_then(record_uuid),
            notation_id: self.notation_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const DOCKET_ENTRY_SELECT: &str = "id, case_id, entry_number, kind, title, party, \
                                   filed_or_served_on, scheduled_on, supersedes, \
                                   document_asset_id, notation_id, inserted_at, updated_at";

/// A hearing or trial the calendar should show: the current appearance on
/// a continuance chain, with a date still in the future.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Appearance {
    pub id: Uuid,
    pub case_id: Uuid,
    pub project_id: Uuid,
    pub kind: String,
    pub title: String,
    pub scheduled_on: DateTime<Utc>,
}

impl Appearance {
    /// The calendar's date cell — UTC, minute precision.
    #[must_use]
    pub fn calendar_date(&self) -> String {
        self.scheduled_on.format("%Y-%m-%d %H:%M UTC").to_string()
    }

    /// The calendar's status cell.
    #[must_use]
    pub fn calendar_status(&self) -> &'static str {
        match self.kind.as_str() {
            "hearing" => "Hearing",
            "trial" => "Trial",
            _ => "Appearance",
        }
    }
}

/// A hearing or trial row that has no `scheduled_on`. Existing databases
/// can hold these from before the field existed; they are reported, never
/// backfilled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UndatedAppearance {
    pub id: Uuid,
    pub case_id: Uuid,
    pub kind: String,
    pub title: String,
}

/// One served set of written discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryRequest {
    pub id: Uuid,
    pub case_id: Uuid,
    /// Optional, because a device is keyed to the case and only optionally
    /// to the entry recording its service.
    pub docket_entry_id: Option<Uuid>,
    /// `interrogatories`, `requests_for_production`, or
    /// `requests_for_admission`.
    pub device: String,
    /// Which set — "Interrogatories, Set Two".
    pub set_number: i32,
    pub propounding_party: String,
    pub responding_party: String,
    pub served_on: Option<String>,
    pub responses_due_on: Option<String>,
    /// `served`, `responded`, or `closed`.
    pub status: String,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct DiscoveryRequestRow {
    id: surrealdb::types::RecordId,
    case_id: surrealdb::types::RecordId,
    docket_entry_id: Option<surrealdb::types::RecordId>,
    device: String,
    set_number: i64,
    propounding_party: String,
    responding_party: String,
    served_on: Option<String>,
    responses_due_on: Option<String>,
    status: String,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl DiscoveryRequestRow {
    fn into_request(self) -> Option<DiscoveryRequest> {
        Some(DiscoveryRequest {
            id: record_uuid(&self.id)?,
            case_id: record_uuid(&self.case_id)?,
            docket_entry_id: self.docket_entry_id.as_ref().and_then(record_uuid),
            device: self.device,
            set_number: i32::try_from(self.set_number).ok()?,
            propounding_party: self.propounding_party,
            responding_party: self.responding_party,
            served_on: self.served_on,
            responses_due_on: self.responses_due_on,
            status: self.status,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const DISCOVERY_REQUEST_SELECT: &str =
    "id, case_id, docket_entry_id, device, set_number, propounding_party, responding_party, \
     served_on, responses_due_on, status, inserted_at, updated_at";

/// One numbered item within a served set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveryItem {
    pub id: Uuid,
    pub discovery_request_id: Uuid,
    /// The item's number within its set.
    pub item_number: i32,
    /// The propounded request, verbatim.
    pub request_text: String,
    /// The response, once drafted. `None` until answered.
    pub response_text: Option<String>,
    /// Objections asserted to this item.
    pub objections: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct DiscoveryItemRow {
    id: surrealdb::types::RecordId,
    discovery_request_id: surrealdb::types::RecordId,
    item_number: i64,
    request_text: String,
    response_text: Option<String>,
    objections: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl DiscoveryItemRow {
    fn into_item(self) -> Option<DiscoveryItem> {
        Some(DiscoveryItem {
            id: record_uuid(&self.id)?,
            discovery_request_id: record_uuid(&self.discovery_request_id)?,
            item_number: i32::try_from(self.item_number).ok()?,
            request_text: self.request_text,
            response_text: self.response_text,
            objections: self.objections,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const DISCOVERY_ITEM_SELECT: &str = "id, discovery_request_id, item_number, request_text, \
                                     response_text, objections, inserted_at, updated_at";

/// What a new [`Case`] needs.
#[derive(Debug, Clone)]
pub struct NewCase<'a> {
    pub project_id: Uuid,
    pub caption: &'a str,
    pub forum: Option<&'a str>,
    pub jurisdiction: Option<&'a str>,
    pub docket_number: Option<&'a str>,
    /// The presiding judge — part of the case masthead.
    pub judge: Option<&'a str>,
    pub posture: &'a str,
}

/// Open a case on a matter. Many cases per matter is the point, so this
/// never checks for an existing one.
///
/// # Errors
/// Propagates any database error.
pub async fn open_case(db: &SurrealDb, new: &NewCase<'_>) -> Result<Case, CaseError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             project_id = $project_id, caption = $caption, forum = $forum, \
             jurisdiction = $jurisdiction, docket_number = $docket_number, judge = $judge, \
             posture = $posture, status = 'open' \
             RETURN {CASE_SELECT}"
        ))
        .bind(("id", record_id(CASE_TABLE, id)))
        .bind((
            "project_id",
            record_id(crate::projects::PROJECT_TABLE, new.project_id),
        ))
        .bind(("caption", new.caption.to_string()))
        .bind(("forum", new.forum.map(str::to_string)))
        .bind(("jurisdiction", new.jurisdiction.map(str::to_string)))
        .bind(("docket_number", new.docket_number.map(str::to_string)))
        .bind(("judge", new.judge.map(str::to_string)))
        .bind(("posture", new.posture.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<CaseRow> = response.take(0)?;
    row.and_then(CaseRow::into_case)
        .ok_or(CaseError::WriteReturnedNothing("case"))
}

/// Every case on `project_id`, oldest first.
///
/// # Errors
/// Propagates any database error.
pub async fn for_project(db: &SurrealDb, project_id: Uuid) -> Result<Vec<Case>, CaseError> {
    let mut response = db
        .query(format!(
            "SELECT {CASE_SELECT} FROM {CASE_TABLE} WHERE project_id = $project ORDER BY id ASC"
        ))
        .bind((
            "project",
            record_id(crate::projects::PROJECT_TABLE, project_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<CaseRow> = response.take(0)?;
    Ok(rows.into_iter().filter_map(CaseRow::into_case).collect())
}

/// Remove the litigation rows that cascade from a matter.
///
/// SurrealDB has no cascade, so the chain is walked explicitly
/// in dependency order: discovery items, requests, docket entries, and
/// finally the cases themselves.
///
/// # Errors
/// Propagates any database error.
pub async fn delete_for_project(db: &SurrealDb, project_id: Uuid) -> Result<(), CaseError> {
    let case_ids: Vec<Uuid> = for_project(db, project_id)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    if case_ids.is_empty() {
        return Ok(());
    }
    let cases: Vec<surrealdb::types::RecordId> = case_ids
        .iter()
        .map(|id| record_id(CASE_TABLE, *id))
        .collect();

    let mut request_ids = Vec::new();
    for case_id in &case_ids {
        request_ids.extend(
            discovery_requests(db, *case_id)
                .await?
                .into_iter()
                .map(|r| r.id),
        );
    }
    if !request_ids.is_empty() {
        let requests: Vec<surrealdb::types::RecordId> = request_ids
            .iter()
            .map(|id| record_id(DISCOVERY_REQUEST_TABLE, *id))
            .collect();
        db.query(format!(
            "DELETE {DISCOVERY_ITEM_TABLE} WHERE $requests CONTAINS discovery_request_id"
        ))
        .bind(("requests", requests))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    }

    // Children before parents, in one explicit transaction — a
    // multi-statement query is not atomic on its own.
    //
    // `$cases CONTAINS case_id`, not `case_id IN $cases`: inside a DELETE,
    // the `IN` form silently matches nothing for a record-link field and
    // the statement reports success having removed no rows. The same `IN`
    // works in a SELECT, which is what makes it so easy to write. The
    // cascade test below is what holds this shape in place.
    db.query(format!(
        "BEGIN; \
         DELETE {DISCOVERY_REQUEST_TABLE} WHERE $cases CONTAINS case_id; \
         DELETE {DOCKET_ENTRY_TABLE} WHERE $cases CONTAINS case_id; \
         DELETE {CASE_TABLE} WHERE id IN $cases; \
         COMMIT;"
    ))
    .bind(("cases", cases))
    .await
    .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

/// What a new docket entry needs.
#[derive(Debug, Clone)]
pub struct NewDocketEntry<'a> {
    pub case_id: Uuid,
    /// Text, so a court's attachment sub-number (`29-1`) round-trips.
    pub entry_number: &'a str,
    pub kind: EntryKind,
    pub title: &'a str,
    pub party: Option<&'a str>,
    pub filed_or_served_on: Option<&'a str>,
    /// Required when `kind` is [`EntryKind::Hearing`] or [`EntryKind::Trial`];
    /// must be `None` otherwise. The schema ASSERT is the write-time gate.
    pub scheduled_on: Option<DateTime<Utc>>,
    /// The prior appearance a continuance replaces.
    pub supersedes: Option<Uuid>,
    /// `None` is a meaningful state — the entry renders *source pending*.
    pub document_asset_id: Option<Uuid>,
    pub notation_id: Option<Uuid>,
}

/// Record an entry on a case's docket.
///
/// # Errors
/// Propagates any database error.
pub async fn record_entry(
    db: &SurrealDb,
    new: &NewDocketEntry<'_>,
) -> Result<DocketEntry, CaseError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             case_id = $case_id, entry_number = $entry_number, kind = $kind, title = $title, \
             party = $party, filed_or_served_on = $filed_or_served_on, \
             scheduled_on = $scheduled_on, supersedes = $supersedes, \
             document_asset_id = $document_asset_id, notation_id = $notation_id \
             RETURN {DOCKET_ENTRY_SELECT}"
        ))
        .bind(("id", record_id(DOCKET_ENTRY_TABLE, id)))
        .bind(("case_id", record_id(CASE_TABLE, new.case_id)))
        .bind(("entry_number", new.entry_number.to_string()))
        .bind(("kind", new.kind.as_str().to_string()))
        .bind(("title", new.title.to_string()))
        .bind(("party", new.party.map(str::to_string)))
        .bind((
            "filed_or_served_on",
            new.filed_or_served_on.map(str::to_string),
        ))
        .bind((
            "scheduled_on",
            new.scheduled_on.map(surrealdb::types::Datetime::from),
        ))
        .bind((
            "supersedes",
            new.supersedes
                .map(|prior| record_id(DOCKET_ENTRY_TABLE, prior)),
        ))
        .bind((
            "document_asset_id",
            new.document_asset_id
                .map(|a| record_id(crate::assets::TABLE, a)),
        ))
        .bind((
            "notation_id",
            new.notation_id
                .map(|n| record_id(crate::notations::TABLE, n)),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DocketEntryRow> = response.take(0)?;
    row.and_then(DocketEntryRow::into_entry)
        .ok_or(CaseError::WriteReturnedNothing("docket entry"))
}

/// The case's docket, in insertion order.
///
/// Not ordered by `entry_number`: it is text carrying sub-numbers like
/// `29-1`, so sorting it as a string would put `10` before `9`. Insertion
/// order is the order the firm recorded the docket in, which is the
/// order a court's own docket is read in.
///
/// # Errors
/// Propagates any database error.
pub async fn docket(db: &SurrealDb, case_id: Uuid) -> Result<Vec<DocketEntry>, CaseError> {
    let mut response = db
        .query(format!(
            "SELECT {DOCKET_ENTRY_SELECT} FROM {DOCKET_ENTRY_TABLE} \
             WHERE case_id = $case ORDER BY id ASC"
        ))
        .bind(("case", record_id(CASE_TABLE, case_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DocketEntryRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(DocketEntryRow::into_entry)
        .collect())
}

/// True when the entry has no document filed against it yet — the
/// *source pending* state.
///
/// A meaningful state, not an error. Derived from the link rather than
/// from a hand-maintained flag, so it cannot disagree with reality.
#[must_use]
pub fn is_source_pending(entry: &DocketEntry) -> bool {
    entry.document_asset_id.is_none()
}

fn is_appearance_kind(kind: &str) -> bool {
    matches!(kind, "hearing" | "trial")
}

fn appearance_from_entry(entry: &DocketEntry, project_id: Uuid) -> Option<Appearance> {
    Some(Appearance {
        id: entry.id,
        case_id: entry.case_id,
        project_id,
        kind: entry.kind.clone(),
        title: entry.title.clone(),
        scheduled_on: entry.scheduled_on?,
    })
}

/// Current upcoming appearances on `case_id`: hearing and trial entries
/// that carry a date, that no later entry supersedes, and whose date is
/// still in the future.
///
/// # Errors
/// Propagates any database error.
pub async fn current_appearances_for_case(
    db: &SurrealDb,
    case_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<Appearance>, CaseError> {
    let entries = docket(db, case_id).await?;
    let superseded: HashSet<Uuid> = entries
        .iter()
        .filter_map(|entry| entry.supersedes)
        .collect();
    let now = Utc::now();
    let mut appearances: Vec<Appearance> = entries
        .iter()
        .filter(|entry| is_appearance_kind(&entry.kind))
        .filter(|entry| !superseded.contains(&entry.id))
        .filter(|entry| entry.scheduled_on.is_some_and(|when| when >= now))
        .filter_map(|entry| appearance_from_entry(entry, project_id))
        .collect();
    appearances.sort_by_key(|appearance| appearance.scheduled_on);
    Ok(appearances)
}

/// Current upcoming appearances on every case belonging to `project_id`.
///
/// # Errors
/// Propagates any database error.
pub async fn current_appearances_for_project(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<Appearance>, CaseError> {
    let mut appearances = Vec::new();
    for case in for_project(db, project_id).await? {
        appearances.extend(current_appearances_for_case(db, case.id, case.project_id).await?);
    }
    appearances.sort_by_key(|appearance| appearance.scheduled_on);
    Ok(appearances)
}

/// Current upcoming appearances across the given matters.
///
/// # Errors
/// Propagates any database error.
pub async fn current_appearances_for_projects(
    db: &SurrealDb,
    project_ids: &[Uuid],
) -> Result<Vec<Appearance>, CaseError> {
    let mut appearances = Vec::new();
    for project_id in project_ids {
        appearances.extend(current_appearances_for_project(db, *project_id).await?);
    }
    appearances.sort_by_key(|appearance| appearance.scheduled_on);
    Ok(appearances)
}

/// Hearing and trial rows that have no `scheduled_on`. The schema refuses
/// new writes in that state; this read is how an operator finds rows that
/// predate the field. It never writes.
///
/// # Errors
/// Propagates any database error.
pub async fn undated_hearings_and_trials(
    db: &SurrealDb,
) -> Result<Vec<UndatedAppearance>, CaseError> {
    let mut response = db
        .query(format!(
            "SELECT {DOCKET_ENTRY_SELECT} FROM {DOCKET_ENTRY_TABLE} \
             WHERE kind IN ['hearing', 'trial'] AND scheduled_on IS NONE \
             ORDER BY id ASC"
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DocketEntryRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(DocketEntryRow::into_entry)
        .map(|entry| UndatedAppearance {
            id: entry.id,
            case_id: entry.case_id,
            kind: entry.kind,
            title: entry.title,
        })
        .collect())
}

/// What a new served discovery set needs.
#[derive(Debug, Clone)]
pub struct NewDiscoveryRequest<'a> {
    pub case_id: Uuid,
    pub docket_entry_id: Option<Uuid>,
    pub device: Device,
    pub set_number: i32,
    pub propounding_party: &'a str,
    pub responding_party: &'a str,
    pub served_on: Option<&'a str>,
    /// What the served papers say. The Deadlines module answers what is
    /// actually due.
    pub responses_due_on: Option<&'a str>,
}

/// Record a served set of written discovery.
///
/// # Errors
/// Propagates any database error.
pub async fn serve_discovery(
    db: &SurrealDb,
    new: &NewDiscoveryRequest<'_>,
) -> Result<DiscoveryRequest, CaseError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             case_id = $case_id, docket_entry_id = $docket_entry_id, device = $device, \
             set_number = $set_number, propounding_party = $propounding_party, \
             responding_party = $responding_party, served_on = $served_on, \
             responses_due_on = $responses_due_on, status = 'served' \
             RETURN {DISCOVERY_REQUEST_SELECT}"
        ))
        .bind(("id", record_id(DISCOVERY_REQUEST_TABLE, id)))
        .bind(("case_id", record_id(CASE_TABLE, new.case_id)))
        .bind((
            "docket_entry_id",
            new.docket_entry_id
                .map(|e| record_id(DOCKET_ENTRY_TABLE, e)),
        ))
        .bind(("device", new.device.as_str().to_string()))
        .bind(("set_number", i64::from(new.set_number)))
        .bind(("propounding_party", new.propounding_party.to_string()))
        .bind(("responding_party", new.responding_party.to_string()))
        .bind(("served_on", new.served_on.map(str::to_string)))
        .bind(("responses_due_on", new.responses_due_on.map(str::to_string)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DiscoveryRequestRow> = response.take(0)?;
    row.and_then(DiscoveryRequestRow::into_request)
        .ok_or(CaseError::WriteReturnedNothing("discovery request"))
}

/// Every served set on a case, oldest first.
///
/// # Errors
/// Propagates any database error.
pub async fn discovery_requests(
    db: &SurrealDb,
    case_id: Uuid,
) -> Result<Vec<DiscoveryRequest>, CaseError> {
    let mut response = db
        .query(format!(
            "SELECT {DISCOVERY_REQUEST_SELECT} FROM {DISCOVERY_REQUEST_TABLE} \
             WHERE case_id = $case ORDER BY id ASC"
        ))
        .bind(("case", record_id(CASE_TABLE, case_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DiscoveryRequestRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(DiscoveryRequestRow::into_request)
        .collect())
}

/// Add a numbered item to a served set.
///
/// # Errors
/// Propagates any database error.
pub async fn add_item(
    db: &SurrealDb,
    discovery_request_id: Uuid,
    item_number: i32,
    request_text: &str,
) -> Result<DiscoveryItem, CaseError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             discovery_request_id = $request_id, item_number = $item_number, \
             request_text = $request_text, response_text = NONE, objections = NONE \
             RETURN {DISCOVERY_ITEM_SELECT}"
        ))
        .bind(("id", record_id(DISCOVERY_ITEM_TABLE, id)))
        .bind((
            "request_id",
            record_id(DISCOVERY_REQUEST_TABLE, discovery_request_id),
        ))
        .bind(("item_number", i64::from(item_number)))
        .bind(("request_text", request_text.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<DiscoveryItemRow> = response.take(0)?;
    row.and_then(DiscoveryItemRow::into_item)
        .ok_or(CaseError::WriteReturnedNothing("discovery item"))
}

/// Answer one item, optionally asserting objections.
///
/// # Errors
/// [`CaseError::ItemNotFound`] when no item carries `item_id`, or any
/// database error.
pub async fn answer_item(
    db: &SurrealDb,
    item_id: Uuid,
    response_text: &str,
    objections: Option<&str>,
) -> Result<DiscoveryItem, CaseError> {
    let mut response = db
        .query(format!(
            "UPDATE $id SET \
             response_text = $response_text, objections = $objections, updated_at = time::now() \
             RETURN {DISCOVERY_ITEM_SELECT}"
        ))
        .bind(("id", record_id(DISCOVERY_ITEM_TABLE, item_id)))
        .bind(("response_text", Some(response_text.to_string())))
        .bind(("objections", objections.map(str::to_string)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DiscoveryItemRow> = response.take(0)?;
    rows.into_iter()
        .find_map(DiscoveryItemRow::into_item)
        .ok_or(CaseError::ItemNotFound(item_id))
}

/// A served set's items, in item order.
///
/// # Errors
/// Propagates any database error.
pub async fn items(
    db: &SurrealDb,
    discovery_request_id: Uuid,
) -> Result<Vec<DiscoveryItem>, CaseError> {
    let mut response = db
        .query(format!(
            "SELECT {DISCOVERY_ITEM_SELECT} FROM {DISCOVERY_ITEM_TABLE} \
             WHERE discovery_request_id = $request ORDER BY item_number ASC"
        ))
        .bind((
            "request",
            record_id(DISCOVERY_REQUEST_TABLE, discovery_request_id),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<DiscoveryItemRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(DiscoveryItemRow::into_item)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{Device, EntryKind};

    #[test]
    fn every_entry_kind_round_trips() {
        for kind in EntryKind::ALL {
            assert_eq!(EntryKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(EntryKind::parse("nonsense"), None);
    }

    #[test]
    fn the_spine_spans_the_stages_of_litigation() {
        // Not just pleadings: the spine has to carry a case from the
        // complaint through appeal without a migration per stage.
        for kind in [
            EntryKind::Pleading,
            EntryKind::Motion,
            EntryKind::Subpoena,
            EntryKind::ExpertDisclosure,
            EntryKind::Hearing,
            EntryKind::Trial,
            EntryKind::Appeal,
            EntryKind::Settlement,
        ] {
            assert!(EntryKind::ALL.contains(&kind));
        }
    }

    #[test]
    fn every_device_round_trips() {
        for device in Device::ALL {
            assert_eq!(Device::parse(device.as_str()), Some(*device));
        }
        assert_eq!(Device::ALL.len(), 3, "the three written-discovery devices");
    }
}
