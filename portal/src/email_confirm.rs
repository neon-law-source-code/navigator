//! Email confirmation for email/password (non-Google) users — the other
//! half of "sign in with Google **or** confirm your email."
//!
//! Google sign-in carries `email_verified: true`, so those users never
//! reach here. A password user whose Identity Platform record reports the
//! address unverified is hard-gated by [`crate::oauth::complete_sign_in`]:
//! it calls [`gate_unverified`], which mints a single-use confirm token
//! ([`store::email_tokens`]), emails the link through SendGrid, and shows a
//! "check your inbox" page **instead of** a session. Clicking the link
//! ([`confirm`]) flips `emailVerified` in Identity Platform via the admin
//! door ([`crate::idp_admin`]); the next sign-in then carries
//! `email_verified: true` and succeeds.
//!
//! The token mechanics, CSRF helpers, throttle, and TTL are shared with
//! [`crate::password_reset`].

use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use chrono::{Duration, Utc};
use serde::Deserialize;
use store::email_tokens::PURPOSE_EMAIL_CONFIRM;
use tower_cookies::Cookies;
use uuid::Uuid;

use crate::oauth::{expired_cookie, AuthState};
use crate::password_reset::{
    mint_csrf, verify_csrf, ACCOUNT_CSRF_COOKIE_NAME, THROTTLE_SECS, TOKEN_TTL_MINUTES,
};
use crate::session::random_token_32;

const CSRF_ERROR: (StatusCode, &str) = (StatusCode::BAD_REQUEST, "invalid or missing CSRF token");

/// Build the email-confirm sub-router (state applied by the caller).
pub fn routes() -> Router<AuthState> {
    Router::new()
        .route("/auth/email/confirm", get(confirm))
        .route("/auth/email/confirm/resend", post(resend))
}

/// Called from the sign-in tail when a password user's address is
/// unverified: mint + email a confirmation link (throttled, best-effort),
/// then render the "check your inbox" gate page **in place of** a session.
/// Returns a `Response` so [`crate::oauth::complete_sign_in`] can early
/// return it.
pub async fn gate_unverified(
    s: &AuthState,
    cookies: &Cookies,
    person_id: Uuid,
    name: &str,
    email: &str,
) -> Response {
    if let Err(e) = try_send_confirm(s, person_id, name, email).await {
        tracing::warn!(error = %e, person_id = %person_id, "email-confirm: gate send failed");
    }
    let csrf = mint_csrf(s, cookies);
    webapp::auth_pages::confirm_email(email, &csrf).into_response()
}

/// Mint a confirm token and email the link, unless a live one was minted
/// inside the throttle window. Best-effort: a mail failure is logged, not
/// surfaced.
async fn try_send_confirm(
    s: &AuthState,
    person_id: Uuid,
    name: &str,
    email: &str,
) -> anyhow::Result<()> {
    if email.is_empty() {
        return Ok(());
    }
    let now = Utc::now();
    if store::email_tokens::has_live_token_since(
        &s.surreal,
        person_id,
        PURPOSE_EMAIL_CONFIRM,
        now - Duration::seconds(THROTTLE_SECS),
        now,
    )
    .await?
    {
        return Ok(());
    }

    let plaintext = random_token_32();
    store::email_tokens::mint(
        &s.surreal,
        person_id,
        email,
        PURPOSE_EMAIL_CONFIRM,
        &plaintext,
        now + Duration::minutes(TOKEN_TTL_MINUTES),
    )
    .await?;

    let base_url = workflows::email::base_url_from_env();
    let confirm_url = format!(
        "{}/auth/email/confirm?token={}",
        base_url.trim_end_matches('/'),
        plaintext,
    );
    let body =
        workflows::email::email_confirm::render_email_confirm_body(name, email, &confirm_url);
    let html = workflows::email::email_confirm::render_email_confirm_html(
        name,
        email,
        &confirm_url,
        &base_url,
    );
    let mut msg = crate::email::OutboundEmail::new(
        email.to_string(),
        workflows::email::email_confirm::email_confirm_subject(),
        body,
    );
    msg.html_body = Some(html);
    msg.template_slug = Some(PURPOSE_EMAIL_CONFIRM.to_string());
    msg.person_id = Some(person_id.to_string());
    if let Err(e) = s.email.send(msg).await {
        tracing::warn!(error = %e, person_id = %person_id, "email-confirm: email send failed");
    }
    Ok(())
}

#[derive(Deserialize)]
struct TokenQuery {
    #[serde(default)]
    token: String,
}

/// Claim a confirmation link: validate the token, flip `emailVerified` in
/// Identity Platform, spend the token, and send the user to sign in.
async fn confirm(State(s): State<AuthState>, Query(q): Query<TokenQuery>) -> Response {
    let now = Utc::now();
    let token_row =
        match store::email_tokens::validate(&s.surreal, &q.token, PURPOSE_EMAIL_CONFIRM, now).await
        {
            Ok(Some(row)) => row,
            Ok(None) => return webapp::auth_pages::invalid_link().into_response(),
            Err(e) => {
                tracing::warn!(error = %e, "email-confirm: validate failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    webapp::error_pages::server_error(),
                )
                    .into_response();
            }
        };

    let Some(admin) = s.identity_admin.as_ref() else {
        tracing::warn!("email-confirm: no admin config; cannot mark verified");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            webapp::error_pages::server_error(),
        )
            .into_response();
    };

    let local_id = match admin.lookup_by_email(&token_row.email).await {
        Ok(Some(info)) => info.local_id,
        Ok(None) => return webapp::auth_pages::invalid_link().into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "email-confirm: lookup failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response();
        }
    };
    if let Err(e) = admin.set_email_verified(&local_id).await {
        tracing::warn!(error = %e, "email-confirm: set_email_verified failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            webapp::error_pages::server_error(),
        )
            .into_response();
    }

    // Best-effort, same as the OIDC callback's own write: a failure here
    // must not block the confirmation the person just completed.
    if let Err(e) = store::persons::set_email_confirmed(&s.surreal, token_row.person_id, true).await
    {
        tracing::warn!(error = %e, person_id = %token_row.person_id, "email-confirm: set_email_confirmed failed");
    }

    if let Err(e) = store::email_tokens::consume(&s.surreal, token_row.id, now).await {
        tracing::warn!(error = %e, "email-confirm: token consume failed (email was verified)");
    }
    tracing::info!(person_id = %token_row.person_id, "email-confirm: email verified");
    Redirect::to("/auth/login?notice=email_confirmed").into_response()
}

#[derive(Deserialize)]
struct ResendForm {
    email: String,
    #[serde(default)]
    csrf_token: String,
}

/// Re-send a confirmation link from the gate page. Neutral: always
/// re-renders the same "check your inbox" page so it can't be used to
/// probe which addresses exist.
async fn resend(
    State(s): State<AuthState>,
    cookies: Cookies,
    Form(form): Form<ResendForm>,
) -> Response {
    if !verify_csrf(&s, &cookies, &form.csrf_token) {
        return CSRF_ERROR.into_response();
    }
    cookies.add(expired_cookie(ACCOUNT_CSRF_COOKIE_NAME));

    let email = form.email.trim();
    match crate::password_reset::find_person_by_email(&s.surreal, email).await {
        Ok(Some(person)) => {
            if let Err(e) = try_send_confirm(&s, person.id, &person.name, &person.email).await {
                tracing::warn!(error = %e, "email-confirm: resend failed");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "email-confirm: resend person lookup failed"),
    }

    let csrf = mint_csrf(&s, &cookies);
    webapp::auth_pages::confirm_email(email, &csrf).into_response()
}
