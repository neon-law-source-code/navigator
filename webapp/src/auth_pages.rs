//! Server-rendered public authentication pages.
//!
//! These handlers sit outside a Dioxus router, so they render through the
//! standalone document seam used by the error pages while retaining native,
//! pre-hydration form submissions.

use axum::response::Html;
use dioxus::prelude::*;

use crate::components::{PublicShell, SiteHeader, SiteNavLink};
use crate::public_chrome::{firm_public_chrome, PublicChrome, PublicFooter};

/// One sign-in button on the chooser: where it goes and what it says.
///
/// The caller owns both, because which providers exist is a deployment fact
/// (`portal::oauth::AuthState::configured_providers`) and this crate renders
/// pages rather than deciding policy.
#[derive(Clone, PartialEq, Eq)]
pub struct SignInProvider {
    /// Absolute path including `?return_to=`, e.g.
    /// `/auth/login/microsoft?return_to=/app/projects`.
    pub href: String,
    /// The button text, e.g. [`MICROSOFT_SIGN_IN`].
    pub label: String,
}

#[derive(Clone, PartialEq, Eq)]
pub enum LoginNotice {
    Danger(String),
    Success(String),
}

impl LoginNotice {
    fn class(&self) -> &'static str {
        match self {
            Self::Danger(_) => "nav-alert nav-alert--danger",
            Self::Success(_) => "nav-alert nav-alert--success",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Danger(message) | Self::Success(message) => message,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum Page {
    Login {
        return_to: String,
        csrf: String,
        /// Every configured identity provider, in render order. Empty renders
        /// no buttons at all, which is what a password-only deployment wants.
        providers: Vec<SignInProvider>,
        /// Whether the email/password form renders. Off for a deployment whose
        /// only doors are identity providers, so the page does not offer a
        /// credential nothing will accept.
        password_enabled: bool,
        error: Option<String>,
        notice: Option<LoginNotice>,
    },
    ResetRequest {
        csrf: String,
        error: Option<String>,
    },
    ResetNew {
        token: String,
        csrf: String,
        error: Option<String>,
    },
    ResetSent,
    InvalidLink,
    ConfirmEmail {
        email: String,
        csrf: String,
    },
}

pub fn login(
    return_to: &str,
    csrf: &str,
    providers: &[SignInProvider],
    password_enabled: bool,
    error: Option<&str>,
    notice: Option<LoginNotice>,
) -> Html<String> {
    render(
        "Sign in",
        Page::Login {
            return_to: return_to.into(),
            csrf: csrf.into(),
            providers: providers.to_vec(),
            password_enabled,
            error: error.map(str::to_string),
            notice,
        },
    )
}

pub fn password_reset_request(csrf: &str, error: Option<&str>) -> Html<String> {
    render(
        "Reset your password",
        Page::ResetRequest {
            csrf: csrf.into(),
            error: error.map(str::to_string),
        },
    )
}

pub fn password_reset_new(token: &str, csrf: &str, error: Option<&str>) -> Html<String> {
    render(
        "Choose a new password",
        Page::ResetNew {
            token: token.into(),
            csrf: csrf.into(),
            error: error.map(str::to_string),
        },
    )
}

pub fn password_reset_sent() -> Html<String> {
    render("Check your inbox", Page::ResetSent)
}
pub fn invalid_link() -> Html<String> {
    render("This link is no longer valid", Page::InvalidLink)
}
pub fn confirm_email(email: &str, csrf: &str) -> Html<String> {
    render(
        "Confirm your email",
        Page::ConfirmEmail {
            email: email.into(),
            csrf: csrf.into(),
        },
    )
}

fn render(title: &str, page: Page) -> Html<String> {
    let chrome = firm_public_chrome(vec![]);
    let brand_name = chrome.brand_name.clone();
    let mut dom = VirtualDom::new_with_props(AuthPage, AuthPageProps { page, chrome });
    dom.rebuild_in_place();
    Html(crate::error_pages::standalone_document(
        title,
        &brand_name,
        &dioxus_ssr::render(&dom),
    ))
}

/// The OIDC button label on the sign-in chooser.
///
/// Named so the covering test asserts the same string the page renders
/// rather than a second copy of it that can drift.
pub const GOOGLE_SIGN_IN: &str = "Sign in with Google";

/// The Microsoft button label on the sign-in chooser.
///
/// Microsoft's branding guidance for apps requires the words "Sign in with
/// Microsoft" and forbids showing end users the "Azure" or "Active Directory"
/// brands, so the wording is fixed here rather than derived from config.
pub const MICROSOFT_SIGN_IN: &str = "Sign in with Microsoft";

#[derive(Props, Clone, PartialEq)]
struct AuthPageProps {
    page: Page,
    chrome: PublicChrome,
}

#[component]
fn AuthPage(props: AuthPageProps) -> Element {
    let header = header(&props.chrome);
    let footer = footer(&props.chrome);
    rsx! { PublicShell { header, footer,
        section { class: "auth-page",
            div { class: "nav-card auth-card",
                match props.page {
                    Page::Login { return_to, csrf, providers, password_enabled, error, notice } => rsx! {
                        h1 { "Sign in" }
                        if let Some(notice) = notice { p { class: "{notice.class()}", "{notice.message()}" } }
                        if let Some(error) = error { p { class: "nav-alert nav-alert--danger", "{error}" } }
                        if password_enabled {
                            form { method: "post", action: "/auth/password",
                                input { r#type: "hidden", name: "return_to", value: "{return_to}" }
                                input { r#type: "hidden", name: "csrf_token", value: "{csrf}" }
                                label { "Email" input { r#type: "email", name: "email", required: true } }
                                label { "Password" input { r#type: "password", name: "password", required: true } }
                                button { r#type: "submit", class: "nav-btn nav-btn--primary", "Sign in" }
                            }
                        }
                        for provider in providers.iter() {
                            p { a { class: "nav-btn nav-btn--secondary", href: "{provider.href}", "{provider.label}" } }
                        }
                        if password_enabled { p { a { href: "/auth/password/reset", "Forgot your password?" } } }
                    },
                    Page::ResetRequest { csrf, error } => rsx! {
                        h1 { "Reset your password" }
                        p { "Enter the email address for your account and we'll send you a link to choose a new password. The link expires in 30 minutes." }
                        if let Some(error) = error { p { class: "nav-alert nav-alert--danger", "{error}" } }
                        form { method: "post", action: "/auth/password/reset",
                            input { r#type: "hidden", name: "csrf_token", value: "{csrf}" }
                            label { "Email" input { r#type: "email", name: "email", required: true } }
                            button { r#type: "submit", class: "nav-btn nav-btn--primary", "Email me a reset link" }
                        }
                        p { a { href: "/auth/login", "Back to sign in" } }
                    },
                    Page::ResetNew { token, csrf, error } => rsx! {
                        h1 { "Choose a new password" }
                        p { "Choose a new password. Use at least 8 characters." }
                        if let Some(error) = error { p { class: "nav-alert nav-alert--danger", "{error}" } }
                        form { method: "post", action: "/auth/password/reset/new",
                            input { r#type: "hidden", name: "token", value: "{token}" }
                            input { r#type: "hidden", name: "csrf_token", value: "{csrf}" }
                            label { "New password" input { r#type: "password", name: "password", required: true } }
                            label { "Confirm new password" input { r#type: "password", name: "confirm", required: true } }
                            button { r#type: "submit", class: "nav-btn nav-btn--primary", "Set new password" }
                        }
                    },
                    Page::ResetSent => rsx! { h1 { "Check your inbox" } p { "If an account exists for that email, we've sent a link to reset its password." } p { a { href: "/auth/password/reset", "Request another reset link" } } },
                    Page::InvalidLink => rsx! { h1 { "This link is no longer valid" } p { "This link has expired or has already been used. Reset links can be used once and are good for 30 minutes." } p { a { href: "/auth/password/reset", "Request a new reset link" } } },
                    Page::ConfirmEmail { email, csrf } => rsx! {
                        h1 { "Confirm your email" }
                        p { "Almost there — please confirm your email address before signing in." }
                        p { "We've sent a confirmation link to your inbox. Click it and you'll be able to sign in. The link expires in 30 minutes." }
                        form { method: "post", action: "/auth/email/confirm/resend",
                            input { r#type: "hidden", name: "email", value: "{email}" }
                            input { r#type: "hidden", name: "csrf_token", value: "{csrf}" }
                            button { r#type: "submit", class: "nav-btn nav-btn--primary", "Resend confirmation email" }
                        }
                    },
                }
            }
        }
    } }
}

fn header(chrome: &PublicChrome) -> Element {
    rsx! { SiteHeader { brand_name: chrome.brand_name.clone(), home_href: chrome.home_href.clone(), logo_href: chrome.logo_href.clone(), destinations: chrome.destinations.iter().map(|link| SiteNavLink::new(link.label.clone(), link.href.clone())).collect(), utility: chrome.utility.iter().map(|link| SiteNavLink::new(link.label.clone(), link.href.clone())).collect() } }
}
fn footer(chrome: &PublicChrome) -> Element {
    rsx! { PublicFooter { chrome: chrome.clone() } }
}

#[cfg(test)]
mod tests {
    use super::{
        confirm_email, invalid_link, login, password_reset_new, password_reset_request,
        LoginNotice, SignInProvider, GOOGLE_SIGN_IN, MICROSOFT_SIGN_IN,
    };

    fn provider(slug: &str, label: &str) -> SignInProvider {
        SignInProvider {
            href: format!("/auth/login/{slug}?return_to=/app/projects"),
            label: label.to_string(),
        }
    }

    #[test]
    fn login_keeps_the_native_password_and_oidc_contracts() {
        let html = login(
            "/app/projects",
            "CSRF",
            &[provider("oidc", GOOGLE_SIGN_IN)],
            true,
            Some("Try again"),
            Some(LoginNotice::Danger("Sign in required".into())),
        )
        .0;
        for needle in [
            "action=\"/auth/password\"",
            "name=\"return_to\" value=\"/app/projects\"",
            "name=\"csrf_token\" value=\"CSRF\"",
            "name=\"email\"",
            "name=\"password\"",
            "/auth/login/oidc?return_to=/app/projects",
            "Sign in required",
            "Try again",
        ] {
            assert!(html.contains(needle), "missing {needle}: {html}");
        }
        assert!(
            html.contains(GOOGLE_SIGN_IN),
            "missing {GOOGLE_SIGN_IN}: {html}"
        );
    }

    /// Two providers render two buttons, each to its own
    /// `/auth/login/{provider}` route, in the order given. The order is
    /// asserted because a signed-out person navigates the page by button
    /// position, and switching a provider on must not shuffle it.
    #[test]
    fn login_renders_one_button_per_configured_provider_in_order() {
        let html = login(
            "/app/projects",
            "CSRF",
            &[
                provider("oidc", GOOGLE_SIGN_IN),
                provider("microsoft", MICROSOFT_SIGN_IN),
            ],
            false,
            None,
            None,
        )
        .0;
        let google = html
            .find(GOOGLE_SIGN_IN)
            .unwrap_or_else(|| panic!("missing {GOOGLE_SIGN_IN}: {html}"));
        let microsoft = html
            .find(MICROSOFT_SIGN_IN)
            .unwrap_or_else(|| panic!("missing {MICROSOFT_SIGN_IN}: {html}"));
        assert!(google < microsoft, "primary provider must render first");
        assert!(html.contains("/auth/login/oidc?return_to=/app/projects"));
        assert!(html.contains("/auth/login/microsoft?return_to=/app/projects"));
    }

    /// With no password door there is no credential form and no reset link —
    /// offering either would invite a password nothing will accept.
    #[test]
    fn login_without_a_password_door_renders_no_credential_form() {
        let html = login(
            "/app/projects",
            "CSRF",
            &[provider("oidc", GOOGLE_SIGN_IN)],
            false,
            None,
            None,
        )
        .0;
        assert!(!html.contains("action=\"/auth/password\""), "{html}");
        assert!(!html.contains("/auth/password/reset"), "{html}");
        assert!(html.contains(GOOGLE_SIGN_IN), "{html}");
    }

    #[test]
    fn recovery_forms_preserve_their_tokens_and_actions() {
        let request = password_reset_request("REQUEST", None).0;
        assert!(request.contains("action=\"/auth/password/reset\""));
        assert!(request.contains("name=\"csrf_token\" value=\"REQUEST\""));
        let new_password = password_reset_new("RESET", "NEW", Some("Too short")).0;
        assert!(new_password.contains("action=\"/auth/password/reset/new\""));
        assert!(new_password.contains("name=\"token\" value=\"RESET\""));
        assert!(new_password.contains("name=\"csrf_token\" value=\"NEW\""));
        assert!(new_password.contains("Too short"));
    }

    #[test]
    fn confirmation_and_invalid_link_remain_complete_documents() {
        let confirmation = confirm_email("libra@example.com", "CONFIRM").0;
        assert!(confirmation.starts_with("<!DOCTYPE html>"));
        assert!(confirmation.contains("action=\"/auth/email/confirm/resend\""));
        assert!(confirmation.contains("name=\"email\" value=\"libra@example.com\""));
        let invalid = invalid_link().0;
        assert!(invalid.contains("Request a new reset link"));
    }
}
