//! Email threading — the "headless Front" loop on top of inbound parse.
//!
//! When SendGrid Inbound Parse hands `portal::inbound_email` a message, this
//! module turns it into a threaded support exchange:
//!
//! - **First contact** (a message to any address *without* a token, e.g.
//!   `test@parse.neonlaw.com` or, once the Workspace rule is in,
//!   `support@neonlaw.com`) opens a new `email_conversations` row and
//!   emails the lawyer cockpit (`NAVIGATOR_LAWYER_NOTIFY_EMAIL`) a
//!   notification whose `Reply-To` is the conversation's token address
//!   `c<token>@<parse_host>`.
//! - **A reply to a token address** looks the conversation up by token.
//!   If the sender is lawyer (a `persons` row with a lawyer/admin role),
//!   the reply is relayed out to the external party as `support@…`; if
//!   the sender is the external party, lawyers are re-notified.
//!
//! The token in `Reply-To` is the whole threading mechanism: lawyer and
//! client both reply to the same shared address, and we disambiguate by
//! authenticated sender, never by address. No internal address ever
//! appears in an outbound external header.
//!
//! Threading is **opt-in per deployment**: it runs only when both
//! `NAVIGATOR_PARSE_HOST` and `NAVIGATOR_LAWYER_NOTIFY_EMAIL` are set
//! (see [`ThreadConfig::from_env`]). Otherwise the webhook just archives
//! the raw `.eml` as before, so the repo ships no Neon-specific defaults.

use std::fmt::Write as _;
use std::sync::Arc;

use cloud::StorageService;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use workflows::{MachineKind, SignalContext, StateMachineRuntime};

use store::documents;
use store::email_conversations as conv;
use store::email_conversations::{
    DIRECTION_FROM_EXTERNAL, DIRECTION_FROM_LAWYER, DIRECTION_SYSTEM, DIRECTION_TO_EXTERNAL,
    DIRECTION_TO_LAWYER, STATUS_AWAITING_CLIENT, STATUS_AWAITING_LAWYER, STATUS_CLOSED,
};

use crate::email::{Attachment, EmailService, OutboundEmail, SendReceipt, DEFAULT_FROM_EMAIL};
use crate::inbound_email::{InboundAttachment, InboundEmail};

const SENDGRID_MAX_MESSAGE_BYTES: usize = 30_000_000;

/// Runtime config for the threading layer, read from the environment.
/// Both fields are required; when either is unset the inbound webhook
/// skips threading and only archives the raw message (legacy behavior).
#[derive(Debug, Clone)]
pub struct ThreadConfig {
    /// Subdomain whose MX points at SendGrid Inbound Parse — the host of
    /// every `Reply-To` token address (`c<token>@<parse_host>`).
    pub parse_host: String,
    /// Where lawyer notifications are sent (e.g. `nick+aida@neonlaw.com`).
    /// A reply from that mailbox's owner relays back to the external
    /// party.
    pub lawyer_notify_email: String,
    /// Firm domain a lawyer reply's DKIM verdict must pass for before the
    /// reply is trusted to relay or fire a workflow command (e.g.
    /// `neonlaw.com`). `None` leaves the channel gated on lawyer-sender +
    /// unguessable token only — the opt-in posture that lets the live
    /// pipeline first confirm SendGrid's `dkim` field arrives before
    /// enforcement is flipped on. Sourced from `NAVIGATOR_DKIM_REQUIRE_DOMAIN`.
    pub verify_dkim_domain: Option<String>,
}

impl ThreadConfig {
    /// Read `NAVIGATOR_PARSE_HOST` + `NAVIGATOR_LAWYER_NOTIFY_EMAIL`.
    /// Returns `None` when either is missing or empty — threading stays
    /// off and the webhook only archives the raw message.
    /// `NAVIGATOR_DKIM_REQUIRE_DOMAIN` is optional: when set it enables the
    /// command-channel DKIM gate; when unset the channel stays on the
    /// lawyer-sender + token gate alone.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Some(Self {
            parse_host: non_empty(std::env::var("NAVIGATOR_PARSE_HOST").ok())?,
            lawyer_notify_email: non_empty(std::env::var("NAVIGATOR_LAWYER_NOTIFY_EMAIL").ok())?,
            verify_dkim_domain: non_empty(std::env::var("NAVIGATOR_DKIM_REQUIRE_DOMAIN").ok()),
        })
    }
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Errors from threading. The webhook treats these as best-effort: the
/// raw `.eml` is already archived, so a failure is logged rather than
/// surfaced to SendGrid (a non-2xx would trigger a retry and duplicate
/// the conversation).
#[derive(Debug, Error)]
pub enum ThreadError {
    #[error("database error: {0}")]
    Db(String),
    #[error("person directory error: {0}")]
    Person(#[from] store::persons::PersonError),
    #[error("send error: {0}")]
    Send(#[from] crate::email::EmailError),
    #[error("workflow runtime error: {0}")]
    Runtime(String),
    #[error("document ingest error: {0}")]
    Storage(String),
    #[error("notation store error: {0}")]
    Notation(#[from] store::notations::NotationError),
    #[error("support thread error: {0}")]
    Conversation(#[from] store::email_conversations::EmailConversationError),
    #[error("conversation log error: {0}")]
    Communication(#[from] store::communications::CommunicationError),
}

impl From<String> for ThreadError {
    fn from(message: String) -> Self {
        Self::Db(message)
    }
}

impl From<documents::IngestError> for ThreadError {
    fn from(e: documents::IngestError) -> Self {
        // `assets` moved to SurrealDB with ENG-121, so an ingest failure no
        // longer carries a `String` to hand straight to `Db`.
        match e {
            documents::IngestError::Storage(s) => Self::Storage(s.to_string()),
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<store::assets::AssetError> for ThreadError {
    fn from(e: store::assets::AssetError) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<store::templates::TemplateError> for ThreadError {
    fn from(e: store::templates::TemplateError) -> Self {
        Self::Storage(e.to_string())
    }
}

/// A directive parsed from a lawyer reply. Commands are line-oriented: a
/// line whose first token is `@<verb>` is a command, is stripped from the
/// relayed prose, and is recorded on the message's `command_payload`.
///
/// The command channel is privileged — a command only runs when the
/// inbound reply both threads to a known conversation (the unguessable
/// token) and comes from a lawyer/admin `persons` row. Cryptographic
/// sender authentication (DKIM `d=neonlaw.com` + the SendGrid webhook
/// signature) is the next hardening layer; see `inbound_email.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// Fire a workflow signal on the conversation's linked notation.
    /// `@approve` → `approved`, `@deny [reason]` → `rejected`,
    /// `@signal <condition> [value]` → arbitrary condition.
    Signal {
        condition: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
    },
    /// Close the conversation; suppress the relay.
    Close,
    /// Internal note to self; suppress the relay.
    Internal,
    /// `@cleared` — the firm-wide conflict check is clear for this
    /// prospective client; release the relay gate. Recorded in the
    /// transcript so subsequent relays flow without re-prompting.
    ConflictCleared,
    /// `@link <notation_id>` — bind this conversation to a running workflow
    /// notation so the `@approve`/`@deny`/`@signal` command channel fires on
    /// it and inbound attachments file onto its matter. The id is captured
    /// verbatim and parsed/validated at execution time. This is what makes
    /// the lawyer-review gate and attachment-filing actually fire on a live
    /// matter — nothing else sets `email_conversations.notation_id` in prod.
    Link { notation_id: String },
}

/// The result of parsing a lawyer reply: the prose to relay (with command
/// lines removed) and the directives found, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReply {
    pub relay_body: String,
    pub commands: Vec<Command>,
}

/// The instruction the firm-wide conflict gate gives lawyer — in the
/// first-contact prompt and the held-relay bounce alike. One source so the
/// policy phrasing (and the `@cleared` verb) never drifts between the two.
const CONFLICT_CHECK_INSTRUCTION: &str = "Run the firm-wide conflict check across all attorneys; \
                                          reply with @cleared to release the relay.";

/// The dependencies every email-loop handler needs, bundled so each one
/// takes a single `&ThreadCtx` instead of re-threading the same
/// `(db, storage, email, runtime, cfg)` convoy by hand. Each handler
/// rebinds only the fields it uses.
struct ThreadCtx<'a> {
    /// The store. Resolving an inbound sender to a `persons` row — the
    /// conflict gate on first contact and the lawyer-tier check on a reply
    /// — reads it.
    surreal: &'a store::surreal::SurrealDb,
    storage: &'a Arc<dyn StorageService>,
    email: &'a dyn EmailService,
    runtime: &'a dyn StateMachineRuntime,
    cfg: &'a ThreadConfig,
}

/// Thread one freshly-received inbound message. `raw_key` is the
/// object-storage key of the archived `.eml`.
///
/// # Errors
///
/// Propagates database and send errors for the caller to log.
#[allow(clippy::too_many_arguments)] // + the Surreal handle (#1093; ENG-19)
pub async fn thread_inbound(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn StorageService>,
    email: &dyn EmailService,
    runtime: &dyn StateMachineRuntime,
    cfg: &ThreadConfig,
    inbound: &InboundEmail,
    raw_key: &str,
) -> Result<(), ThreadError> {
    let ctx = ThreadCtx {
        surreal,
        storage,
        email,
        runtime,
        cfg,
    };
    let body = extract_body(inbound);
    match token_from_to(&inbound.to, &cfg.parse_host) {
        None => open_first_contact(&ctx, inbound, raw_key, &body).await,
        Some(token) => continue_thread(&ctx, inbound, raw_key, &body, &token).await,
    }
}

async fn open_first_contact(
    ctx: &ThreadCtx<'_>,
    inbound: &InboundEmail,
    raw_key: &str,
    body: &str,
) -> Result<(), ThreadError> {
    let surreal = ctx.surreal;
    let external_email = extract_addr(&inbound.from);
    let external_name = extract_name(&inbound.from);
    let token = mint_token();
    let person_id = person_lookup(surreal, &external_email).await?.map(|p| p.id);

    // Auto-route: a known client who has exactly one open matter threads
    // straight onto it — no manual `@link` needed for the common case. An
    // unknown sender or an ambiguous (multi-matter) client stays unlinked and
    // is triaged by a lawyer. This never weakens the conflict gate, but not
    // because a `persons` row exists — that proves nothing on its own. It is
    // that auto-linking requires an *open matter*, which the intake only
    // reaches after its conflict check returned without blocking.
    let notation_id = match person_id {
        Some(pid) => store::projects::sole_open_matter_for_person(surreal, pid).await?,
        None => None,
    };

    let conversation_id = conv::open(
        surreal,
        &conv::NewConversation {
            token: &token,
            external_email: &external_email,
            external_name: external_name.as_deref(),
            subject: &inbound.subject,
            person_id,
            notation_id,
        },
    )
    .await?;

    conv::append(
        surreal,
        &conv::NewMessage {
            conversation_id,
            direction: DIRECTION_FROM_EXTERNAL,
            from_addr: &external_email,
            to_addr: &inbound.to,
            subject: &inbound.subject,
            body_text: body,
            raw_storage_key: Some(raw_key),
            provider_message_id: inbound.message_id.as_deref(),
            ..Default::default()
        },
    )
    .await?;

    let mut notes: Vec<String> = Vec::new();
    // Firm-wide imputed-conflicts gate (RPC 1.10): a first contact from a
    // sender the firm has not screened is a prospective client. Prompt the
    // lawyer to run the conflict check before the first substantive relay —
    // the relay itself is held until `@cleared` (see `handle_lawyer_reply`).
    //
    // "Screened" is `store::conflicts::is_screened_client`, never "a
    // `persons` row resolved". A row is an identity; the question here is
    // whether a decision was taken about that identity.
    if !is_screened(surreal, person_id).await? {
        notes.push(format!(
            "⚠ Prospective client — {} is not yet in the system. {CONFLICT_CHECK_INSTRUCTION}",
            external_name.as_deref().unwrap_or(&external_email)
        ));
    }
    if let Some(conversation) = conv::by_id(surreal, conversation_id).await? {
        if let Some(note) = process_attachments(ctx, &conversation, &inbound.attachments).await? {
            notes.push(note);
        }
    }
    if let Some(note) = quarantine_note(inbound) {
        notes.push(note);
    }
    let extra_note = (!notes.is_empty()).then(|| notes.join("\n\n"));

    notify_lawyer(
        ctx,
        conversation_id,
        &token,
        external_name.as_deref(),
        &external_email,
        &inbound.subject,
        body,
        extra_note.as_deref(),
        &inbound.attachments,
    )
    .await?;
    // Mirror the exchange into the matter's conversation log (no-op until the
    // thread is matter-linked; idempotent if it already is).
    sync_conversation_to_spine(ctx, conversation_id).await
}

/// Mirror a conversation's client-facing email hops into the matter's
/// `communications` spine, so the privileged conversation log shows the email
/// exchange interleaved with document comments. Idempotent — keyed on
/// `(channel, the conversation-message id)` — so it is safe to call after
/// every hop and as a back-fill the instant a thread is linked to a matter.
///
/// A no-op until the conversation is matter-linked (the spine is
/// project-scoped). Only the client conversation is mirrored: `from_external`
/// (the client writing in) and `to_external` (the firm's relayed reply).
/// Lawyer notifications and `system` hops are firm plumbing, not the
/// conversation with the client, so they are skipped.
async fn sync_conversation_to_spine(
    ctx: &ThreadCtx<'_>,
    conversation_id: uuid::Uuid,
) -> Result<(), ThreadError> {
    let surreal = ctx.surreal;
    let Some(conversation) = conv::by_id(surreal, conversation_id).await? else {
        return Ok(());
    };
    let Some(notation_id) = conversation.notation_id else {
        return Ok(());
    };
    let Some(notation) = store::notations::find_by_id(ctx.surreal, notation_id).await? else {
        return Ok(());
    };
    let project_id = notation.project_id;

    for m in conv::messages(surreal, conversation_id).await? {
        let (channel, direction, author) = match m.direction.as_str() {
            DIRECTION_FROM_EXTERNAL => (
                store::communications::channel::EMAIL_INBOUND,
                store::communications::direction::INBOUND,
                conversation.person_id,
            ),
            DIRECTION_TO_EXTERNAL => (
                store::communications::channel::EMAIL_OUTBOUND,
                store::communications::direction::OUTBOUND,
                None,
            ),
            _ => continue,
        };
        // The conversation-message id is the stable idempotency key, so a
        // re-sync of an already-mirrored hop returns the existing spine row.
        let source_ref = m.id.to_string();
        store::communications::ingest(
            surreal,
            &store::communications::IngestArgs {
                project_id,
                channel,
                direction,
                author_person_id: author,
                counterparty: Some(conversation.external_email.as_str()),
                subject: Some(m.subject.as_str()),
                body: &m.body_text,
                source_ref: Some(&source_ref),
                asset_id: None,
                occurred_at: &m.inserted_at.to_rfc3339(),
            },
        )
        .await?;
    }
    Ok(())
}

/// Send one outbound hop from `support@` on a conversation and journal it
/// to the transcript with the provider's returned message-id. Always
/// carries the per-conversation `Reply-To` token and the thread's
/// message-id chain, so a support exchange threads in the recipient's
/// client and no internal address ever leaks — the three outbound paths
/// (lawyer notification, client relay, conflict-hold prompt) can't forget
/// either, because the helper owns them.
async fn send_and_journal(
    ctx: &ThreadCtx<'_>,
    conversation_id: uuid::Uuid,
    token: &str,
    direction: &str,
    to_addr: &str,
    subject: &str,
    body: &str,
) -> Result<SendReceipt, ThreadError> {
    send_and_journal_with_attachments(
        ctx,
        conversation_id,
        token,
        direction,
        to_addr,
        subject,
        body,
        &[],
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_and_journal_with_attachments(
    ctx: &ThreadCtx<'_>,
    conversation_id: uuid::Uuid,
    token: &str,
    direction: &str,
    to_addr: &str,
    subject: &str,
    body: &str,
    attachments: &[InboundAttachment],
) -> Result<SendReceipt, ThreadError> {
    let reply_to = reply_address(token, &ctx.cfg.parse_host);
    let thread_refs = thread_message_ids(ctx.surreal, conversation_id).await?;
    let html = workflows::email::render_email_html(body, &workflows::email::base_url_from_env());
    let outbound = attachments.iter().fold(
        OutboundEmail::new(to_addr, subject, body)
            .with_html(html)
            .with_reply_to(reply_to.as_str())
            .with_thread_refs(&thread_refs),
        |mail, attachment| {
            mail.with_attachment(Attachment::new(
                non_empty_or(&attachment.filename, "attachment"),
                non_empty_or(&attachment.content_type, "application/octet-stream"),
                attachment.bytes.clone(),
            ))
        },
    );
    let receipt = ctx.email.send(outbound).await?;
    conv::append(
        ctx.surreal,
        &conv::NewMessage {
            conversation_id,
            direction,
            from_addr: DEFAULT_FROM_EMAIL,
            to_addr,
            subject,
            body_text: body,
            provider_message_id: receipt.message_id.as_deref(),
            ..Default::default()
        },
    )
    .await?;
    Ok(receipt)
}

#[allow(clippy::too_many_arguments)]
async fn notify_lawyer(
    ctx: &ThreadCtx<'_>,
    conversation_id: uuid::Uuid,
    token: &str,
    external_name: Option<&str>,
    external_email: &str,
    subject: &str,
    body: &str,
    extra_note: Option<&str>,
    attachments: &[InboundAttachment],
) -> Result<(), ThreadError> {
    let display = external_name.unwrap_or(external_email);
    let out_subject = format!("[{display}] {subject}");
    let mut out_body = lawyer_notification_body(display, external_email, subject, body, extra_note);
    let attachments = if outbound_attachments_fit(&out_body, attachments) {
        attachments
    } else {
        out_body.push_str(
            "\n\nAttachments were scanned and retained in the matter/raw message, but were not \
             forwarded because base64 expansion would exceed SendGrid's 30 MB message limit.",
        );
        &[]
    };

    send_and_journal_with_attachments(
        ctx,
        conversation_id,
        token,
        DIRECTION_TO_LAWYER,
        ctx.cfg.lawyer_notify_email.as_str(),
        &out_subject,
        &out_body,
        attachments,
    )
    .await?;
    conv::set_status(ctx.surreal, conversation_id, STATUS_AWAITING_LAWYER).await?;
    Ok(())
}

fn outbound_attachments_fit(body: &str, attachments: &[InboundAttachment]) -> bool {
    let encoded_bytes = attachments
        .iter()
        .map(|attachment| attachment.bytes.len().div_ceil(3) * 4)
        .sum::<usize>();
    let metadata_bytes = attachments
        .iter()
        .map(|attachment| attachment.filename.len() + attachment.content_type.len() + 128)
        .sum::<usize>();
    body.len()
        .saturating_add(encoded_bytes)
        .saturating_add(metadata_bytes)
        < SENDGRID_MAX_MESSAGE_BYTES
}

async fn continue_thread(
    ctx: &ThreadCtx<'_>,
    inbound: &InboundEmail,
    raw_key: &str,
    body: &str,
    token: &str,
) -> Result<(), ThreadError> {
    let surreal = ctx.surreal;
    let Some(conversation) = conv::by_token(surreal, token).await? else {
        tracing::warn!(
            token,
            "inbound reply for an unknown conversation token; ignoring"
        );
        return Ok(());
    };
    let sender = extract_addr(&inbound.from);
    let acting_person = person_lookup(surreal, &sender)
        .await?
        .filter(|person| person.role.is_lawyer_tier());

    if let Some(acting_person) = acting_person {
        handle_lawyer_reply(
            ctx,
            inbound,
            raw_key,
            body,
            token,
            &conversation,
            &acting_person,
        )
        .await?;
    } else {
        // Client follow-up on an open thread — re-notify lawyer.
        conv::append(
            surreal,
            &conv::NewMessage {
                conversation_id: conversation.id,
                direction: DIRECTION_FROM_EXTERNAL,
                from_addr: &sender,
                to_addr: &inbound.to,
                subject: &inbound.subject,
                body_text: body,
                raw_storage_key: Some(raw_key),
                provider_message_id: inbound.message_id.as_deref(),
                ..Default::default()
            },
        )
        .await?;
        // File any attachments as documents on the linked matter and fold a
        // review request into the lawyer notification.
        let attachments_note =
            process_attachments(ctx, &conversation, &inbound.attachments).await?;
        let extra_note = combine_notes(attachments_note, quarantine_note(inbound));
        notify_lawyer(
            ctx,
            conversation.id,
            token,
            conversation.external_name.as_deref(),
            &conversation.external_email,
            &conversation.subject,
            body,
            extra_note.as_deref(),
            &inbound.attachments,
        )
        .await?;
    }
    // Mirror the (possibly newly-linked, via `@link`) exchange into the
    // matter's conversation log. Idempotent and a no-op until linked.
    sync_conversation_to_spine(ctx, conversation.id).await
}

fn quarantine_note(inbound: &InboundEmail) -> Option<String> {
    if inbound.quarantined_attachments.is_empty() {
        return None;
    }
    let filenames = inbound
        .quarantined_attachments
        .iter()
        .map(|attachment| attachment.filename.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "{} attachment(s) quarantined after malware scanning and not forwarded or filed: {filenames}",
        inbound.quarantined_attachments.len()
    ))
}

fn combine_notes(left: Option<String>, right: Option<String>) -> Option<String> {
    match (left, right) {
        (Some(left), Some(right)) => Some(format!("{left}\n\n{right}")),
        (Some(note), None) | (None, Some(note)) => Some(note),
        (None, None) => None,
    }
}

/// File a client's inbound attachments into the canonical `documents`
/// lane and record the ingest as a `system` hop in the transcript, so a
/// PDF a client emails to `support@` becomes a reviewable matter document.
///
/// Ingestion needs an owning project, which we resolve through the
/// conversation's linked `notation` — so it runs only when the thread is
/// tied to a matter (mirroring how [`fire_signal`] no-ops without one).
/// On an unlinked thread the bytes stay in the archived `.eml` and we
/// return a note telling lawyer to link the thread first. Returns the
/// lawyer-facing review note, or `None` when there were no attachments.
async fn process_attachments(
    ctx: &ThreadCtx<'_>,
    conversation: &conv::EmailConversation,
    attachments: &[InboundAttachment],
) -> Result<Option<String>, ThreadError> {
    let (surreal, storage) = (ctx.surreal, ctx.storage);
    if attachments.is_empty() {
        return Ok(None);
    }
    let Some(notation_id) = conversation.notation_id else {
        tracing::info!(
            conversation_id = %conversation.id,
            count = attachments.len(),
            "inbound attachments on a thread with no linked matter; archived in raw .eml only"
        );
        return Ok(Some(format!(
            "{} attachment(s) received and archived in the raw message; link this thread to a \
             matter to file them as documents.",
            attachments.len()
        )));
    };
    let Some(notation) = store::notations::find_by_id(surreal, notation_id).await? else {
        tracing::warn!(%notation_id, "conversation references a missing notation; attachments not filed");
        return Ok(None);
    };

    // File each attachment as the external sender, so the matter repo's
    // `git log` attributes it to whoever emailed it in.
    let author = repos::Author {
        name: conversation
            .external_name
            .as_deref()
            .unwrap_or(conversation.external_email.as_str()),
        email: conversation.external_email.as_str(),
    };

    let mut lines = Vec::new();
    for att in attachments {
        let filename = non_empty_or(att.filename.as_str(), "attachment");
        let content_type = non_empty_or(att.content_type.as_str(), "application/octet-stream");
        let ingested = crate::matter_documents::record_document(
            surreal,
            storage,
            author,
            &documents::IngestArgs {
                project_id: notation.project_id,
                source: documents::source::EMAIL,
                filename,
                kind: "unclassified",
                content_type,
                description: Some("received via support@ email"),
                secondary_storage_key: None,
                visibility: documents::visibility::INTERNAL,
            },
            &att.bytes,
        )
        .await?;
        lines.push(format!(
            "• {filename} ({} bytes) → document {}",
            ingested.byte_size, ingested.asset_id
        ));
    }

    let summary = format!(
        "{} document(s) received for review and filed to the matter:\n{}",
        attachments.len(),
        lines.join("\n")
    );

    conv::append(
        surreal,
        &conv::NewMessage {
            conversation_id: conversation.id,
            direction: DIRECTION_SYSTEM,
            from_addr: &conversation.external_email,
            to_addr: DEFAULT_FROM_EMAIL,
            subject: &conversation.subject,
            body_text: &summary,
            ..Default::default()
        },
    )
    .await?;

    Ok(Some(summary))
}

/// `s` trimmed, or `fallback` when `s` is blank.
fn non_empty_or<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    if s.trim().is_empty() {
        fallback
    } else {
        s
    }
}

/// Handle a reply from a lawyer/admin sender on a known conversation: the
/// privileged path that may relay to the client and fire workflow commands.
///
/// Every command variant and the relay are gated on
/// [`dkim_passes_for_domain`] for the sender's own domain — a lawyer-tier
/// `From:` header plus a thread token is not evidence of authorship. When
/// `verify_dkim_domain` is configured it pins replies to that one firm domain
/// as a strictly narrower check on top.
async fn handle_lawyer_reply(
    ctx: &ThreadCtx<'_>,
    inbound: &InboundEmail,
    raw_key: &str,
    body: &str,
    token: &str,
    conversation: &conv::EmailConversation,
    acting_person: &store::persons::Person,
) -> Result<(), ThreadError> {
    let (surreal, cfg) = (ctx.surreal, ctx.cfg);
    let sender = acting_person.email.as_str();
    // Command-channel authentication (Scorpio's non-negotiable): a lawyer
    // reply may relay or fire a workflow signal only when its DKIM verdict
    // passes for the firm domain. Without this a forged `From:
    // nick@neonlaw.com` to a leaked token could approve a retainer or relay
    // arbitrary content to the client as support@. Enforced only when
    // configured; the raw `.eml` is archived regardless, and the failed
    // attempt is journaled for the transcript.
    if let Some(domain) = cfg.verify_dkim_domain.as_deref() {
        if !dkim_passes_for_domain(&inbound.dkim, domain) {
            tracing::warn!(
                token,
                sender,
                dkim = %inbound.dkim,
                "lawyer reply failed DKIM for {domain}; not relaying or executing commands"
            );
            conv::append(
                surreal,
                &conv::NewMessage {
                    conversation_id: conversation.id,
                    direction: DIRECTION_FROM_LAWYER,
                    from_addr: sender,
                    to_addr: &inbound.to,
                    subject: &inbound.subject,
                    body_text: body,
                    raw_storage_key: Some(raw_key),
                    provider_message_id: inbound.message_id.as_deref(),
                    ..Default::default()
                },
            )
            .await?;
            return Ok(());
        }
    }

    // Is the `From:` header cryptographically authentic? The lawyer-tier
    // `persons` lookup above trusts that header to name the sender, and a
    // header is free to write. DKIM is what makes it evidence: requiring a
    // `pass` for the sender's OWN domain is the alignment check that turns
    // "claims to be Nick" into "was signed by neonlaw.com".
    //
    // This gate covers the whole privileged path — every command variant and
    // the client relay — rather than one command. Scoping it to `@approve`
    // left four other verbs and the relay running on sender-plus-token trust,
    // and the token is not a secret: it is stamped as `Reply-To` on every
    // outbound hop, so any client who has received a relayed reply holds a
    // valid one for their own thread. `@link` is the sharpest of them: it
    // accepts any existing notation id with no participation check, so a
    // forged one cross-links a stranger's thread onto a client's matter,
    // files their attachments into it, and — because `is_prospect` reads
    // `notation_id.is_none()` — lifts the RPC 1.10 imputed-conflicts hold
    // without ever sending `@cleared`.
    //
    // This is deliberately not `cfg.verify_dkim_domain` — that option pins
    // replies to one firm domain and is unset in every deployment manifest in
    // the tree, so keying the command channel to it would leave the channel
    // open by default. This check needs no configuration and is therefore on
    // everywhere. Where the option IS set, the stricter gate above has already
    // returned, so the two compose.
    //
    // The disposition is a hold, not a drop: nothing executes and nothing
    // relays, but the reply is journaled and the cockpit is told, so a
    // genuine attorney whose mail arrives without a usable verdict sees that
    // it was held and can act from `/app/lawyer/*`, where the session names
    // them. Refusing a signature is recoverable; silently swallowing
    // attorney correspondence is not.
    let sender_domain = extract_domain(&inbound.from);
    if !dkim_passes_for_domain(&inbound.dkim, &sender_domain) {
        hold_unauthenticated_reply(ctx, inbound, raw_key, body, token, conversation, sender)
            .await?;
        return Ok(());
    }

    let cleaned = strip_quoted(body);
    let parsed = parse_reply(&cleaned);
    let command_payload = (!parsed.commands.is_empty())
        .then(|| serde_json::to_string(&parsed.commands).unwrap_or_default());

    // The full cleaned reply (commands included) is journaled; the relay
    // carries only the prose with command lines stripped.
    conv::append(
        surreal,
        &conv::NewMessage {
            conversation_id: conversation.id,
            direction: DIRECTION_FROM_LAWYER,
            from_addr: sender,
            to_addr: &inbound.to,
            subject: &inbound.subject,
            body_text: &cleaned,
            raw_storage_key: Some(raw_key),
            provider_message_id: inbound.message_id.as_deref(),
            command_payload: command_payload.as_deref(),
            ..Default::default()
        },
    )
    .await?;

    // Execute directives. `@close`/`@internal` suppress the relay;
    // `@signal`/`@approve`/`@deny` fire a workflow signal on the linked
    // notation (the production lawyer-review gate) and still relay any
    // accompanying prose. A successful `@link` updates this local model so a
    // same-message `@approve` and the relay gate below both see the new link.
    //
    // Every variant below runs only on a sender the gate above authenticated.
    let mut conversation = conversation.clone();
    let mut suppress_relay = false;
    for command in &parsed.commands {
        match command {
            Command::Close => {
                conv::set_status(surreal, conversation.id, STATUS_CLOSED).await?;
                suppress_relay = true;
            }
            Command::Internal => suppress_relay = true,
            // Clearance is journaled via this message's `command_payload`;
            // the relay gate below reads it back. Nothing else to do here.
            Command::ConflictCleared => {}
            Command::Link { notation_id } => {
                if let Some(linked) = link_notation(ctx, &conversation, notation_id, token).await? {
                    conversation.notation_id = Some(linked);
                }
            }
            Command::Signal { condition, value } => {
                // An attorney signature must name its signer. `approved`
                // leaves `lawyer_review` for `generate_pdf__*` and then
                // `sent_for_signature__pending`, so a signal is the one
                // command that may never run on sender-trust alone: a `From:`
                // header is forgeable, and the thread token is a bearer
                // credential that travels in cleartext on every notification
                // the cockpit receives. When the deployment names no domain
                // to verify against, we cannot establish who signed — and an
                // ingress that cannot name the signer must refuse to sign.
                //
                // Refusing is the recoverable direction: a genuine approval
                // can be re-made from `/app/lawyer/*`, which authenticates
                // the attorney by session. Accepting a forged one is not
                // recoverable, because the letter has already gone out. That
                // refusal now happens for every command at the gate above,
                // because a forged `@link` or `@cleared` is no less final.
                fire_signal(
                    ctx,
                    &conversation,
                    condition,
                    value.as_deref(),
                    acting_person.id,
                )
                .await?;
            }
        }
    }

    if !suppress_relay && !parsed.relay_body.trim().is_empty() {
        // Firm-wide imputed-conflicts gate (RPC 1.10): the first substantive
        // relay to a prospective client is held until lawyers have run the
        // conflict check and released it with `@cleared`. A prospective
        // client is an external party the firm has neither screened
        // (`store::conflicts::is_screened_client`) nor threaded onto a matter
        // (`notation_id`). The check is a gate in the loop, never a courtesy
        // after the fact.
        //
        // The screen is asked of the conflict graph rather than of the
        // `persons` table, because a row is an identity and not a decision.
        // Reading presence as clearance inverted this gate on the one
        // population it exists for: a refused intake leaves its person row
        // behind, so the firm's own refusal was what marked someone cleared.
        let is_prospect = !is_screened(surreal, conversation.person_id).await?
            && conversation.notation_id.is_none();
        if is_prospect && !is_conflict_cleared(surreal, conversation.id).await? {
            hold_relay_for_conflict_check(ctx, &conversation, token).await?;
        } else {
            relay_to_external(ctx, &conversation, token, &parsed.relay_body).await?;
        }
    }
    Ok(())
}

/// Hold a lawyer reply whose sender could not be cryptographically
/// authenticated: journal the hop, tell the cockpit nothing was acted on, and
/// park the conversation for a human. No command runs and nothing relays.
///
/// A silently dropped reply is the same class of defect as a silently accepted
/// one: in both cases the thread's state does not match what the attorney
/// believes they did. So this holds rather than drops — the same disposition
/// [`hold_relay_for_conflict_check`] uses when it declines to relay.
async fn hold_unauthenticated_reply(
    ctx: &ThreadCtx<'_>,
    inbound: &InboundEmail,
    raw_key: &str,
    body: &str,
    token: &str,
    conversation: &conv::EmailConversation,
    sender: &str,
) -> Result<(), ThreadError> {
    tracing::error!(
        conversation_id = %conversation.id,
        dkim = %inbound.dkim,
        "holding a lawyer reply from an unauthenticated sender; no command ran and nothing relayed"
    );
    // Journaled WITHOUT `command_payload`: the directives on this message were
    // never executed, and `is_conflict_cleared` reads that field back off the
    // transcript. Recording them would let a forged `@cleared` release the
    // RPC 1.10 hold from inside the very record of its refusal.
    conv::append(
        ctx.surreal,
        &conv::NewMessage {
            conversation_id: conversation.id,
            direction: DIRECTION_FROM_LAWYER,
            from_addr: sender,
            to_addr: &inbound.to,
            subject: &inbound.subject,
            body_text: body,
            raw_storage_key: Some(raw_key),
            provider_message_id: inbound.message_id.as_deref(),
            ..Default::default()
        },
    )
    .await?;
    send_and_journal(
        ctx,
        conversation.id,
        token,
        DIRECTION_SYSTEM,
        ctx.cfg.lawyer_notify_email.as_str(),
        &format!("[refused] {}", conversation.subject),
        concat!(
            "This reply was NOT acted on and NOT relayed to the client: its sender ",
            "could not be cryptographically authenticated (no DKIM pass for the ",
            "sending domain). Any commands it carried were ignored. The thread is ",
            "waiting for a lawyer; act from the portal instead, where the session ",
            "names the attorney."
        ),
    )
    .await?;
    conv::set_status(ctx.surreal, conversation.id, STATUS_AWAITING_LAWYER).await?;
    Ok(())
}

/// True once a `@cleared` directive has been recorded on the conversation
/// (the current reply's directives are already journaled by the time the
/// relay gate consults this).
async fn is_conflict_cleared(
    surreal: &store::surreal::SurrealDb,
    conversation_id: uuid::Uuid,
) -> Result<bool, store::email_conversations::EmailConversationError> {
    for m in conv::messages(surreal, conversation_id).await? {
        let Some(payload) = m.command_payload.as_deref() else {
            continue;
        };
        if let Ok(commands) = serde_json::from_str::<Vec<Command>>(payload) {
            if commands.contains(&Command::ConflictCleared) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Hold a lawyer relay that targets a not-yet-cleared prospective client:
/// journal a `system` hop and bounce a prompt back to the cockpit asking
/// for the firm-wide conflict check before the message reaches the client.
/// Nothing is relayed to the external party.
async fn hold_relay_for_conflict_check(
    ctx: &ThreadCtx<'_>,
    conversation: &conv::EmailConversation,
    token: &str,
) -> Result<(), ThreadError> {
    // `conversation_id` already identifies which relay was held; the external
    // address adds nothing to the signal and is client-identifying content in a
    // record that leaves the firm's trust boundary. The prospective client's
    // address is on the conversation row for anyone who needs it.
    tracing::warn!(
        conversation_id = %conversation.id,
        "relay held: firm-wide conflict check not cleared for this prospective client"
    );
    let note = format!(
        "Your reply was NOT relayed. {} is a prospective client not yet in the system. \
         {CONFLICT_CHECK_INSTRUCTION}",
        conversation
            .external_name
            .as_deref()
            .unwrap_or(&conversation.external_email)
    );
    // Journaled as a `system` hop (not `to_lawyer`): this records the loop's
    // decision to hold, and `system` hops are excluded from the message-id
    // chain so the prompt never pollutes the client-facing References.
    let subject = format!("[conflict check] {}", conversation.subject);
    send_and_journal(
        ctx,
        conversation.id,
        token,
        DIRECTION_SYSTEM,
        ctx.cfg.lawyer_notify_email.as_str(),
        &subject,
        &note,
    )
    .await?;
    conv::set_status(ctx.surreal, conversation.id, STATUS_AWAITING_LAWYER).await?;
    Ok(())
}

async fn relay_to_external(
    ctx: &ThreadCtx<'_>,
    conversation: &conv::EmailConversation,
    token: &str,
    cleaned: &str,
) -> Result<(), ThreadError> {
    let subject = re_subject(&conversation.subject);
    send_and_journal(
        ctx,
        conversation.id,
        token,
        DIRECTION_TO_EXTERNAL,
        conversation.external_email.as_str(),
        &subject,
        cleaned,
    )
    .await?;
    conv::set_status(ctx.surreal, conversation.id, STATUS_AWAITING_CLIENT).await?;
    Ok(())
}

/// The RFC 5322 message-ids of the inbound hops on a conversation, oldest
/// first — the chain put in outbound `References`/`In-Reply-To` so the
/// attorney's mail client threads the whole support exchange. Only inbound
/// hops (`from_external`/`from_lawyer`) carry a real RFC message-id;
/// outbound hops carry SendGrid's `X-Message-Id` (a different namespace),
/// so they're excluded.
async fn thread_message_ids(
    surreal: &store::surreal::SurrealDb,
    conversation_id: uuid::Uuid,
) -> Result<Vec<String>, store::email_conversations::EmailConversationError> {
    Ok(conv::messages(surreal, conversation_id)
        .await?
        .into_iter()
        .filter(|m| m.direction == DIRECTION_FROM_EXTERNAL || m.direction == DIRECTION_FROM_LAWYER)
        .filter_map(|m| m.provider_message_id)
        .collect())
}

async fn person_lookup(
    surreal: &store::surreal::SurrealDb,
    email: &str,
) -> Result<Option<store::persons::Person>, store::persons::PersonError> {
    store::persons::find_by_email_ci(surreal, email).await
}

/// Has the firm run a conflict check on this external party?
///
/// `None` — no `persons` row resolved for the address at all — is the
/// clear case: nobody can have screened someone the directory has never
/// heard of. A resolved row is the case that needs asking, and the answer
/// comes from `store::conflicts`, which owns the definition of a party the
/// firm serves. This lane must not invent a second one.
async fn is_screened(
    surreal: &store::surreal::SurrealDb,
    person_id: Option<uuid::Uuid>,
) -> Result<bool, String> {
    match person_id {
        None => Ok(false),
        Some(id) => store::conflicts::is_screened_client(surreal, id).await,
    }
}

/// Execute `@link <notation_id>`: validate the id, confirm the matter
/// exists, then bind the conversation to it so the command channel and
/// attachment-filing fire on a live matter. Either outcome is reported back
/// to the cockpit as a `system` hop (never relayed to the client); a bad or
/// unknown id leaves the conversation unlinked. Returns the linked notation
/// id on success.
async fn link_notation(
    ctx: &ThreadCtx<'_>,
    conversation: &conv::EmailConversation,
    raw_id: &str,
    token: &str,
) -> Result<Option<uuid::Uuid>, ThreadError> {
    let outcome = match uuid::Uuid::parse_str(raw_id.trim()) {
        Err(_) => {
            tracing::warn!(conversation_id = %conversation.id, raw_id, "could not @link: invalid notation id");
            Err(format!(
                "Could not link: \"{raw_id}\" is not a valid matter id. \
                 Reply @link <notation_id> with the matter's id."
            ))
        }
        Ok(notation_id)
            if store::notations::find_by_id(ctx.surreal, notation_id)
                .await?
                .is_none() =>
        {
            tracing::warn!(conversation_id = %conversation.id, %notation_id, "could not @link: no such matter");
            Err(format!(
                "Could not link: no matter found for {notation_id}."
            ))
        }
        Ok(notation_id) => {
            conv::set_notation(ctx.surreal, conversation.id, notation_id).await?;
            tracing::info!(conversation_id = %conversation.id, %notation_id, "conversation linked to matter via @link");
            Ok(notation_id)
        }
    };

    // Report either outcome back to the cockpit as a `system` hop — never
    // relayed to the client.
    let note = match &outcome {
        Ok(notation_id) => format!(
            "Linked this conversation to matter {notation_id}. \
             Lawyer commands (@approve / @deny / @signal) and inbound attachments \
             now act on this matter."
        ),
        Err(msg) => msg.clone(),
    };
    send_and_journal(
        ctx,
        conversation.id,
        token,
        DIRECTION_SYSTEM,
        ctx.cfg.lawyer_notify_email.as_str(),
        &format!("[link] {}", conversation.subject),
        &note,
    )
    .await?;
    Ok(outcome.ok())
}

/// Fire a workflow signal on the conversation's linked notation, then
/// mirror the resulting state into the `notations` row (the same pattern
/// the e-signature webhook uses) so the admin UI reflects the advance. A
/// command on a conversation with no linked notation is a logged no-op.
async fn fire_signal(
    ctx: &ThreadCtx<'_>,
    conversation: &conv::EmailConversation,
    condition: &str,
    value: Option<&str>,
    acting_person_id: uuid::Uuid,
) -> Result<(), ThreadError> {
    let runtime = ctx.runtime;
    let Some(notation_id) = conversation.notation_id else {
        tracing::warn!(
            conversation_id = %conversation.id,
            condition,
            "lawyer command signal but conversation has no linked notation; ignoring"
        );
        return Ok(());
    };
    let next = runtime
        .signal_with_context(
            MachineKind::Workflow,
            notation_id,
            condition,
            value,
            SignalContext { acting_person_id },
        )
        .await
        .map_err(|e| ThreadError::Runtime(e.to_string()))?;
    if store::notations::find_by_id(ctx.surreal, notation_id)
        .await?
        .is_some()
    {
        store::notations::update_state(ctx.surreal, notation_id, next.as_str()).await?;
    }
    tracing::info!(%notation_id, condition, next_state = %next.as_str(), "lawyer command advanced workflow");
    Ok(())
}

/// Parse a lawyer reply into the prose to relay plus any directives. A
/// line whose trimmed form is `@<verb> …` for a recognized verb is a
/// command and is removed from the relay; unrecognized `@…` lines pass
/// through untouched (e.g. an `@mention` to the client).
fn parse_reply(body: &str) -> ParsedReply {
    let mut kept = Vec::new();
    let mut commands = Vec::new();
    for line in body.lines() {
        if let Some(command) = parse_command_line(line) {
            commands.push(command);
        } else {
            kept.push(line);
        }
    }
    ParsedReply {
        relay_body: kept.join("\n").trim().to_string(),
        commands,
    }
}

fn parse_command_line(line: &str) -> Option<Command> {
    let rest = line.trim().strip_prefix('@')?;
    let mut parts = rest.split_whitespace();
    let verb = parts.next()?.to_lowercase();
    match verb.as_str() {
        "approve" => Some(Command::Signal {
            condition: "approved".to_string(),
            value: None,
        }),
        "deny" | "reject" => {
            let reason = parts.collect::<Vec<_>>().join(" ");
            Some(Command::Signal {
                condition: "rejected".to_string(),
                value: (!reason.is_empty()).then_some(reason),
            })
        }
        "signal" => {
            let condition = parts.next()?.to_string();
            let value = parts.collect::<Vec<_>>().join(" ");
            Some(Command::Signal {
                condition,
                value: (!value.is_empty()).then_some(value),
            })
        }
        "close" => Some(Command::Close),
        "internal" => Some(Command::Internal),
        "cleared" | "clear" => Some(Command::ConflictCleared),
        // Always a command (even bare) so the `@link` line is stripped from
        // the relay and never leaks to the client; a missing/invalid id is
        // reported back to the cockpit at execution time.
        "link" => Some(Command::Link {
            notation_id: parts.next().unwrap_or_default().to_string(),
        }),
        // Unknown `@verb` — not a command; leave the line in the relay.
        _ => None,
    }
}

/// True when SendGrid's DKIM verdict reports `pass` for `domain`.
///
/// The verdict field is a Ruby-hash-shaped string — `{@neonlaw.com : pass}`,
/// or `{@a.com : pass, @b.com : fail}` when a message carries multiple
/// signatures. We require an explicit `pass` for the target firm domain;
/// any other domain's result, a `fail`/`none`, or an empty/absent field
/// is untrusted.
fn dkim_passes_for_domain(dkim_field: &str, domain: &str) -> bool {
    let target = domain.trim().trim_start_matches('@').to_lowercase();
    if target.is_empty() {
        return false;
    }
    let inner = dkim_field
        .trim()
        .trim_start_matches('{')
        .trim_end_matches('}');
    inner.split(',').any(|entry| {
        let mut parts = entry.splitn(2, ':');
        let d = parts
            .next()
            .unwrap_or_default()
            .trim()
            .trim_start_matches('@')
            .to_lowercase();
        let result = parts.next().unwrap_or_default().trim().to_lowercase();
        d == target && result == "pass"
    })
}

/// The plain-text body of an inbound message. In raw mode SendGrid sends
/// no parsed `text` part, so fall back to MIME-parsing the raw bytes.
fn extract_body(inbound: &InboundEmail) -> String {
    if !inbound.text.trim().is_empty() {
        return inbound.text.clone();
    }
    mail_parser::MessageParser::default()
        .parse(&inbound.raw)
        .and_then(|m| m.body_text(0).map(std::borrow::Cow::into_owned))
        .unwrap_or_default()
}

/// An unguessable 32-hex-char thread token (16 random bytes).
fn mint_token() -> String {
    let bytes: [u8; 16] = rand::random();
    let mut out = String::with_capacity(32);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The `Reply-To` address that threads a reply back to a conversation.
fn reply_address(token: &str, parse_host: &str) -> String {
    format!("c{token}@{parse_host}")
}

/// Pull the bare, lowercased email out of a header value that may be
/// `Name <addr@host>` or just `addr@host`.
fn extract_addr(raw: &str) -> String {
    let s = raw.trim();
    if let (Some(lt), Some(gt)) = (s.find('<'), s.find('>')) {
        if lt < gt {
            return s[lt + 1..gt].trim().to_lowercase();
        }
    }
    s.to_lowercase()
}

/// The lowercased domain of a header value that may be `Name <addr@host>` or
/// just `addr@host`. Empty when the address carries no `@`.
fn extract_domain(raw: &str) -> String {
    extract_addr(raw)
        .rsplit_once('@')
        .map_or_else(String::new, |(_, domain)| domain.to_string())
}

/// The display name from a `Name <addr>` header value, if present.
fn extract_name(raw: &str) -> Option<String> {
    let s = raw.trim();
    let lt = s.find('<')?;
    let name = s[..lt].trim().trim_matches('"').trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// If any address in `to` is a token address on `parse_host`
/// (`c<32-hex>@parse_host`), return the token. Scans all addresses so a
/// reply carrying extra recipients still threads.
fn token_from_to(to: &str, parse_host: &str) -> Option<String> {
    let suffix = format!("@{}", parse_host.to_lowercase());
    to.split([' ', '\t', '\r', '\n', ',', ';', '<', '>'])
        .map(|t| t.trim().trim_matches('"').to_lowercase())
        .find_map(|addr| {
            let local = addr.strip_suffix(&suffix)?;
            let token = local.strip_prefix('c')?;
            (token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit()))
                .then(|| token.to_string())
        })
}

/// Strip quoted history and signature from a lawyer reply so only the new
/// prose is relayed. Heuristic: cut at the first Gmail-style attribution
/// line, the first quoted (`>`) line, or the `-- ` signature delimiter.
fn strip_quoted(body: &str) -> String {
    let mut kept = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        if line == "-- "
            || line.starts_with('>')
            || (trimmed.starts_with("On ") && trimmed.ends_with("wrote:"))
        {
            break;
        }
        kept.push(line);
    }
    kept.join("\n").trim_end().to_string()
}

/// Ensure a subject carries a single `Re:` prefix for the relay.
fn re_subject(subject: &str) -> String {
    if subject.trim_start().to_lowercase().starts_with("re:") {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

fn lawyer_notification_body(
    display: &str,
    external_email: &str,
    subject: &str,
    body: &str,
    extra_note: Option<&str>,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "New message via {DEFAULT_FROM_EMAIL}");
    let _ = writeln!(s);
    let _ = writeln!(s, "From:    {display} <{external_email}>");
    let _ = writeln!(s, "Subject: {subject}");
    let _ = writeln!(s);
    let _ = writeln!(s, "{}", body.trim_end());
    if let Some(note) = extra_note {
        let _ = writeln!(s);
        let _ = writeln!(s, "{}", note.trim_end());
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "--");
    let _ = writeln!(
        s,
        "Reply to this email to respond. Your reply is relayed from \
         {DEFAULT_FROM_EMAIL}; the client never sees your address."
    );
    s
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use std::sync::Arc;

    use cloud::StorageService;

    use super::{
        dkim_passes_for_domain, extract_addr, extract_name, mint_token, parse_reply, re_subject,
        strip_quoted, thread_inbound, token_from_to, Command, ThreadConfig,
    };
    use crate::email::CapturingEmail;
    use crate::inbound_email::{InboundAttachment, InboundEmail};
    use store::test_support::mem_surreal;

    /// A throwaway filesystem-backed `StorageService` for tests. The temp
    /// dir is intentionally leaked so it outlives the test even though the
    /// `TempDir` handle is dropped — the document-ingest path needs the
    /// directory to stay writable for the whole run.
    async fn storage() -> Arc<dyn StorageService> {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        Arc::new(cloud::FsStorage::new(root).await.unwrap())
    }
    use workflows::{
        MachineKind, StateMachineRuntime, StateName, WorkflowEvent, WorkflowRuntimeError,
        WorkflowSpec,
    };

    /// Minimal `StateMachineRuntime` that records the signals fired at it,
    /// so command tests can assert `@approve` reached the workflow.
    #[derive(Default)]
    struct RecordingRuntime {
        signals: Mutex<Vec<(uuid::Uuid, String, Option<String>)>>,
        contexts: Mutex<Vec<workflows::SignalContext>>,
    }

    #[async_trait::async_trait]
    impl StateMachineRuntime for RecordingRuntime {
        async fn start(
            &self,
            _kind: MachineKind,
            _notation_id: uuid::Uuid,
            _spec: &WorkflowSpec,
        ) -> Result<(), WorkflowRuntimeError> {
            Ok(())
        }
        async fn signal(
            &self,
            _kind: MachineKind,
            notation_id: uuid::Uuid,
            condition: &str,
            payload: Option<&str>,
        ) -> Result<StateName, WorkflowRuntimeError> {
            self.signals.lock().unwrap().push((
                notation_id,
                condition.to_string(),
                payload.map(str::to_string),
            ));
            Ok(StateName::from(condition))
        }
        async fn signal_with_context(
            &self,
            kind: MachineKind,
            notation_id: uuid::Uuid,
            condition: &str,
            payload: Option<&str>,
            context: workflows::SignalContext,
        ) -> Result<StateName, WorkflowRuntimeError> {
            self.contexts.lock().unwrap().push(context);
            self.signal(kind, notation_id, condition, payload).await
        }
        async fn current_state(
            &self,
            _kind: MachineKind,
            _notation_id: uuid::Uuid,
        ) -> Option<StateName> {
            None
        }
        async fn events(&self, _kind: MachineKind, _notation_id: uuid::Uuid) -> Vec<WorkflowEvent> {
            Vec::new()
        }
    }

    #[test]
    fn mint_token_is_32_hex_and_unique() {
        let a = mint_token();
        let b = mint_token();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn token_from_to_matches_only_token_addresses() {
        let host = "parse.neonlaw.com";
        let token = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            token_from_to(&format!("c{token}@{host}"), host).as_deref(),
            Some(token)
        );
        // first contact — no token
        assert_eq!(token_from_to("test@parse.neonlaw.com", host), None);
        // right shape, wrong host
        assert_eq!(token_from_to(&format!("c{token}@evil.com"), host), None);
        // display-name wrapped + extra recipient still threads
        assert_eq!(
            token_from_to(&format!("\"Support\" <c{token}@{host}>, cc@x.com"), host).as_deref(),
            Some(token)
        );
    }

    #[test]
    fn addr_and_name_extraction() {
        assert_eq!(
            extract_addr("AIDA Smoke <smoke@neonlaw.com>"),
            "smoke@neonlaw.com"
        );
        assert_eq!(extract_addr("plain@example.com"), "plain@example.com");
        assert_eq!(extract_addr("UPPER@Example.COM"), "upper@example.com");
        assert_eq!(
            extract_name("AIDA Smoke <smoke@neonlaw.com>").as_deref(),
            Some("AIDA Smoke")
        );
        assert_eq!(extract_name("plain@example.com"), None);
    }

    #[test]
    fn strip_quoted_cuts_history_and_signature() {
        let body = "Approved — here is your answer.\n\nOn Tue, Jun 3, 2026 at 9:00 AM Pisces <c@x.com> wrote:\n> original question";
        assert_eq!(strip_quoted(body), "Approved — here is your answer.");

        let with_sig = "Thanks, that works.\n-- \nNick\nNeon Law";
        assert_eq!(strip_quoted(with_sig), "Thanks, that works.");
    }

    #[test]
    fn dkim_verdict_parsing_requires_pass_for_the_target_domain() {
        assert!(dkim_passes_for_domain(
            "{@neonlaw.com : pass}",
            "neonlaw.com"
        ));
        // case-insensitive on the domain; tolerant of the @ in config
        assert!(dkim_passes_for_domain(
            "{@NeonLaw.com : pass}",
            "@neonlaw.com"
        ));
        // multi-signature: the firm domain passes even if another fails
        assert!(dkim_passes_for_domain(
            "{@sendgrid.me : fail, @neonlaw.com : pass}",
            "neonlaw.com"
        ));
        // a fail for the firm domain is not trusted
        assert!(!dkim_passes_for_domain(
            "{@neonlaw.com : fail}",
            "neonlaw.com"
        ));
        // a pass for a different domain does not authorize the firm domain
        assert!(!dkim_passes_for_domain("{@evil.com : pass}", "neonlaw.com"));
        // empty / absent verdict is untrusted
        assert!(!dkim_passes_for_domain("", "neonlaw.com"));
        assert!(!dkim_passes_for_domain("{}", "neonlaw.com"));
    }

    #[test]
    fn re_subject_prefixes_once() {
        assert_eq!(
            re_subject("Question about my LLC"),
            "Re: Question about my LLC"
        );
        assert_eq!(re_subject("Re: already replied"), "Re: already replied");
    }

    fn cfg() -> ThreadConfig {
        ThreadConfig {
            parse_host: "parse.neonlaw.com".into(),
            lawyer_notify_email: "nick+aida@neonlaw.com".into(),
            verify_dkim_domain: None,
        }
    }

    /// A fixture inbound message. `dkim` defaults to a **pass for the
    /// sender's own domain**, because that is what SendGrid Inbound Parse
    /// actually posts for genuine mail — the field is always populated. A
    /// test that means to model a forgery overrides `msg.dkim` explicitly.
    /// An empty verdict is not a neutral default: the command channel
    /// requires positive proof the `From:` header is authentic, so a blank
    /// field would silently turn every fixture into an unauthenticated
    /// sender.
    fn inbound(from: &str, to: &str, subject: &str, text: &str) -> InboundEmail {
        let sender_domain = super::extract_domain(from);
        InboundEmail {
            from: from.into(),
            to: to.into(),
            subject: subject.into(),
            text: text.into(),
            raw: Vec::new(),
            dkim: format!("{{@{sender_domain} : pass}}"),
            attachments: Vec::new(),
            quarantined_attachments: Vec::new(),
            message_id: None,
        }
    }

    async fn seed_lawyer(surreal: &store::surreal::SurrealDb, email: &str) -> uuid::Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role("Nick", email, store::persons::Role::Admin),
        )
        .await
        .expect("seed lawyer person")
        .id
    }

    /// Seed a client of record so a conversation with this external party
    /// opens past the prospective-client conflict gate: a person, an open
    /// matter, and the client-DRI marker the intake writes once its conflict
    /// check has returned without blocking. A bare `person` row is not
    /// enough and must not be — see
    /// [`tests::a_bare_person_row_does_not_clear_the_relay_gate`].
    async fn seed_client(surreal: &store::surreal::SurrealDb, email: &str) {
        let person_id = seed_orphan_person(surreal, email).await;
        let project_id = seed_open_matter(surreal).await;
        store::projects::designate_dri_in_surreal(
            surreal,
            project_id,
            person_id,
            store::projects::DriSide::Client,
        )
        .await
        .expect("designate client of record");
    }

    /// The residue a refused intake leaves: a `person` row for the address
    /// and nothing that references it. `discard_pending_intake_project`
    /// removes the participations, the notation and the project; the person
    /// and the entity stay.
    async fn seed_orphan_person(surreal: &store::surreal::SurrealDb, email: &str) -> uuid::Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role("Pisces", email, store::persons::Role::Client),
        )
        .await
        .expect("seed orphan person")
        .id
    }

    /// A person on an open matter whose client-DRI marker was never written
    /// — the state `start_post` leaves when the conflict traversal itself
    /// errors, after `add_participation` and before the designation.
    async fn seed_unscreened_participant(surreal: &store::surreal::SurrealDb, email: &str) {
        let person_id = seed_orphan_person(surreal, email).await;
        let project_id = seed_open_matter(surreal).await;
        store::projects::add_participation(surreal, project_id, person_id, "client")
            .await
            .expect("seed participation");
    }

    /// An open Project hung off a throwaway `Human` entity.
    async fn seed_open_matter(surreal: &store::surreal::SurrealDb) -> uuid::Uuid {
        let entity_type_id = store::entity_types::find_or_create(surreal, "Human")
            .await
            .expect("entity type")
            .id;
        let jurisdiction_id = store::jurisdictions::find_or_create(
            surreal,
            &store::jurisdictions::NewJurisdiction::new("United States", "US", "country"),
        )
        .await
        .expect("jurisdiction")
        .id;
        let entity_id = store::entities::create(
            surreal,
            &store::entities::NewEntity {
                name: "Sample Client".into(),
                entity_type_id,
                jurisdiction_id,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .expect("seed entity")
        .id;
        store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: format!("sample-matter-{}", uuid::Uuid::now_v7().simple()),
                name: "Sample matter".into(),
                status: "open".into(),
                entity_id,
                description: None,
            },
        )
        .await
        .expect("seed project")
        .id
    }

    #[tokio::test]
    async fn first_contact_opens_conversation_and_notifies_lawyer() {
        let surreal = mem_surreal().await;
        let cap = CapturingEmail::new();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &RecordingRuntime::default(),
            &cfg(),
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "Question about my LLC",
                "Hi, I have a question.",
            ),
            "inbound/1-pisces.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert_eq!(sent.len(), 1, "one lawyer notification");
        let note = &sent[0];
        assert_eq!(note.to, "nick+aida@neonlaw.com");
        assert_eq!(note.subject, "[Pisces] Question about my LLC");
        let reply_to = note.reply_to.as_deref().expect("reply_to set");
        assert!(reply_to.ends_with("@parse.neonlaw.com"));
        assert!(reply_to.starts_with('c'));
        // never leak an internal address to a header the client would see
        assert!(note.body.contains("Hi, I have a question."));
    }

    #[tokio::test]
    async fn lawyer_reply_relays_to_the_external_party() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let cfg = cfg();
        // A known client — past the prospective-client conflict gate.
        seed_client(&surreal, "pisces@example.com").await;

        // 1. external first contact
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "Question about my LLC",
                "Hi, I have a question.",
            ),
            "inbound/1-pisces.eml",
        )
        .await
        .unwrap();
        let token_addr = cap.captured()[0].reply_to.clone().unwrap();

        // 2. lawyer replies to the token address (with quoted history)
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("\"Support\" <{token_addr}>"),
                "Re: Question about my LLC",
                "Happy to help — here's your answer.\n\nOn Tue wrote:\n> Hi",
            ),
            "inbound/2-nick.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert_eq!(sent.len(), 2, "notification + relay");
        let relay = &sent[1];
        assert_eq!(relay.to, "pisces@example.com", "relayed to the client");
        assert_eq!(relay.body, "Happy to help — here's your answer.");
        assert_eq!(relay.subject, "Re: Question about my LLC");
        // the relay must not expose the attorney's address anywhere
        assert!(!relay.to.contains("nick@"));
        assert_ne!(relay.reply_to.as_deref(), Some("nick@neonlaw.com"));
    }

    #[test]
    fn parse_reply_extracts_commands_and_relay_prose() {
        // @approve alongside prose
        let p = parse_reply("Here you go.\n@approve");
        assert_eq!(p.relay_body, "Here you go.");
        assert_eq!(
            p.commands,
            vec![Command::Signal {
                condition: "approved".into(),
                value: None
            }]
        );
        // @deny with a reason
        let p = parse_reply("@deny missing signature");
        assert_eq!(p.relay_body, "");
        assert_eq!(
            p.commands,
            vec![Command::Signal {
                condition: "rejected".into(),
                value: Some("missing signature".into())
            }]
        );
        // generic @signal <condition> <value>
        let p = parse_reply("@signal filed receipt-123");
        assert_eq!(
            p.commands,
            vec![Command::Signal {
                condition: "filed".into(),
                value: Some("receipt-123".into())
            }]
        );
        // @close is a command; the prose stays in relay_body (the caller
        // suppresses the relay on Close)
        let p = parse_reply("ok\n@close");
        assert_eq!(p.commands, vec![Command::Close]);
        assert_eq!(p.relay_body, "ok");
        // a mid-line @mention is NOT a command — only lines starting with @
        let p = parse_reply("hi @pisces\nthanks");
        assert!(p.commands.is_empty());
        assert_eq!(p.relay_body, "hi @pisces\nthanks");
        // @cleared (and its @clear alias) release the conflict gate
        assert_eq!(
            parse_reply("@cleared").commands,
            vec![Command::ConflictCleared]
        );
        assert_eq!(
            parse_reply("@clear").commands,
            vec![Command::ConflictCleared]
        );
        // @link captures the matter id verbatim; the line never relays
        let p = parse_reply("@link 1e2d3c4b-0000-0000-0000-000000000000\nthanks");
        assert_eq!(
            p.commands,
            vec![Command::Link {
                notation_id: "1e2d3c4b-0000-0000-0000-000000000000".into()
            }]
        );
        assert_eq!(p.relay_body, "thanks");
        // bare @link is still a command (id validated at execution) so it is
        // stripped rather than leaking "@link" to the client
        let p = parse_reply("@link");
        assert_eq!(
            p.commands,
            vec![Command::Link {
                notation_id: String::new()
            }]
        );
        assert_eq!(p.relay_body, "");
    }

    #[tokio::test]
    async fn lawyer_approve_fires_workflow_signal_and_relays_prose() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let lawyer_id = seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        // A conversation already linked to a matter (the lawyer-review gate).
        let token = "0000000000000000000000000000000a";
        store::email_conversations::open(
            &surreal,
            &store::email_conversations::NewConversation {
                token,
                external_email: "pisces@example.com",
                external_name: Some("Pisces"),
                subject: "Estate plan",
                person_id: None,
                notation_id: Some(notation_id),
            },
        )
        .await
        .unwrap();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                "Looks good.\n@approve",
            ),
            "inbound/approve.eml",
        )
        .await
        .unwrap();

        // the workflow signal fired on the linked notation
        let sigs = rt.signals.lock().unwrap();
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0], (notation_id, "approved".to_string(), None));
        assert_eq!(
            rt.contexts.lock().unwrap().as_slice(),
            [workflows::SignalContext {
                acting_person_id: lawyer_id,
            }]
        );
        // and the accompanying prose still relayed to the client
        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, "pisces@example.com");
        assert_eq!(sent[0].body, "Looks good.");
    }

    #[tokio::test]
    async fn lawyer_close_suppresses_relay_and_closes_conversation() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000b";
        store::email_conversations::open(
            &surreal,
            &store::email_conversations::NewConversation {
                token,
                external_email: "pisces@example.com",
                external_name: None,
                subject: "Question",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Question",
                "handled offline\n@close",
            ),
            "inbound/close.eml",
        )
        .await
        .unwrap();

        // @close suppresses the relay entirely
        assert!(cap.captured().is_empty(), "no relay on @close");
        // and the conversation is closed
        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conv.status, "closed");
    }

    /// Resolve a notation's matter (project) id for spine assertions.
    async fn project_of(
        surreal: &store::surreal::SurrealDb,
        notation_id: uuid::Uuid,
    ) -> uuid::Uuid {
        store::notations::find_by_id(surreal, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id
    }

    #[tokio::test]
    async fn email_exchange_mirrors_into_the_linked_matters_spine() {
        use store::communications::{channel, direction, for_project};

        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let project_id = project_of(&surreal, notation_id).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "00000000000000000000000000000abc";
        seed_linked_conversation(&surreal, token, notation_id).await;

        // 1. The client writes in (reply to the token address).
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Pisces <pisces@example.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                "Here is the information you asked for.",
            ),
            "inbound/client-1.eml",
        )
        .await
        .unwrap();

        // 2. Lawyer relays a reply back to the client.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                "Thanks — received, we'll proceed.",
            ),
            "inbound/lawyer-1.eml",
        )
        .await
        .unwrap();

        let thread = for_project(&surreal, project_id).await.unwrap();
        let inbound_rows: Vec<_> = thread
            .iter()
            .filter(|c| c.channel == channel::EMAIL_INBOUND)
            .collect();
        let outbound_rows: Vec<_> = thread
            .iter()
            .filter(|c| c.channel == channel::EMAIL_OUTBOUND)
            .collect();
        assert_eq!(inbound_rows.len(), 1, "client message mirrored inbound");
        assert_eq!(outbound_rows.len(), 1, "firm relay mirrored outbound");
        assert_eq!(
            inbound_rows[0].body,
            "Here is the information you asked for."
        );
        assert_eq!(inbound_rows[0].direction, direction::INBOUND);
        assert_eq!(outbound_rows[0].direction, direction::OUTBOUND);

        // Idempotent: re-mirroring the same conversation adds nothing.
        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        super::sync_conversation_to_spine(
            &super::ThreadCtx {
                surreal: &surreal,
                storage: &storage().await,
                email: &cap,
                runtime: &rt,
                cfg: &cfg(),
            },
            conv.id,
        )
        .await
        .unwrap();
        assert_eq!(
            for_project(&surreal, project_id).await.unwrap().len(),
            thread.len(),
            "re-sync must not duplicate spine rows"
        );
    }

    #[tokio::test]
    async fn first_contact_auto_links_known_client_with_one_open_matter() {
        use store::communications::channel;

        let surreal = mem_surreal().await;
        // seed_notation makes libra@example.com the client on one open matter.
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let project_id = project_of(&surreal, notation_id).await;
        let cap = CapturingEmail::new();

        // First contact (no token) from that known client.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &RecordingRuntime::default(),
            &cfg(),
            &inbound(
                "Libra <libra@example.com>",
                "support@parse.neonlaw.com",
                "A new question",
                "Hello, one more thing.",
            ),
            "inbound/libra-first.eml",
        )
        .await
        .unwrap();

        // The conversation auto-linked to the sole open matter, so the first
        // inbound hop already lands in that matter's conversation log.
        let thread = for_project_helper(&surreal, project_id).await;
        assert_eq!(thread.len(), 1);
        assert_eq!(thread[0].channel, channel::EMAIL_INBOUND);
        assert_eq!(thread[0].body, "Hello, one more thing.");
    }

    async fn for_project_helper(
        surreal: &store::surreal::SurrealDb,
        project_id: uuid::Uuid,
    ) -> Vec<store::communications::Communication> {
        store::communications::for_project(surreal, project_id)
            .await
            .unwrap()
    }

    /// A `ThreadConfig` with the command-channel DKIM gate enabled.
    fn cfg_dkim() -> ThreadConfig {
        ThreadConfig {
            verify_dkim_domain: Some("neonlaw.com".into()),
            ..cfg()
        }
    }

    /// Open a conversation linked to `notation_id` under a fixed token.
    async fn seed_linked_conversation(
        surreal: &store::surreal::SurrealDb,
        token: &str,
        notation_id: uuid::Uuid,
    ) {
        store::email_conversations::open(
            surreal,
            &store::email_conversations::NewConversation {
                token,
                external_email: "pisces@example.com",
                external_name: Some("Pisces"),
                subject: "Estate plan",
                person_id: None,
                notation_id: Some(notation_id),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dkim_failure_blocks_lawyer_command_and_relay_when_enforced() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000c";
        seed_linked_conversation(&surreal, token, notation_id).await;

        // A reply that claims to be from a lawyer but whose DKIM verdict is a
        // fail for the firm domain — the forged-From / leaked-token case.
        let mut msg = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "Looks good.\n@approve",
        );
        msg.dkim = "{@neonlaw.com : fail}".into();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg_dkim(),
            &msg,
            "inbound/forged.eml",
        )
        .await
        .unwrap();

        // The privileged actions are both refused: no workflow signal, no relay.
        assert!(
            rt.signals.lock().unwrap().is_empty(),
            "no workflow signal may fire on a DKIM failure"
        );
        assert!(
            cap.captured().is_empty(),
            "no content may relay on a DKIM failure"
        );
    }

    #[tokio::test]
    async fn dkim_pass_allows_lawyer_command_when_enforced() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000d";
        seed_linked_conversation(&surreal, token, notation_id).await;

        let mut msg = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "Looks good.\n@approve",
        );
        msg.dkim = "{@neonlaw.com : pass}".into();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg_dkim(),
            &msg,
            "inbound/genuine.eml",
        )
        .await
        .unwrap();

        // DKIM passed → the signal fires and the prose relays, exactly as
        // the un-gated path does.
        let sigs = rt.signals.lock().unwrap();
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0], (notation_id, "approved".to_string(), None));
        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, "pisces@example.com");
        assert_eq!(sent[0].body, "Looks good.");
    }

    /// The approval gate must refuse a signature it cannot authenticate.
    ///
    /// `@approve` is an attorney-signature step: from `lawyer_review` it
    /// fires `approved`, which releases the draft into `generate_pdf__*` and
    /// then `sent_for_signature__pending`. Fourteen shipped specs carry that
    /// state, including `onboarding__letter`, `nv__llc_formation`, and
    /// `us__naturalization`.
    ///
    /// The two tests above prove the DKIM fence works *when configured*.
    /// This one pins the posture deployments actually run:
    /// `NAVIGATOR_DKIM_REQUIRE_DOMAIN` is set in no manifest in the tree, so
    /// `verify_dkim_domain` is `None` and the fence is never consulted. In
    /// that posture the only things standing between an outsider and an
    /// attorney signature are a `From:` header - trivially forged - and a
    /// token that travels in cleartext on every notification the cockpit
    /// receives.
    ///
    /// So the invariant is not "DKIM is checked when we asked for it" but
    /// "an unauthenticated sender never signs." A deployment that cannot
    /// verify the signer must refuse the signature, not assume it.
    #[tokio::test]
    async fn approval_gate_refuses_a_signature_it_cannot_authenticate() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000e";
        seed_linked_conversation(&surreal, token, notation_id).await;

        // An outsider with a leaked token, forging the firm's own address.
        // The DKIM verdict is an explicit *fail* for the firm domain: the
        // message carries positive evidence of forgery, not merely an
        // absence of proof.
        let mut msg = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "Looks good.
@approve",
        );
        msg.dkim = "{@neonlaw.com : fail}".into();

        // `cfg()` - not `cfg_dkim()`. This is the shipped configuration.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/forged-unenforced.eml",
        )
        .await
        .unwrap();

        assert!(
            rt.signals.lock().unwrap().is_empty(),
            "an unauthenticated sender signed a legal instrument: the approval \
             gate fired `approved` on a message whose DKIM verdict is a fail \
             for the firm domain, because the deployment sets no \
             NAVIGATOR_DKIM_REQUIRE_DOMAIN"
        );

        // The refusal must actually reach the cockpit — a silently dropped
        // approval is the defect this gate exists to prevent, so proving the
        // signal didn't fire is not enough on its own.
        let sent = cap.captured();
        let refusal = sent
            .iter()
            .find(|m| m.to == cfg().lawyer_notify_email)
            .expect("no refusal notice was sent to the cockpit");
        assert!(refusal.subject.starts_with("[refused]"));
        assert!(
            !refusal.body.contains("  "),
            "refusal body has a formatting defect (a run of spaces): {:?}",
            refusal.body
        );
    }

    /// Open a support conversation with no linked matter — the prod state
    /// before any `@link`, where `@approve` would no-op.
    async fn seed_unlinked_conversation(surreal: &store::surreal::SurrealDb, token: &str) {
        store::email_conversations::open(
            surreal,
            &store::email_conversations::NewConversation {
                token,
                external_email: "pisces@example.com",
                external_name: Some("Pisces"),
                subject: "Estate plan",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn link_binds_conversation_and_same_reply_approve_fires() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000e";
        seed_unlinked_conversation(&surreal, token).await;

        // One lawyer reply links the thread to the matter, then approves on it.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                &format!("@link {notation_id}\n@approve"),
            ),
            "inbound/link.eml",
        )
        .await
        .unwrap();

        // The conversation is bound to the matter...
        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conv.notation_id, Some(notation_id));
        // ...and the same-message @approve fired on it (the freshly linked
        // notation is visible to the later command in the same reply).
        let sigs = rt.signals.lock().unwrap();
        assert_eq!(sigs.len(), 1);
        assert_eq!(sigs[0], (notation_id, "approved".to_string(), None));
        // Nothing relayed to the client — both lines were commands; the only
        // outbound is the cockpit-facing [link] confirmation.
        assert!(
            cap.captured().iter().all(|m| m.to != "pisces@example.com"),
            "@link/@approve must not relay to the client"
        );
    }

    #[tokio::test]
    async fn link_to_unknown_matter_leaves_conversation_unlinked() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000f";
        seed_unlinked_conversation(&surreal, token).await;

        let bogus = "11111111-2222-3333-4444-555555555555";
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                &format!("@link {bogus}\n@approve"),
            ),
            "inbound/link-bad.eml",
        )
        .await
        .unwrap();

        // An id with no matter behind it links nothing and fires nothing.
        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conv.notation_id, None);
        assert!(rt.signals.lock().unwrap().is_empty());
    }

    /// A forged reply carrying `@close`. #268 gated only `Command::Signal`,
    /// so this verb ran on a `From:` header plus a bearer token that every
    /// outbound hop stamps as `Reply-To`. Closing a live client thread drops
    /// it out of the firm's queue - a duty-of-communication risk - and
    /// `by_token` never filters on status, so nothing is locked out in
    /// exchange.
    ///
    /// Asserting the command did not run is not enough: a refusal the cockpit
    /// never sees is the same defect from the attorney's side. So this pins
    /// the notice's recipient, subject prefix, and body.
    #[tokio::test]
    async fn forged_close_does_not_close_and_tells_the_cockpit() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "00000000000000000000000000000010";
        seed_unlinked_conversation(&surreal, token).await;

        let mut msg = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "@close",
        );
        msg.dkim = "{@neonlaw.com : fail}".into();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/forged-close.eml",
        )
        .await
        .unwrap();

        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(
            conv.status,
            store::email_conversations::STATUS_CLOSED,
            "a forged @close closed a live client thread"
        );
        // Held, not dropped: the thread is parked for a human rather than
        // left looking untouched.
        assert_eq!(
            conv.status,
            store::email_conversations::STATUS_AWAITING_LAWYER
        );

        let sent = cap.captured();
        let notice = sent
            .iter()
            .find(|m| m.to == cfg().lawyer_notify_email)
            .expect("no refusal notice reached the cockpit");
        assert!(
            notice.subject.starts_with("[refused]"),
            "unexpected subject: {:?}",
            notice.subject
        );
        assert!(
            notice.body.contains("NOT acted on") && notice.body.contains("NOT relayed"),
            "notice does not say what was refused: {:?}",
            notice.body
        );
        assert!(
            notice
                .body
                .contains("could not be cryptographically authenticated"),
            "notice does not say why: {:?}",
            notice.body
        );
        assert!(
            !notice.body.contains("  "),
            "notice body has a formatting defect (a run of spaces): {:?}",
            notice.body
        );
        // Nothing reached the client.
        assert!(sent.iter().all(|m| m.to != "pisces@example.com"));
    }

    /// A forged `@cleared` must not release the RPC 1.10 firm-wide
    /// imputed-conflicts hold. The refusal journals the hop WITHOUT its
    /// `command_payload`, because `is_conflict_cleared` reads that field back
    /// off the transcript - recording the directives of a refused message
    /// would let the forgery clear the gate from inside the record of its own
    /// refusal. Proven end-to-end: a following genuine relay is still held.
    #[tokio::test]
    async fn forged_cleared_does_not_release_the_conflict_hold() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "00000000000000000000000000000011";
        seed_unlinked_conversation(&surreal, token).await;
        let store_svc = storage().await;

        let mut forged = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "@cleared",
        );
        forged.dkim = "{@neonlaw.com : fail}".into();
        thread_inbound(
            &surreal,
            &store_svc,
            &cap,
            &rt,
            &cfg(),
            &forged,
            "inbound/forged-cleared.eml",
        )
        .await
        .unwrap();

        let conv_id = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap()
            .id;
        assert!(
            !super::is_conflict_cleared(&surreal, conv_id).await.unwrap(),
            "a forged @cleared released the firm-wide conflicts hold"
        );

        // A genuine reply now tries to relay substantive prose. The
        // conversation is an unscreened, unlinked prospective client, so the
        // hold must still be in force.
        thread_inbound(
            &surreal,
            &store_svc,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                "Here is our advice on your estate plan.",
            ),
            "inbound/genuine-after-forgery.eml",
        )
        .await
        .unwrap();

        assert!(
            cap.captured().iter().all(|m| m.to != "pisces@example.com"),
            "relay reached a prospective client whose conflict check was never cleared"
        );
        assert!(
            cap.captured()
                .iter()
                .any(|m| m.subject.starts_with("[conflict check]")),
            "the relay was dropped rather than held for the conflict check"
        );
    }

    /// Forged prose carrying no command at all - the relay path on its own.
    /// This is the hop that impersonates an attorney to the client over the
    /// firm's aligned outbound lane AND fabricates the firm's own record of
    /// what it advised, because `sync_conversation_to_spine` mirrors
    /// `to_external` into the matter's `communications` spine as an outbound
    /// firm communication. Both halves are asserted.
    #[tokio::test]
    async fn forged_prose_does_not_relay_or_enter_the_matter_spine() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let project_id = store::notations::find_by_id(&surreal, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        // Linked, so the conflict gate is already satisfied and the ONLY
        // thing that can stop this relay is the authentication gate.
        let token = "00000000000000000000000000000012";
        seed_linked_conversation(&surreal, token, notation_id).await;

        let mut msg = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "You should sign the settlement offer today.",
        );
        msg.dkim = "{@neonlaw.com : fail}".into();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/forged-prose.eml",
        )
        .await
        .unwrap();

        assert!(
            cap.captured().iter().all(|m| m.to != "pisces@example.com"),
            "attacker prose was relayed to the client as the firm"
        );

        let conv_id = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap()
            .id;
        let hops = store::email_conversations::messages(&surreal, conv_id)
            .await
            .unwrap();
        assert!(
            hops.iter().all(|m| m.direction != "to_external"),
            "a to_external hop was journaled for a message that never relayed"
        );

        let spine = store::communications::for_project(&surreal, project_id)
            .await
            .unwrap();
        assert!(
            !spine
                .iter()
                .any(|c| c.body.contains("sign the settlement offer")),
            "attacker prose entered the matter's communications spine as a firm communication"
        );
    }

    /// A forged `@link` is the sharpest verb of the five, not the mildest:
    /// `link_notation` accepts any existing notation id with no participation
    /// check, so binding one cross-links a stranger's thread onto a real
    /// client's matter, files their attachments into it, and - because
    /// `is_prospect` reads `notation_id.is_none()` - lifts the conflicts hold
    /// without ever sending `@cleared`.
    #[tokio::test]
    async fn forged_link_leaves_the_conversation_unbound() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "00000000000000000000000000000013";
        seed_unlinked_conversation(&surreal, token).await;

        // A real, resolvable notation id - the id is not the secret.
        let mut msg = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            &format!("@link {notation_id}"),
        );
        msg.dkim = "{@neonlaw.com : fail}".into();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/forged-link.eml",
        )
        .await
        .unwrap();

        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            conv.notation_id, None,
            "a forged @link bound an outsider's thread to a client's matter"
        );
    }

    /// The regression guard for the gate itself: a genuine reply on the
    /// shipped `cfg()` posture still links, clears, relays, and closes.
    ///
    /// This is the risk the council named - that a blanket gate turns the
    /// whole lawyer lane off rather than just the command channel. The
    /// fixture's DKIM verdict is what SendGrid Inbound Parse posts for
    /// genuine mail, and on it every verb still works.
    #[tokio::test]
    async fn a_genuine_reply_still_links_clears_relays_and_closes() {
        let surreal = mem_surreal().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let store_svc = storage().await;

        let token = "00000000000000000000000000000014";
        seed_unlinked_conversation(&surreal, token).await;

        // One genuine reply: link the matter, clear the conflict hold, and
        // relay the prose that follows the command lines.
        let body = format!("@link {notation_id}\n@cleared\n\nWe have your estate plan in hand.");
        thread_inbound(
            &surreal,
            &store_svc,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                &body,
            ),
            "inbound/genuine.eml",
        )
        .await
        .unwrap();

        let conv = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conv.notation_id, Some(notation_id), "@link did not bind");
        assert!(
            super::is_conflict_cleared(&surreal, conv.id).await.unwrap(),
            "@cleared did not register"
        );
        let relayed = cap
            .captured()
            .into_iter()
            .find(|m| m.to == "pisces@example.com")
            .expect("a genuine reply did not relay to the client");
        assert!(relayed.body.contains("We have your estate plan in hand."));

        // ...and a second genuine reply still closes the thread.
        thread_inbound(
            &surreal,
            &store_svc,
            &cap,
            &rt,
            &cfg(),
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("c{token}@parse.neonlaw.com"),
                "Re: Estate plan",
                "@close",
            ),
            "inbound/genuine-close.eml",
        )
        .await
        .unwrap();
        assert_eq!(
            store::email_conversations::by_token(&surreal, token)
                .await
                .unwrap()
                .unwrap()
                .status,
            store::email_conversations::STATUS_CLOSED,
            "@close did not close on a genuine reply"
        );
    }

    #[tokio::test]
    async fn first_contact_threads_notification_to_client_message_id() {
        let surreal = mem_surreal().await;
        let cap = CapturingEmail::new();

        let mut msg = inbound(
            "Pisces <pisces@example.com>",
            "test@parse.neonlaw.com",
            "Question about my LLC",
            "Hi, I have a question.",
        );
        msg.message_id = Some("client-1@mail.example.com".into());

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &RecordingRuntime::default(),
            &cfg(),
            &msg,
            "inbound/1-pisces.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        // The lawyer notification references the client's message so the
        // attorney's mail client threads the exchange.
        assert_eq!(
            sent[0].in_reply_to.as_deref(),
            Some("<client-1@mail.example.com>")
        );
        assert_eq!(
            sent[0].references.as_deref(),
            Some("<client-1@mail.example.com>")
        );
    }

    #[tokio::test]
    async fn lawyer_relay_threads_with_the_full_message_id_chain() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let cfg = cfg();
        // A known client — past the prospective-client conflict gate.
        seed_client(&surreal, "pisces@example.com").await;

        // 1. Client first contact carries a message-id.
        let mut first = inbound(
            "Pisces <pisces@example.com>",
            "test@parse.neonlaw.com",
            "Question about my LLC",
            "Hi.",
        );
        first.message_id = Some("client-1@mail".into());
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &first,
            "inbound/1.eml",
        )
        .await
        .unwrap();
        let token_addr = cap.captured()[0].reply_to.clone().unwrap();

        // 2. Lawyer reply (its own message-id) relays back to the client.
        let mut reply = inbound(
            "Nick <nick@neonlaw.com>",
            &format!("\"Support\" <{token_addr}>"),
            "Re: Question about my LLC",
            "Here's your answer.",
        );
        reply.message_id = Some("lawyer-1@mail".into());
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &reply,
            "inbound/2.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        let relay = &sent[1];
        assert_eq!(relay.to, "pisces@example.com");
        // References carries the whole inbound chain, oldest first;
        // In-Reply-To points at the most recent inbound hop (the lawyer reply).
        assert_eq!(
            relay.references.as_deref(),
            Some("<client-1@mail> <lawyer-1@mail>")
        );
        assert_eq!(relay.in_reply_to.as_deref(), Some("<lawyer-1@mail>"));
    }

    #[tokio::test]
    async fn first_contact_from_unknown_prompts_conflict_check() {
        let surreal = mem_surreal().await;
        let cap = CapturingEmail::new();
        // pisces is NOT seeded → a prospective client.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &RecordingRuntime::default(),
            &cfg(),
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "New matter",
                "Can you help?",
            ),
            "inbound/prospect.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        // The notification prompts lawyer to run the firm-wide conflict check.
        assert!(sent[0].body.contains("Prospective client"));
        assert!(sent[0].body.contains("conflict check"));
        assert!(sent[0].body.contains("@cleared"));
    }

    #[tokio::test]
    async fn relay_to_uncleared_prospect_is_held() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let cfg = cfg();

        // Prospect first contact (pisces unseeded, no linked matter).
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "New matter",
                "Help?",
            ),
            "inbound/1.eml",
        )
        .await
        .unwrap();
        let token_addr = cap.captured()[0].reply_to.clone().unwrap();

        // Lawyer replies with prose but NO @cleared → the relay is held.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("\"Support\" <{token_addr}>"),
                "Re: New matter",
                "Sure, happy to help.",
            ),
            "inbound/2.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        // Nothing reaches the prospect...
        assert!(
            sent.iter().all(|m| m.to != "pisces@example.com"),
            "no relay to an uncleared prospect"
        );
        // ...and lawyers are prompted to run the conflict check.
        assert!(
            sent.iter()
                .any(|m| m.to == "nick+aida@neonlaw.com" && m.body.contains("NOT relayed")),
            "lawyer prompted to run the conflict check before the relay"
        );
    }

    /// A refused intake leaves a bare `person` row behind — the project,
    /// the participations and the notation are undone by
    /// `retainer_walk::discard_pending_intake_project`, the person is not.
    /// That row must not read as evidence that anyone screened them: the
    /// firm's own act of refusing someone cannot be what marks them
    /// cleared. Nobody has run a conflict check here, so the relay holds.
    #[tokio::test]
    async fn a_bare_person_row_does_not_clear_the_relay_gate() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        // Exactly what a refused intake leaves: a person, and nothing else.
        seed_orphan_person(&surreal, "pisces@example.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let cfg = cfg();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "New matter",
                "Help?",
            ),
            "inbound/1.eml",
        )
        .await
        .unwrap();
        let token_addr = cap.captured()[0].reply_to.clone().unwrap();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("\"Support\" <{token_addr}>"),
                "Re: New matter",
                "Sure, happy to help.",
            ),
            "inbound/2.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert!(
            sent.iter().all(|m| m.to != "pisces@example.com"),
            "a refused intake's leftover person row must not release the relay"
        );
        assert!(
            sent.iter()
                .any(|m| m.to == "nick+aida@neonlaw.com" && m.body.contains("NOT relayed")),
            "lawyer still prompted to run the conflict check"
        );
    }

    /// The second path to the same residue: `start_post` runs the conflict
    /// traversal before it designates the client DRI, so a store failure in
    /// the traversal returns with the participation row already written and
    /// no DRI marker on it. A participation alone is not a screen either —
    /// the DRI designation is the write ordered *after* the check.
    #[tokio::test]
    async fn a_participation_without_the_client_dri_marker_does_not_clear_the_gate() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        seed_unscreened_participant(&surreal, "pisces@example.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let cfg = cfg();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "New matter",
                "Help?",
            ),
            "inbound/1.eml",
        )
        .await
        .unwrap();
        let token_addr = cap.captured()[0].reply_to.clone().unwrap();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Nick <nick@neonlaw.com>",
                &format!("\"Support\" <{token_addr}>"),
                "Re: New matter",
                "Sure, happy to help.",
            ),
            "inbound/2.eml",
        )
        .await
        .unwrap();

        assert!(
            cap.captured().iter().all(|m| m.to != "pisces@example.com"),
            "an un-designated participation must not release the relay"
        );
    }

    #[tokio::test]
    async fn cleared_releases_the_relay_to_a_prospect() {
        let surreal = mem_surreal().await;
        seed_lawyer(&surreal, "nick@neonlaw.com").await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let cfg = cfg();

        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Pisces <pisces@example.com>",
                "test@parse.neonlaw.com",
                "New matter",
                "Help?",
            ),
            "inbound/1.eml",
        )
        .await
        .unwrap();
        let token_addr = cap.captured()[0].reply_to.clone().unwrap();
        let lawyer_to = format!("\"Support\" <{token_addr}>");

        // Lawyer clears the check and answers in one reply.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Nick <nick@neonlaw.com>",
                &lawyer_to,
                "Re: New matter",
                "@cleared\nHappy to help.",
            ),
            "inbound/2.eml",
        )
        .await
        .unwrap();

        let relay = cap
            .captured()
            .into_iter()
            .find(|m| m.to == "pisces@example.com")
            .expect("relay released after @cleared");
        assert_eq!(relay.body, "Happy to help.");

        // Clearance persists: a later plain reply relays without re-prompting.
        thread_inbound(
            &surreal,
            &storage().await,
            &cap,
            &rt,
            &cfg,
            &inbound(
                "Nick <nick@neonlaw.com>",
                &lawyer_to,
                "Re: New matter",
                "Just following up.",
            ),
            "inbound/3.eml",
        )
        .await
        .unwrap();
        assert_eq!(
            cap.captured()
                .iter()
                .filter(|m| m.to == "pisces@example.com")
                .count(),
            2,
            "clearance persists for subsequent relays"
        );
    }

    #[tokio::test]
    async fn clean_first_contact_forwards_attachment_to_lawyer() {
        let surreal = mem_surreal().await;
        let storage = storage().await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let mut msg = inbound(
            "Pisces <pisces@example.com>",
            "support@neonlaw.com",
            "Help",
            "My intake is attached.",
        );
        msg.attachments = vec![InboundAttachment {
            filename: "intake.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF clean".to_vec(),
        }];

        thread_inbound(
            &surreal,
            &storage,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/first-contact.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].attachments.len(), 1);
        assert_eq!(sent[0].attachments[0].filename, "intake.pdf");
        assert_eq!(sent[0].attachments[0].bytes, b"%PDF clean");
    }

    #[tokio::test]
    async fn quarantined_first_contact_sends_text_only_notice() {
        use crate::inbound_email::QuarantinedAttachment;

        let surreal = mem_surreal().await;
        let storage = storage().await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();
        let mut msg = inbound(
            "Pisces <pisces@example.com>",
            "support@neonlaw.com",
            "Help",
            "A test fixture is attached.",
        );
        msg.quarantined_attachments = vec![QuarantinedAttachment {
            filename: "eicar.txt".into(),
            signature: "Eicar-Signature".into(),
        }];

        thread_inbound(
            &surreal,
            &storage,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/quarantined.eml",
        )
        .await
        .unwrap();

        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].attachments.is_empty());
        assert!(sent[0].body.contains("quarantined"));
        assert!(sent[0].body.contains("eicar.txt"));
    }

    #[tokio::test]
    async fn attachment_on_linked_thread_files_a_document_and_notifies() {
        let surreal = mem_surreal().await;
        let storage = storage().await;
        let notation_id = store::test_support::seed_notation(&surreal).await;
        let project_id = store::notations::find_by_id(&surreal, notation_id)
            .await
            .unwrap()
            .unwrap()
            .project_id;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        let token = "0000000000000000000000000000000e";
        seed_linked_conversation(&surreal, token, notation_id).await;

        // The client replies on the matter thread with a PDF attached.
        let mut msg = inbound(
            "Pisces <pisces@example.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Estate plan",
            "Here is my signed form.",
        );
        msg.attachments = vec![InboundAttachment {
            filename: "signed-form.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF-1.4 fake".to_vec(),
        }];

        thread_inbound(
            &surreal,
            &storage,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/form.eml",
        )
        .await
        .unwrap();

        // The attachment is filed as a document asset on the matter's project.
        let docs = store::assets::for_project(&surreal, project_id)
            .await
            .unwrap();
        assert_eq!(docs.len(), 1, "one document filed");
        assert_eq!(docs[0].filename.as_deref(), Some("signed-form.pdf"));
        assert_eq!(docs[0].source.as_deref(), Some("email"));
        assert_eq!(docs[0].kind.as_deref(), Some("unclassified"));

        // The transcript records the ingest as a `system` hop.
        let convo = store::email_conversations::by_token(&surreal, token)
            .await
            .unwrap()
            .unwrap();
        let msgs = store::email_conversations::messages(&surreal, convo.id)
            .await
            .unwrap();
        assert!(
            msgs.iter()
                .any(|m| m.direction == "system" && m.body_text.contains("signed-form.pdf")),
            "a system hop records the filed document"
        );

        // Lawyers are notified with the review request folded into the body.
        let sent = cap.captured();
        assert_eq!(sent.len(), 1, "one lawyer notification");
        assert!(sent[0].body.contains("document(s) received for review"));
        assert!(sent[0].body.contains("signed-form.pdf"));
        assert_eq!(sent[0].attachments.len(), 1);
        assert_eq!(sent[0].attachments[0].filename, "signed-form.pdf");
        assert_eq!(sent[0].attachments[0].bytes, b"%PDF-1.4 fake");
    }

    #[tokio::test]
    async fn attachment_on_unlinked_thread_is_archived_not_filed() {
        let surreal = mem_surreal().await;
        let storage = storage().await;
        let cap = CapturingEmail::new();
        let rt = RecordingRuntime::default();

        // A conversation with NO linked matter — nothing to file documents under.
        let token = "0000000000000000000000000000000f";
        store::email_conversations::open(
            &surreal,
            &store::email_conversations::NewConversation {
                token,
                external_email: "pisces@example.com",
                external_name: Some("Pisces"),
                subject: "Question",
                person_id: None,
                notation_id: None,
            },
        )
        .await
        .unwrap();

        let mut msg = inbound(
            "Pisces <pisces@example.com>",
            &format!("c{token}@parse.neonlaw.com"),
            "Re: Question",
            "A document for you.",
        );
        msg.attachments = vec![InboundAttachment {
            filename: "doc.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF bytes".to_vec(),
        }];

        thread_inbound(
            &surreal,
            &storage,
            &cap,
            &rt,
            &cfg(),
            &msg,
            "inbound/unlinked.eml",
        )
        .await
        .unwrap();

        // No document is filed...
        let docs = store::assets::list_all(&surreal).await.unwrap();
        assert!(
            docs.iter().all(|d| d.project_id.is_none()),
            "no document filed on a matter without a linked one"
        );
        // ...but lawyers are told it arrived and how to file it.
        let sent = cap.captured();
        assert_eq!(sent.len(), 1);
        assert!(sent[0]
            .body
            .contains("link this thread to a matter to file them"));
        assert_eq!(sent[0].attachments.len(), 1);
        assert_eq!(sent[0].attachments[0].filename, "doc.pdf");
    }
}
