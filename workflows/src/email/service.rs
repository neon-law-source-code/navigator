//! Outbound email abstraction.
//!
//! The trait + value types live in `workflows::email` so both `web`
//! (direct sends from the admin "Send welcome" button) and
//! `workflows-service` (the durable workflow worker) reach the same
//! contract without a crate-graph cycle.
//!
//! Two backends ship here:
//!
//! - [`CapturingEmail`] — in-memory recorder for tests and local
//!   dev (the default, since outbound mail in a notebook just
//!   clutters real inboxes).
//! - [`SendGridEmail`] — production backend that POSTs to
//!   SendGrid's v3 REST API. Picked because GCP has no managed
//!   transactional-email service; SendGrid is the path of least
//!   resistance for a "GCP only" workspace (see `AGENTS.md`).
//!   We hit the API directly with `reqwest` rather than pulling a
//!   sendgrid SDK — the surface we need is one POST.
//!
//! Decorators (retry, audit logging, env-based factory) live in
//! `portal::email` — only `web` needs them today.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use thiserror::Error;

use super::sendgrid_openapi;

/// Default `From:` address. Used when `SENDGRID_FROM_EMAIL` is unset.
/// Matches the address the inbound webhook expects (`support@neonlaw.com`)
/// so a reply lands back in the mailroom.
///
/// Deliberately `support@` where the site publishes `contact@`
/// (`views::brand::firm_email`). Moving what the site advertises must not
/// re-point the mail pipeline: this is the address threading keys on.
pub const DEFAULT_FROM_EMAIL: &str = "support@neonlaw.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundEmail {
    pub to: String,
    pub subject: String,
    pub body: String,
    /// Rendered HTML alternative for the body. When `Some`, the
    /// SendGrid backend sends a multipart message (`text/plain` +
    /// `text/html`) so rich clients show the styled version while
    /// text-only clients fall back to [`Self::body`]. `None` sends a
    /// plain-text-only message. Only [`Self::body`] is journaled to
    /// the `sent_emails` audit table — the HTML is reconstructible
    /// from the template + plain body and would bloat the log.
    pub html_body: Option<String>,
    /// Slug of the template that rendered the body (e.g. `welcome`).
    /// Carried through to the audit log so operators can answer
    /// "how many welcomes did we send last week?" with one SQL
    /// query. `None` for ad-hoc messages.
    pub template_slug: Option<String>,
    /// Envelope `From:` address override. `None` falls back to the
    /// configured `SENDGRID_FROM_EMAIL`. Most callers leave this
    /// `None`; the audit decorator reads the effective value off the
    /// backend it wraps and records that.
    pub from: Option<String>,
    /// `Reply-To:` address override. When `Some`, the SendGrid backend
    /// adds a `reply_to` so replies route somewhere other than the
    /// `From:` — the email-thread layer (`portal::email_threads`) points it
    /// at a per-conversation token address (`c<token>@parse.…`) so the
    /// reply threads back to the right conversation without exposing an
    /// internal address. `None` omits the header.
    pub reply_to: Option<String>,
    /// `persons.id` of the recipient, when the send is addressed to a
    /// known person. Stamped into SendGrid `custom_args` at send time
    /// so the delivery-side Event Webhook stream
    /// (`portal::email_events`) carries it back and the analytics join
    /// `email_events.person_id → persons.id` works without parsing
    /// the recipient address. `None` for ad-hoc / unaddressed sends.
    pub person_id: Option<String>,
    /// RFC 5322 `In-Reply-To:` header — the message-id this send replies
    /// to (already `<…>`-wrapped). Set by the email-thread layer so a
    /// mail client threads a support exchange. `None` omits the header.
    pub in_reply_to: Option<String>,
    /// RFC 5322 `References:` header — the space-separated message-id
    /// chain of the thread (each `<…>`-wrapped). `None` omits the header.
    pub references: Option<String>,
    /// File attachments. Empty for the common case. The SendGrid backend
    /// base64-encodes each attachment's bytes into the top-level
    /// `attachments` array; the [`CapturingEmail`] backend keeps them in
    /// memory for test assertions. Used by the workshop completion
    /// certificate, which rides a generated PDF along with the email.
    pub attachments: Vec<Attachment>,
}

/// A single email attachment: the raw bytes plus the metadata SendGrid
/// needs to encode it (`filename`, MIME `content_type`). Bytes are held
/// decoded; the SendGrid backend base64-encodes them at send time so the
/// audit log and the in-memory recorder both see the original payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl Attachment {
    #[must_use]
    pub fn new(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
            bytes,
        }
    }
}

impl OutboundEmail {
    /// Convenience constructor for the common case: only `to`,
    /// `subject`, `body`. The new `template_slug` and `from` fields
    /// default to `None`.
    #[must_use]
    pub fn new(to: impl Into<String>, subject: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            subject: subject.into(),
            body: body.into(),
            html_body: None,
            template_slug: None,
            from: None,
            reply_to: None,
            person_id: None,
            in_reply_to: None,
            references: None,
            attachments: Vec::new(),
        }
    }

    /// Builder-style: set the template slug. Callers using a named
    /// template (welcome, password-reset, etc.) should set this so
    /// the audit row carries the slug.
    #[must_use]
    pub fn with_template(mut self, slug: impl Into<String>) -> Self {
        self.template_slug = Some(slug.into());
        self
    }

    /// Builder-style: attach a rendered HTML alternative so the
    /// message is sent multipart (`text/plain` + `text/html`).
    #[must_use]
    pub fn with_html(mut self, html: impl Into<String>) -> Self {
        self.html_body = Some(html.into());
        self
    }

    /// Builder-style: tag the send with the recipient's `persons.id`
    /// so the delivery-side Event Webhook can join back to the person.
    #[must_use]
    pub fn with_person(mut self, person_id: impl Into<String>) -> Self {
        self.person_id = Some(person_id.into());
        self
    }

    /// Builder-style: set the `Reply-To:` address. The email-thread
    /// layer uses this to route replies to a per-conversation token
    /// address instead of the `From:`.
    #[must_use]
    pub fn with_reply_to(mut self, reply_to: impl Into<String>) -> Self {
        self.reply_to = Some(reply_to.into());
        self
    }

    /// Builder-style: override the envelope `From:` address. The workshop
    /// certificate uses this to send from `support@neonlaw.org` instead of
    /// the backend's configured default. The address's domain must be
    /// authenticated in SendGrid or the send is rejected.
    #[must_use]
    pub fn with_from(mut self, from: impl Into<String>) -> Self {
        self.from = Some(from.into());
        self
    }

    /// Builder-style: add a file attachment. Repeat to add several.
    #[must_use]
    pub fn with_attachment(mut self, attachment: Attachment) -> Self {
        self.attachments.push(attachment);
        self
    }

    /// Builder-style: set RFC 5322 threading headers from a chain of the
    /// thread's prior message-ids, oldest first. `References` becomes the
    /// whole chain and `In-Reply-To` the most recent id, each normalized
    /// to `<id>` form so a mail client threads the support conversation
    /// visually. A no-op on an empty (or all-blank) chain, so ordinary
    /// sends keep their previous byte-identical request body.
    #[must_use]
    pub fn with_thread_refs(mut self, message_ids: &[String]) -> Self {
        let normalized: Vec<String> = message_ids
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(angle_wrap)
            .collect();
        if let Some(last) = normalized.last() {
            self.in_reply_to = Some(last.clone());
            self.references = Some(normalized.join(" "));
        }
        self
    }
}

/// Wrap a bare message-id in RFC 5322 angle brackets, leaving an
/// already-wrapped id untouched.
fn angle_wrap(id: &str) -> String {
    let id = id.trim();
    if id.starts_with('<') && id.ends_with('>') {
        id.to_string()
    } else {
        format!("<{id}>")
    }
}

/// Receipt for a successful send. Carries SendGrid's `X-Message-Id`
/// response header when the backend is [`SendGridEmail`] — that id is
/// the join key against the delivery-side Event Webhook stream. The
/// [`CapturingEmail`] backend has no upstream id, so `message_id` is
/// `None` there.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendReceipt {
    pub message_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum EmailError {
    #[error("invalid recipient: {0}")]
    InvalidRecipient(String),
    #[error("transport error: {0}")]
    Transport(String),
}

#[async_trait]
pub trait EmailService: Send + Sync {
    async fn send(&self, email: OutboundEmail) -> Result<SendReceipt, EmailError>;
}

/// Captures every sent message in memory. Used by tests and
/// development environments where outbound email would just clutter
/// real inboxes.
#[derive(Clone, Default)]
pub struct CapturingEmail {
    sent: Arc<Mutex<Vec<OutboundEmail>>>,
}

impl CapturingEmail {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn captured(&self) -> Vec<OutboundEmail> {
        self.sent.lock().expect("email lock poisoned").clone()
    }
}

#[async_trait]
impl EmailService for CapturingEmail {
    async fn send(&self, email: OutboundEmail) -> Result<SendReceipt, EmailError> {
        if !email.to.contains('@') {
            return Err(EmailError::InvalidRecipient(email.to));
        }
        self.sent.lock().expect("email lock poisoned").push(email);
        Ok(SendReceipt::default())
    }
}

/// SendGrid v3 backend. Authenticates with a bearer API key and
/// posts to `{base_url}/v3/mail/send`. The constructor pins
/// `base_url` so tests can route the request at a `wiremock` server
/// instead of the live API.
#[derive(Clone)]
pub struct SendGridEmail {
    http: reqwest::Client,
    api_key: String,
    from_email: String,
    base_url: String,
}

const SENDGRID_API_URL: &str = "https://api.sendgrid.com";

impl SendGridEmail {
    /// Production constructor: targets the SendGrid public API.
    #[must_use]
    pub fn new(api_key: impl Into<String>, from_email: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.into(),
            from_email: from_email.into(),
            base_url: SENDGRID_API_URL.into(),
        }
    }

    /// Override the base URL — only useful for tests pointing at a
    /// `wiremock` server. Production deploys should call [`Self::new`].
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Build the JSON body for SendGrid's `/v3/mail/send` endpoint.
    /// Pure — exposed for unit testing the request shape without an
    /// HTTP round-trip.
    #[must_use]
    pub fn build_request_body(&self, email: &OutboundEmail) -> serde_json::Value {
        sendgrid_openapi::SendMailRequest::from_email(email, &self.from_email).into_json()
    }
}

#[async_trait]
impl EmailService for SendGridEmail {
    async fn send(&self, email: OutboundEmail) -> Result<SendReceipt, EmailError> {
        if !email.to.contains('@') {
            return Err(EmailError::InvalidRecipient(email.to));
        }
        let request = sendgrid_openapi::SendMailRequest::from_email(&email, &self.from_email);
        let resp = sendgrid_openapi::SendMailClient::new(
            self.http.clone(),
            self.api_key.clone(),
            self.base_url.clone(),
        )
        .send(request)
        .await
        .map_err(|e| EmailError::Transport(e.to_string()))?;
        let status = resp.status();
        if status.is_success() {
            // SendGrid returns the message id in the `X-Message-Id`
            // response header on a 202. It's the stable join key
            // against the delivery-side Event Webhook (whose events
            // carry the same id, suffixed per-event). Absent on
            // non-SendGrid mocks, so it's optional.
            let message_id = resp
                .headers()
                .get("x-message-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            return Ok(SendReceipt { message_id });
        }
        let body = resp.text().await.unwrap_or_default();
        Err(EmailError::Transport(format!(
            "sendgrid responded {status}: {body}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::{CapturingEmail, EmailError, EmailService, OutboundEmail, SendGridEmail};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn build_request_body_omits_custom_args_for_ad_hoc_sends() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(&OutboundEmail::new("a@example.com", "Hi", "Body"));
        // No template + no person → the key is absent, preserving the
        // previous byte-identical body for unaddressed sends.
        assert!(body.get("custom_args").is_none());
    }

    #[test]
    fn build_request_body_stamps_template_and_person_custom_args() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(
            &OutboundEmail::new("a@example.com", "Welcome", "Hi")
                .with_template("welcome")
                .with_person("0190f000-0000-7000-8000-000000000001"),
        );
        assert_eq!(body["custom_args"]["template_slug"], "welcome");
        assert_eq!(
            body["custom_args"]["person_id"],
            "0190f000-0000-7000-8000-000000000001"
        );
    }

    #[tokio::test]
    async fn sendgrid_captures_x_message_id_from_202_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/mail/send"))
            .respond_with(ResponseTemplate::new(202).insert_header("X-Message-Id", "sg-abc-123"))
            .expect(1)
            .mount(&server)
            .await;

        let svc = SendGridEmail::new("KEY", "support@neonlaw.com").with_base_url(server.uri());
        let receipt = svc.send(message("libra@example.com")).await.unwrap();
        assert_eq!(receipt.message_id.as_deref(), Some("sg-abc-123"));
    }

    fn message(to: &str) -> OutboundEmail {
        OutboundEmail::new(to, "Hello", "Body")
    }

    #[tokio::test]
    async fn capturing_email_records_sent_messages() {
        let svc = CapturingEmail::new();
        svc.send(message("libra@example.com")).await.unwrap();
        svc.send(message("taurus@example.com")).await.unwrap();
        let captured = svc.captured();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].to, "libra@example.com");
        assert_eq!(captured[1].to, "taurus@example.com");
    }

    #[tokio::test]
    async fn invalid_recipient_returns_error_and_is_not_captured() {
        let svc = CapturingEmail::new();
        let err = svc.send(message("not-an-email")).await.unwrap_err();
        assert!(matches!(err, EmailError::InvalidRecipient(_)));
        assert!(svc.captured().is_empty());
    }

    #[test]
    fn with_thread_refs_sets_in_reply_to_and_references() {
        let msg = OutboundEmail::new("a@example.com", "Re: x", "body")
            .with_thread_refs(&["client-1@mail".into(), "lawyer-1@mail".into()]);
        // In-Reply-To is the most recent id; References is the whole chain,
        // each angle-wrapped.
        assert_eq!(msg.in_reply_to.as_deref(), Some("<lawyer-1@mail>"));
        assert_eq!(
            msg.references.as_deref(),
            Some("<client-1@mail> <lawyer-1@mail>")
        );
    }

    #[test]
    fn with_thread_refs_is_a_noop_on_empty_chain_and_preserves_wrapping() {
        let empty = OutboundEmail::new("a@example.com", "x", "b").with_thread_refs(&[]);
        assert!(empty.in_reply_to.is_none());
        assert!(empty.references.is_none());
        // Already-wrapped ids are left untouched (not double-wrapped).
        let wrapped =
            OutboundEmail::new("a@example.com", "x", "b").with_thread_refs(&["<abc@m>".into()]);
        assert_eq!(wrapped.in_reply_to.as_deref(), Some("<abc@m>"));
    }

    #[test]
    fn build_request_body_emits_threading_headers_when_set() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(
            &OutboundEmail::new("a@example.com", "Re: x", "Body")
                .with_thread_refs(&["client-1@mail".into()]),
        );
        let headers = &body["personalizations"][0]["headers"];
        assert_eq!(headers["In-Reply-To"], "<client-1@mail>");
        assert_eq!(headers["References"], "<client-1@mail>");
    }

    #[test]
    fn build_request_body_omits_threading_headers_when_unset() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(&OutboundEmail::new("a@example.com", "Hi", "Body"));
        assert!(body["personalizations"][0].get("headers").is_none());
    }

    #[test]
    fn sendgrid_build_request_body_matches_v3_schema() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(&OutboundEmail::new(
            "libra@example.com",
            "Welcome",
            "Hi Libra",
        ));
        assert_eq!(
            body["personalizations"][0]["to"][0]["email"],
            "libra@example.com"
        );
        assert_eq!(body["from"]["email"], "support@neonlaw.com");
        assert_eq!(body["subject"], "Welcome");
        assert_eq!(body["content"][0]["type"], "text/plain");
        assert_eq!(body["content"][0]["value"], "Hi Libra");
        // Plain-only message: exactly one content part, no html.
        assert_eq!(body["content"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn sendgrid_build_request_body_appends_html_part_when_present() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(
            &OutboundEmail::new("libra@example.com", "Welcome", "Hi Libra")
                .with_html("<p>Hi Libra</p>"),
        );
        // text/plain must come first (SendGrid orders by increasing
        // client preference), text/html second.
        assert_eq!(body["content"].as_array().map(Vec::len), Some(2));
        assert_eq!(body["content"][0]["type"], "text/plain");
        assert_eq!(body["content"][0]["value"], "Hi Libra");
        assert_eq!(body["content"][1]["type"], "text/html");
        assert_eq!(body["content"][1]["value"], "<p>Hi Libra</p>");
    }

    #[test]
    fn build_request_body_omits_attachments_when_none() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(&OutboundEmail::new("a@example.com", "Hi", "Body"));
        assert!(body.get("attachments").is_none());
    }

    #[test]
    fn build_request_body_base64_encodes_attachment() {
        use super::Attachment;
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(
            &OutboundEmail::new("a@example.com", "Your certificate", "Attached.").with_attachment(
                Attachment::new("certificate.pdf", "application/pdf", b"%PDF-1.7".to_vec()),
            ),
        );
        let att = &body["attachments"][0];
        // "%PDF-1.7" base64-encodes to "JVBERi0xLjc=".
        assert_eq!(att["content"], "JVBERi0xLjc=");
        assert_eq!(att["type"], "application/pdf");
        assert_eq!(att["filename"], "certificate.pdf");
        assert_eq!(att["disposition"], "attachment");
    }

    #[test]
    fn build_request_body_honors_from_override() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(
            &OutboundEmail::new("a@example.com", "Hi", "Body").with_from("support@neonlaw.org"),
        );
        // The explicit `.with_from(...)` wins over the backend default.
        assert_eq!(body["from"]["email"], "support@neonlaw.org");
    }

    #[test]
    fn build_request_body_falls_back_to_backend_from_when_unset() {
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com");
        let body = svc.build_request_body(&OutboundEmail::new("a@example.com", "Hi", "Body"));
        assert_eq!(body["from"]["email"], "support@neonlaw.com");
    }

    #[tokio::test]
    async fn sendgrid_posts_bearer_authed_request_and_returns_ok_on_202() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/mail/send"))
            .and(header("authorization", "Bearer TEST_KEY"))
            .and(header("content-type", "application/json"))
            .respond_with(ResponseTemplate::new(202))
            .expect(1)
            .mount(&server)
            .await;

        let svc = SendGridEmail::new("TEST_KEY", "support@neonlaw.com").with_base_url(server.uri());
        svc.send(message("libra@example.com"))
            .await
            .expect("send succeeds");
        // `expect(1)` above asserts exactly one matching request was
        // captured before the server drops at end of scope.
    }

    #[tokio::test]
    async fn sendgrid_maps_non_2xx_response_to_transport_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v3/mail/send"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .expect(1)
            .mount(&server)
            .await;

        let svc = SendGridEmail::new("BAD_KEY", "support@neonlaw.com").with_base_url(server.uri());
        let err = svc.send(message("libra@example.com")).await.unwrap_err();
        match err {
            EmailError::Transport(msg) => {
                assert!(msg.contains("401"), "expected 401 in error, got: {msg}");
                assert!(msg.contains("Unauthorized"));
            }
            EmailError::InvalidRecipient(r) => {
                panic!("expected Transport error, got InvalidRecipient({r})")
            }
        }
    }

    #[tokio::test]
    async fn sendgrid_rejects_invalid_recipient_without_calling_api() {
        // No MockServer setup — if the impl actually hit the
        // network the test would fail with a connection error
        // instead of the expected validation error.
        let svc = SendGridEmail::new("KEY", "support@neonlaw.com")
            .with_base_url("http://unreachable.invalid");
        let err = svc.send(message("not-an-email")).await.unwrap_err();
        assert!(matches!(err, EmailError::InvalidRecipient(_)));
    }

    #[tokio::test]
    async fn sendgrid_propagates_transport_error_on_connection_failure() {
        // Point at a port nothing is listening on.
        let svc =
            SendGridEmail::new("KEY", "support@neonlaw.com").with_base_url("http://127.0.0.1:1");
        let err = svc.send(message("libra@example.com")).await.unwrap_err();
        assert!(matches!(err, EmailError::Transport(_)));
    }
}
