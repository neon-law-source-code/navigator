//! SendGrid Inbound Parse webhook handler.
//!
//! SendGrid POSTs `multipart/form-data` to a configured URL whenever
//! mail lands at our MX-pointed domain. The standard fields are:
//!
//! - `from`, `to`, `subject` — required for any useful message.
//! - `text`, `html` — body parts.
//! - `email` — the raw RFC 5322 message (we store this verbatim).
//! - `attachments` — count of attachment parts (ignored for v1).
//!
//! We persist the raw message to object storage (filesystem in dev,
//! GCS in prod via the [`cloud::StorageService`] trait) and insert a
//! `letters` row so the admin UI can surface it.
//!
//! Auth: the endpoint sits at `/webhook/sendgrid/inbound/:secret` and
//! the path segment is compared (constant-time) against
//! `AppState::inbound_email_secret`, loaded from
//! `SENDGRID_INBOUND_SECRET`. In dev/tests the configured secret is
//! `None` and any token is accepted; in production the deploy
//! invariant requires the env var, so a missing secret crashes the
//! binary at boot rather than silently letting the world POST mail
//! at us. HMAC verification of SendGrid's signed-event header is the
//! natural next layer once Twilio finalizes the signing format.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use cloud::StorageService;

use crate::attachment_scanner::{AttachmentScanner, ScanError, ScanVerdict};
use crate::audit_fields::domain_of;

/// SendGrid accepts complete inbound messages up to 30 MB. This is also the
/// scanner's per-attachment ceiling, and the router applies it to the full
/// multipart request before Axum's smaller default limit can reject it.
pub const MAX_INBOUND_MESSAGE_BYTES: usize = crate::attachment_scanner::DEFAULT_MAX_STREAM_BYTES;

/// One attachment carried on an inbound message. SendGrid Inbound Parse
/// splits each MIME attachment into its own multipart field
/// (`attachment1`, `attachment2`, …), with the original filename and
/// content type carried on the part itself — so we read them straight
/// off the field rather than parsing the `attachment-info` JSON.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InboundAttachment {
    /// Original filename from the part's `Content-Disposition`.
    pub filename: String,
    /// MIME type from the part's `Content-Type`.
    pub content_type: String,
    /// Raw attachment bytes.
    pub bytes: Vec<u8>,
}

/// Parsed multipart payload — the subset of fields we actually use.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InboundEmail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub text: String,
    pub raw: Vec<u8>,
    /// SendGrid Inbound Parse's DKIM verdict, e.g. `{@neonlaw.com : pass}`.
    /// SendGrid validates the message's DKIM signature against the sending
    /// domain's published key and reports the result here. The threading
    /// layer uses it to authenticate the privileged command channel — a
    /// lawyer `@approve` is trusted only when DKIM passes for the firm
    /// domain (see `email_threads`). Empty when SendGrid omits the field.
    pub dkim: String,
    /// Attachments split out by SendGrid as `attachment1`, `attachment2`, …
    /// The threading layer files these into the `documents` lane when the
    /// conversation is linked to a matter (see `email_threads`).
    pub attachments: Vec<InboundAttachment>,
    /// Safe metadata for attachments quarantined by the scanner. Their bytes
    /// remain only in the archived raw `.eml`.
    pub quarantined_attachments: Vec<QuarantinedAttachment>,
    /// The message's RFC 5322 `Message-ID` (without angle brackets),
    /// parsed from the raw MIME. Recorded as the conversation hop's
    /// `provider_message_id` and chained into outbound `References` /
    /// `In-Reply-To` so the attorney's mail client threads the exchange.
    /// `None` when the raw bytes carry no parseable id.
    pub message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinedAttachment {
    pub filename: String,
    pub signature: String,
}

/// Extract the RFC 5322 `Message-ID` (without angle brackets) from raw
/// MIME bytes, or `None` when absent/unparseable.
#[must_use]
pub fn message_id_from_raw(raw: &[u8]) -> Option<String> {
    mail_parser::MessageParser::default()
        .parse(raw)
        .and_then(|m| m.message_id().map(str::to_string))
}

/// True for a SendGrid attachment-part field (`attachment1`, `attachment2`,
/// …) — i.e. `attachment` followed by a positive integer. Excludes the
/// `attachments` count field and the `attachment-info` JSON metadata field.
#[must_use]
pub fn is_attachment_field(name: &str) -> bool {
    name.strip_prefix("attachment")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Reasons the webhook cannot proceed.
#[derive(Debug, thiserror::Error)]
pub enum InboundError {
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("malformed multipart payload: {0}")]
    Multipart(String),
    #[error("no mailroom configured to route inbound mail through")]
    NoMailroom,
    #[error("storage write failed: {0}")]
    Storage(String),
    #[error("database write failed: {0}")]
    Database(String),
    #[error("unauthorized: webhook secret mismatch")]
    Unauthorized,
    #[error("attachment scanner failed: {0}")]
    Scanner(#[from] ScanError),
}

impl IntoResponse for InboundError {
    fn into_response(self) -> axum::response::Response {
        let code = match &self {
            Self::MissingField(_) | Self::Multipart(_) => StatusCode::BAD_REQUEST,
            Self::NoMailroom | Self::Scanner(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Storage(_) | Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
        };
        (code, self.to_string()).into_response()
    }
}

/// Scan each attachment once, retaining bytes only for clean results.
pub async fn scan_attachments(
    scanner: &dyn AttachmentScanner,
    email: &mut InboundEmail,
) -> Result<(), InboundError> {
    let attachments = std::mem::take(&mut email.attachments);
    for attachment in attachments {
        match scanner.scan(&attachment.bytes).await? {
            ScanVerdict::Clean => email.attachments.push(attachment),
            ScanVerdict::Found { signature } => {
                tracing::warn!(
                    signature,
                    "inbound attachment quarantined after malware scan"
                );
                email.quarantined_attachments.push(QuarantinedAttachment {
                    filename: non_empty_metadata(&attachment.filename, "attachment"),
                    signature,
                });
            }
        }
    }
    Ok(())
}

fn non_empty_metadata(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

/// Constant-time string comparison. Leaks length (acceptable here —
/// the secret length isn't sensitive), but XORs every byte so a
/// timing attack cannot probe the secret one character at a time.
#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Pull every relevant field off the multipart body and assemble an
/// [`InboundEmail`]. Required fields (`from`, `to`, `subject`)
/// produce [`InboundError::MissingField`] on absence; unrecognized
/// fields are silently dropped.
#[allow(clippy::too_many_lines)] // multipart normalization covers both SendGrid modes
pub async fn parse_multipart(mut form: Multipart) -> Result<InboundEmail, InboundError> {
    let mut out = InboundEmail::default();
    let mut saw_from = false;
    let mut saw_to = false;
    let mut saw_subject = false;
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| InboundError::Multipart(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        // `email` carries the raw RFC 5322 bytes — keep as bytes so
        // we don't UTF-8-mangle a binary attachment. Everything else
        // is text we read as a String.
        if name == "email" {
            out.raw = field
                .bytes()
                .await
                .map_err(|e| InboundError::Multipart(e.to_string()))?
                .to_vec();
            continue;
        }
        // Attachment parts carry their filename + content type on the part;
        // capture those (owned) before consuming the field for its bytes.
        if is_attachment_field(&name) {
            let filename = non_empty_metadata(field.file_name().unwrap_or_default(), "attachment");
            let content_type = non_empty_metadata(
                field.content_type().unwrap_or_default(),
                "application/octet-stream",
            );
            let bytes = field
                .bytes()
                .await
                .map_err(|e| InboundError::Multipart(e.to_string()))?
                .to_vec();
            out.attachments.push(InboundAttachment {
                filename,
                content_type,
                bytes,
            });
            continue;
        }
        let value = field
            .text()
            .await
            .map_err(|e| InboundError::Multipart(e.to_string()))?;
        match name.as_str() {
            "from" => {
                out.from = value;
                saw_from = true;
            }
            "to" => {
                out.to = value;
                saw_to = true;
            }
            "subject" => {
                out.subject = value;
                saw_subject = true;
            }
            "text" => out.text = value,
            "dkim" => out.dkim = value,
            _ => {}
        }
    }
    // In SendGrid raw mode the outer multipart can contain only `email`.
    // Recover the envelope fields from the RFC 5322 headers before applying
    // the required-field contract, so raw-mode messages are normalized just
    // like the default parsed-field mode.
    if (!saw_from || !saw_to || !saw_subject || out.text.is_empty()) && !out.raw.is_empty() {
        if let Some(message) = mail_parser::MessageParser::default().parse(&out.raw) {
            if !saw_from {
                out.from = message
                    .from()
                    .and_then(|addresses| addresses.first())
                    .map(format_mailbox)
                    .unwrap_or_default();
                saw_from = !out.from.is_empty();
            }
            if !saw_to {
                out.to = message
                    .to()
                    .and_then(|addresses| addresses.first())
                    .map(format_mailbox)
                    .unwrap_or_default();
                saw_to = !out.to.is_empty();
            }
            if !saw_subject {
                out.subject = message.subject().unwrap_or_default().to_owned();
                saw_subject = !out.subject.is_empty();
            }
            if out.text.is_empty() {
                out.text = message
                    .body_text(0)
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
            }
        }
    }
    if !saw_from {
        return Err(InboundError::MissingField("from"));
    }
    if !saw_to {
        return Err(InboundError::MissingField("to"));
    }
    if !saw_subject {
        return Err(InboundError::MissingField("subject"));
    }
    // `send_raw=true` nests attachments inside the raw RFC 5322 message.
    // Use that only as a fallback so providers that supply both shapes do
    // not duplicate attachments.
    if out.attachments.is_empty() {
        out.attachments = attachments_from_raw(&out.raw);
    }
    out.message_id = message_id_from_raw(&out.raw);
    Ok(out)
}

fn format_mailbox(address: &mail_parser::Addr<'_>) -> String {
    match (address.name(), address.address()) {
        (Some(name), Some(address)) if !name.is_empty() => format!("{name} <{address}>"),
        (_, Some(address)) => address.to_owned(),
        _ => String::new(),
    }
}

/// Recover attachments from SendGrid raw mode.
#[must_use]
pub fn attachments_from_raw(raw: &[u8]) -> Vec<InboundAttachment> {
    use mail_parser::MimeHeaders as _;

    let Some(message) = mail_parser::MessageParser::default().parse(raw) else {
        return Vec::new();
    };
    message
        .attachments()
        .map(|part| {
            let content_type = part.content_type().map_or_else(
                || "application/octet-stream".to_owned(),
                |mime| {
                    format!(
                        "{}/{}",
                        mime.c_type,
                        mime.c_subtype.as_deref().unwrap_or("octet-stream")
                    )
                },
            );
            InboundAttachment {
                filename: non_empty_metadata(
                    part.attachment_name().unwrap_or_default(),
                    "attachment",
                ),
                content_type: non_empty_metadata(&content_type, "application/octet-stream"),
                bytes: part.contents().to_vec(),
            }
        })
        .collect()
}

/// Storage key for a freshly received inbound message. Includes a
/// unix-epoch millisecond stamp so collisions between concurrent
/// deliveries are vanishingly rare without needing a UUID dep.
#[must_use]
pub fn storage_key_for(email: &InboundEmail) -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    // Stable, lowercase, alphanumeric-ish slug from the sender so
    // the object lists are scannable by domain.
    let sender_slug: String = email
        .from
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(40)
        .collect();
    format!("inbound/{now_ms}-{sender_slug}.eml")
}

/// Persist the inbound message: raw bytes to storage, summary row
/// to the `letters` table under the first mailroom available
/// (caller-configurable routing belongs to a future change). Returns
/// the object-storage key of the archived `.eml` so the threading layer
/// can reference it from the conversation transcript.
pub async fn persist(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn StorageService>,
    email: &InboundEmail,
) -> Result<String, InboundError> {
    let key = storage_key_for(email);
    storage
        .put(&key, &email.raw, "message/rfc822")
        .await
        .map_err(|e| InboundError::Storage(e.to_string()))?;

    // Route via the first registered mailroom. We'll grow a
    // per-recipient routing layer when the second mailroom appears.
    let mailrooms = store::mailrooms::list_all(surreal)
        .await
        .map_err(|e| InboundError::Database(e.to_string()))?;
    let mailroom_id = mailrooms
        .first()
        .map(|m| m.id)
        .ok_or(InboundError::NoMailroom)?;

    store::letters::record(
        surreal,
        &store::letters::NewLetter {
            mailroom_id,
            // Inbound mail lands unclassified — no matter is known at this
            // hop, so nothing here could name one without guessing (ENG-310).
            project_id: None,
            direction: store::letters::DIRECTION_INCOMING.to_string(),
            sender: email.from.clone(),
            recipient: email.to.clone(),
            summary: email.subject.clone(),
        },
    )
    .await
    .map_err(|e| InboundError::Database(e.to_string()))?;
    Ok(key)
}

/// Webhook handler — verifies the path-embedded secret, parses the
/// multipart, persists, returns 200.
///
/// If `AppState::inbound_email_secret` is `Some`, the path token
/// must match exactly (constant-time). If `None` (dev/test default),
/// the token is accepted unconditionally — production runs gate this
/// via `enforce_deployment_invariants` so a missing env var crashes at
/// boot rather than letting the world POST mail at the pod.
pub async fn webhook(
    State(state): State<crate::AppState>,
    Path(provided): Path<String>,
    form: Multipart,
) -> Result<StatusCode, InboundError> {
    if let Some(configured) = state.inbound_email_secret.as_deref() {
        if !constant_time_eq(&provided, configured) {
            tracing::warn!("inbound webhook: secret mismatch");
            return Err(InboundError::Unauthorized);
        }
    }
    let mut email = parse_multipart(form).await?;
    // Any scanner error returns 503 before storage, conversation,
    // notification, or document side effects.
    scan_attachments(state.attachment_scanner.as_ref(), &mut email).await?;
    // Surface SendGrid's DKIM verdict on every inbound message — domains and
    // counts only, never an address, a subject, or a body. This is the signal
    // the operator watches before flipping NAVIGATOR_DKIM_REQUIRE_DOMAIN on:
    // confirm real support mail arrives as `{@neonlaw.com : pass}` while the
    // command-channel gate is still trust-on-token, then enforce. The sending
    // domain is the half that verdict is about, so dropping the local part
    // costs the operator nothing and keeps a correspondent's identity out of
    // telemetry.
    tracing::info!(
        from_domain = %domain_of(&email.from),
        to_domain = %domain_of(&email.to),
        dkim = %email.dkim,
        attachments = email.attachments.len(),
        "inbound parse received"
    );
    let raw_key = persist(&state.surreal, &state.storage, &email).await?;

    // Thread the message into a support conversation when the feature is
    // configured (both NAVIGATOR_PARSE_HOST + NAVIGATOR_LAWYER_NOTIFY_EMAIL).
    // Best-effort: the raw `.eml` is already archived, so a threading
    // failure is logged rather than returned — a non-2xx would make
    // SendGrid retry and duplicate the conversation.
    if let Some(cfg) = crate::email_threads::ThreadConfig::from_env() {
        if let Err(e) = crate::email_threads::thread_inbound(
            &state.surreal,
            &state.storage,
            state.email.as_ref(),
            state.workflow_runtime.as_ref(),
            &cfg,
            &email,
            &raw_key,
        )
        .await
        {
            tracing::error!(error = %e, "inbound threading failed (message archived; conversation not advanced)");
        }
    }
    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use crate::attachment_scanner::{FakeAttachmentScanner, ScanError, ScanVerdict};

    use super::{
        attachments_from_raw, constant_time_eq, is_attachment_field, message_id_from_raw,
        scan_attachments, storage_key_for, InboundAttachment, InboundEmail,
    };

    #[test]
    fn message_id_from_raw_extracts_unwrapped_id() {
        let raw = b"Message-ID: <abc123@mail.example.com>\r\nFrom: a@b.com\r\n\
                    Subject: x\r\n\r\nbody";
        assert_eq!(
            message_id_from_raw(raw).as_deref(),
            Some("abc123@mail.example.com")
        );
        // No Message-ID header → None.
        assert!(message_id_from_raw(b"Subject: x\r\n\r\nbody").is_none());
    }

    #[test]
    fn raw_mime_fallback_preserves_decoded_attachment_metadata() {
        let raw = b"From: pisces@example.com\r\nTo: support@example.com\r\nSubject: Intake\r\n\
Content-Type: multipart/mixed; boundary=nav\r\n\r\n--nav\r\nContent-Type: text/plain\r\n\r\nhello\r\n\
--nav\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; \
filename*=UTF-8''signed%20intake.pdf\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjc=\r\n\
--nav--\r\n";
        let attachments = attachments_from_raw(raw);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "signed intake.pdf");
        assert_eq!(attachments[0].content_type, "application/pdf");
        assert_eq!(attachments[0].bytes, b"%PDF-1.7");
    }

    #[test]
    fn raw_mode_can_supply_envelope_fields_without_outer_duplicates() {
        let raw = b"From: Pisces <pisces@example.com>\r\nTo: support@example.com\r\n\
Subject: Intake\r\nContent-Type: multipart/mixed; boundary=nav\r\n\r\n--nav\r\n\
Content-Type: text/plain\r\n\r\nhello\r\n--nav--\r\n";
        let parsed = mail_parser::MessageParser::default().parse(raw).unwrap();
        assert_eq!(
            parsed
                .from()
                .and_then(|a| a.first())
                .and_then(|a| a.address()),
            Some("pisces@example.com")
        );
        assert_eq!(parsed.subject(), Some("Intake"));
    }

    #[tokio::test]
    async fn found_attachment_is_reduced_to_safe_quarantine_metadata() {
        let scanner = FakeAttachmentScanner::new(Ok(ScanVerdict::Found {
            signature: "Eicar-Signature".into(),
        }));
        let mut email = InboundEmail {
            attachments: vec![InboundAttachment {
                filename: "eicar.txt".into(),
                content_type: "text/plain".into(),
                bytes: b"harmless test fixture".to_vec(),
            }],
            ..Default::default()
        };
        scan_attachments(&scanner, &mut email).await.unwrap();
        assert_eq!(scanner.calls(), 1);
        assert!(email.attachments.is_empty());
        assert_eq!(email.quarantined_attachments.len(), 1);
        assert_eq!(email.quarantined_attachments[0].filename, "eicar.txt");
    }

    #[tokio::test]
    async fn scanner_error_fails_closed_without_retaining_bytes() {
        let scanner = FakeAttachmentScanner::new(Err(ScanError::Timeout));
        let mut email = InboundEmail {
            attachments: vec![InboundAttachment {
                filename: "document.pdf".into(),
                content_type: "application/pdf".into(),
                bytes: b"document".to_vec(),
            }],
            ..Default::default()
        };
        let error = scan_attachments(&scanner, &mut email).await.unwrap_err();
        assert!(matches!(
            error,
            super::InboundError::Scanner(ScanError::Timeout)
        ));
        assert_eq!(scanner.calls(), 1);
        assert!(email.attachments.is_empty());
        assert!(email.quarantined_attachments.is_empty());
    }

    #[test]
    fn is_attachment_field_matches_only_numbered_parts() {
        assert!(is_attachment_field("attachment1"));
        assert!(is_attachment_field("attachment12"));
        // the count field and the JSON metadata field are not parts
        assert!(!is_attachment_field("attachments"));
        assert!(!is_attachment_field("attachment-info"));
        // unrelated fields
        assert!(!is_attachment_field("attachment"));
        assert!(!is_attachment_field("from"));
        assert!(!is_attachment_field("attachmentx"));
    }

    #[test]
    fn constant_time_eq_matches_identical_strings() {
        assert!(constant_time_eq("hello", "hello"));
    }

    #[test]
    fn constant_time_eq_rejects_different_strings() {
        assert!(!constant_time_eq("hello", "world"));
    }

    #[test]
    fn constant_time_eq_rejects_different_lengths() {
        assert!(!constant_time_eq("hello", "hellox"));
        assert!(!constant_time_eq("", "x"));
    }

    #[test]
    fn constant_time_eq_handles_empty_strings() {
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn storage_key_carries_inbound_prefix_and_sender_slug() {
        let email = InboundEmail {
            from: "Aries <aries@example.com>".into(),
            to: "support@example.com".into(),
            subject: "Hello".into(),
            text: String::new(),
            raw: vec![],
            dkim: String::new(),
            attachments: vec![],
            quarantined_attachments: vec![],
            message_id: None,
        };
        let key = storage_key_for(&email);
        assert!(key.starts_with("inbound/"));
        assert!(std::path::Path::new(&key)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("eml")));
        // Punctuation, spaces, brackets all become `_`.
        assert!(key.contains("aries__aries_example_com_"));
    }

    #[test]
    fn storage_key_truncates_long_sender_slugs() {
        let email = InboundEmail {
            from: "a".repeat(200),
            ..Default::default()
        };
        let key = storage_key_for(&email);
        // 40 chars of slug + ".eml" + prefix + millisecond stamp.
        // Asserting the slug is capped — full-length sender doesn't
        // run away into a multi-kilobyte path.
        let slug = key
            .rsplit_once('-')
            .map_or("", |(_, tail)| tail.trim_end_matches(".eml"));
        assert!(slug.len() <= 40, "slug too long: {} chars", slug.len());
    }
}
