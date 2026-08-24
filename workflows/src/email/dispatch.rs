//! Worker-side step dispatch for `email_send__<slug>` states.
//!
//! When a workflow transitions into an `email_send__welcome` state,
//! `workflows-service` calls [`dispatch_state`] to:
//!
//! 1. Parse the `<slug>` out of the state name.
//! 2. Look up the [`super::Template`] for that slug.
//! 3. Render the body using the per-template renderer (currently
//!    only `welcome::render`; new templates each export their own).
//! 4. Hand the resulting [`OutboundEmail`] to the injected
//!    [`EmailService`].
//!
//! The fn is pure relative to the injected service — i.e. it does
//! one `send` call and returns the outcome, leaving journaling to
//! the caller (`workflows-service` wraps it inside `ctx.run` so the
//! Restate journal stamps each side effect).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::service::{EmailError, EmailService, OutboundEmail};
use super::{certificate, welcome};

/// Recipient payload for an `email_send__*` step. Carried as the
/// workflow's input through Restate state so the dispatch handler
/// can render the template at signal time.
///
/// `workshop_title` and `issued_date` are only set for the workshop
/// completion certificate (`email_send__certificate`); they
/// `skip_serializing_if` so every other send (welcome, …) keeps its
/// previous byte-identical JSON shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailPayload {
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_date: Option<String>,
}

impl EmailPayload {
    #[must_use]
    pub fn new(name: impl Into<String>, email: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            workshop_title: None,
            issued_date: None,
        }
    }

    /// Payload for the workshop completion certificate: carries the
    /// workshop title and pre-formatted issue date the certificate PDF
    /// and email body need. The date is computed by the trigger (so it is
    /// journaled in the Restate signal value and a replay stays
    /// deterministic), never read from the clock in the worker.
    #[must_use]
    pub fn certificate(
        name: impl Into<String>,
        email: impl Into<String>,
        workshop_title: impl Into<String>,
        issued_date: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            email: email.into(),
            workshop_title: Some(workshop_title.into()),
            issued_date: Some(issued_date.into()),
        }
    }
}

#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("state `{0}` is not an `email_send__<slug>` step")]
    NotAnEmailSendState(String),
    #[error("no email template registered for slug `{0}`")]
    UnknownTemplate(String),
    #[error("email send failed: {0}")]
    Send(#[from] EmailError),
    /// A slug needs a payload field the caller didn't supply (e.g. the
    /// certificate needs `workshop_title`). Deterministic — not retried.
    #[error("slug `{slug}` requires payload field `{field}`")]
    MissingField {
        slug: &'static str,
        field: &'static str,
    },
    /// The certificate PDF failed to render.
    #[error("certificate pdf: {0}")]
    Pdf(String),
}

/// Resolve `email_send__<slug>` → `<slug>`, render the matching
/// template against `payload`, and send through `svc`. Returns the
/// [`OutboundEmail`] that was handed to the service so the caller
/// can journal it.
///
/// Errors deterministically (not retried by Restate) on
/// [`DispatchError::NotAnEmailSendState`] and
/// [`DispatchError::UnknownTemplate`]; only [`DispatchError::Send`]
/// reflects a transient-or-not transport result whose retry
/// behavior is the caller's choice.
pub async fn dispatch_state(
    svc: &dyn EmailService,
    state_name: &str,
    payload: &EmailPayload,
) -> Result<OutboundEmail, DispatchError> {
    let slug = parse_slug(state_name)?;
    let (subject, body, html) = render_for_slug(slug, payload)?;
    let mut email = OutboundEmail {
        to: payload.email.clone(),
        subject,
        body,
        html_body: Some(html),
        template_slug: Some(slug.to_string()),
        from: None,
        reply_to: None,
        // `EmailPayload` carries no `persons.id` yet, so workflow-driven
        // sends can't stamp `person_id` for the delivery-side join.
        // Threading it through the payload is a follow-up; the join
        // still works on `template_slug` in the meantime.
        person_id: None,
        // Workflow-driven sends aren't part of a support thread.
        in_reply_to: None,
        references: None,
        attachments: Vec::new(),
    };
    // The workshop completion certificate rides a generated PDF and sends
    // from the brand's own support address (`layout::support_email`, via
    // `certificate::cert_from_email`) rather than the backend default. Every
    // other slug keeps the previous shape.
    if slug == "certificate" {
        // `render_for_slug` above already validated `workshop_title` for the
        // certificate slug and returned `Err` if it was missing, so this can
        // only be `Some` here. The `?` is defensive (never panics) rather
        // than a second real validation.
        let workshop = payload
            .workshop_title
            .as_deref()
            .ok_or(DispatchError::MissingField {
                slug: "certificate",
                field: "workshop_title",
            })?;
        let issued = payload
            .issued_date
            .as_deref()
            .ok_or(DispatchError::MissingField {
                slug: "certificate",
                field: "issued_date",
            })?;
        let attachment = certificate::certificate_attachment(&payload.name, workshop, issued)
            .map_err(|e| DispatchError::Pdf(e.to_string()))?;
        email.from = Some(certificate::cert_from_email());
        email.attachments.push(attachment);
    }
    svc.send(email.clone()).await?;
    Ok(email)
}

/// Returns the `<slug>` part of an `email_send__<slug>` state name,
/// or `None` if the prefix doesn't match.
pub fn parse_slug(state_name: &str) -> Result<&str, DispatchError> {
    state_name
        .strip_prefix("email_send__")
        .ok_or_else(|| DispatchError::NotAnEmailSendState(state_name.to_string()))
}

/// Render a template to `(subject, plain_body, html_body)`. The HTML
/// body uses the logo origin from [`super::layout::base_url_from_env`]
/// (driven by `NAV_BASE_URL`); the worker runs per-deploy so reading
/// the env here keeps the dispatch signature payload-only.
fn render_for_slug(
    slug: &str,
    payload: &EmailPayload,
) -> Result<(String, String, String), DispatchError> {
    match slug {
        "welcome" => {
            let base_url = super::layout::base_url_from_env();
            Ok((
                welcome::welcome_subject(),
                welcome::render_welcome_body(&payload.name, &payload.email),
                welcome::render_welcome_html(&payload.name, &payload.email, &base_url),
            ))
        }
        "certificate" => {
            let workshop =
                payload
                    .workshop_title
                    .as_deref()
                    .ok_or(DispatchError::MissingField {
                        slug: "certificate",
                        field: "workshop_title",
                    })?;
            let base_url = super::layout::base_url_from_env();
            Ok((
                certificate::certificate_subject(),
                certificate::render_certificate_body(&payload.name, workshop),
                certificate::render_certificate_html(&payload.name, workshop, &base_url),
            ))
        }
        other => Err(DispatchError::UnknownTemplate(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::super::service::CapturingEmail;
    use super::{dispatch_state, parse_slug, DispatchError, EmailPayload};

    #[test]
    fn parse_slug_extracts_welcome() {
        assert_eq!(parse_slug("email_send__welcome").unwrap(), "welcome");
    }

    #[test]
    fn parse_slug_rejects_non_email_send_state() {
        let err = parse_slug("signature__signed").unwrap_err();
        assert!(matches!(err, DispatchError::NotAnEmailSendState(_)));
    }

    #[tokio::test]
    async fn dispatch_state_sends_welcome_through_capturing_backend() {
        let svc = CapturingEmail::new();
        let payload = EmailPayload::new("Aries", "aries@example.com");
        let sent = dispatch_state(&svc, "email_send__welcome", &payload)
            .await
            .expect("welcome dispatch must succeed");

        // The return value mirrors the in-flight OutboundEmail so the
        // caller can journal it without re-rendering.
        assert_eq!(sent.to, "aries@example.com");
        assert_eq!(sent.subject, "Welcome to Neon Law");
        assert_eq!(sent.template_slug.as_deref(), Some("welcome"));
        assert!(sent.body.contains("Aries"));
        assert!(sent.body.contains("aries@example.com"));

        // A styled HTML alternative is attached so rich clients render
        // the logo + formatting; text-only clients fall back to `body`.
        let html = sent.html_body.as_deref().expect("html alternative set");
        assert!(html.contains("logo.png"));
        assert!(html.contains("Aries"));

        // And the same email actually went through the service.
        let captured = svc.captured();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].to, "aries@example.com");
        assert_eq!(captured[0].subject, "Welcome to Neon Law");
        assert_eq!(captured[0].template_slug.as_deref(), Some("welcome"));
    }

    #[tokio::test]
    async fn dispatch_state_rejects_unknown_template_slug() {
        let svc = CapturingEmail::new();
        let payload = EmailPayload::new("x", "x@y");
        let err = dispatch_state(&svc, "email_send__password_reset", &payload)
            .await
            .unwrap_err();
        assert!(matches!(err, DispatchError::UnknownTemplate(slug) if slug == "password_reset"));
        assert!(svc.captured().is_empty());
    }

    #[tokio::test]
    async fn dispatch_state_rejects_non_email_send_state_name() {
        let svc = CapturingEmail::new();
        let payload = EmailPayload::new("x", "x@y");
        let err = dispatch_state(&svc, "BEGIN", &payload).await.unwrap_err();
        assert!(matches!(err, DispatchError::NotAnEmailSendState(_)));
    }
}
