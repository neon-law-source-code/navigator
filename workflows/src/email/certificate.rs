//! Workshop completion certificate email + render + PDF attachment.
//!
//! Triggered by a student who has worked through every slide of a
//! workshop and asked for their certificate. The flow is durable: `web`
//! dispatches the `workshop__certificate` workflow against the shared
//! [`StateMachineRuntime`], which lands on `email_send__certificate`;
//! the dispatcher then renders this email, generates the certificate PDF
//! ([`pdf::render_certificate`]), attaches it, and sends from the firm's
//! published address (`contact@neonlaw.com` by default).
//!
//! Progress is tracked entirely client-side (browser `localStorage`, no
//! telemetry); this module never learns *which* slides were seen, only
//! that a student requested the certificate for a named workshop.

use uuid::Uuid;

use super::dispatch::EmailPayload;
use super::service::Attachment;
use super::Template;
use crate::runtime::{StateMachineRuntime, WorkflowRuntimeError};
use crate::spec::MachineKind;
use crate::specs::workshop_certificate_spec;

/// Default subject. Mirrors the template's `subject:` frontmatter default;
/// brand-aware sends use [`certificate_subject`].
pub const CERTIFICATE_SUBJECT: &str = "Your Neon Law certificate of completion";

/// Raw certificate email body (markdown with YAML frontmatter), bundled
/// so the binary needn't read it off disk.
pub const CERTIFICATE_TEMPLATE: &str = include_str!("../../content/email/certificate.md");

/// Static [`Template`] entry used by [`super::template_for_slug`].
pub const TEMPLATE: Template = Template {
    subject: CERTIFICATE_SUBJECT,
    raw: CERTIFICATE_TEMPLATE,
};

/// Brand-aware subject, resolved through the brand seam so a rebranded fork's
/// certificate greets under its own name.
#[must_use]
pub fn certificate_subject() -> String {
    format!(
        "Your {} certificate of completion",
        super::layout::brand_name()
    )
}

/// The envelope `From:` for the certificate — the firm's published address
/// (default `contact@neonlaw.com`). The domain must be authenticated with the
/// mail provider (see `docs/deployment-secrets.md`) or the send is rejected.
#[must_use]
pub fn cert_from_email() -> String {
    super::layout::support_email().to_string()
}

/// Render the certificate email body: strip the frontmatter, substitute the
/// recipient + workshop tokens and the brand tokens.
#[must_use]
pub fn render_certificate_body(name: &str, workshop_title: &str) -> String {
    let brand = super::layout::brand_name();
    let support = super::layout::support_email();
    let site_url = super::layout::base_url_from_env();
    let body = super::strip_frontmatter(CERTIFICATE_TEMPLATE);
    body.replace("{{recipient_name}}", name)
        .replace("{{workshop_title}}", workshop_title)
        .replace("{{brand}}", brand)
        .replace("{{support_email}}", support)
        .replace("{{site_url}}", &site_url)
}

/// Render the certificate email's HTML alternative: the substituted body
/// wrapped in the shared email layout.
#[must_use]
pub fn render_certificate_html(name: &str, workshop_title: &str, base_url: &str) -> String {
    super::layout::render_email_html(&render_certificate_body(name, workshop_title), base_url)
}

/// Generate the PDF certificate as an email [`Attachment`].
///
/// # Errors
///
/// Surfaces [`pdf::PdfError`] if the Typst render fails (practically only
/// an internal regression — all inputs are escaped into string literals).
pub fn certificate_attachment(
    name: &str,
    workshop_title: &str,
    issued_date: &str,
) -> Result<Attachment, pdf::PdfError> {
    let bytes = pdf::render_certificate(&pdf::CertificateParams {
        recipient_name: name.to_string(),
        workshop_title: workshop_title.to_string(),
        issued_date: issued_date.to_string(),
    })?;
    Ok(Attachment::new("certificate.pdf", "application/pdf", bytes))
}

/// Run the ephemeral `workshop__certificate` workflow against the given
/// runtime: `start_ephemeral` keyed off `key`, then signal `requested`
/// (which lands on `email_send__certificate`, where the dispatcher
/// renders + attaches the PDF and sends), then `email_sent` to close to
/// `END`. `issued_date` is computed by the caller so it is journaled in
/// the signal value and a Restate replay stays deterministic.
pub async fn trigger_certificate(
    runtime: &dyn StateMachineRuntime,
    key: Uuid,
    name: &str,
    email: &str,
    workshop_title: &str,
    issued_date: &str,
) -> Result<(), WorkflowRuntimeError> {
    let spec = workshop_certificate_spec();
    runtime
        .start_ephemeral(MachineKind::Workflow, key, &spec)
        .await?;

    let payload = EmailPayload::certificate(name, email, workshop_title, issued_date);
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| WorkflowRuntimeError::Transport(format!("payload encode: {e}")))?;

    runtime
        .signal_ephemeral(MachineKind::Workflow, key, "requested", Some(&payload_json))
        .await?;
    runtime
        .signal_ephemeral(MachineKind::Workflow, key, "email_sent", None)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        certificate_attachment, render_certificate_body, render_certificate_html,
        trigger_certificate,
    };
    use crate::runtime::{InMemoryRuntime, StateMachineRuntime};
    use crate::spec::{MachineKind, StateName};
    use uuid::Uuid;

    #[test]
    fn body_substitutes_recipient_and_workshop_and_drops_frontmatter() {
        let body = render_certificate_body("Aries Tenant", "Deploy the Neon Law Navigator");
        assert!(!body.starts_with("---"), "frontmatter must be stripped");
        assert!(body.contains("Aries Tenant"));
        assert!(body.contains("Deploy the Neon Law Navigator"));
        assert!(!body.contains("{{"), "no placeholder may survive: {body}");
    }

    #[test]
    fn html_wraps_body_with_the_firm_brand() {
        let html = render_certificate_html(
            "Aries",
            "Deploy the Neon Law Navigator",
            "https://example.test",
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("Aries"));
        assert!(html.contains("logo.png"));
        assert!(
            !html.contains("Foundation"),
            "the certificate is the firm's artifact now: {html}"
        );
    }

    #[test]
    fn attachment_is_a_named_pdf() {
        let att = certificate_attachment("Aries", "Deploy the Neon Law Navigator", "June 24, 2026")
            .expect("certificate pdf renders");
        assert_eq!(att.filename, "certificate.pdf");
        assert_eq!(att.content_type, "application/pdf");
        assert!(att.bytes.starts_with(b"%PDF-"));
    }

    #[tokio::test]
    async fn trigger_drives_inmemory_runtime_through_to_end() {
        let rt = InMemoryRuntime::new();
        let key = Uuid::from_u128(11);
        trigger_certificate(
            &rt,
            key,
            "Aries",
            "aries@example.com",
            "Deploy the Neon Law Navigator",
            "June 24, 2026",
        )
        .await
        .expect("certificate trigger drives in-memory runtime to END");
        let final_state = rt.current_state(MachineKind::Workflow, key).await;
        assert_eq!(final_state, Some(StateName::end()));
    }
}
