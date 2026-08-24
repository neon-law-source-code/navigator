//! Welcome-email template + render.
//!
//! Three consumers today: the OAuth callback fires a welcome on a
//! brand-new `persons` insert (via the workflow worker), the
//! `/admin/person/{id}` "Send welcome" button re-fires it on demand (direct
//! send from `web`), and the `workflows-service` worker dispatches
//! `email_send__welcome` steps in any workflow. Keeping the template +
//! render in one module means a change to the copy (or the subject)
//! shows up everywhere at once.

use uuid::Uuid;

use super::dispatch::EmailPayload;
use super::Template;
use crate::runtime::{StateMachineRuntime, WorkflowRuntimeError};
use crate::spec::MachineKind;
use crate::specs::welcome_spec;

/// Default subject for the welcome email. Mirrors the template's
/// `subject:` frontmatter default; kept as a constant so a rename in
/// the template has to update this line too (visible in the diff).
/// Brand-aware sends use [`welcome_subject`].
pub const WELCOME_SUBJECT: &str = "Welcome to Neon Law";

/// Subject for the welcome email, resolved through mounted branding so a
/// deployment greets its own clients by name.
#[must_use]
pub fn welcome_subject() -> String {
    format!("Welcome to {}", super::layout::brand_name())
}

/// Raw welcome template body (markdown with YAML frontmatter).
/// Bundled via `include_str!` so the binary doesn't need to read the
/// file off disk to send mail.
pub const WELCOME_TEMPLATE: &str = include_str!("../../content/email/welcome.md");

/// Static [`Template`] entry used by [`super::template_for_slug`].
pub const TEMPLATE: Template = Template {
    subject: WELCOME_SUBJECT,
    raw: WELCOME_TEMPLATE,
};

/// Render the welcome email body: strip the YAML frontmatter, then
/// substitute the recipient tokens (`{{client_name}}`,
/// `{{client_email}}`) and the brand tokens (`{{brand}}`,
/// `{{support_email}}`, `{{site_url}}`). The brand tokens resolve
/// through the same mounted brand as the rest of the email shell
/// so a rebranded fork's welcome never carries NeonLaw's name, address,
/// or domain.
#[must_use]
pub fn render_welcome_body(name: &str, email: &str) -> String {
    let brand = super::layout::brand_name();
    let support = super::layout::support_email();
    let site_url = super::layout::base_url_from_env();
    let body = super::strip_frontmatter(WELCOME_TEMPLATE);
    body.replace("{{client_name}}", name)
        .replace("{{client_email}}", email)
        .replace("{{brand}}", brand)
        .replace("{{support_email}}", support)
        .replace("{{site_url}}", &site_url)
}

/// Render the welcome email's HTML alternative: the same substituted markdown
/// body as [`render_welcome_body`], wrapped in the inline-styled email layout
/// with the firm logo. `base_url` is the public origin serving
/// `/logo.png` (see [`super::layout::base_url_from_env`]).
#[must_use]
pub fn render_welcome_html(name: &str, email: &str, base_url: &str) -> String {
    super::layout::render_email_html(&render_welcome_body(name, email), base_url)
}

/// Run the ephemeral `onboarding__welcome` workflow against the
/// given runtime: `start_ephemeral` with the welcome spec keyed
/// off `person_id`, then `signal_ephemeral("signup_recorded", …)`
/// to advance into `email_send__welcome` (the worker dispatches
/// the email there), then `signal_ephemeral("email_sent", None)`
/// to close out to `END`. The `person_id` doubles as the Restate
/// invocation key so repeated triggers idempotently no-op on the
/// broker side. Errors surface as [`WorkflowRuntimeError`]; the
/// caller (`portal/src/oauth.rs`) wraps the whole call in
/// fire-and-forget so a flaky broker doesn't block the OAuth
/// redirect.
pub async fn trigger_welcome(
    runtime: &dyn StateMachineRuntime,
    person_id: Uuid,
    name: &str,
    email: &str,
) -> Result<(), WorkflowRuntimeError> {
    let spec = welcome_spec();
    runtime
        .start_ephemeral(MachineKind::Workflow, person_id, &spec)
        .await?;

    let payload = EmailPayload::new(name.to_string(), email.to_string());
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| WorkflowRuntimeError::Transport(format!("payload encode: {e}")))?;

    runtime
        .signal_ephemeral(
            MachineKind::Workflow,
            person_id,
            "signup_recorded",
            Some(&payload_json),
        )
        .await?;
    runtime
        .signal_ephemeral(MachineKind::Workflow, person_id, "email_sent", None)
        .await?;
    Ok(())
}

/// Render and dispatch the welcome email for one Person, synchronously,
/// through the injected [`EmailService`].
///
/// This is the **command** every door goes through — the JSON API route
/// (`POST /app/api/people/{id}/welcome`), the `/admin/person/{id}` "Send
/// welcome" button, and the `aida_send_welcome_email` MCP tool. It lives
/// here rather than in `portal` because `mcp` cannot depend on `portal`
/// (that crate depends on `mcp`), and a command only two of the three
/// doors could reach is how the agent door drifted in the first place.
///
/// It is deliberately NOT [`trigger_welcome`]. The difference is the
/// audit row: the service injected here is wrapped in `portal::email`'s
/// `LoggingEmail` decorator, whose `send` writes one `sent_emails` row
/// per attempt — tagged with the template slug and the `person_id` by
/// the builder calls below. `store::sent_emails::record` has exactly one
/// production caller in the tree, that decorator, and the
/// `workflows-service` worker deliberately runs a *bare* backend without
/// it (see `workflows_service::email_config`). So a send that goes
/// through the Restate worker leaves no `sent_emails` row, and one that
/// goes through this command does.
///
/// It also reports the truth: the send either happened or it did not, so
/// `SendFailed` reaches the caller. [`trigger_welcome`] can only report
/// that the workflow *started*.
///
/// Returns the recipient on success so an adapter can personalize its
/// confirmation. The recipient is always read from the `persons` row, so
/// no caller — human or model — chooses where the mail lands.
pub async fn send_welcome(
    surreal: &store::surreal::SurrealDb,
    email: &dyn super::EmailService,
    base_url: &str,
    id: Uuid,
) -> Result<store::persons::Person, store::people_commands::PeopleCommandError> {
    use store::people_commands::PeopleCommandError;

    let person = store::persons::find_by_id(surreal, id)
        .await
        .map_err(PeopleCommandError::Db)?
        .ok_or(PeopleCommandError::NotFound)?;
    let body = render_welcome_body(&person.name, &person.email);
    let html = render_welcome_html(&person.name, &person.email, base_url);
    let msg = super::OutboundEmail::new(person.email.clone(), welcome_subject(), body)
        .with_template("welcome")
        .with_html(html)
        .with_person(id.to_string());
    match email.send(msg).await {
        Ok(_) => Ok(person),
        Err(e) => {
            tracing::warn!(error = %e, person_id = %id, "people: welcome email send failed");
            Err(PeopleCommandError::SendFailed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        render_welcome_body, render_welcome_html, trigger_welcome, welcome_subject, WELCOME_SUBJECT,
    };
    use crate::runtime::{InMemoryRuntime, StateMachineRuntime};
    use crate::spec::{MachineKind, StateName};
    use uuid::Uuid;

    #[tokio::test]
    async fn mounted_brand_scopes_welcome_copy_and_raster_logo() {
        let manifest: views::brand_bundle::BrandManifest = serde_yaml::from_str(
            "version: 1\nportal_only: true\nbrand:\n  firm: Acme Law\n  support_email: help@acme.example\n  base_url: https://portal.acme.example\n  terms_url: https://acme.example/terms\nassets:\n  firm_logo_raster: logo.png\n",
        )
        .unwrap();
        let branding = views::brand::Branding::from_manifest(&manifest);
        views::brand::scope(branding, async {
            assert_eq!(welcome_subject(), "Welcome to Acme Law");
            let body = render_welcome_body("Aries", "aries@example.com");
            assert!(body.contains("help@acme.example"));
            assert!(body.contains("https://portal.acme.example"));
            assert!(!body.contains("access-to-justice programs"));
            let html = render_welcome_html("Aries", "aries@example.com", views::brand::base_url());
            assert!(
                html.contains(r#"src="https://portal.acme.example/public/brand/firm-logo.png""#)
            );
        })
        .await;
    }

    #[test]
    fn render_substitutes_client_name_and_email_and_drops_frontmatter() {
        let body = render_welcome_body("Aries", "aries@example.com");
        assert!(!body.starts_with("---"), "frontmatter must be stripped");
        assert!(body.contains("Aries"));
        assert!(body.contains("aries@example.com"));
        // No template placeholder of any kind survives into the body —
        // the recipient tokens and the brand tokens (`{{brand}}`,
        // `{{support_email}}`, `{{site_url}}`) must all be substituted.
        assert!(
            !body.contains("{{"),
            "no `{{{{` placeholder may survive: {body}"
        );
    }

    #[test]
    fn render_substitutes_brand_tokens_with_defaults() {
        // Unscoped rendering resolves to complete NeonLaw defaults.
        let body = render_welcome_body("Aries", "aries@example.com");
        // `{{brand}}` → the firm brand name (default "Neon Law").
        assert!(
            body.contains("Welcome to Neon Law"),
            "brand token substituted: {body}"
        );
        // `{{support_email}}` → the firm's published address (default).
        assert!(
            body.contains("contact@neonlaw.com"),
            "support token substituted: {body}"
        );
    }

    /// The welcome email names no retired surface.
    ///
    /// It used to close on a plug for the firm's 501(c)(3) arm, linking
    /// `/foundation`. That surface is retired and the URL answers `410 Gone`,
    /// so an email carrying the link would send every new account at a dead
    /// page — and unlike a page, a sent email cannot be corrected.
    #[test]
    fn welcome_links_no_retired_surface() {
        let body = render_welcome_body("Aries", "aries@example.com");
        for retired in ["/foundation", "Foundation", "access-to-justice programs"] {
            assert!(
                !body.contains(retired),
                "the welcome email names the retired {retired}: {body}"
            );
        }
    }

    #[test]
    fn welcome_subject_defaults_to_brand_greeting() {
        assert_eq!(welcome_subject(), WELCOME_SUBJECT);
    }

    #[test]
    fn render_html_wraps_substituted_body_with_logo() {
        let html = render_welcome_html("Aries", "aries@example.com", "https://example.test");
        assert!(html.starts_with("<!doctype html>"), "full HTML document");
        assert!(html.contains("Aries"), "name substituted into HTML");
        assert!(
            html.contains(r#"src="https://example.test/public/logo.png""#),
            "logo PNG embedded at the exempt /public base URL",
        );
        // The frontmatter must not survive into the rendered HTML.
        assert!(!html.contains("subject:"));
    }

    #[test]
    fn render_keeps_signature_footer_with_published_address() {
        // Pins the address on the body so a template edit cannot quietly drop
        // the way back to the firm. It is the *published* address
        // (`views::brand`'s `firm_email`), which is deliberately allowed to
        // differ from `DEFAULT_FROM_EMAIL` — the envelope `From` and the mailbox
        // `portal::email_threads` ingests inbound replies on.
        let body = render_welcome_body("X", "x@y");
        assert!(body.contains("contact@neonlaw.com"));
    }

    #[test]
    fn welcome_subject_matches_template_title() {
        // Frontmatter `subject:` is the authoritative subject. Pin it
        // so a template rename also has to update the constant.
        assert_eq!(WELCOME_SUBJECT, "Welcome to Neon Law");
    }

    #[tokio::test]
    async fn trigger_welcome_drives_inmemory_runtime_through_to_end() {
        // Smoke-tests the trigger orchestration: start + two signals
        // land the welcome workflow at END. The in-memory runtime
        // ignores the ephemeral flag (no journal), so this only
        // pins the state-transition shape — wire-level ephemeral
        // bits are covered in `runtime_restate` tests.
        let rt = InMemoryRuntime::new();
        let person_id = Uuid::from_u128(7);
        trigger_welcome(&rt, person_id, "Aries", "aries@example.com")
            .await
            .expect("welcome trigger drives in-memory runtime to END");
        let final_state = rt.current_state(MachineKind::Workflow, person_id).await;
        assert_eq!(final_state, Some(StateName::end()));
    }
}
