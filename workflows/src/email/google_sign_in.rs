//! "Sign in with Google" notice template + render.
//!
//! Sent **directly from `web`** when someone requests a password reset for
//! an address that signs in with Google — Identity Platform holds no
//! password for it, so there is nothing to reset. Rather than stay silent
//! (which leaves the requester staring at the neutral "check your inbox"
//! page wondering why no link arrived), `portal::password_reset` mails this
//! notice telling them to use the Google button. The body carries no
//! token — there is no reset link — so it lives here next to
//! [`password_reset`](super::password_reset) only to share the brand seam.

use super::Template;

/// Subject for the Google-sign-in notice, resolved through the firm brand
/// mounted brand. Mirrors the template's `subject:`
/// frontmatter.
#[must_use]
pub fn google_sign_in_subject() -> String {
    format!("Sign in to {} with Google", super::layout::brand_name())
}

/// Raw template body (markdown with YAML frontmatter), bundled via
/// `include_str!`.
pub const GOOGLE_SIGN_IN_TEMPLATE: &str = include_str!("../../content/email/google_sign_in.md");

/// Static [`Template`] entry used by [`super::template_for_slug`].
pub const TEMPLATE: Template = Template {
    subject: "Sign in to Neon Law with Google",
    raw: GOOGLE_SIGN_IN_TEMPLATE,
};

/// Render the notice body: strip the YAML frontmatter, then substitute the
/// recipient tokens (`{{client_name}}`, `{{client_email}}`, `{{login_url}}`)
/// and the brand tokens (`{{brand}}`, `{{support_email}}`, `{{site_url}}`).
#[must_use]
pub fn render_google_sign_in_body(name: &str, email: &str, login_url: &str) -> String {
    let brand = super::layout::brand_name();
    let support = super::layout::support_email();
    let site_url = super::layout::base_url_from_env();
    let body = super::strip_frontmatter(GOOGLE_SIGN_IN_TEMPLATE);
    body.replace("{{client_name}}", name)
        .replace("{{client_email}}", email)
        .replace("{{login_url}}", login_url)
        .replace("{{brand}}", brand)
        .replace("{{support_email}}", support)
        .replace("{{site_url}}", &site_url)
}

/// Render the HTML alternative wrapped in the inline-styled firm email
/// layout. `base_url` is the public origin serving `/public/logo-neon.png`.
#[must_use]
pub fn render_google_sign_in_html(
    name: &str,
    email: &str,
    login_url: &str,
    base_url: &str,
) -> String {
    super::layout::render_email_html(
        &render_google_sign_in_body(name, email, login_url),
        base_url,
    )
}

#[cfg(test)]
mod tests {
    use super::{render_google_sign_in_body, render_google_sign_in_html, TEMPLATE};

    #[test]
    fn render_substitutes_every_token_and_drops_frontmatter() {
        let body =
            render_google_sign_in_body("Nick", "nick@neonlaw.com", "https://app.test/auth/login");
        assert!(!body.starts_with("---"), "frontmatter must be stripped");
        assert!(body.contains("Nick"));
        assert!(body.contains("nick@neonlaw.com"));
        assert!(
            body.contains("Google"),
            "names Google as the sign-in method"
        );
        assert!(body.contains("https://app.test/auth/login"));
        assert!(
            !body.contains("{{"),
            "no `{{{{` placeholder may survive: {body}"
        );
    }

    #[test]
    fn render_carries_no_reset_link() {
        let body =
            render_google_sign_in_body("Nick", "nick@neonlaw.com", "https://app.test/auth/login");
        assert!(
            !body.contains("/auth/password/reset/new"),
            "a Google account is never offered a reset link"
        );
    }

    #[test]
    fn html_wraps_body_with_firm_logo_and_login_link() {
        let html = render_google_sign_in_html(
            "Nick",
            "nick@neonlaw.com",
            "https://app.test/auth/login",
            "https://app.test",
        );
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("logo-neon.png"));
        assert!(html.contains("https://app.test/auth/login"));
    }

    #[test]
    fn template_subject_pins_the_default_brand_copy() {
        assert_eq!(TEMPLATE.subject, "Sign in to Neon Law with Google");
        assert!(TEMPLATE.raw.starts_with("---\n"));
    }
}
