//! Sliding-window session renewal.
//!
//! The browser session cookie carries a fixed `exp` ([`DEFAULT_SESSION_TTL_SECS`]
//! from mint time). Without renewal an active user is signed out at that
//! hard wall mid-task even though they never left. This middleware
//! re-issues the cookie on activity, sliding `exp` forward by the full
//! TTL — so only a *genuinely idle* session (no request for the whole
//! TTL) ever expires.
//!
//! To avoid emitting a `Set-Cookie` on every single request, renewal
//! only fires once a session is past the half-way point of its
//! lifetime (see [`should_renew`]). CLI bearer tokens
//! ([`SessionSource::Cli`]) are left untouched — they are file
//! credentials with their own tighter TTL, not a cookie we can refresh.
//!
//! Layered *inside* `tower_cookies::CookieManagerLayer` (the innermost
//! application layer) so the added cookie is serialized into the
//! response's `Set-Cookie` header.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use tower_cookies::Cookies;

use crate::session::{
    now_unix_secs, SessionData, SessionSource, SessionStore, DEFAULT_SESSION_TTL_SECS,
    SESSION_COOKIE_NAME,
};

/// Middleware state: the signing store plus whether to set the `Secure`
/// flag on the re-issued cookie (mirrors `oauth::AuthState::secure_cookies`).
#[derive(Clone)]
pub struct RenewState {
    pub sessions: SessionStore,
    pub secure: bool,
}

/// True when an active browser session is far enough into its lifetime
/// to warrant sliding `exp` forward. Renews only browser cookies and
/// only in the second half of the TTL, so a steadily-active user keeps
/// their session indefinitely without a `Set-Cookie` on every request.
#[must_use]
pub fn should_renew(data: &SessionData, now: i64) -> bool {
    data.source == SessionSource::Browser && (data.exp - now) < DEFAULT_SESSION_TTL_SECS / 2
}

pub async fn renew_session(
    State(st): State<RenewState>,
    cookies: Cookies,
    req: Request,
    next: Next,
) -> Response {
    if let Some(c) = cookies.get(SESSION_COOKIE_NAME) {
        // `decode` already rejects expired / tampered cookies, so a
        // value here is a live, valid session.
        if let Some(mut data) = st.sessions.decode(c.value()) {
            if should_renew(&data, now_unix_secs()) {
                data.exp = now_unix_secs() + DEFAULT_SESSION_TTL_SECS;
                cookies.add(crate::oauth::session_cookie(
                    st.sessions.encode(&data),
                    st.secure,
                ));
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::should_renew;
    use crate::session::{now_unix_secs, SessionData, SessionSource, DEFAULT_SESSION_TTL_SECS};
    use store::persons::Role;

    fn session(source: SessionSource, exp: i64) -> SessionData {
        SessionData {
            sub: "sub-123".into(),
            email: None,
            person_id: None,
            exp,
            role: Role::Client,
            csrf_token: "csrf".into(),
            source,
            provider: None,
            impersonation: None,
        }
    }

    #[test]
    fn fresh_browser_session_is_not_renewed() {
        let now = now_unix_secs();
        // Just minted: a full TTL remaining — still in the first half.
        let s = session(SessionSource::Browser, now + DEFAULT_SESSION_TTL_SECS);
        assert!(!should_renew(&s, now));
    }

    #[test]
    fn aged_browser_session_is_renewed() {
        let now = now_unix_secs();
        // Three-quarters elapsed: well into the second half.
        let s = session(SessionSource::Browser, now + DEFAULT_SESSION_TTL_SECS / 4);
        assert!(should_renew(&s, now));
    }

    #[test]
    fn cli_session_is_never_renewed() {
        let now = now_unix_secs();
        // Even deep into its life, a CLI bearer is not a cookie to slide.
        let s = session(SessionSource::Cli, now + DEFAULT_SESSION_TTL_SECS / 4);
        assert!(!should_renew(&s, now));
    }
}
