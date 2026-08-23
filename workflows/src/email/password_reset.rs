//! Password-reset email template + render.
//!
//! Sent **directly from `web`** (not via the workflow dispatcher) because
//! the body carries a per-request, single-use reset URL that only the
//! request handler holds — see `portal::password_reset`. The template lives
//! here next to [`welcome`](super::welcome) so all outbound copy shares
//! one home and one brand seam.

use super::Template;

/// Subject for the password-reset email, resolved through mounted branding
/// so a rebranded deployment greets its own
/// clients. Mirrors the template's `subject:` frontmatter.
#[must_use]
pub fn password_reset_subject() -> String {
    format!("Reset your {} password", super::layout::brand_name())
}

/// Raw template body (markdown with YAML frontmatter). Bundled via
/// `include_str!` so the binary doesn't read the file off disk to send.
pub const PASSWORD_RESET_TEMPLATE: &str = include_str!("../../content/email/password_reset.md");

/// Static [`Template`] entry used by [`super::template_for_slug`]. The
/// subject mirrors [`password_reset_subject`] with the default brand.
pub const TEMPLATE: Template = Template {
    subject: "Reset your Neon Law password",
    raw: PASSWORD_RESET_TEMPLATE,
};

/// Render the password-reset email body: strip the YAML frontmatter, then
/// substitute the recipient tokens (`{{client_name}}`, `{{client_email}}`,
/// `{{reset_url}}`) and the brand tokens (`{{brand}}`, `{{support_email}}`,
/// `{{site_url}}`). The brand tokens resolve through the same firm-brand
/// env seams as the rest of the email shell so a rebranded fork's reset
/// email never carries NeonLaw's name or domain.
#[must_use]
pub fn render_password_reset_body(name: &str, email: &str, reset_url: &str) -> String {
    let brand = super::layout::brand_name();
    let support = super::layout::support_email();
    let site_url = super::layout::base_url_from_env();
    let body = super::strip_frontmatter(PASSWORD_RESET_TEMPLATE);
    body.replace("{{client_name}}", name)
        .replace("{{client_email}}", email)
        .replace("{{reset_url}}", reset_url)
        .replace("{{brand}}", brand)
        .replace("{{support_email}}", support)
        .replace("{{site_url}}", &site_url)
}

/// Render the HTML alternative: the same substituted markdown wrapped in
/// the inline-styled firm email layout with the firm logo. `base_url` is
/// the public origin serving `/public/logo-neon.png`.
#[must_use]
pub fn render_password_reset_html(
    name: &str,
    email: &str,
    reset_url: &str,
    base_url: &str,
) -> String {
    super::layout::render_email_html(
        &render_password_reset_body(name, email, reset_url),
        base_url,
    )
}

#[cfg(test)]
mod tests {
    use super::{render_password_reset_body, render_password_reset_html, TEMPLATE};

    #[test]
    fn render_substitutes_every_token_and_drops_frontmatter() {
        let body = render_password_reset_body(
            "Aries",
            "aries@example.com",
            "https://app.test/auth/password/reset?token=abc",
        );
        assert!(!body.starts_with("---"), "frontmatter must be stripped");
        assert!(body.contains("Aries"));
        assert!(body.contains("aries@example.com"));
        assert!(body.contains("https://app.test/auth/password/reset?token=abc"));
        assert!(
            !body.contains("{{"),
            "no `{{{{` placeholder may survive: {body}"
        );
    }

    #[test]
    fn html_wraps_body_with_firm_logo_and_link() {
        let html = render_password_reset_html(
            "Aries",
            "aries@example.com",
            "https://app.test/auth/password/reset?token=abc",
            "https://app.test",
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("logo-neon.png"));
        assert!(html.contains("https://app.test/auth/password/reset?token=abc"));
        assert!(!html.contains("subject:"));
    }

    #[test]
    fn template_subject_pins_the_default_brand_copy() {
        // A brand rename in the template has to update this constant too.
        assert_eq!(TEMPLATE.subject, "Reset your Neon Law password");
        assert!(TEMPLATE.raw.starts_with("---\n"));
    }
}
