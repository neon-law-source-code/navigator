//! The id a third-party system issues for a Person — an address book,
//! and nothing more.
//!
//! # This table carries no authorization meaning
//!
//! A row here turns "this Person" into "this account" for an API call:
//! creating a repository and putting the right people on it means
//! telling GitHub *which* user, and the API wants an id, not an email
//! address. Inviting someone to a Slack channel, as a Docusign
//! envelope recipient, or into a Notion relation is the same problem.
//!
//! Three separate things are involved and this module owns exactly one:
//!
//! | | Lives where |
//! | -- | -- |
//! | **Who** — the account id to name in the call | here |
//! | **Credential** — what authenticates the call | firm-level service configuration, managed as secrets |
//! | **Authority** — whether the call should be made at all | `persons.role` and policy |
//!
//! So **no code may read this table to make an access decision.** That
//! is not a scoping convenience, it is the safety property.
//! [`crate::persons::Role`] is the authorization tier and
//! `person_project_roles.participation` is the scope; an external
//! identity is neither. A row saying a Clerk is GitHub user `12345`
//! must never be the reason that Clerk can read a repository — the
//! glossary rule that a Clerk "never receives lawyer-work, advice, Git,
//! MCP, or `/app/lawyer` authority by inheritance" holds precisely because
//! this table is inert, and `cli/tests/external_identity_is_inert.rs`
//! asserts that against the authorization surfaces by name rather than
//! leaving it to be remembered.
//!
//! Project participation likewise never grants source-forge access. The
//! rule is per-system, not per-role — a `client` Person holding a
//! `google` identity for Drive sharing is legitimate, and the same
//! Person is still never provisioned into the forge that hosts the
//! source — so the schema carries no blanket role constraint and the
//! enforcement belongs where provisioning happens. See
//! [`docs/access-model.md`](../../../docs/access-model.md) and
//! [`docs/project-repositories.md`](../../../docs/project-repositories.md).
//!
//! # The id, never the handle
//!
//! [`ExternalIdentity::external_id`] is the provider's **immutable** id:
//! a GitHub numeric id, a Slack `U…`, a Google `sub`, a Linear uuid, or a
//! Notion workspace user id.
//! [`ExternalIdentity::handle`] is display only and expected to drift.
//! Handles are renameable, and a mapping keyed on one breaks quietly —
//! the provisioning call simply fails to find a user, at exactly the
//! moment a matter is opening.
//!
//! # Values are unverified
//!
//! Navigator does not confirm that GitHub user `12345` really is this
//! Person. The identifier is entered or imported, the way
//! `person.xero_contact_id` already is. A wrong id is a data-entry bug
//! with data-entry consequences, and a stale row is wrong data rather
//! than a security incident — which is what makes reconciliation,
//! webhooks, and drift detection out of scope rather than missing.
//!
//! # Two engine facts this module is shaped around
//!
//! **The engine does not validate a link.** `record<person>` accepts a
//! link to a row that was never written, exactly as
//! [`crate::credentials`] documents. [`link`] reads the person back
//! through [`crate::persons::find_by_id`] before writing, and that check
//! is the only thing between a typo and an identity attached to nobody.
//!
//! **The key-value layer is optimistic, so a write can lose a race.**
//! The loser is rolled back and the engine reports
//! `QueryError::TransactionConflict`, so [`writing`] re-runs under the
//! crate's one retry policy, [`crate::surreal::retry`], rather than
//! reporting a simultaneous save as a database fault.

use chrono::{DateTime, Utc};
use surrealdb::types::SurrealValue;
use uuid::Uuid;

use crate::persons::{self, PersonError};
use crate::surreal::{record_id, record_uuid, retry, SurrealDb};

/// The table these rows live in.
const TABLE: &str = "person_external_identity";
/// The table `person_id` links into.
const PERSON_TABLE: &str = "person";

/// A third-party system Navigator can name a Person in.
///
/// Stored as a `string` with an `ASSERT $value IN [...]` on the field —
/// the same pair [`crate::persons::Role`] forms with `person.role`. The
/// vocabulary is closed: [`ExternalSystem::parse`] fails closed, with no
/// `Other(String)` escape, so an unknown system is refused rather than
/// resolved. Adding one costs a variant here and an entry in the ASSERT,
/// not a table.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ExternalSystem {
    /// GitHub / GitHub Enterprise. `external_id` is the numeric user id.
    ///
    /// One variant covers both, and naming GitHub Enterprise here is
    /// deliberate rather than left over: Navigator runs on github.com, and
    /// a self-hoster running it against their own tenant records ids issued
    /// by that tenant through this same variant. The system is the kind of
    /// forge, not one deployment's host.
    Github,
    /// Slack. `external_id` is the `U…` member id.
    Slack,
    /// Docusign. `external_id` is the envelope-recipient user id — not
    /// the firm-level `DOCUSIGN_USER_ID`, which is one impersonated API
    /// user and is service configuration rather than a per-Person map.
    Docusign,
    /// Google. `external_id` is the OIDC `sub`.
    Google,
    /// Linear. `external_id` is the user uuid.
    Linear,
    /// Claude. `external_id` is the workspace member id.
    Claude,
    /// ChatGPT. `external_id` is the workspace member id.
    Chatgpt,
    /// Notion. `external_id` is the stable workspace user id.
    Notion,
}

impl ExternalSystem {
    /// Every system, in the order the schema's ASSERT lists them. The
    /// pairing is held by
    /// [`the_assert_and_the_enum_name_the_same_systems`].
    pub const ALL: &'static [Self] = &[
        Self::Github,
        Self::Slack,
        Self::Docusign,
        Self::Google,
        Self::Linear,
        Self::Claude,
        Self::Chatgpt,
        Self::Notion,
    ];

    /// The stored spelling — what goes in the column and what the ASSERT
    /// gates.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Slack => "slack",
            Self::Docusign => "docusign",
            Self::Google => "google",
            Self::Linear => "linear",
            Self::Claude => "claude",
            Self::Chatgpt => "chatgpt",
            Self::Notion => "notion",
        }
    }

    /// Read a stored spelling back, or `None` when it names no system
    /// this build knows.
    ///
    /// Fail-closed on purpose, and deliberately without an
    /// `Other(String)` variant to fall into. A system Navigator cannot
    /// name is a system Navigator cannot call, so resolving it to
    /// *something* would only move the failure to the call site with a
    /// value that looks legitimate.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|system| system.as_str() == value)
    }
}

impl std::fmt::Display for ExternalSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors reading or writing an external identity.
#[derive(Debug, thiserror::Error)]
pub enum ExternalIdentityError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// Reading the person a write links to failed.
    #[error(transparent)]
    Person(#[from] PersonError),
    /// The person named by a write does not exist. The engine would have
    /// accepted the dangling link; this is the check that does not.
    #[error("no person {0}")]
    NoSuchPerson(Uuid),
    /// Another person already holds this account — the write collided
    /// with `person_external_identity_account`.
    #[error("that {system} account is already held by another person")]
    AccountTaken { system: ExternalSystem },
    /// This person already holds an account on this system — the write
    /// collided with `person_external_identity_person_system`.
    #[error("that person already holds a {system} identity")]
    SystemAlreadyLinked { system: ExternalSystem },
    /// A write reported success but returned no row, or returned one
    /// this module could not read back — see
    /// [`ExternalIdentityRow::into_identity`].
    #[error("writing an external identity returned no usable row")]
    WriteReturnedNothing,
}

/// Turn a write failure into the caller-correctable conflict it names,
/// or leave it as a database fault. A unique violation carries **no**
/// typed detail — the index name in the message is the only
/// discriminator through the shared classifier in [`crate::surreal::retry`],
/// and the two
/// indexes on this table mean two different corrections, so they are
/// separated here rather than collapsed into one "taken".
fn classify_write(error: surrealdb::Error, system: ExternalSystem) -> ExternalIdentityError {
    match crate::surreal::retry::unique_violation(&error) {
        Some("person_external_identity_account") => ExternalIdentityError::AccountTaken { system },
        Some("person_external_identity_person_system") => {
            ExternalIdentityError::SystemAlreadyLinked { system }
        }
        _ => ExternalIdentityError::Db(error),
    }
}

/// Run a write under the shared retry policy
/// ([`crate::surreal::retry`]), mapping whatever finally comes back to
/// this module's error.
///
/// Only the mapping lives here. How long a lost race is re-run, and
/// which engine conditions count as a lost race, are one policy for the
/// whole crate.
async fn writing<F, Q>(
    system: ExternalSystem,
    attempt: F,
) -> Result<surrealdb::IndexedResults, ExternalIdentityError>
where
    F: FnMut() -> Q,
    Q: std::future::IntoFuture<Output = Result<surrealdb::IndexedResults, surrealdb::Error>>,
{
    retry::writing(attempt)
        .await
        .map_err(|error| classify_write(error, system))
}

/// One Person's account on one external system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIdentity {
    pub id: Uuid,
    pub person_id: Uuid,
    pub system: ExternalSystem,
    /// The provider's immutable id — the value an API call names.
    pub external_id: String,
    /// The display handle, if one was recorded. Expected to drift; never
    /// the key.
    pub handle: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The row as the engine reads and writes it. Separate from
/// [`ExternalIdentity`] because the SDK owns its own `RecordId` and
/// `Datetime`, and `system` arrives as the stored string; the conversion
/// belongs at this seam rather than in every caller.
#[derive(SurrealValue)]
struct ExternalIdentityRow {
    id: surrealdb::types::RecordId,
    person_id: surrealdb::types::RecordId,
    system: String,
    external_id: String,
    handle: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl ExternalIdentityRow {
    /// `None` when a record id is not a native UUID key (see
    /// [`crate::surreal`] for why the two key spellings differ) or when
    /// `system` is not one the vocabulary names. Both are rows this
    /// workspace could not have written; reporting them would invent an
    /// id or a system.
    fn into_identity(self) -> Option<ExternalIdentity> {
        Some(ExternalIdentity {
            id: record_uuid(&self.id)?,
            person_id: record_uuid(&self.person_id)?,
            system: ExternalSystem::parse(&self.system)?,
            external_id: self.external_id,
            handle: self.handle,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

/// The projection every read shares, so one field list describes the row
/// and a new column cannot reach [`ExternalIdentityRow`] from only one
/// query.
const SELECT: &str = "id, person_id, system, external_id, handle, inserted_at, updated_at";

/// Record that `person_id` is `external_id` on `system`.
///
/// `handle` is stored as given and never matched on.
///
/// # Errors
///
/// [`ExternalIdentityError::NoSuchPerson`] when the person does not
/// exist — checked here because the engine would accept the dangling
/// link. [`ExternalIdentityError::AccountTaken`] when another person
/// already holds this account, and
/// [`ExternalIdentityError::SystemAlreadyLinked`] when this person
/// already holds one on this system. [`ExternalIdentityError::Db`] for
/// anything else.
pub async fn link(
    db: &SurrealDb,
    person_id: Uuid,
    system: ExternalSystem,
    external_id: &str,
    handle: Option<&str>,
) -> Result<ExternalIdentity, ExternalIdentityError> {
    if persons::find_by_id(db, person_id).await?.is_none() {
        return Err(ExternalIdentityError::NoSuchPerson(person_id));
    }

    let id = Uuid::now_v7();
    let mut response = writing(system, || {
        db.query(format!(
            "CREATE $id SET \
             person_id = $person_id, \
             system = $system, \
             external_id = $external_id, \
             handle = $handle \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("system", system.as_str().to_string()))
        .bind(("external_id", external_id.trim().to_string()))
        .bind(("handle", handle.map(str::to_string)))
    })
    .await?;

    let row: Option<ExternalIdentityRow> = response.take(0)?;
    row.and_then(ExternalIdentityRow::into_identity)
        .ok_or(ExternalIdentityError::WriteReturnedNothing)
}

/// This person's account on this system, if one is recorded. The pair is
/// the natural key `person_external_identity_person_system` carries.
///
/// # Errors
///
/// [`ExternalIdentityError::Db`] if the lookup fails.
pub async fn find_by_person_and_system(
    db: &SurrealDb,
    person_id: Uuid,
    system: ExternalSystem,
) -> Result<Option<ExternalIdentity>, ExternalIdentityError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE person_id = $person_id AND system = $system \
             LIMIT 1"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .bind(("system", system.as_str().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let row: Option<ExternalIdentityRow> = response.take(0)?;
    Ok(row.and_then(ExternalIdentityRow::into_identity))
}

/// The reverse lookup: who is `system` account `external_id`? An indexed
/// hit on `person_external_identity_account`.
///
/// Finding a row does **not** mean the person may be added to anything —
/// see the module header. This resolves an id; authority is a separate
/// question answered elsewhere.
///
/// # Errors
///
/// [`ExternalIdentityError::Db`] if the lookup fails.
pub async fn find_by_account(
    db: &SurrealDb,
    system: ExternalSystem,
    external_id: &str,
) -> Result<Option<ExternalIdentity>, ExternalIdentityError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM ONLY {TABLE} \
             WHERE system = $system AND external_id = $external_id \
             LIMIT 1"
        ))
        .bind(("system", system.as_str().to_string()))
        .bind(("external_id", external_id.trim().to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let row: Option<ExternalIdentityRow> = response.take(0)?;
    Ok(row.and_then(ExternalIdentityRow::into_identity))
}

/// Resolve the Person named by an external account id.
///
/// This is the provisioning-side relation resolver. It deliberately accepts
/// only the provider's stable account id and never takes, searches, or falls
/// back to a display name or email. Finding a Person here does not authorize
/// any action; the caller still applies the provider and Navigator policy
/// before making an outbound call.
///
/// # Errors
///
/// [`ExternalIdentityError::Db`] if either lookup fails.
pub async fn find_person_by_account(
    db: &SurrealDb,
    system: ExternalSystem,
    external_id: &str,
) -> Result<Option<persons::Person>, ExternalIdentityError> {
    let Some(identity) = find_by_account(db, system, external_id).await? else {
        return Ok(None);
    };
    persons::find_by_id(db, identity.person_id)
        .await
        .map_err(ExternalIdentityError::Person)
}

/// The Notion user ids for every attorney DRI on a Project.
///
/// The Projects database's `Lead` relation is a set: every lawyer DRI is
/// included, in stable external-id order. The relation is resolved from the
/// Notion identity rows only; names and email addresses are never used as
/// relation keys. A DRI without a Notion identity is omitted so an
/// unconfigured account cannot be mistaken for another user.
///
/// # Errors
///
/// [`ExternalIdentityError::Db`] if the participation or identity lookup
/// fails.
pub async fn notion_user_ids_for_project_leads(
    db: &SurrealDb,
    project_id: Uuid,
) -> Result<Vec<String>, ExternalIdentityError> {
    let mut response = db
        .query(
            "SELECT VALUE person_id FROM person_project_role \
             WHERE project_id = $project_id AND is_lawyer_dri = true",
        )
        .bind(("project_id", record_id("project", project_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let person_ids: Vec<surrealdb::types::RecordId> = response.take(0)?;

    let mut notion_ids = Vec::new();
    for person_id in person_ids {
        let Some(person_id) = record_uuid(&person_id) else {
            continue;
        };
        if let Some(identity) =
            find_by_person_and_system(db, person_id, ExternalSystem::Notion).await?
        {
            notion_ids.push(identity.external_id);
        }
    }
    notion_ids.sort();
    notion_ids.dedup();
    Ok(notion_ids)
}

/// Every identity recorded for one person, oldest first.
///
/// # Errors
///
/// [`ExternalIdentityError::Db`] if the lookup fails.
pub async fn for_person(
    db: &SurrealDb,
    person_id: Uuid,
) -> Result<Vec<ExternalIdentity>, ExternalIdentityError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE person_id = $person_id ORDER BY inserted_at ASC"
        ))
        .bind(("person_id", record_id(PERSON_TABLE, person_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;

    let rows: Vec<ExternalIdentityRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(ExternalIdentityRow::into_identity)
        .collect())
}

/// Link the identity unless this person already holds one on this
/// system. Race-safe without a lock, the same way
/// [`crate::credentials::find_or_grant`] is: a concurrent writer that
/// wins `person_external_identity_person_system` turns this call's
/// insert into [`ExternalIdentityError::SystemAlreadyLinked`], which is
/// re-read as the winner's row.
///
/// An existing row is returned as it stands — a differing `external_id`
/// does not overwrite it. Repointing a Person at a different account is
/// a correction someone should make deliberately through [`unlink`] and
/// [`link`], not a silent effect of the next import.
///
/// # Errors
///
/// [`ExternalIdentityError::NoSuchPerson`] as [`link`] documents,
/// [`ExternalIdentityError::AccountTaken`] when the account belongs to
/// somebody else — a real conflict, not a race — and
/// [`ExternalIdentityError::Db`] if a lookup or the insert fails.
pub async fn find_or_link(
    db: &SurrealDb,
    person_id: Uuid,
    system: ExternalSystem,
    external_id: &str,
    handle: Option<&str>,
) -> Result<ExternalIdentity, ExternalIdentityError> {
    if let Some(existing) = find_by_person_and_system(db, person_id, system).await? {
        return Ok(existing);
    }
    match link(db, person_id, system, external_id, handle).await {
        Ok(linked) => Ok(linked),
        Err(ExternalIdentityError::SystemAlreadyLinked { .. }) => {
            find_by_person_and_system(db, person_id, system)
                .await?
                .ok_or(ExternalIdentityError::WriteReturnedNothing)
        }
        Err(error) => Err(error),
    }
}

/// Set or clear one Person's identity on one external system.
///
/// Setting an existing identity updates its immutable account id only after
/// the database has checked the `(system, external_id)` unique index. Clearing
/// removes the row. This is the deliberate correction path for an admin or an
/// import; [`find_or_link`] intentionally keeps its existing-idempotent,
/// never-repoint behavior.
///
/// # Errors
///
/// [`ExternalIdentityError::NoSuchPerson`] when the Person does not exist,
/// [`ExternalIdentityError::AccountTaken`] when another Person holds the
/// account, or [`ExternalIdentityError::Db`] for another database failure.
pub async fn set_for_person(
    db: &SurrealDb,
    person_id: Uuid,
    system: ExternalSystem,
    external_id: Option<&str>,
) -> Result<Option<ExternalIdentity>, ExternalIdentityError> {
    if persons::find_by_id(db, person_id).await?.is_none() {
        return Err(ExternalIdentityError::NoSuchPerson(person_id));
    }
    let external_id = external_id.map(str::trim).filter(|id| !id.is_empty());
    let existing = find_by_person_and_system(db, person_id, system).await?;

    let Some(external_id) = external_id else {
        if let Some(identity) = existing {
            unlink(db, identity.id).await?;
        }
        return Ok(None);
    };

    if let Some(identity) = existing {
        if identity.external_id == external_id {
            return Ok(Some(identity));
        }
        let mut response = writing(system, || {
            db.query(format!(
                "UPDATE $id SET external_id = $external_id, updated_at = time::now() \
                 RETURN {SELECT}"
            ))
            .bind(("id", record_id(TABLE, identity.id)))
            .bind(("external_id", external_id.to_string()))
        })
        .await?;
        let row: Option<ExternalIdentityRow> = response.take(0)?;
        return row
            .and_then(ExternalIdentityRow::into_identity)
            .map(Some)
            .ok_or(ExternalIdentityError::WriteReturnedNothing);
    }

    link(db, person_id, system, external_id, None)
        .await
        .map(Some)
}

/// Forget an identity — the account was closed, or recorded in error.
/// Idempotent: unlinking one that is not there is a no-op.
///
/// # Errors
///
/// [`ExternalIdentityError::Db`] if the delete fails.
pub async fn unlink(db: &SurrealDb, identity_id: Uuid) -> Result<(), ExternalIdentityError> {
    db.query("DELETE $id")
        .bind(("id", record_id(TABLE, identity_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        find_by_account, find_by_person_and_system, find_or_link, find_person_by_account,
        for_person, link, notion_user_ids_for_project_leads, set_for_person, unlink,
        ExternalIdentityError, ExternalSystem, PERSON_TABLE,
    };
    use crate::git_access_tokens;
    use crate::persons::{self, NewPerson, Role};
    use crate::surreal::test_support::mem;
    use crate::surreal::{record_id, SurrealDb};
    use uuid::Uuid;

    async fn a_person(db: &SurrealDb, email: &str, role: Role) -> Uuid {
        persons::create(db, &NewPerson::with_role("Scorpio", email, role))
            .await
            .unwrap()
            .id
    }

    /// The vocabulary is closed, and it fails closed. There is no
    /// `Other(String)` to fall into, so a system Navigator cannot name
    /// is refused here rather than reaching a call site as a value that
    /// looks legitimate.
    #[test]
    fn parse_refuses_a_system_the_vocabulary_does_not_name() {
        for unknown in ["", "GitHub", "github ", "gitlab", "bitbucket", "xero"] {
            assert_eq!(
                ExternalSystem::parse(unknown),
                None,
                "{unknown:?} is not a system this build knows"
            );
        }
    }

    #[test]
    fn every_system_round_trips_through_its_stored_spelling() {
        for system in ExternalSystem::ALL {
            assert_eq!(ExternalSystem::parse(system.as_str()), Some(*system));
        }
    }

    /// The enum and the schema's ASSERT are two spellings of one
    /// vocabulary, and a variant added to only one of them is either a
    /// value the engine refuses or a value nothing can read back.
    #[test]
    fn the_assert_and_the_enum_name_the_same_systems() {
        let definitions = include_str!("schema/navigator.surql");
        let assertion = definitions
            .lines()
            .skip_while(|line| !line.contains("system ON person_external_identity"))
            .nth(1)
            .expect("the ASSERT sits on the line after the system field definition");

        for system in ExternalSystem::ALL {
            assert!(
                assertion.contains(&format!("'{}'", system.as_str())),
                "{} is an ExternalSystem variant the ASSERT does not admit: {assertion}",
                system.as_str()
            );
        }
        let admitted = assertion.matches('\'').count() / 2;
        assert_eq!(
            admitted,
            ExternalSystem::ALL.len(),
            "the ASSERT admits {admitted} systems and the enum names {}: {assertion}",
            ExternalSystem::ALL.len()
        );
    }

    #[tokio::test]
    async fn a_linked_identity_reads_back_by_person_and_by_account() {
        let db = mem().await;
        let person_id = a_person(&db, "scorpio@example.com", Role::Lawyer).await;

        let linked = link(
            &db,
            person_id,
            ExternalSystem::Github,
            "12345",
            Some("scorpio"),
        )
        .await
        .unwrap();

        assert_eq!(linked.person_id, person_id);
        assert_eq!(linked.system, ExternalSystem::Github);
        assert_eq!(linked.external_id, "12345");
        assert_eq!(linked.handle.as_deref(), Some("scorpio"));

        assert_eq!(
            find_by_person_and_system(&db, person_id, ExternalSystem::Github)
                .await
                .unwrap(),
            Some(linked.clone())
        );
        assert_eq!(
            find_by_account(&db, ExternalSystem::Github, "12345")
                .await
                .unwrap(),
            Some(linked.clone()),
            "the reverse lookup is what a provisioning call resolves through"
        );
        assert_eq!(for_person(&db, person_id).await.unwrap(), vec![linked]);
    }

    #[tokio::test]
    async fn notion_user_ids_resolve_to_people_by_stable_id_only() {
        let db = mem().await;
        let person_id = a_person(&db, "notion@example.com", Role::Lawyer).await;
        link(
            &db,
            person_id,
            ExternalSystem::Notion,
            "notion-user-123",
            Some("Renamed"),
        )
        .await
        .unwrap();

        assert_eq!(
            find_person_by_account(&db, ExternalSystem::Notion, "notion-user-123")
                .await
                .unwrap()
                .unwrap()
                .id,
            person_id
        );
        assert!(
            find_person_by_account(&db, ExternalSystem::Notion, "Renamed")
                .await
                .unwrap()
                .is_none(),
            "a Notion display name is never a relation key"
        );
        assert!(
            find_person_by_account(&db, ExternalSystem::Notion, "notion@example.com")
                .await
                .unwrap()
                .is_none(),
            "a Person email is never a Notion relation key"
        );
    }

    #[tokio::test]
    async fn notion_identity_correction_preserves_uniqueness_and_supports_clearing() {
        let db = mem().await;
        let first = a_person(&db, "first@example.com", Role::Lawyer).await;
        let second = a_person(&db, "second@example.com", Role::Lawyer).await;
        set_for_person(&db, first, ExternalSystem::Notion, Some("notion-1"))
            .await
            .unwrap();

        let duplicate = set_for_person(&db, second, ExternalSystem::Notion, Some("notion-1")).await;
        assert!(matches!(
            duplicate,
            Err(ExternalIdentityError::AccountTaken {
                system: ExternalSystem::Notion
            })
        ));

        set_for_person(&db, first, ExternalSystem::Notion, Some("notion-2"))
            .await
            .unwrap();
        assert!(find_by_account(&db, ExternalSystem::Notion, "notion-1")
            .await
            .unwrap()
            .is_none());
        set_for_person(&db, first, ExternalSystem::Notion, None)
            .await
            .unwrap();
        assert!(
            find_by_person_and_system(&db, first, ExternalSystem::Notion)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn project_lead_notion_ids_include_every_attorney_dri() {
        let db = mem().await;
        let first = a_person(&db, "first@example.com", Role::Lawyer).await;
        let second = a_person(&db, "second@example.com", Role::Admin).await;
        let unrelated = a_person(&db, "unrelated@example.com", Role::Lawyer).await;
        let project = crate::projects::create(
            &db,
            &crate::projects::NewProject {
                code: "sample-notion-leads".into(),
                name: "Sample".into(),
                status: "open".into(),
                entity_id: Uuid::now_v7(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let project_id = project.id;
        for person_id in [first, second] {
            crate::projects::designate_dri_in_surreal(
                &db,
                project_id,
                person_id,
                crate::projects::DriSide::Lawyer,
            )
            .await
            .unwrap();
        }
        link(&db, first, ExternalSystem::Notion, "notion-z", None)
            .await
            .unwrap();
        link(&db, second, ExternalSystem::Notion, "notion-a", None)
            .await
            .unwrap();
        link(
            &db,
            unrelated,
            ExternalSystem::Notion,
            "notion-unrelated",
            None,
        )
        .await
        .unwrap();

        assert_eq!(
            notion_user_ids_for_project_leads(&db, project_id)
                .await
                .unwrap(),
            ["notion-a", "notion-z"]
        );
    }

    /// The immutable id is the key and the handle is not, so a rename
    /// leaves the mapping intact — the failure mode `github_handle` had.
    #[tokio::test]
    async fn the_account_lookup_keys_on_the_id_and_not_the_handle() {
        let db = mem().await;
        let person_id = a_person(&db, "scorpio@example.com", Role::Lawyer).await;
        link(
            &db,
            person_id,
            ExternalSystem::Github,
            "12345",
            Some("old-handle"),
        )
        .await
        .unwrap();

        assert!(find_by_account(&db, ExternalSystem::Github, "12345")
            .await
            .unwrap()
            .is_some());
        assert!(
            find_by_account(&db, ExternalSystem::Github, "old-handle")
                .await
                .unwrap()
                .is_none(),
            "the handle is display only and must not resolve an account"
        );
    }

    /// The `(system, external_id)` unique: two Persons cannot claim one
    /// account.
    #[tokio::test]
    async fn a_second_person_claiming_one_account_is_refused() {
        let db = mem().await;
        let first = a_person(&db, "first@example.com", Role::Lawyer).await;
        let second = a_person(&db, "second@example.com", Role::Lawyer).await;

        link(&db, first, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();
        let duplicate = link(&db, second, ExternalSystem::Github, "12345", None).await;

        assert!(
            matches!(
                duplicate,
                Err(ExternalIdentityError::AccountTaken {
                    system: ExternalSystem::Github
                })
            ),
            "the unique `person_external_identity_account` index is the gate, got {duplicate:?}"
        );
        assert!(for_person(&db, second).await.unwrap().is_empty());
    }

    /// The `(person_id, system)` unique: one Person holds at most one
    /// account per system.
    #[tokio::test]
    async fn a_second_account_for_one_person_on_one_system_is_refused() {
        let db = mem().await;
        let person_id = a_person(&db, "scorpio@example.com", Role::Lawyer).await;

        link(&db, person_id, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();
        let duplicate = link(&db, person_id, ExternalSystem::Github, "67890", None).await;

        assert!(
            matches!(
                duplicate,
                Err(ExternalIdentityError::SystemAlreadyLinked {
                    system: ExternalSystem::Github
                })
            ),
            "the unique `person_external_identity_person_system` index is the gate, \
             got {duplicate:?}"
        );
        assert_eq!(for_person(&db, person_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn one_person_may_hold_an_account_on_every_system() {
        let db = mem().await;
        let person_id = a_person(&db, "scorpio@example.com", Role::Lawyer).await;

        for (index, system) in ExternalSystem::ALL.iter().enumerate() {
            link(&db, person_id, *system, &format!("id-{index}"), None)
                .await
                .unwrap();
        }

        assert_eq!(
            for_person(&db, person_id).await.unwrap().len(),
            ExternalSystem::ALL.len()
        );
    }

    #[tokio::test]
    async fn an_identity_for_a_person_who_does_not_exist_is_refused() {
        let db = mem().await;
        let nobody = Uuid::now_v7();

        // The engine would accept this link — `record<person>` is not
        // validated against an existing row. This check is the only
        // thing that refuses it.
        let refused = link(&db, nobody, ExternalSystem::Github, "12345", None).await;
        assert!(matches!(
            refused,
            Err(ExternalIdentityError::NoSuchPerson(id)) if id == nobody
        ));
    }

    #[tokio::test]
    async fn the_engine_itself_would_have_accepted_the_dangling_link() {
        let db = mem().await;

        // Pins the reason `link` checks: writing the same row without
        // the guard succeeds, so nothing below this module catches it.
        db.query("CREATE $id SET person_id = $person, system = 'github', external_id = '12345'")
            .bind(("id", record_id("person_external_identity", Uuid::now_v7())))
            .bind(("person", record_id(PERSON_TABLE, Uuid::now_v7())))
            .await
            .unwrap()
            .check()
            .expect("the engine does not validate a record link");
    }

    /// The ASSERT is the engine-side half of the closed vocabulary:
    /// `parse` refuses to read an unknown system back, and this refuses
    /// to write one in the first place.
    #[tokio::test]
    async fn the_schema_refuses_a_system_outside_the_vocabulary() {
        let db = mem().await;

        let rejected = db
            .query("CREATE $id SET person_id = $person, system = 'gitlab', external_id = '1'")
            .bind(("id", record_id("person_external_identity", Uuid::now_v7())))
            .bind(("person", record_id(PERSON_TABLE, Uuid::now_v7())))
            .await
            .unwrap()
            .check();

        assert!(rejected.is_err(), "the engine accepted system 'gitlab'");
    }

    #[tokio::test]
    async fn find_or_link_is_idempotent_on_the_person_system_pair() {
        let db = mem().await;
        let person_id = a_person(&db, "scorpio@example.com", Role::Lawyer).await;

        let first = find_or_link(&db, person_id, ExternalSystem::Slack, "U123", Some("s"))
            .await
            .unwrap();
        let second = find_or_link(&db, person_id, ExternalSystem::Slack, "U999", Some("t"))
            .await
            .unwrap();

        assert_eq!(first, second, "the second call returns the existing row");
        assert_eq!(
            second.external_id, "U123",
            "an existing identity is returned as it stands, not repointed"
        );
        assert_eq!(for_person(&db, person_id).await.unwrap().len(), 1);
    }

    /// An account another person holds is a real conflict, not a race,
    /// so find-or-link must surface it rather than swallow it the way it
    /// swallows its own index collision.
    #[tokio::test]
    async fn find_or_link_still_reports_an_account_another_person_holds() {
        let db = mem().await;
        let first = a_person(&db, "first@example.com", Role::Lawyer).await;
        let second = a_person(&db, "second@example.com", Role::Lawyer).await;
        link(&db, first, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();

        let refused = find_or_link(&db, second, ExternalSystem::Github, "12345", None).await;
        assert!(matches!(
            refused,
            Err(ExternalIdentityError::AccountTaken {
                system: ExternalSystem::Github
            })
        ));
    }

    #[tokio::test]
    async fn an_identity_is_scoped_to_its_own_person() {
        let db = mem().await;
        let linked = a_person(&db, "linked@example.com", Role::Lawyer).await;
        let unlinked = a_person(&db, "unlinked@example.com", Role::Lawyer).await;
        link(&db, linked, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();

        assert!(for_person(&db, unlinked).await.unwrap().is_empty());
        assert_eq!(
            find_by_person_and_system(&db, unlinked, ExternalSystem::Github)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn unlinking_forgets_the_identity_and_is_idempotent() {
        let db = mem().await;
        let person_id = a_person(&db, "scorpio@example.com", Role::Lawyer).await;
        let linked = link(&db, person_id, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();

        unlink(&db, linked.id).await.unwrap();
        assert!(for_person(&db, person_id).await.unwrap().is_empty());

        // Unlinking again, and unlinking one that never existed, are
        // no-ops.
        unlink(&db, linked.id).await.unwrap();
        unlink(&db, Uuid::now_v7()).await.unwrap();
    }

    /// An account released by one person can be recorded for another —
    /// the unique index must not keep the pair reserved after the row is
    /// gone.
    #[tokio::test]
    async fn a_released_account_can_be_claimed_by_someone_else() {
        let db = mem().await;
        let leaver = a_person(&db, "leaver@example.com", Role::Lawyer).await;
        let joiner = a_person(&db, "joiner@example.com", Role::Lawyer).await;

        let held = link(&db, leaver, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();
        unlink(&db, held.id).await.unwrap();

        let reclaimed = link(&db, joiner, ExternalSystem::Github, "12345", None)
            .await
            .unwrap();
        assert_eq!(reclaimed.person_id, joiner);
    }

    /// The glossary rule, held as behaviour: a Clerk "never receives
    /// lawyer-work, advice, Git, MCP, or `/app/lawyer` authority by
    /// inheritance." Recording that a Clerk *is* a GitHub user is a
    /// record of who they are; it is not a grant, and every authority
    /// answer is the same before and after the row exists.
    #[tokio::test]
    async fn a_clerk_carrying_a_github_identity_is_granted_no_git_authority() {
        let db = mem().await;
        let clerk = a_person(&db, "clerk@example.com", Role::Clerk).await;

        // The Git transport's own authority is a token, and a Clerk who
        // was never minted one has none — before the identity, and
        // after it.
        let before = git_access_tokens::validate(&db, "not-a-token", chrono::Utc::now())
            .await
            .unwrap();
        assert!(before.is_none());

        let identity = link(
            &db,
            clerk,
            ExternalSystem::Github,
            "12345",
            Some("clerk-handle"),
        )
        .await
        .unwrap();

        assert!(
            git_access_tokens::validate(&db, "not-a-token", chrono::Utc::now())
                .await
                .unwrap()
                .is_none(),
            "an external identity is not a Git credential"
        );
        assert!(
            git_access_tokens::validate(&db, &identity.external_id, chrono::Utc::now())
                .await
                .unwrap()
                .is_none(),
            "the account id must not authenticate anything"
        );

        // And the tier is untouched: the row records who they are, it
        // does not promote them.
        assert_eq!(
            persons::find_by_id(&db, clerk).await.unwrap().unwrap().role,
            Role::Clerk
        );
    }

    /// The other half of the same rule, from ENG-45 and
    /// `docs/project-repositories.md`: Project participation never grants
    /// GHE access, and an identity does not *become* participation. A
    /// `client` holding a `google` identity for Drive sharing is
    /// legitimate, and scope still comes from `person_project_role`
    /// alone.
    #[tokio::test]
    async fn linking_an_identity_creates_no_participation() {
        let db = mem().await;
        let client = a_person(&db, "client@example.com", Role::Client).await;

        link(&db, client, ExternalSystem::Google, "sub-12345", None)
            .await
            .unwrap();

        let participations: Vec<surrealdb::types::RecordId> = db
            .query("SELECT VALUE id FROM person_project_role")
            .await
            .unwrap()
            .check()
            .unwrap()
            .take(0)
            .unwrap();
        assert!(
            participations.is_empty(),
            "scope comes from person_project_role, and an identity row writes none"
        );

        // One row, in one table, and it is this one.
        assert_eq!(for_person(&db, client).await.unwrap().len(), 1);
    }

    /// This change adds a vocabulary; it does not touch the one that
    /// decides authority. The five stored tiers are the whole ladder in
    /// `docs/access-model.md`, the schema's ASSERT still admits exactly
    /// those five, and an `ExternalSystem` is not a sixth.
    #[test]
    fn the_role_vocabulary_is_untouched() {
        let ladder = [
            Role::Owner,
            Role::Admin,
            Role::Lawyer,
            Role::Clerk,
            Role::Client,
        ];
        for role in ladder {
            assert_eq!(Role::parse(role.as_str()), Some(role));
        }

        let definitions = include_str!("schema/navigator.surql");
        let assertion = definitions
            .lines()
            .skip_while(|line| !line.contains("role ON person TYPE string"))
            .nth(1)
            .expect("the ASSERT sits on the line after the role field definition");
        for role in ladder {
            assert!(
                assertion.contains(&format!("'{}'", role.as_str())),
                "{} left the role ASSERT: {assertion}",
                role.as_str()
            );
        }
        assert_eq!(
            assertion.matches('\'').count() / 2,
            ladder.len(),
            "the role ASSERT no longer admits exactly the five tiers: {assertion}"
        );

        for system in ExternalSystem::ALL {
            assert_eq!(
                Role::parse(system.as_str()),
                None,
                "{} must not read as an authorization tier",
                system.as_str()
            );
        }
    }
}
