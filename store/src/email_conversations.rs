//! Helpers for the `email_conversations` + `email_conversation_messages`
//! tables — the threaded support inbox behind `support@neonlaw.com`.
//!
//! `web` reaches these so it can open a thread when a new message lands,
//! look one up by its `Reply-To` token when a reply comes back, append
//! each hop to the transcript, and project the conversation's `status`.
//! The thread token is **caller-supplied** — `web` mints an unguessable
//! value; `store` stays free of randomness so its behavior is
//! deterministic under test.
//!
//! # These tables live in SurrealDB
//!
//! Both moved with wave six of #1093 (ENG-160), in the communications
//! slice alongside [`crate::communications`].

use chrono::{DateTime, Utc};
use serde::Serialize;
use surrealdb::types::SurrealValue;
use thiserror::Error;
use uuid::Uuid;

use crate::surreal::{record_id, record_uuid, SurrealDb};

/// Bound as `$thread_token`, never `$token`: SurrealDB reserves `$token`
/// as a session variable and refuses to let a statement set it.
const TABLE: &str = "email_conversation";
const MESSAGE_TABLE: &str = "email_conversation_message";
const PERSON_TABLE: &str = "person";

/// Freshly opened; not yet acted on.
pub const STATUS_OPEN: &str = "open";
/// Lawyers have been notified; waiting on the attorney to reply.
pub const STATUS_AWAITING_LAWYER: &str = "awaiting_lawyer";
/// A reply was relayed out; waiting on the external party.
pub const STATUS_AWAITING_CLIENT: &str = "awaiting_client";
/// Closed — no further relay.
pub const STATUS_CLOSED: &str = "closed";

/// Inbound from the external party (client / prospective client).
pub const DIRECTION_FROM_EXTERNAL: &str = "from_external";
/// The notification we send the attorney's cockpit inbox.
pub const DIRECTION_TO_LAWYER: &str = "to_lawyer";
/// The attorney's reply back into the thread.
pub const DIRECTION_FROM_LAWYER: &str = "from_lawyer";
/// The relay we send the external party as `support@`.
pub const DIRECTION_TO_EXTERNAL: &str = "to_external";
/// A system note (e.g. conflict-check result); not an email hop.
pub const DIRECTION_SYSTEM: &str = "system";

/// What can go wrong reading or writing a support thread.
#[derive(Debug, Error)]
pub enum EmailConversationError {
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] surrealdb::Error),
    /// The token already belongs to another thread. Its own variant
    /// because a token collision is the one failure a caller can act on:
    /// mint another and retry.
    #[error("that thread token is already in use")]
    TokenTaken,
}

/// One threaded support exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailConversation {
    pub id: Uuid,
    /// Opaque, unguessable thread token; the VERP key in `Reply-To`.
    pub token: String,
    /// The external party's address (client or prospective client).
    pub external_email: String,
    /// The external party's display name, if the inbound carried one.
    pub external_name: Option<String>,
    /// The matched [`crate::persons`] row; `None` until conflict-checked.
    pub person_id: Option<Uuid>,
    /// Subject of the originating message.
    pub subject: String,
    /// One of the `STATUS_*` constants.
    pub status: String,
    /// The matter this thread drives, if any.
    pub notation_id: Option<Uuid>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct ConversationRow {
    id: surrealdb::types::RecordId,
    token: String,
    external_email: String,
    external_name: Option<String>,
    person_id: Option<surrealdb::types::RecordId>,
    subject: String,
    status: String,
    notation_id: Option<surrealdb::types::RecordId>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl ConversationRow {
    fn into_conversation(self) -> Option<EmailConversation> {
        Some(EmailConversation {
            id: record_uuid(&self.id)?,
            token: self.token,
            external_email: self.external_email,
            external_name: self.external_name,
            person_id: self.person_id.as_ref().and_then(record_uuid),
            subject: self.subject,
            status: self.status,
            notation_id: self.notation_id.as_ref().and_then(record_uuid),
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const SELECT: &str = "id, token, external_email, external_name, person_id, subject, status, \
                      notation_id, inserted_at, updated_at";

/// One hop in a thread's append-only transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EmailConversationMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    /// One of the `DIRECTION_*` constants.
    pub direction: String,
    pub from_addr: String,
    pub to_addr: String,
    pub subject: String,
    /// Cleaned body — quoted history and signature stripped on lawyer
    /// replies, so a relayed message carries only the new prose.
    pub body_text: String,
    /// Object-storage key of the raw `.eml` for inbound hops; `None` for
    /// messages we generated.
    pub raw_storage_key: Option<String>,
    /// The provider message id — the join key to the delivery stream and
    /// the dedup key on retries.
    pub provider_message_id: Option<String>,
    /// RFC 5322 `In-Reply-To` of this hop, when present.
    pub in_reply_to: Option<String>,
    /// Parsed lawyer-reply directives (`@approve`/`@deny`/…) as JSON;
    /// `None` when the reply carried no command.
    pub command_payload: Option<String>,
    pub inserted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(SurrealValue)]
struct MessageRow {
    id: surrealdb::types::RecordId,
    conversation_id: surrealdb::types::RecordId,
    direction: String,
    from_addr: String,
    to_addr: String,
    subject: String,
    body_text: String,
    raw_storage_key: Option<String>,
    provider_message_id: Option<String>,
    in_reply_to: Option<String>,
    command_payload: Option<String>,
    inserted_at: surrealdb::types::Datetime,
    updated_at: surrealdb::types::Datetime,
}

impl MessageRow {
    fn into_message(self) -> Option<EmailConversationMessage> {
        Some(EmailConversationMessage {
            id: record_uuid(&self.id)?,
            conversation_id: record_uuid(&self.conversation_id)?,
            direction: self.direction,
            from_addr: self.from_addr,
            to_addr: self.to_addr,
            subject: self.subject,
            body_text: self.body_text,
            raw_storage_key: self.raw_storage_key,
            provider_message_id: self.provider_message_id,
            in_reply_to: self.in_reply_to,
            command_payload: self.command_payload,
            inserted_at: self.inserted_at.into(),
            updated_at: self.updated_at.into(),
        })
    }
}

const MESSAGE_SELECT: &str = "id, conversation_id, direction, from_addr, to_addr, subject, \
                              body_text, raw_storage_key, provider_message_id, in_reply_to, \
                              command_payload, inserted_at, updated_at";

/// What to record when opening a new support thread. `status` defaults to
/// [`STATUS_OPEN`] via [`open`].
#[derive(Debug, Clone)]
pub struct NewConversation<'a> {
    /// Unguessable thread token; the VERP key carried in `Reply-To`.
    pub token: &'a str,
    pub external_email: &'a str,
    pub external_name: Option<&'a str>,
    pub subject: &'a str,
    /// Matched `persons.id`, if the sender is already known.
    pub person_id: Option<Uuid>,
    /// Linked matter, if this thread drives one.
    pub notation_id: Option<Uuid>,
}

/// One hop to append to a thread's transcript. See the `DIRECTION_*`
/// constants above.
///
/// `Default` lets callers fill only the fields a given hop needs and
/// elide the optional tail (`raw_storage_key` / `provider_message_id` /
/// `in_reply_to` / `command_payload`) with `..Default::default()` — the
/// required fields are always set explicitly at the call site.
#[derive(Debug, Clone, Default)]
pub struct NewMessage<'a> {
    pub conversation_id: Uuid,
    pub direction: &'a str,
    pub from_addr: &'a str,
    pub to_addr: &'a str,
    pub subject: &'a str,
    pub body_text: &'a str,
    pub raw_storage_key: Option<&'a str>,
    pub provider_message_id: Option<&'a str>,
    pub in_reply_to: Option<&'a str>,
    pub command_payload: Option<&'a str>,
}

/// Open a new conversation at `status = open`, returning its id.
///
/// # Errors
///
/// [`EmailConversationError::TokenTaken`] when the token already threads
/// another conversation, or any other database error.
pub async fn open(
    db: &SurrealDb,
    new: &NewConversation<'_>,
) -> Result<Uuid, EmailConversationError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             token = $thread_token, external_email = $external_email, external_name = $external_name, \
             person_id = $person_id, subject = $subject, status = $status, \
             notation_id = $notation_id \
             RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("thread_token", new.token.to_string()))
        .bind(("external_email", new.external_email.to_string()))
        .bind(("external_name", new.external_name.map(str::to_string)))
        .bind((
            "person_id",
            new.person_id.map(|p| record_id(PERSON_TABLE, p)),
        ))
        .bind(("subject", new.subject.to_string()))
        .bind(("status", STATUS_OPEN.to_string()))
        .bind((
            "notation_id",
            new.notation_id
                .map(|n| record_id(crate::notations::TABLE, n)),
        ))
        .await
        .and_then(surrealdb::IndexedResults::check)
        .map_err(classify_open)?;
    let row: Option<ConversationRow> = response.take(0)?;
    Ok(row
        .and_then(ConversationRow::into_conversation)
        .map_or(id, |c| c.id))
}

/// Name the token index specifically. A collision is the one failure the
/// caller can act on — mint another token and retry — so it must not
/// arrive as an opaque database error.
fn classify_open(error: surrealdb::Error) -> EmailConversationError {
    if crate::surreal::retry::unique_violation(&error) == Some("email_conversation_token") {
        return EmailConversationError::TokenTaken;
    }
    EmailConversationError::Db(error)
}

/// Look up a conversation by its `Reply-To` token — the threading lookup
/// run on every inbound reply.
///
/// # Errors
///
/// Propagates any database error.
pub async fn by_token(
    db: &SurrealDb,
    token: &str,
) -> Result<Option<EmailConversation>, EmailConversationError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} WHERE token = $thread_token LIMIT 1"
        ))
        .bind(("thread_token", token.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<ConversationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .find_map(ConversationRow::into_conversation))
}

/// The most recent conversation opened by an external address — the
/// "is this person already talking to us?" lookup, which the
/// `email_conversation_external_email` index serves.
///
/// Newest first, because a person who writes in twice is asking about
/// their current exchange, not a closed one.
///
/// # Errors
///
/// Propagates any database error.
pub async fn by_external_email(
    db: &SurrealDb,
    external_email: &str,
) -> Result<Option<EmailConversation>, EmailConversationError> {
    let mut response = db
        .query(format!(
            "SELECT {SELECT} FROM {TABLE} \
             WHERE external_email = $external_email ORDER BY id DESC LIMIT 1"
        ))
        .bind(("external_email", external_email.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<ConversationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .find_map(ConversationRow::into_conversation))
}

/// Load one conversation by id.
///
/// # Errors
///
/// Propagates any database error.
pub async fn by_id(
    db: &SurrealDb,
    id: Uuid,
) -> Result<Option<EmailConversation>, EmailConversationError> {
    let mut response = db
        .query(format!("SELECT {SELECT} FROM $id"))
        .bind(("id", record_id(TABLE, id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<ConversationRow> = response.take(0)?;
    Ok(row.and_then(ConversationRow::into_conversation))
}

/// Append one hop to a thread's transcript, returning its id. Does not
/// touch the conversation's `status` — callers advance that explicitly
/// via [`set_status`] so the projection stays under their control.
///
/// # Errors
///
/// Propagates any database error.
pub async fn append(db: &SurrealDb, new: &NewMessage<'_>) -> Result<Uuid, EmailConversationError> {
    let id = Uuid::now_v7();
    let mut response = db
        .query(format!(
            "CREATE $id SET \
             conversation_id = $conversation_id, direction = $direction, \
             from_addr = $from_addr, to_addr = $to_addr, subject = $subject, \
             body_text = $body_text, raw_storage_key = $raw_storage_key, \
             provider_message_id = $provider_message_id, in_reply_to = $in_reply_to, \
             command_payload = $command_payload \
             RETURN {MESSAGE_SELECT}"
        ))
        .bind(("id", record_id(MESSAGE_TABLE, id)))
        .bind(("conversation_id", record_id(TABLE, new.conversation_id)))
        .bind(("direction", new.direction.to_string()))
        .bind(("from_addr", new.from_addr.to_string()))
        .bind(("to_addr", new.to_addr.to_string()))
        .bind(("subject", new.subject.to_string()))
        .bind(("body_text", new.body_text.to_string()))
        .bind(("raw_storage_key", new.raw_storage_key.map(str::to_string)))
        .bind((
            "provider_message_id",
            new.provider_message_id.map(str::to_string),
        ))
        .bind(("in_reply_to", new.in_reply_to.map(str::to_string)))
        .bind(("command_payload", new.command_payload.map(str::to_string)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let row: Option<MessageRow> = response.take(0)?;
    Ok(row.and_then(MessageRow::into_message).map_or(id, |m| m.id))
}

/// The full transcript of a conversation, oldest hop first.
///
/// # Errors
///
/// Propagates any database error.
pub async fn messages(
    db: &SurrealDb,
    conversation_id: Uuid,
) -> Result<Vec<EmailConversationMessage>, EmailConversationError> {
    let mut response = db
        .query(format!(
            "SELECT {MESSAGE_SELECT} FROM {MESSAGE_TABLE} \
             WHERE conversation_id = $conversation ORDER BY id ASC"
        ))
        .bind(("conversation", record_id(TABLE, conversation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<MessageRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(MessageRow::into_message)
        .collect())
}

/// Move a conversation to a new `status`. Returns the updated row, or
/// `Ok(None)` if no row matched.
///
/// # Errors
///
/// Propagates any database error.
pub async fn set_status(
    db: &SurrealDb,
    id: Uuid,
    status: &str,
) -> Result<Option<EmailConversation>, EmailConversationError> {
    let mut response = db
        .query(format!(
            "UPDATE $id SET status = $status, updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("status", status.to_string()))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<ConversationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .find_map(ConversationRow::into_conversation))
}

/// Link a conversation to a running workflow notation (the `@link` lawyer
/// command). Once set, the `@approve`/`@deny`/`@signal` command channel
/// fires on this notation and inbound attachments file onto its matter.
/// Returns `None` when no conversation has `id`.
///
/// # Errors
///
/// Propagates any database error.
pub async fn set_notation(
    db: &SurrealDb,
    id: Uuid,
    notation_id: Uuid,
) -> Result<Option<EmailConversation>, EmailConversationError> {
    let mut response = db
        .query(format!(
            "UPDATE $id SET notation_id = $notation, updated_at = time::now() RETURN {SELECT}"
        ))
        .bind(("id", record_id(TABLE, id)))
        .bind(("notation", record_id(crate::notations::TABLE, notation_id)))
        .await
        .and_then(surrealdb::IndexedResults::check)?;
    let rows: Vec<ConversationRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .find_map(ConversationRow::into_conversation))
}

#[cfg(test)]
mod tests {
    use super::{
        append, by_external_email, by_id, by_token, messages, open, set_notation, set_status,
        EmailConversationError, NewConversation, NewMessage, DIRECTION_FROM_EXTERNAL,
        DIRECTION_TO_LAWYER, STATUS_AWAITING_LAWYER, STATUS_OPEN,
    };
    use crate::surreal::test_support::mem;
    use crate::test_support::seed_notation;

    #[tokio::test]
    async fn open_then_thread_back_by_token() {
        let db = mem().await;

        let id = open(
            &db,
            &NewConversation {
                token: "tok_pisces_001",
                external_email: "pisces@example.com",
                external_name: Some("Pisces"),
                subject: "Question about my LLC",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();

        // The inbound reply path looks the thread up by its Reply-To token.
        let found = by_token(&db, "tok_pisces_001").await.unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.status, STATUS_OPEN);
        assert_eq!(found.external_name.as_deref(), Some("Pisces"));

        // An unknown token threads to nothing.
        assert!(by_token(&db, "tok_nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn transcript_is_ordered_and_status_projects() {
        let db = mem().await;
        let id = open(
            &db,
            &NewConversation {
                token: "tok_pisces_002",
                external_email: "pisces@example.com",
                external_name: None,
                subject: "PDF to authorize",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();

        append(
            &db,
            &NewMessage {
                conversation_id: id,
                direction: DIRECTION_FROM_EXTERNAL,
                from_addr: "pisces@example.com",
                to_addr: "support@neonlaw.com",
                subject: "PDF to authorize",
                body_text: "Please review the attached.",
                raw_storage_key: Some("inbound/1234-pisces.eml"),
                provider_message_id: Some("<msg-1@mail>"),
                in_reply_to: None,
                command_payload: None,
            },
        )
        .await
        .unwrap();
        append(
            &db,
            &NewMessage {
                conversation_id: id,
                direction: DIRECTION_TO_LAWYER,
                from_addr: "support@neonlaw.com",
                to_addr: "nick+aida@neonlaw.com",
                subject: "[Pisces] PDF to authorize",
                body_text: "Pisces sent a PDF to authorize.",
                raw_storage_key: None,
                provider_message_id: Some("<msg-2@sg>"),
                in_reply_to: None,
                command_payload: None,
            },
        )
        .await
        .unwrap();

        let transcript = messages(&db, id).await.unwrap();
        assert_eq!(transcript.len(), 2);
        assert_eq!(transcript[0].direction, DIRECTION_FROM_EXTERNAL);
        assert_eq!(transcript[1].direction, DIRECTION_TO_LAWYER);
        assert_eq!(transcript[1].to_addr, "nick+aida@neonlaw.com");

        let updated = set_status(&db, id, STATUS_AWAITING_LAWYER)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, STATUS_AWAITING_LAWYER);
    }

    #[tokio::test]
    async fn token_is_unique() {
        // The token is the routing key every inbound reply is resolved by,
        // so two threads sharing one would splice a stranger's exchange into
        // a client's. A collision has to be refused, and named, so the
        // caller can mint another and retry.
        let db = mem().await;
        let new = NewConversation {
            token: "tok_dupe",
            external_email: "a@example.com",
            external_name: None,
            subject: "first",
            person_id: None,
            notation_id: None,
        };
        open(&db, &new).await.unwrap();
        let err = open(&db, &new).await.unwrap_err();
        assert!(
            matches!(err, EmailConversationError::TokenTaken),
            "expected a token collision, got {err:?}"
        );
    }

    #[tokio::test]
    async fn linking_a_notation_is_readable_back() {
        // The `@link` lawyer command is what arms the `@approve`/`@deny`
        // channel, so the link has to survive a re-read.
        let db = mem().await;
        let notation_id = seed_notation(&db).await;
        let id = open(
            &db,
            &NewConversation {
                token: "tok_link",
                external_email: "aries@example.com",
                external_name: None,
                subject: "Authorize the filing",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();
        assert!(by_id(&db, id).await.unwrap().unwrap().notation_id.is_none());

        let linked = set_notation(&db, id, notation_id).await.unwrap().unwrap();
        assert_eq!(linked.notation_id, Some(notation_id));
        assert_eq!(
            by_id(&db, id).await.unwrap().unwrap().notation_id,
            Some(notation_id)
        );
    }

    #[tokio::test]
    async fn by_external_email_returns_the_newest_thread() {
        // Someone who writes in twice is asking about their current
        // exchange, so the newest wins — and another party's thread never
        // does.
        let db = mem().await;
        for (token, subject) in [("tok_e1", "first"), ("tok_e2", "second")] {
            open(
                &db,
                &NewConversation {
                    token,
                    external_email: "pisces@example.com",
                    external_name: None,
                    subject,
                    person_id: None,
                    notation_id: None,
                },
            )
            .await
            .unwrap();
        }
        open(
            &db,
            &NewConversation {
                token: "tok_other",
                external_email: "aries@example.com",
                external_name: None,
                subject: "someone else",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();

        let found = by_external_email(&db, "pisces@example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.subject, "second");
        assert!(by_external_email(&db, "nobody@example.com")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn resolving_an_unknown_conversation_is_none() {
        let db = mem().await;
        assert!(by_id(&db, uuid::Uuid::now_v7()).await.unwrap().is_none());
        assert!(
            set_status(&db, uuid::Uuid::now_v7(), STATUS_AWAITING_LAWYER)
                .await
                .unwrap()
                .is_none()
        );
    }
}
