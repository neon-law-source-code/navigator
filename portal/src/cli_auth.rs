//! Loopback-OAuth endpoints for the `navigator` CLI.
//!
//! These two routes let the CLI obtain a short-lived bearer credential
//! the same way `gcloud` / `restate` do — open a browser, authenticate
//! against the existing OIDC session, and land a token on a loopback
//! listener — without building a second auth system. The token the CLI
//! receives is the **same** HMAC-signed [`SessionData`] blob the browser
//! holds in its cookie; it is presented as `Authorization: Bearer` and
//! resolved back into a session by [`crate::auth::inject_bearer_session`].
//!
//! Routes (mounted at the router root, under the authentication-exempt
//! `/auth/*` prefix):
//!
//! - `GET /auth/cli/start?redirect=<loopback>&state=<nonce>` — requires
//!   an authenticated browser session (the cookie). Mints a fresh ~8h
//!   `SessionData` for that person and 302s to the loopback `redirect`
//!   with `?token=…&state=…`. An anonymous caller is bounced through the
//!   normal `/auth/login` flow first and returns here with a cookie. The
//!   `redirect` MUST be a `127.0.0.1`/`localhost` loopback URL — anything
//!   else is refused (open-redirect / token-exfiltration guard).
//! - `GET /auth/cli/whoami` — bearer-gated. Returns
//!   `{ "email", "role", "exp" }` so the CLI can verify a freshly minted
//!   token and record the identity + expiry in its credential file.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use serde::Deserialize;
use tower_cookies::Cookies;

use crate::session::{
    now_unix_secs, random_token_32, SessionData, SessionSource, SessionStore, CLI_SESSION_TTL_SECS,
    SESSION_COOKIE_NAME,
};

/// Query parameters for `GET /auth/cli/start`.
#[derive(Debug, Deserialize)]
pub struct StartQuery {
    /// The CLI's loopback callback, e.g. `http://127.0.0.1:54321/cb`.
    pub redirect: String,
    /// Opaque nonce the CLI generated and will verify on the callback.
    pub state: String,
}

/// Build the `/auth/cli/*` sub-router. The `whoami` route carries the
/// bearer-session layer so it can read the caller's `SessionData`;
/// `start` reads the browser cookie and is unaffected by it.
pub fn routes(sessions: SessionStore) -> Router {
    Router::new()
        .route("/auth/cli/start", get(start_get))
        .route("/auth/cli/whoami", get(whoami_get))
        .route_layer(axum::middleware::from_fn_with_state(
            sessions.clone(),
            crate::auth::inject_bearer_session,
        ))
        .with_state(sessions)
}

/// `GET /auth/cli/start` — mint a CLI bearer token for the
/// already-authenticated browser session and hand it to the loopback.
async fn start_get(
    State(sessions): State<SessionStore>,
    cookies: Cookies,
    Query(q): Query<StartQuery>,
) -> Response {
    // Guard the redirect target first: a non-loopback redirect would
    // exfiltrate the token to an arbitrary origin, so refuse it before
    // we even look at the session.
    if !is_loopback_redirect(&q.redirect) {
        return (
            StatusCode::BAD_REQUEST,
            "redirect must be a loopback (127.0.0.1 / localhost) http URL",
        )
            .into_response();
    }

    // Require an authenticated browser session. When absent, send the
    // human through the normal OIDC login and return them right back
    // here (cookie in hand) to finish minting.
    let Some(session) = cookies
        .get(SESSION_COOKIE_NAME)
        .and_then(|c| sessions.decode(c.value()))
    else {
        let return_to = format!(
            "/auth/cli/start?redirect={}&state={}",
            urlencode(&q.redirect),
            urlencode(&q.state),
        );
        return Redirect::to(&format!("/auth/login?return_to={}", urlencode(&return_to)))
            .into_response();
    };

    // Mint a fresh, SHORT-LIVED (1h) token for the SAME person: same
    // subject, email, person_id and role, but a new expiry, CSRF token,
    // and a `Cli` source marker. Carrying `person_id` is what makes a CLI
    // `approve-send` record the same `authored_by` provenance as the UI —
    // the binding act stays attributable to the lawyer. The tighter TTL
    // and the source tag bound and label this portable file credential.
    let token = sessions.encode(&SessionData {
        exp: now_unix_secs() + CLI_SESSION_TTL_SECS,
        csrf_token: random_token_32(),
        source: SessionSource::Cli,
        ..session.clone()
    });
    tracing::info!(
        target: "audit",
        event = "cli.token.minted",
        subject = %session.sub,
        person_id = ?session.person_id,
        ttl_secs = CLI_SESSION_TTL_SECS,
        "cli: minted a short-lived CLI bearer token",
    );

    let sep = if q.redirect.contains('?') { '&' } else { '?' };
    let location = format!(
        "{}{sep}token={}&state={}",
        q.redirect,
        urlencode(&token),
        urlencode(&q.state),
    );
    Redirect::to(&location).into_response()
}

/// `GET /auth/cli/whoami` — echo the bearer caller's identity so the CLI
/// can confirm the token works and persist `{email, role, exp}`.
async fn whoami_get(session: Option<Extension<SessionData>>) -> Response {
    let Some(Extension(session)) = session else {
        return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
    };
    Json(serde_json::json!({
        "email": session.email,
        "role": session.role.as_str(),
        "exp": session.exp,
    }))
    .into_response()
}

/// True when `redirect` is an `http://` URL whose host is a loopback
/// address (`127.0.0.1`, `localhost`, or `::1`). Hand-rolled rather than
/// pulling the `url` crate into `web`'s runtime deps: the check is small
/// and the failure mode (refuse a legitimate loopback) is safe.
#[must_use]
pub fn is_loopback_redirect(redirect: &str) -> bool {
    // Loopback is plain http by definition; https to a loopback makes no
    // sense and we don't want to accept `https://127.0.0.1.evil.com`.
    let Some(rest) = redirect.strip_prefix("http://") else {
        return false;
    };
    // Authority is everything up to the first `/`, `?`, or `#`.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    // Strip credentials if present (`user:pass@host`) — never expected
    // here, and their presence is reason enough to refuse.
    if authority.contains('@') {
        return false;
    }
    // Split host[:port]. IPv6 literals are bracketed (`[::1]:port`).
    let host = if let Some(after) = authority.strip_prefix('[') {
        // `[::1]:port` → take up to the closing bracket.
        match after.split_once(']') {
            Some((h, _)) => h,
            None => return false,
        }
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// Minimal percent-encoder for the query values we build (the token and
/// the loopback URL). Mirrors the OAuth module's encoder — RFC 3986
/// unreserved characters pass through, everything else is `%XX`.
fn urlencode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::is_loopback_redirect;

    #[test]
    fn accepts_loopback_hosts() {
        assert!(is_loopback_redirect("http://127.0.0.1:54321/cb"));
        assert!(is_loopback_redirect("http://127.0.0.1/cb"));
        assert!(is_loopback_redirect("http://localhost:8888/"));
        assert!(is_loopback_redirect("http://localhost"));
        assert!(is_loopback_redirect("http://[::1]:9000/cb"));
    }

    #[test]
    fn rejects_non_loopback_and_non_http() {
        // Public hosts.
        assert!(!is_loopback_redirect("http://evil.example/cb"));
        assert!(!is_loopback_redirect("http://10.0.0.5/cb"));
        // The classic "subdomain that starts with the loopback string".
        assert!(!is_loopback_redirect("http://127.0.0.1.evil.example/cb"));
        assert!(!is_loopback_redirect("http://localhost.evil.example/cb"));
        // https / other schemes.
        assert!(!is_loopback_redirect("https://127.0.0.1/cb"));
        assert!(!is_loopback_redirect("ftp://localhost/cb"));
        // Embedded credentials.
        assert!(!is_loopback_redirect("http://user@127.0.0.1/cb"));
        // Not a URL at all.
        assert!(!is_loopback_redirect("127.0.0.1:54321"));
        assert!(!is_loopback_redirect(""));
    }
}
