//! Bridge from `web`'s auth layer to `mcp`'s [`mcp::Principal`].
//!
//! Both `require_google_oauth` (production) and `require_auth`
//! (KIND HS256/JWKS) leave a verified [`crate::auth::AuthClaims`]
//! in request extensions. The MCP dispatcher reads
//! `Option<Extension<mcp::Principal>>` — this middleware translates
//! one into the other so the tools see a typed, trusted email
//! without knowing about JWT internals.
//!
//! The translation stays conservative about what counts as a trusted
//! email, and there are exactly two sources of one.
//!
//! A **first-party session** — the bearer `navigator site login` stores,
//! resolved by [`crate::auth::inject_bearer_session`] — carries a typed
//! `email` field set by the OIDC login that minted it. That is read
//! directly, because the field says what it is.
//!
//! Otherwise, when `require_google_oauth` is enforced, the claims' `sub`
//! holds the OAuth-verified email (`google_oauth.rs` does the
//! assignment). In the bare HS256/JWKS path the `sub` is whatever the IdP
//! put there — often a user id, not an email — so we still don't pretend
//! it's trusted email, and no `Principal` is inserted.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::AuthClaims;
use crate::google_oauth::GoogleOauthConfig;

/// Axum middleware. Run on the `/mcp` route AFTER
/// `require_google_oauth` + `require_auth` so any `AuthClaims`
/// have already been populated.
pub async fn inject_principal(
    axum::extract::State(google_oauth): axum::extract::State<GoogleOauthConfig>,
    mut req: Request,
    next: Next,
) -> Response {
    // A first-party session carries a typed `email` field, so when one is
    // present it is the better source — no guessing what an IdP put in
    // `sub`. `inject_bearer_session` inserts a `SessionData` only for a
    // blob whose HMAC this deployment signed, and the email on it is the
    // one the OIDC login resolved, so it is trusted on the same footing
    // as a tokeninfo-verified address.
    //
    // Checked before the enforcement branch deliberately: the `navigator`
    // CLI's credential is not a Google token, so on a deployment where
    // Google OAuth is enforced the branch below would find claims whose
    // `sub` came from the session rather than from tokeninfo. Reading the
    // explicit `email` instead makes the source of the identity plain
    // rather than incidental.
    if let Some(email) = req
        .extensions()
        .get::<crate::session::SessionData>()
        .and_then(|s| s.email.clone())
    {
        req.extensions_mut().insert(mcp::Principal::new(email));
        return next.run(req).await;
    }
    if google_oauth.is_enforced() {
        if let Some(claims) = req.extensions().get::<AuthClaims>().cloned() {
            req.extensions_mut().insert(mcp::Principal::new(claims.sub));
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::inject_principal;
    use crate::auth::AuthClaims;
    use crate::google_oauth::GoogleOauthConfig;
    use crate::session::{SessionData, SessionSource};
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use store::persons::Role;
    use tower::ServiceExt;

    const ALLOWED_CLIENT: &str = "123-abc.apps.googleusercontent.com";

    /// Reports the injected principal's email, or `none`.
    async fn echo(principal: Option<Extension<mcp::Principal>>) -> String {
        principal.map_or_else(|| "none".to_string(), |p| p.email.clone())
    }

    fn app(cfg: GoogleOauthConfig) -> Router {
        Router::new()
            .route("/probe", get(echo))
            .route_layer(axum::middleware::from_fn_with_state(cfg, inject_principal))
    }

    async fn probe(
        cfg: GoogleOauthConfig,
        session: Option<SessionData>,
        claims: Option<AuthClaims>,
    ) -> String {
        let mut req = Request::builder()
            .uri("/probe")
            .body(Body::empty())
            .unwrap();
        if let Some(s) = session {
            req.extensions_mut().insert(s);
        }
        if let Some(c) = claims {
            req.extensions_mut().insert(c);
        }
        let resp = app(cfg).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn cli_session(email: &str) -> SessionData {
        let mut s = SessionData::fresh(email, Role::Lawyer);
        s.email = Some(email.to_string());
        s.source = SessionSource::Cli;
        s
    }

    fn claims(sub: &str) -> AuthClaims {
        AuthClaims {
            sub: sub.to_string(),
            exp: i64::MAX,
            role: Role::Lawyer,
        }
    }

    #[tokio::test]
    async fn a_first_party_session_supplies_the_principal_from_its_email_field() {
        // The `navigator site mcp` path. Without this the CLI reaches the
        // endpoint but every side-effecting tool refuses for want of a
        // principal.
        assert_eq!(
            probe(
                GoogleOauthConfig::passthrough(),
                Some(cli_session("lawyer@neonlaw.com")),
                None
            )
            .await,
            "lawyer@neonlaw.com"
        );
    }

    #[tokio::test]
    async fn the_session_email_wins_over_jwt_claims() {
        // Both present: read the typed field, not whatever `sub` holds.
        // On an enforced deployment the CLI's own claims come from the
        // session, so preferring `email` keeps the identity's source
        // explicit rather than incidental.
        assert_eq!(
            probe(
                GoogleOauthConfig::for_test(
                    [ALLOWED_CLIENT],
                    Some("neonlaw.com"),
                    "http://127.0.0.1:1/tokeninfo".to_string()
                ),
                Some(cli_session("lawyer@neonlaw.com")),
                Some(claims("a-user-id-not-an-email"))
            )
            .await,
            "lawyer@neonlaw.com"
        );
    }

    #[tokio::test]
    async fn google_enforced_still_reads_the_verified_email_from_claims() {
        // The Google-OAuth agent path: no session,
        // enforcement on, `sub` is the tokeninfo-verified address.
        assert_eq!(
            probe(
                GoogleOauthConfig::for_test(
                    [ALLOWED_CLIENT],
                    Some("neonlaw.com"),
                    "http://127.0.0.1:1/tokeninfo".to_string()
                ),
                None,
                Some(claims("agent@neonlaw.com"))
            )
            .await,
            "agent@neonlaw.com"
        );
    }

    #[tokio::test]
    async fn a_bare_jwt_with_no_session_injects_nothing() {
        // KIND's HS256/JWKS path with enforcement off: `sub` may be an
        // IdP user id rather than an email, so it is not passed off as a
        // trusted address.
        assert_eq!(
            probe(
                GoogleOauthConfig::passthrough(),
                None,
                Some(claims("some-idp-user-id"))
            )
            .await,
            "none"
        );
    }

    #[tokio::test]
    async fn a_session_without_an_email_injects_nothing() {
        // `SessionData::email` is optional. An anonymous or email-less
        // session must not become a principal named by the empty string.
        let mut s = SessionData::fresh("sub-only", Role::Lawyer);
        s.email = None;
        s.source = SessionSource::Cli;
        assert_eq!(
            probe(GoogleOauthConfig::passthrough(), Some(s), None).await,
            "none"
        );
    }
}
