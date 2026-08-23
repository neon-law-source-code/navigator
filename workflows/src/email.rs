//! Email templates rendered by the workflow worker.
//!
//! `email_send__<slug>` steps in a workflow spec resolve `<slug>` to
//! a [`Template`] in this module. The `workflows-service` worker
//! looks up the template by slug, renders the body with the
//! invocation's payload, and hands the result to its injected
//! [`service::EmailService`]. Both `web` (for non-workflow direct sends,
//! e.g. the admin "Send welcome" button) and `workflows-service` (for
//! the durable workflow path) read the same template source so a copy
//! change shows up everywhere at once.

/// A renderable email template.
///
/// Held by value (no `dyn`) because the set of templates is fixed at
/// compile time — `welcome` today, password-reset / signature-ready /
/// etc. as they're added. Lookups by slug return `Option<Template>`.
#[derive(Debug, Clone, Copy)]
pub struct Template {
    /// Subject line; mirrored from the markdown source's
    /// `subject:` frontmatter.
    pub subject: &'static str,
    /// Raw template body — markdown with YAML frontmatter and
    /// `{{placeholder}}` slots.
    pub raw: &'static str,
}

pub mod certificate;
pub mod dispatch;
pub mod email_confirm;
pub mod google_sign_in;
pub mod layout;
pub mod password_reset;
pub(crate) mod sendgrid_openapi;
pub mod service;
pub mod welcome;

pub use dispatch::{dispatch_state, parse_slug, DispatchError, EmailPayload};
pub use layout::{base_url_from_env, brand_name, render_email_html, support_email};
pub use service::{
    Attachment, CapturingEmail, EmailError, EmailService, OutboundEmail, SendGridEmail,
    SendReceipt, DEFAULT_FROM_EMAIL,
};

/// Look up an email template by `email_send__<slug>` slug.
///
/// `email_send__welcome` → `Some(WELCOME)`; unknown slugs return
/// `None` so the worker can refuse the step with a clear error
/// rather than guessing.
#[must_use]
pub fn template_for_slug(slug: &str) -> Option<Template> {
    match slug {
        "welcome" => Some(welcome::TEMPLATE),
        "certificate" => Some(certificate::TEMPLATE),
        // `password_reset` and `email_confirm` are sent directly from
        // `web` (the body carries a per-request, single-use URL the
        // dispatcher's name+email payload can't supply), so they are not
        // wired into `dispatch::render_for_slug`. The entries here exist
        // so a slug → subject lookup and the `sent_emails` audit join
        // still resolve for them.
        "password_reset" => Some(password_reset::TEMPLATE),
        "email_confirm" => Some(email_confirm::TEMPLATE),
        "google_sign_in" => Some(google_sign_in::TEMPLATE),
        _ => None,
    }
}

/// Strip a leading `---\n…\n---\n` YAML frontmatter block from a
/// markdown template. Returns the original string unchanged if the
/// frontmatter markers aren't present.
pub(crate) fn strip_frontmatter(markdown: &str) -> &str {
    let Some(after_open) = markdown.strip_prefix("---\n") else {
        return markdown;
    };
    match after_open.find("\n---\n") {
        Some(end) => after_open[end + 5..].trim_start_matches('\n'),
        None => markdown,
    }
}

#[cfg(test)]
mod tests {
    use super::{strip_frontmatter, template_for_slug};

    #[test]
    fn template_for_welcome_slug_resolves() {
        let tpl = template_for_slug("welcome").expect("welcome template must resolve");
        assert_eq!(tpl.subject, "Welcome to Neon Law");
        assert!(
            tpl.raw.starts_with("---\n"),
            "raw body includes frontmatter"
        );
    }

    #[test]
    fn template_for_unknown_slug_is_none() {
        assert!(template_for_slug("does-not-exist").is_none());
    }

    #[test]
    fn strip_frontmatter_removes_yaml_block() {
        let stripped = strip_frontmatter("---\nsubject: x\n---\nbody\n");
        assert_eq!(stripped, "body\n");
    }

    #[test]
    fn strip_frontmatter_passthrough_when_absent() {
        assert_eq!(strip_frontmatter("just a body"), "just a body");
    }
}
