//! Email-confirmation template + render.
//!
//! Sent **directly from `web`** when a password (non-Google) user with an
//! unverified address tries to sign in: the sign-in tail hard-gates them
//! and fires this email, whose single-use link flips `emailVerified` in
//! Identity Platform — see `portal::email_confirm`. Lives next to
//! [`welcome`](super::welcome) and [`password_reset`](super::password_reset)
//! so all outbound copy shares one home and one brand seam.

use super::Template;

/// Subject for the confirm-email message, resolved through the firm brand
/// mounted brand. Mirrors the template's `subject:`
/// frontmatter.
#[must_use]
pub fn email_confirm_subject() -> String {
    format!("Confirm your email for {}", super::layout::brand_name())
}

/// Raw template body (markdown with YAML frontmatter), bundled via
/// `include_str!`.
pub const EMAIL_CONFIRM_TEMPLATE: &str = include_str!("../../content/email/email_confirm.md");

/// Static [`Template`] entry used by [`super::template_for_slug`].
pub const TEMPLATE: Template = Template {
    subject: "Confirm your email for Neon Law",
    raw: EMAIL_CONFIRM_TEMPLATE,
};

/// Render the confirm-email body: strip the YAML frontmatter, then
/// substitute the recipient tokens (`{{client_name}}`, `{{client_email}}`,
/// `{{confirm_url}}`) and the brand tokens (`{{brand}}`,
/// `{{support_email}}`, `{{site_url}}`).
#[must_use]
pub fn render_email_confirm_body(name: &str, email: &str, confirm_url: &str) -> String {
    let brand = super::layout::brand_name();
    let support = super::layout::support_email();
    let site_url = super::layout::base_url_from_env();
    let body = super::strip_frontmatter(EMAIL_CONFIRM_TEMPLATE);
    body.replace("{{client_name}}", name)
        .replace("{{client_email}}", email)
        .replace("{{confirm_url}}", confirm_url)
        .replace("{{brand}}", brand)
        .replace("{{support_email}}", support)
        .replace("{{site_url}}", &site_url)
}

/// Render the HTML alternative wrapped in the inline-styled firm email
/// layout. `base_url` is the public origin serving `/public/logo.png`.
#[must_use]
pub fn render_email_confirm_html(
    name: &str,
    email: &str,
    confirm_url: &str,
    base_url: &str,
) -> String {
    super::layout::render_email_html(
        &render_email_confirm_body(name, email, confirm_url),
        base_url,
    )
}

#[cfg(test)]
mod tests {
    use super::{render_email_confirm_body, render_email_confirm_html, TEMPLATE};

    #[test]
    fn render_substitutes_every_token_and_drops_frontmatter() {
        let body = render_email_confirm_body(
            "Aries",
            "aries@example.com",
            "https://app.test/auth/email/confirm?token=abc",
        );
        assert!(!body.starts_with("---"), "frontmatter must be stripped");
        assert!(body.contains("Aries"));
        assert!(body.contains("aries@example.com"));
        assert!(body.contains("https://app.test/auth/email/confirm?token=abc"));
        assert!(
            !body.contains("{{"),
            "no `{{{{` placeholder may survive: {body}"
        );
    }

    #[test]
    fn html_wraps_body_with_firm_logo_and_link() {
        let html = render_email_confirm_html(
            "Aries",
            "aries@example.com",
            "https://app.test/auth/email/confirm?token=abc",
            "https://app.test",
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("logo.png"));
        assert!(html.contains("https://app.test/auth/email/confirm?token=abc"));
    }

    #[test]
    fn template_subject_pins_the_default_brand_copy() {
        assert_eq!(TEMPLATE.subject, "Confirm your email for Neon Law");
        assert!(TEMPLATE.raw.starts_with("---\n"));
    }
}
