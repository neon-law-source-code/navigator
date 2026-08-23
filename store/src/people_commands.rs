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
    pub email: String,
    #[serde(default)]
    pub role: String,
    #[serde(default, deserialize_with = "double_option")]
    pub given_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub family_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub middle_name: Option<Option<String>>,
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

pub async fn create_person(
    db: &SurrealDb,
    input: &CreatePersonCommand,
) -> Result<Person, PeopleCommandError> {
    if let Some(message) = input.validation_message() {
        return Err(PeopleCommandError::Invalid(message));
    }
    persons::create(
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
    .map_err(classify_write)
}

/// Update one Person by id. Rejects any edit to the bootstrap Owner record,
/// and rejects callers attempting to edit a higher-ranked person. Otherwise preserves the existing
/// role when the caller can't change roles or submits a blank role; leaves
/// an omitted structured-name part untouched and nulls a present-but-blank
/// one.
pub async fn update_person(
    db: &SurrealDb,
    id: Uuid,
    input: &UpdatePersonCommand,
    ctx: &UpdateContext<'_>,
) -> Result<Person, PeopleCommandError> {
    if let Some(message) = validate_name_email(&input.name, &input.email) {
        return Err(PeopleCommandError::Invalid(message));
    }
    let existing = persons::find_by_id(db, id)
        .await
        .map_err(PeopleCommandError::Db)?
        .ok_or(PeopleCommandError::NotFound)?;

    // The bootstrap Owner record is immutable: reject every
    // edit before any write. It is the row `oauth::resolve_person_from_claims`
    // re-tags on each login and the operator who can see the whole firm's
    // data, so the UI locks it and this boundary refuses a hand-crafted PATCH
    // that would slip past the disabled fields.
    if is_bootstrap_owner_email(ctx.bootstrap_owner_email, &existing.email) {
        return Err(PeopleCommandError::Blocked(
            "The bootstrap Owner record is immutable. Change it via \
             NAVIGATOR_BOOTSTRAP_OWNER_EMAIL or a direct database write.",
        ));
    }
    if existing.role.authority_rank() > ctx.actor_role.authority_rank() {
        return Err(PeopleCommandError::Blocked(
            "You cannot edit a person with a higher system role.",
        ));
    }

    let new_role = if !ctx.may_change_roles || input.role.trim().is_empty() {
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

    persons::edit(
        db,
        id,
        &PersonEdit {
            name: Some(input.name.trim().to_string()),
            email: Some(input.email.trim().to_string()),
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
        },
    )
    .await
    .map_err(classify_write)?
    .ok_or(PeopleCommandError::NotFound)
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
        }
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
    async fn update_rejects_the_bootstrap_owner() {
        let db = db().await;
        let mut cmd = create("Boss", "boss@example.com");
        cmd.role = "owner".into();
        let row = create_person(&db, &cmd).await.unwrap();

        let input = UpdatePersonCommand {
            name: "Renamed".into(),
            email: "boss@example.com".into(),
            role: "client".into(),
            given_name: None,
            family_name: None,
            middle_name: None,
        };
        let ctx = UpdateContext {
            bootstrap_owner_email: Some("boss@example.com"),
            actor_role: Role::Owner,
            may_change_roles: true,
        };
        assert!(matches!(
            update_person(&db, row.id, &input, &ctx).await,
            Err(PeopleCommandError::Blocked(_))
        ));
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
