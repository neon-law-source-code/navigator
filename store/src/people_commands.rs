//! Shared People command layer for every write adapter.
//!
//! Every People mutation — create, update, delete — and the fuzzy
//! People lookup live here so the JSON `/app/api/people*` surface, the
//! browser lawyer forms, and the AIDA MCP tools travel one command
//! boundary. This crate owns the persistence and business rules
//! (validation, role rules, the bootstrap-owner guards, duplicate-email
//! conflicts); the adapters render and authorize but never re-implement
//! the write. This module carries no HTTP or email machinery, so `mcp`
//! and `cli` can call it directly; the welcome-email command, which
//! needs the mailer, stays in `web`.

use serde::Deserialize;
use uuid::Uuid;

use crate::external_identities::{self, ExternalIdentityError, ExternalSystem};
use crate::persons::{self, ContactUpdate, NewPerson, Person, PersonEdit, PersonError, Role};
use crate::surreal::SurrealDb;

/// Request body for creating a Person through the command boundary.
#[derive(Debug, Deserialize)]
pub struct CreatePersonCommand {
    pub name: String,
    pub email: String,
    /// `owner`, `admin`, `lawyer`, `clerk`, or `client`. Missing or blank values fall
    /// back to `client`.
    #[serde(default)]
    pub role: String,
    /// Structured legal-name parts for filings that split the name.
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default)]
    pub middle_name: Option<String>,
    /// The stable Notion workspace user id. Stored in the external-identity
    /// table, never on the Person row.
    #[serde(default)]
    pub notion_user_id: Option<String>,
    /// Which Firm a new Lawyer or Clerk joins (ENG-495). Omitted or blank
    /// defaults to the deployment's anchor Firm
    /// ([`crate::firms::anchor_firm`]); a creating surface names a different
    /// one by supplying it. Read only for a `lawyer`/`clerk` role — Owner and
    /// Admin membership stays explicit and out of scope, and a `client`
    /// reaches a matter through `person_project_role`, never this join.
    #[serde(default)]
    pub firm_id: Option<Uuid>,
}

impl CreatePersonCommand {
    #[must_use]
    pub fn validation_message(&self) -> Option<&'static str> {
        validate_name_email(&self.name, &self.email).or_else(|| role_validation_message(&self.role))
    }
}

/// Request body for updating a Person through the command boundary.
///
/// A blank/absent `role` preserves the row's existing role rather than
/// resetting it. The structured name parts use a **double option** so an
/// *omitted* field (outer `None`) is left untouched, while a *present*
/// field — whether JSON `null`, a blank string, or a value — is applied:
/// `null`/blank clear the column, a value sets it. That lets a JSON
/// client following the nullable schema clear a stale legal-name part,
/// which a single `Option` (where `null` and "omitted" collapse to the
/// same `None`) could not express.
#[derive(Debug, Deserialize)]
// `Option<Option<String>>` is deliberate here: the outer option is
// "field present?" and the inner is "null vs a value", which is exactly
// the PATCH clear-vs-preserve distinction. That's the sanctioned use the
// `option_option` lint warns is usually a mistake — it isn't one here.
#[allow(clippy::option_option)]
pub struct UpdatePersonCommand {
    pub name: String,
    /// Defaults to blank when omitted: the bootstrap Owner's email field
    /// renders `disabled` (see `webapp::person_show`), and a disabled HTML
    /// control submits nothing at all. `update_person` ignores this value
    /// for that row regardless, so a missing key is never mistaken for a
    /// blank-out on an ordinary person either.
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub role: String,
    #[serde(default, deserialize_with = "double_option")]
    pub given_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub family_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub middle_name: Option<Option<String>>,
    /// Omit to preserve the Notion identity; send null or blank to clear it.
    #[serde(default, deserialize_with = "double_option")]
    pub notion_user_id: Option<Option<String>>,
    /// The public `LinkedIn` profile URL shown on `/team`. Omit to preserve
    /// it; send null or blank to clear it.
    #[serde(default, deserialize_with = "double_option")]
    pub linkedin_url: Option<Option<String>>,
}

/// Deserialize a "double option" that keeps a present JSON `null`
/// distinct from an absent field. `#[serde(default)]` supplies the outer
/// `None` when the key is missing; this runs only when the key is
/// present, so a bare derive's `null → None` becomes `null → Some(None)`
/// (clear) and a value becomes `Some(Some(value))` (set).
#[allow(clippy::option_option)] // the doubled option is the whole point — see the DTO.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

/// The per-request authorization facts an update needs, resolved from
/// the caller's session/config by the adapter (web route or API handler)
/// so the command itself stays free of session types.
#[derive(Debug, Clone, Copy)]
pub struct UpdateContext<'a> {
    /// The configured `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL`, if any. When it
    /// matches the target row the role is pinned to `owner` no matter
    /// what the request body carries, so an accidental (or hostile)
    /// demotion can't lock out the grant.
    pub bootstrap_owner_email: Option<&'a str>,
    /// The caller's stored system role. A caller cannot edit a higher-ranked
    /// person, even when the surrounding route is otherwise writable.
    pub actor_role: Role,
    /// The caller may set roles (Owner/Admin, not impersonating). When false,
    /// a submitted role is ignored and the existing role is preserved.
    pub may_change_roles: bool,
}

#[derive(Debug)]
pub enum PeopleCommandError {
    Invalid(&'static str),
    /// The email belongs to another person. Raised by
    /// [`PersonError::EmailTaken`], which the `person_email_lower`
    /// unique index produces.
    EmailConflict,
    /// No Person with the requested id.
    NotFound,
    /// The action is refused by a business rule (e.g. deleting the
    /// bootstrap Owner). Carries a human-readable reason.
    Blocked(&'static str),
    /// The welcome email could not be dispatched. The row exists; the
    /// send failed. Constructed by the `web` welcome command.
    SendFailed,
    Db(PersonError),
    /// A Notion identity belongs to another Person, or the external identity
    /// store rejected the update. No identity value is included in the user
    /// message.
    ExternalIdentity,
    /// ENG-495: granting the default Firm membership on create failed for a
    /// reason other than a bad `firm_id` (mapped to [`Self::Invalid`]
    /// instead, since a caller naming one can correct it).
    FirmMembership(String),
}

impl PeopleCommandError {
    /// The message to show a human — a toast on the People forms, or the
    /// error banner in the inline client-create modal. Kept here so every
    /// adapter renders the same wording per failure.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            PeopleCommandError::Invalid(m) | PeopleCommandError::Blocked(m) => (*m).to_string(),
            PeopleCommandError::EmailConflict => "That email is already in use.".to_string(),
            PeopleCommandError::NotFound => "That person no longer exists.".to_string(),
            PeopleCommandError::SendFailed => {
                "Couldn't send the welcome email. Check the email log.".to_string()
            }
            PeopleCommandError::Db(_) => "Something went wrong. Please try again.".to_string(),
            PeopleCommandError::ExternalIdentity => {
                "That Notion user is already linked to another person.".to_string()
            }
            PeopleCommandError::FirmMembership(_) => {
                "Something went wrong. Please try again.".to_string()
            }
        }
    }
}

/// A trimmed form value, or `None` when it is blank.
#[must_use]
pub fn none_if_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[must_use]
pub fn parse_role(s: &str) -> Option<Role> {
    Role::parse(s)
}

/// `true` when `configured` names the same email as `row_email`,
/// case-insensitively — i.e. the row is the bootstrap Owner.
#[must_use]
pub fn is_bootstrap_owner_email(configured: Option<&str>, row_email: &str) -> bool {
    matches!(configured, Some(e) if e.eq_ignore_ascii_case(row_email))
}

/// Shared name/email shape check: name required, email must contain an
/// `@` and carry no internal whitespace.
fn validate_name_email(name: &str, email: &str) -> Option<&'static str> {
    let email = email.trim();
    if name.trim().is_empty() || !email.contains('@') || email.chars().any(char::is_whitespace) {
        Some("Name is required and email must contain an @.")
    } else {
        None
    }
}

fn role_validation_message(s: &str) -> Option<&'static str> {
    match s.trim() {
        "" | "owner" | "admin" | "lawyer" | "clerk" | "client" => None,
        _ => Some("Role must be owner, admin, lawyer, clerk, or client."),
    }
}

/// A blank or unknown role on a create falls back to `client`. The
/// validation above has already rejected an unknown one, so this only
/// ever resolves the blank.
fn parse_create_role(s: &str) -> Role {
    Role::parse(s).unwrap_or_default()
}

/// Map a write failure to the right command error: the email conflict is
/// caller-correctable, everything else a server-side fault. The OIDC
/// conflict cannot reach here — nothing in this boundary writes
/// `oidc_subject`.
fn classify_write(error: PersonError) -> PeopleCommandError {
    match error {
        PersonError::EmailTaken => PeopleCommandError::EmailConflict,
        other => PeopleCommandError::Db(other),
    }
}

fn classify_external_identity(_error: ExternalIdentityError) -> PeopleCommandError {
    PeopleCommandError::ExternalIdentity
}

fn classify_firm_write(error: crate::firms::FirmError) -> PeopleCommandError {
    match error {
        crate::firms::FirmError::NoSuchFirm(_) => PeopleCommandError::Invalid("No such firm."),
        other => PeopleCommandError::FirmMembership(other.to_string()),
    }
}

/// ENG-495: a newly created Lawyer or Clerk joins a Firm the same way the
/// one-time backfill (#ENG-462) pointed every *existing* person at one —
/// except this runs on every create, not once. `input.firm_id` lets the
/// creating surface name a different Firm; omitted, it defaults to
/// [`crate::firms::anchor_firm`]. Owner and Admin are untouched (their
/// membership stays explicit), and a Client never reaches this branch at
/// all — `store::firms::add_membership` would refuse it, but a Client should
/// never even attempt the write.
///
/// A deployment with no anchor Firm yet (no other Firm named either) grants
/// nothing rather than failing the person create over it: the omission is
/// exactly what this issue reports being invisible, but it is not this door's
/// place to invent a Firm.
async fn grant_default_firm_membership(
    db: &SurrealDb,
    person: &Person,
    firm_id: Option<Uuid>,
) -> Result<(), PeopleCommandError> {
    // Matched on the tier itself, not on whether `FirmMembership::for_role`
    // returns `Some` — that also holds for Admin, whose membership this
    // issue explicitly leaves explicit and out of scope.
    let membership = match person.role {
        Role::Lawyer => crate::firms::FirmMembership::Lawyer,
        Role::Clerk => crate::firms::FirmMembership::Clerk,
        Role::Owner | Role::Admin | Role::Client => return Ok(()),
    };
    let firm_id = match firm_id {
        Some(id) => Some(id),
        None => crate::firms::anchor_firm(db)
            .await
            .map_err(classify_firm_write)?
            .map(|firm| firm.id),
    };
    let Some(firm_id) = firm_id else {
        return Ok(());
    };
    crate::firms::ensure_membership(
        db,
        &crate::firms::NewPersonFirmRole {
            person_id: person.id,
            firm_id,
            membership,
            is_dri: false,
        },
    )
    .await
    .map_err(classify_firm_write)
}

pub async fn create_person(
    db: &SurrealDb,
    input: &CreatePersonCommand,
) -> Result<Person, PeopleCommandError> {
    if let Some(message) = input.validation_message() {
        return Err(PeopleCommandError::Invalid(message));
    }
    let created = persons::create(
        db,
        &NewPerson {
            role: parse_create_role(&input.role),
            given_name: none_if_blank(input.given_name.as_deref()),
            family_name: none_if_blank(input.family_name.as_deref()),
            middle_name: none_if_blank(input.middle_name.as_deref()),
            ..NewPerson::new(input.name.trim(), input.email.trim())
        },
    )
    .await
    .map_err(classify_write)?;
    if input.notion_user_id.is_some() {
        if let Err(error) = external_identities::set_for_person(
            db,
            created.id,
            ExternalSystem::Notion,
            input.notion_user_id.as_deref(),
        )
        .await
        {
            let _ = persons::delete(db, created.id).await;
            return Err(classify_external_identity(error));
        }
    }
    if let Err(error) = grant_default_firm_membership(db, &created, input.firm_id).await {
        let _ = persons::delete(db, created.id).await;
        return Err(error);
    }
    Ok(created)
}

/// Update one Person by id. The bootstrap Owner's email and role are pinned —
/// any submitted change to either is silently dropped rather than rejecting
/// the whole write — while its name and legal-name parts edit normally.
/// Otherwise rejects callers attempting to edit a higher-ranked person, and
/// preserves the existing role when the caller can't change roles or submits
/// a blank role; leaves an omitted structured-name part untouched and nulls a
/// present-but-blank one.
pub async fn update_person(
    db: &SurrealDb,
    id: Uuid,
    input: &UpdatePersonCommand,
    ctx: &UpdateContext<'_>,
) -> Result<Person, PeopleCommandError> {
    let existing = persons::find_by_id(db, id)
        .await
        .map_err(PeopleCommandError::Db)?
        .ok_or(PeopleCommandError::NotFound)?;

    // The bootstrap Owner's identity is pinned: the email that keys the
    // `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` carve-out (and, by extension, its
    // role) cannot move through this boundary. A submitted change to either
    // is silently dropped — the same preserve-not-error pattern the
    // `may_change_roles` branch below already uses for a caller without role
    // authority — rather than rejecting the whole write, since name and the
    // legal-name parts are not what `oauth::resolve_person_from_claims` keys
    // its re-tag on and edit like any other person's.
    let is_bootstrap = is_bootstrap_owner_email(ctx.bootstrap_owner_email, &existing.email);
    let email = if is_bootstrap {
        existing.email.clone()
    } else {
        input.email.trim().to_string()
    };

    if let Some(message) = validate_name_email(&input.name, &email) {
        return Err(PeopleCommandError::Invalid(message));
    }
    if existing.role.authority_rank() > ctx.actor_role.authority_rank() {
        return Err(PeopleCommandError::Blocked(
            "You cannot edit a person with a higher system role.",
        ));
    }

    let new_role = if is_bootstrap {
        Role::Owner
    } else if !ctx.may_change_roles || input.role.trim().is_empty() {
        existing.role
    } else {
        let requested = parse_role(&input.role).ok_or(PeopleCommandError::Invalid(
            "Role must be owner, admin, lawyer, clerk, or client.",
        ))?;
        if requested.authority_rank() > ctx.actor_role.authority_rank() {
            return Err(PeopleCommandError::Blocked(
                "You cannot assign a system role above your own.",
            ));
        }
        requested
    };

    let updated = persons::edit(
        db,
        id,
        &PersonEdit {
            name: Some(input.name.trim().to_string()),
            email: Some(email),
            role: Some(new_role),
            // Only touch a name part when the request carried it (outer
            // `Some`), so a caller that posts just name/email/role leaves
            // the structured legal name untouched rather than nulling it
            // out from under a future N-400. A present `null`/blank
            // clears the column; a value sets it.
            given_name: input
                .given_name
                .as_ref()
                .map(|part| none_if_blank(part.as_deref())),
            family_name: input
                .family_name
                .as_ref()
                .map(|part| none_if_blank(part.as_deref())),
            middle_name: input
                .middle_name
                .as_ref()
                .map(|part| none_if_blank(part.as_deref())),
            profile_image_url: None,
            linkedin_url: input
                .linkedin_url
                .as_ref()
                .map(|part| none_if_blank(part.as_deref())),
        },
    )
    .await
    .map_err(classify_write)?
    .ok_or(PeopleCommandError::NotFound)?;

    if let Some(notion_user_id) = input.notion_user_id.as_ref() {
        external_identities::set_for_person(
            db,
            id,
            ExternalSystem::Notion,
            notion_user_id.as_deref(),
        )
        .await
        .map_err(classify_external_identity)?;
    }
    Ok(updated)
}

/// Delete one Person by id. Only **client** records are deletable: a
/// lawyer can't delete another privileged person, so a non-client
/// target is refused here at the command boundary (the lawyer People list
/// also hides the control, but this is the enforcing check a hand-crafted
/// `DELETE` still hits). The configured bootstrap Owner is undeletable on
/// top of that — without the guard an admin could wipe the row that
/// `oauth::resolve_person_from_claims` re-tags on every login, locking out
/// the grant. Returns the deleted row on success.
pub async fn delete_person(
    db: &SurrealDb,
    id: Uuid,
    bootstrap_owner_email: Option<&str>,
) -> Result<Person, PeopleCommandError> {
    let target = persons::find_by_id(db, id)
        .await
        .map_err(PeopleCommandError::Db)?
        .ok_or(PeopleCommandError::NotFound)?;
    if is_bootstrap_owner_email(bootstrap_owner_email, &target.email) {
        return Err(PeopleCommandError::Blocked(
            "Cannot delete the bootstrap Owner person (configured via NAVIGATOR_BOOTSTRAP_OWNER_EMAIL).",
        ));
    }
    if target.role != Role::Client {
        return Err(PeopleCommandError::Blocked(
            "Only client records can be deleted. Owner, admin, lawyer, and clerk people are edit-only.",
        ));
    }
    persons::delete(db, id)
        .await
        .map_err(PeopleCommandError::Db)?;
    Ok(target)
}

/// Fuzzy-find people by an optional name and/or email substring. Both
/// needles are matched case-insensitively as substrings and ANDed when
/// both are supplied; the caller is responsible for rejecting the
/// all-`None` case (a blank query would return the whole directory).
/// Results are ordered by name and capped at `limit`.
///
/// This is the read half of the People command boundary: the AIDA
/// `aida_show_person` tool and any web lookup share this one query
/// instead of re-implementing the `LIKE` predicate.
pub async fn search_people(
    db: &SurrealDb,
    name: Option<&str>,
    email: Option<&str>,
    limit: u64,
) -> Result<Vec<Person>, PersonError> {
    persons::search(db, name, email, limit).await
}

/// Apply a directory import's contact facts. The import's own
/// find-or-create decides *which* row; this is the write half, and it
/// deliberately cannot reach `email` or `role`.
///
/// # Errors
///
/// [`PeopleCommandError::NotFound`] when the person no longer exists.
pub async fn update_person_contact(
    db: &SurrealDb,
    id: Uuid,
    input: &ContactUpdate,
) -> Result<Person, PeopleCommandError> {
    persons::update_contact(db, id, input)
        .await
        .map_err(classify_write)?
        .ok_or(PeopleCommandError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::{
        create_person, delete_person, search_people, update_person, CreatePersonCommand,
        PeopleCommandError, UpdateContext, UpdatePersonCommand,
    };
    use crate::persons::{self, NewPerson, Role};
    use crate::surreal::test_support::mem;
    use crate::surreal::SurrealDb;

    async fn db() -> SurrealDb {
        mem().await
    }

    fn create(name: &str, email: &str) -> CreatePersonCommand {
        CreatePersonCommand {
            name: name.into(),
            email: email.into(),
            role: String::new(),
            given_name: None,
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            firm_id: None,
        }
    }

    /// An Entity carrying `firm_anchor_key`, wrapped in a Firm — the shape
    /// [`crate::firms::anchor_firm`] resolves. No test here sets
    /// `NAVIGATOR_BOOTSTRAP_COMPANY` (a process-wide variable every parallel
    /// test would race on), so the default fallback,
    /// [`crate::seed::FIRM_ENTITY_NAME`], is always the key.
    async fn anchor_firm(db: &SurrealDb) -> crate::firms::Firm {
        let entity_id = crate::entities::create(
            db,
            &crate::entities::NewEntity {
                name: crate::seed::FIRM_ENTITY_NAME.to_string(),
                entity_type_id: crate::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: crate::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                firm_anchor_key: Some(crate::seed::FIRM_ENTITY_NAME.to_lowercase()),
            },
        )
        .await
        .unwrap()
        .id;
        crate::firms::create(
            db,
            &crate::firms::NewFirm {
                name: crate::seed::FIRM_ENTITY_NAME.to_string(),
                status: "active".to_string(),
                entity_id,
            },
        )
        .await
        .unwrap()
    }

    /// An ordinary Firm — no `firm_anchor_key` on its Entity — for the
    /// two-firm case: a deployment holding this one *and* the anchor.
    async fn ordinary_firm(db: &SurrealDb, name: &str) -> crate::firms::Firm {
        let entity_id = crate::test_support::seed_entity(db).await;
        crate::firms::create(
            db,
            &crate::firms::NewFirm {
                name: name.to_string(),
                status: "active".to_string(),
                entity_id,
            },
        )
        .await
        .unwrap()
    }

    /// Every Firm id `person_id` holds a membership row on (ENG-463 removed
    /// the narrower `firms::firm_ids_for_person` in favor of the full rows
    /// `memberships_for_person` returns; this maps down to ids for the tests
    /// below, which only ever ask "which Firms, if any").
    async fn firm_ids_for(db: &SurrealDb, person_id: uuid::Uuid) -> Vec<uuid::Uuid> {
        crate::firms::memberships_for_person(db, person_id)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.firm_id)
            .collect()
    }

    #[tokio::test]
    async fn create_defaults_role_to_client_and_persists() {
        let db = db().await;
        let row = create_person(&db, &create("Libra", "libra@example.com"))
            .await
            .unwrap();
        assert_eq!(row.role, Role::Client);
        let all = persons::list_directory(&db, "", "", &[]).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].email, "libra@example.com");
    }

    #[tokio::test]
    async fn create_accepts_the_explicit_non_lawyer_clerk_role() {
        let db = db().await;
        let mut command = create("Clio", "clio@neonlaw.com");
        command.role = "clerk".into();

        let row = create_person(&db, &command).await.unwrap();

        assert_eq!(row.role, Role::Clerk);
        assert!(!row.role.is_lawyer_tier());
    }

    #[tokio::test]
    async fn create_trims_name_and_email() {
        let db = db().await;
        let row = create_person(&db, &create("  Libra ", "  libra@example.com "))
            .await
            .unwrap();
        assert_eq!(row.name, "Libra");
        assert_eq!(row.email, "libra@example.com");
    }

    // ── ENG-495: a newly created Lawyer or Clerk joins a Firm ──────────────

    /// A newly created Lawyer appears in the anchor Firm's membership with no
    /// manual step — the standing rule the one-time #ENG-462 backfill never
    /// was.
    #[tokio::test]
    async fn create_grants_a_new_lawyers_membership_on_the_anchor_firm() {
        let db = db().await;
        let anchor = anchor_firm(&db).await;
        let mut command = create("New Lawyer", "new-lawyer@neonlaw.com");
        command.role = "lawyer".into();

        let row = create_person(&db, &command).await.unwrap();

        let membership = crate::firms::membership_for_person(&db, row.id, anchor.id)
            .await
            .unwrap()
            .expect("the new lawyer's membership row");
        assert_eq!(membership.membership, crate::firms::FirmMembership::Lawyer);
        assert!(!membership.is_dri);
    }

    /// Same rule, the other supervised firm-side tier.
    #[tokio::test]
    async fn create_grants_a_new_clerks_membership_on_the_anchor_firm() {
        let db = db().await;
        let anchor = anchor_firm(&db).await;
        let mut command = create("New Clerk", "new-clerk@neonlaw.com");
        command.role = "clerk".into();

        let row = create_person(&db, &command).await.unwrap();

        let membership = crate::firms::membership_for_person(&db, row.id, anchor.id)
            .await
            .unwrap()
            .expect("the new clerk's membership row");
        assert_eq!(membership.membership, crate::firms::FirmMembership::Clerk);
    }

    /// A new Client gets no Firm row — a client reaches a matter through
    /// `person_project_role`, never through firm membership.
    #[tokio::test]
    async fn create_grants_no_firm_row_for_a_new_client() {
        let db = db().await;
        anchor_firm(&db).await;
        let row = create_person(&db, &create("Libra", "libra@example.com"))
            .await
            .unwrap();

        assert!(firm_ids_for(&db, row.id).await.is_empty());
    }

    /// Owner and Admin membership stays explicit: creating either grants no
    /// Firm row, unlike Lawyer and Clerk.
    #[tokio::test]
    async fn create_grants_no_firm_row_for_a_new_owner_or_admin() {
        let db = db().await;
        anchor_firm(&db).await;
        for (tag, role) in [("owner", "owner"), ("admin", "admin")] {
            let mut command = create(&format!("New {tag}"), &format!("new-{tag}@neonlaw.com"));
            command.role = role.into();

            let row = create_person(&db, &command).await.unwrap();

            assert!(
                firm_ids_for(&db, row.id).await.is_empty(),
                "{tag} must get no firm row"
            );
        }
    }

    /// The two-firm case: a deployment holding the anchor Firm *and* an
    /// ordinary one still lands the new lawyer on the anchor specifically —
    /// the assertion that would pass vacuously against a single seeded firm
    /// and is the whole point of ENG-495.
    #[tokio::test]
    async fn create_lands_a_new_lawyer_on_the_anchor_when_two_firms_exist() {
        let db = db().await;
        let anchor = anchor_firm(&db).await;
        let other = ordinary_firm(&db, "Other Practice").await;
        let mut command = create("Two Firm Lawyer", "two-firm-lawyer@neonlaw.com");
        command.role = "lawyer".into();

        let row = create_person(&db, &command).await.unwrap();

        assert_eq!(firm_ids_for(&db, row.id).await, vec![anchor.id]);
        assert!(crate::firms::membership_for_person(&db, row.id, other.id)
            .await
            .unwrap()
            .is_none());
    }

    /// The override: a creating surface names a different Firm than the
    /// anchor, and that Firm — not the anchor — is the one that gets the row.
    #[tokio::test]
    async fn create_honors_an_explicit_firm_id_over_the_anchor() {
        let db = db().await;
        let anchor = anchor_firm(&db).await;
        let other = ordinary_firm(&db, "Named Practice").await;
        let mut command = create("Named Lawyer", "named-lawyer@neonlaw.com");
        command.role = "lawyer".into();
        command.firm_id = Some(other.id);

        let row = create_person(&db, &command).await.unwrap();

        assert_eq!(firm_ids_for(&db, row.id).await, vec![other.id]);
        assert!(crate::firms::membership_for_person(&db, row.id, anchor.id)
            .await
            .unwrap()
            .is_none());
    }

    /// No anchor Firm exists yet and none was named: the create still
    /// succeeds, granting nothing — this door does not invent a Firm.
    #[tokio::test]
    async fn create_succeeds_with_no_firm_row_when_no_anchor_firm_exists() {
        let db = db().await;
        let mut command = create("Homeless Lawyer", "homeless-lawyer@neonlaw.com");
        command.role = "lawyer".into();

        let row = create_person(&db, &command).await.unwrap();

        assert!(firm_ids_for(&db, row.id).await.is_empty());
    }

    #[tokio::test]
    async fn create_and_update_store_the_notion_user_id_as_an_external_identity() {
        let db = db().await;
        let mut command = create("Notion Person", "notion-person@example.com");
        command.notion_user_id = Some("notion-user-123".into());
        let row = create_person(&db, &command).await.unwrap();
        let identity = crate::external_identities::find_by_person_and_system(
            &db,
            row.id,
            crate::external_identities::ExternalSystem::Notion,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(identity.external_id, "notion-user-123");

        let updated = update_person(
            &db,
            row.id,
            &UpdatePersonCommand {
                name: row.name.clone(),
                email: row.email.clone(),
                role: String::new(),
                given_name: None,
                family_name: None,
                middle_name: None,
                notion_user_id: Some(Some("notion-user-456".into())),
                linkedin_url: None,
            },
            &UpdateContext {
                bootstrap_owner_email: None,
                actor_role: Role::Owner,
                may_change_roles: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.id, row.id);
        assert_eq!(
            crate::external_identities::find_by_account(
                &db,
                crate::external_identities::ExternalSystem::Notion,
                "notion-user-456"
            )
            .await
            .unwrap()
            .unwrap()
            .person_id,
            row.id
        );
    }

    #[tokio::test]
    async fn create_rejects_blank_name_and_at_less_email() {
        let db = db().await;
        assert!(matches!(
            create_person(&db, &create("   ", "libra@example.com")).await,
            Err(PeopleCommandError::Invalid(_))
        ));
        assert!(matches!(
            create_person(&db, &create("Libra", "not-an-email")).await,
            Err(PeopleCommandError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn create_rejects_unknown_role() {
        let db = db().await;
        let mut cmd = create("Libra", "libra@example.com");
        cmd.role = "wizard".into();
        assert!(matches!(
            create_person(&db, &cmd).await,
            Err(PeopleCommandError::Invalid(_))
        ));
    }

    #[tokio::test]
    async fn duplicate_email_is_an_email_conflict() {
        let db = db().await;
        create_person(&db, &create("Libra", "dup@example.com"))
            .await
            .unwrap();
        assert!(matches!(
            create_person(&db, &create("Other", "dup@example.com")).await,
            Err(PeopleCommandError::EmailConflict)
        ));
    }

    #[tokio::test]
    async fn update_preserves_role_when_caller_cannot_change_roles() {
        let db = db().await;
        let mut cmd = create("Cap", "cap@example.com");
        cmd.role = "lawyer".into();
        let row = create_person(&db, &cmd).await.unwrap();

        let input = UpdatePersonCommand {
            name: "Capricorn".into(),
            email: "cap@example.com".into(),
            role: "admin".into(),
            given_name: None,
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            linkedin_url: None,
        };
        let ctx = UpdateContext {
            bootstrap_owner_email: None,
            actor_role: Role::Lawyer,
            may_change_roles: false,
        };
        let updated = update_person(&db, row.id, &input, &ctx).await.unwrap();
        assert_eq!(updated.name, "Capricorn");
        // Role stays `lawyer` — the admin submission is ignored.
        assert_eq!(updated.role, Role::Lawyer);
    }

    #[tokio::test]
    async fn update_leaves_omitted_name_parts_untouched() {
        let db = db().await;
        let mut cmd = create("Gem", "gem@example.com");
        cmd.given_name = Some("Gemma".into());
        let row = create_person(&db, &cmd).await.unwrap();

        let input = UpdatePersonCommand {
            name: "Gemini".into(),
            email: "gem@example.com".into(),
            role: String::new(),
            given_name: None, // omitted → preserved
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            linkedin_url: None,
        };
        let ctx = UpdateContext {
            bootstrap_owner_email: None,
            actor_role: Role::Owner,
            may_change_roles: true,
        };
        let updated = update_person(&db, row.id, &input, &ctx).await.unwrap();
        assert_eq!(updated.given_name.as_deref(), Some("Gemma"));
    }

    #[tokio::test]
    async fn update_clears_a_present_blank_name_part() {
        let db = db().await;
        let mut cmd = create("Gem", "gem@example.com");
        cmd.given_name = Some("Gemma".into());
        let row = create_person(&db, &cmd).await.unwrap();

        let input = UpdatePersonCommand {
            name: "Gemini".into(),
            email: "gem@example.com".into(),
            role: String::new(),
            given_name: Some(Some(String::new())), // present blank → clear
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            linkedin_url: None,
        };
        let ctx = UpdateContext {
            bootstrap_owner_email: None,
            actor_role: Role::Owner,
            may_change_roles: true,
        };
        let updated = update_person(&db, row.id, &input, &ctx).await.unwrap();
        assert!(updated.given_name.is_none());
    }

    #[tokio::test]
    async fn update_pins_the_bootstrap_owners_email_and_role_but_allows_the_name() {
        let db = db().await;
        let mut cmd = create("Boss", "boss@example.com");
        cmd.role = "owner".into();
        let row = create_person(&db, &cmd).await.unwrap();

        // Attempts to steal the email and demote the role alongside a
        // legitimate rename — the command layer applies the name and
        // silently drops the other two rather than rejecting the whole
        // write.
        let input = UpdatePersonCommand {
            name: "Renamed".into(),
            email: "attacker@example.com".into(),
            role: "client".into(),
            given_name: None,
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            linkedin_url: None,
        };
        let ctx = UpdateContext {
            bootstrap_owner_email: Some("boss@example.com"),
            actor_role: Role::Owner,
            may_change_roles: true,
        };
        let updated = update_person(&db, row.id, &input, &ctx).await.unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.email, "boss@example.com");
        assert_eq!(updated.role, Role::Owner);
    }

    #[tokio::test]
    async fn admin_cannot_edit_or_assign_owner_but_owner_can_assign_owner() {
        let db = db().await;
        let mut owner = create("Owner", "owner@example.com");
        owner.role = "owner".into();
        let owner = create_person(&db, &owner).await.unwrap();
        let client = create_person(&db, &create("Client", "client@example.com"))
            .await
            .unwrap();
        let owner_input = UpdatePersonCommand {
            name: "Owner Changed".into(),
            email: owner.email.clone(),
            role: "admin".into(),
            given_name: None,
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            linkedin_url: None,
        };
        let admin_ctx = UpdateContext {
            bootstrap_owner_email: None,
            actor_role: Role::Admin,
            may_change_roles: true,
        };
        assert!(matches!(
            update_person(&db, owner.id, &owner_input, &admin_ctx).await,
            Err(PeopleCommandError::Blocked(_))
        ));

        let promote_input = UpdatePersonCommand {
            name: client.name.clone(),
            email: client.email.clone(),
            role: "owner".into(),
            given_name: None,
            family_name: None,
            middle_name: None,
            notion_user_id: None,
            linkedin_url: None,
        };
        assert!(matches!(
            update_person(&db, client.id, &promote_input, &admin_ctx).await,
            Err(PeopleCommandError::Blocked(_))
        ));
        let owner_ctx = UpdateContext {
            bootstrap_owner_email: None,
            actor_role: Role::Owner,
            may_change_roles: true,
        };
        let promoted = update_person(&db, client.id, &promote_input, &owner_ctx)
            .await
            .unwrap();
        assert_eq!(promoted.role, Role::Owner);
    }

    #[tokio::test]
    async fn delete_refuses_non_client_and_bootstrap_owner() {
        let db = db().await;
        // Lawyer is not deletable.
        let mut lawyer = create("Stella", "stella@example.com");
        lawyer.role = "lawyer".into();
        let lawyer_row = create_person(&db, &lawyer).await.unwrap();
        assert!(matches!(
            delete_person(&db, lawyer_row.id, None).await,
            Err(PeopleCommandError::Blocked(_))
        ));

        // A client is deletable, unless it is the bootstrap Owner email.
        let client_row = create_person(&db, &create("Cleo", "cleo@example.com"))
            .await
            .unwrap();
        assert!(matches!(
            delete_person(&db, client_row.id, Some("cleo@example.com")).await,
            Err(PeopleCommandError::Blocked(_))
        ));
        // Without the bootstrap guard it deletes.
        let deleted = delete_person(&db, client_row.id, None).await.unwrap();
        assert_eq!(deleted.email, "cleo@example.com");
        assert!(persons::find_by_id(&db, client_row.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_missing_id_is_not_found() {
        let db = db().await;
        // A fixed id no seeded row will ever carry (rows get random ids).
        assert!(matches!(
            delete_person(&db, uuid::Uuid::from_u128(0xdead_beef), None).await,
            Err(PeopleCommandError::NotFound)
        ));
    }

    async fn seed(db: &SurrealDb, name: &str, email: &str) {
        persons::create(db, &NewPerson::new(name, email))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn search_matches_substring_case_insensitively_and_sorts() {
        let db = db().await;
        seed(&db, "Sagittarius", "sagittarius@example.com").await;
        seed(&db, "Aquarius", "aquarius@example.com").await;
        seed(&db, "Aries", "aries@example.com").await;
        let rows = search_people(&db, Some("ARI"), None, 50).await.unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Aquarius", "Aries", "Sagittarius"]);
    }

    #[tokio::test]
    async fn search_ands_name_and_email() {
        let db = db().await;
        seed(&db, "Aquarius", "aquarius@neonlaw.com").await;
        seed(&db, "Aries", "aries@example.com").await;
        seed(&db, "Sagittarius", "sagittarius@neonlaw.com").await;
        let rows = search_people(&db, Some("ari"), Some("neonlaw"), 50)
            .await
            .unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Aquarius", "Sagittarius"]);
    }

    #[tokio::test]
    async fn search_respects_the_limit() {
        let db = db().await;
        seed(&db, "Aquarius", "aquarius@example.com").await;
        seed(&db, "Aries", "aries@example.com").await;
        let rows = search_people(&db, Some("a"), None, 1).await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn search_no_match_is_empty_not_an_error() {
        let db = db().await;
        seed(&db, "Libra", "libra@example.com").await;
        let rows = search_people(&db, Some("ghost"), None, 50).await.unwrap();
        assert!(rows.is_empty());
    }
}
