//! Google OAuth 2.0 access-token validator for `/mcp`.
//!
//! Why this exists instead of Identity-Aware Proxy: Google IAP
//! requires a JWT-shaped ID token (`eyJ...`) on incoming requests,
//! but Gemini Enterprise's Custom MCP Server data store sends the
//! standard *opaque* OAuth 2.0 access token (`ya29....`) instead.
//! IAP responds `"Invalid IAP credentials: Unable to parse JWT"` and
//! the request never reaches the pod. To accept what Gemini
//! actually sends, we drop IAP at the LB and validate the access
//! token in-process via Google's `tokeninfo` endpoint.
//!
//! Validation rules (env-driven, all required for "enforced"):
//!
//! - `GOOGLE_OAUTH_CLIENT_IDS` — comma-separated allowlist of OAuth
//!   client IDs (with or without the `.apps.googleusercontent.com`
//!   suffix). The token's `aud` / `azp` must match one of them. This
//!   is the equivalent of IAP's `programmaticClients` allowlist.
//! - `GOOGLE_OAUTH_REQUIRED_HD` — Workspace domain (e.g.
//!   `example.com`). The token's `email` suffix must match, and
//!   `email_verified` must be true.
//!
//! When `GOOGLE_OAUTH_CLIENT_IDS` is unset the middleware is a
//! pass-through (KIND / local dev). The Bearer JWT path through
//! `require_auth` continues to work for in-cluster smoke tests.
//!
//! Endpoint reference:
//! <https://oauth2.googleapis.com/tokeninfo?access_token=ACCESS_TOKEN>
//! returns a JSON body with `aud`, `azp`, `sub`, `email`,
//! `email_verified`, `exp`, `scope`. We trust the response on HTTP
//! 200; any other status (including 400 for expired / revoked
//! tokens) is treated as a rejection.

use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde::Deserialize;

use crate::audit_fields::{domain_of, person_id_field};
use crate::auth::AuthClaims;

/// Google's tokeninfo endpoint. Overridable via
/// `GOOGLE_TOKENINFO_URL` in tests.
pub const DEFAULT_TOKENINFO_URL: &str = "https://oauth2.googleapis.com/tokeninfo";

/// Middleware configuration. `Clone`-cheap (single `Arc`).
#[derive(Clone)]
pub struct GoogleOauthConfig(Arc<GoogleOauthConfigInner>);

struct GoogleOauthConfigInner {
    /// `None` ⇒ middleware is a pass-through. Populated when
    /// `GOOGLE_OAUTH_CLIENT_IDS` env is set.
    allowed_client_ids: Option<HashSet<String>>,
    /// Optional Workspace domain enforcement (`@<this>` email
    /// suffix). `None` ⇒ no domain check.
    required_hd: Option<String>,
    /// Tokeninfo endpoint. Overridable in tests.
    tokeninfo_url: String,
    /// HTTP client; pooled connections to tokeninfo cost ~50ms.
    http: reqwest::Client,
    /// SurrealDB handle, wired at mount time via
    /// [`GoogleOauthConfig::with_db`]. Used to resolve the verified email
    /// to its **real** `persons.role` — the table moved to this engine
    /// with ENG-19 — rather than stamping every validated token as lawyer.
    /// `None` only in the pass-through / unit-test configs (which never
    /// reach the role-resolution path).
    surreal: Option<store::surreal::SurrealDb>,
}

impl GoogleOauthConfig {
    /// Build from environment. Returns a pass-through config when
    /// `GOOGLE_OAUTH_CLIENT_IDS` is unset (the dev / KIND case).
    #[must_use]
    pub fn from_env() -> Self {
        let allowed_client_ids = std::env::var("GOOGLE_OAUTH_CLIENT_IDS")
            .ok()
            .map(|csv| csv.split(',').map(|s| s.trim().to_string()).collect());
        let required_hd = std::env::var("GOOGLE_OAUTH_REQUIRED_HD").ok();
        let tokeninfo_url =
            std::env::var("GOOGLE_TOKENINFO_URL").unwrap_or_else(|_| DEFAULT_TOKENINFO_URL.into());
        Self(Arc::new(GoogleOauthConfigInner {
            allowed_client_ids,
            required_hd,
            tokeninfo_url,
            http: reqwest::Client::new(),
            surreal: None,
        }))
    }

    /// Attach the database handle used to resolve the verified email to
    /// its real `persons.role`. Wired in `bootstrap` / the A2A
    /// router so role resolution is real in production; the
    /// pass-through and unit-test configs leave it `None`.
    #[must_use]
    pub fn with_db(self, surreal: store::surreal::SurrealDb) -> Self {
        let inner = &*self.0;
        Self(Arc::new(GoogleOauthConfigInner {
            allowed_client_ids: inner.allowed_client_ids.clone(),
            required_hd: inner.required_hd.clone(),
            tokeninfo_url: inner.tokeninfo_url.clone(),
            http: inner.http.clone(),
            surreal: Some(surreal),
        }))
    }

    /// Pass-through (KIND / local-dev) — middleware never blocks.
    #[must_use]
    pub fn passthrough() -> Self {
        Self(Arc::new(GoogleOauthConfigInner {
            allowed_client_ids: None,
            required_hd: None,
            tokeninfo_url: DEFAULT_TOKENINFO_URL.into(),
            http: reqwest::Client::new(),
            surreal: None,
        }))
    }

    /// Construct for tests with explicit values. The `tokeninfo_url`
    /// should point at a wiremock server.
    #[must_use]
    pub fn for_test(
        allowed_client_ids: impl IntoIterator<Item = impl Into<String>>,
        required_hd: Option<&str>,
        tokeninfo_url: impl Into<String>,
    ) -> Self {
        Self(Arc::new(GoogleOauthConfigInner {
            allowed_client_ids: Some(allowed_client_ids.into_iter().map(Into::into).collect()),
            required_hd: required_hd.map(str::to_string),
            tokeninfo_url: tokeninfo_url.into(),
            http: reqwest::Client::new(),
            surreal: None,
        }))
    }

    /// True when the middleware will challenge incoming requests.
    #[must_use]
    pub fn is_enforced(&self) -> bool {
        self.0.allowed_client_ids.is_some()
    }

    async fn verify(&self, token: &str) -> Result<TokenInfo, String> {
        let allowed = self
            .0
            .allowed_client_ids
            .as_ref()
            .ok_or("middleware not enforced")?;
        let resp = self
            .0
            .http
            .get(&self.0.tokeninfo_url)
            .query(&[("access_token", token)])
            .send()
            .await
            .map_err(|e| format!("tokeninfo request: {e}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("tokeninfo http {status}: {body}"));
        }
        let info: TokenInfo = serde_json::from_str(&body)
            .map_err(|e| format!("tokeninfo parse: {e}; body={body}"))?;
        // aud OR azp must match an allowlisted client; tokeninfo
        // returns both for some flows, only one for others. Normalize
        // both sides (drop the `.apps.googleusercontent.com` suffix)
        // so a bare numeric ID in the token still matches a
        // fully-qualified entry in the allowlist (and vice versa).
        let normalized_allowed: HashSet<&str> = allowed
            .iter()
            .map(|s| strip_oauth_suffix_borrowed(s))
            .collect();
        let matches_allowed = |claim: &str| -> bool {
            normalized_allowed.contains(strip_oauth_suffix_borrowed(claim))
        };
        let aud_match = info.aud.as_deref().is_some_and(matches_allowed);
        let azp_match = info.azp.as_deref().is_some_and(matches_allowed);
        if !aud_match && !azp_match {
            return Err(format!(
                "aud={:?} azp={:?} not in allowlist (size {})",
                info.aud,
                info.azp,
                allowed.len()
            ));
        }
        let verified = matches!(info.email_verified.as_deref(), Some("true" | "True"));
        if !verified {
            return Err(format!(
                "email_verified={:?} (need \"true\")",
                info.email_verified
            ));
        }
        Ok(info)
    }
}

fn strip_oauth_suffix_borrowed(s: &str) -> &str {
    s.trim_end_matches(".apps.googleusercontent.com")
}

/// Subset of Google's tokeninfo JSON. Most fields are strings even
/// when they represent booleans / numbers — that's what the API
/// returns. We keep the schema permissive to survive Google's
/// evolution of optional fields.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenInfo {
    pub aud: Option<String>,
    pub azp: Option<String>,
    pub sub: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

/// Axum middleware. When `GOOGLE_OAUTH_CLIENT_IDS` is unset, passes
/// through — `require_auth` then handles the Bearer-JWT path used by
/// KIND. When configured, every request must carry an
/// `Authorization: Bearer <google-access-token>` header that
/// tokeninfo validates.
pub async fn require_google_oauth(
    State(cfg): State<GoogleOauthConfig>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if !cfg.is_enforced() {
        return Ok(next.run(req).await);
    }
    // A first-party credential this deployment minted itself is already
    // authenticated, so it does not have to be a Google token as well.
    //
    // `inject_bearer_session` runs ahead of this layer on the A2A route
    // and inserts a `SessionData` only when `SessionStore::decode`
    // verifies the blob's HMAC against this deployment's own key — so a
    // session in the extensions here is proof of an authenticated
    // `navigator site login`, carrying the role and expiry that login
    // resolved. Handing it to Google's tokeninfo would reject it for not
    // being something it never claimed to be, and the `navigator` CLI
    // would have no way to reach this endpoint at all.
    //
    // What this does NOT do is skip authorization. The chain continues to
    // `require_policy`, where the same Rego lawyer-gate decides, from the
    // role on that session, exactly as it does for a Google caller.
    if let Some(session) = req.extensions().get::<crate::session::SessionData>() {
        tracing::debug!(
            source = ?session.source,
            "google_oauth: first-party session already resolved; skipping tokeninfo"
        );
        return Ok(next.run(req).await);
    }
    let Some(token) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        tracing::warn!("google_oauth: missing Authorization: Bearer header; returning 401");
        return Err(StatusCode::UNAUTHORIZED);
    };
    let info = match cfg.verify(token).await {
        Ok(i) => i,
        Err(reason) => {
            tracing::warn!(reason = %reason, "google_oauth: tokeninfo rejected token; returning 401");
            return Err(StatusCode::UNAUTHORIZED);
        }
    };
    if let Some(required) = cfg.0.required_hd.as_deref() {
        let suffix = format!("@{required}");
        let email_ok = info.email.as_deref().is_some_and(|e| e.ends_with(&suffix));
        if !email_ok {
            tracing::warn!(
                required_hd = required,
                got_domain = %domain_of(info.email.as_deref().unwrap_or_default()),
                "google_oauth: email-domain mismatch; returning 403"
            );
            return Err(StatusCode::FORBIDDEN);
        }
    }
    let email = info
        .email
        .clone()
        .unwrap_or_else(|| info.sub.clone().unwrap_or_default());
    // Resolve the caller's REAL tier from `persons.role`. A valid Google
    // token from the allowlisted client/domain is an *identity*, not an
    // authorization: it does not by itself confer lawyer access. An email
    // with no Neon Law Navigator account (or a client-tier one) gets `Client`, and
    // the embedded Rego policy lawyer-gate on `/mcp` + `/app/api/mcp/rpc` then denies it.
    // Operators must seed legitimate agent identities as lawyer/admin in
    // `persons`, exactly as for the browser/CLI paths.
    let person = resolve_person(cfg.0.surreal.as_ref(), &email).await;
    let role = person
        .as_ref()
        .map_or(store::persons::Role::Client, |p| p.role);
    if role == store::persons::Role::Client {
        tracing::warn!(
            target: "audit",
            event = "google_oauth.role.client_or_unknown",
            person_id = %person_id_field(person.as_ref()),
            domain = %domain_of(&email),
            "google_oauth: validated token resolved to client/unknown tier — lawyer-gated routes will deny",
        );
    }
    let auth = AuthClaims {
        sub: email,
        // tokeninfo's `exp` would be useful for caching later;
        // for the per-request check the http 200 itself suffices.
        exp: 0,
        role,
    };
    req.extensions_mut().insert(auth);
    Ok(next.run(req).await)
}

/// Resolve `email` to its `persons` row.
///
/// Returns `None` when the db is absent or no row matches. The caller reads
/// `role` off it and treats `None` as `Client` (the least-privileged tier) —
/// the secure default, so a missing account never yields lawyer access. The
/// whole row rather than just the tier because the audit record needs the
/// person's opaque id: an address in a log field is exactly what
/// `cli/tests/no_address_in_telemetry.rs` gates against.
async fn resolve_person(
    surreal: Option<&store::surreal::SurrealDb>,
    email: &str,
) -> Option<store::persons::Person> {
    store::persons::find_by_email_ci(surreal?, email)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::{require_google_oauth, GoogleOauthConfig};
    use crate::auth::AuthClaims;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use serde_json::json;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use store::test_support::mem_surreal;
    const ALLOWED_CLIENT: &str =
        "123456789012-abcdefghijklmnopqrstuvwxyzabcdef.apps.googleusercontent.com";

    async fn handler(Extension(claims): Extension<AuthClaims>) -> String {
        claims.sub
    }

    /// Reports the resolved session's email — the shape the A2A route
    /// depends on when the caller is the `navigator` CLI.
    async fn session_email(Extension(s): Extension<crate::session::SessionData>) -> String {
        s.email.unwrap_or_default()
    }

    fn app(cfg: GoogleOauthConfig) -> Router {
        Router::new().route("/protected", get(handler)).route_layer(
            axum::middleware::from_fn_with_state(cfg, require_google_oauth),
        )
    }

    async fn call(app: Router, token: Option<&str>) -> axum::response::Response {
        let mut b = Request::builder().uri("/protected");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        app.oneshot(b.body(Body::empty()).unwrap()).await.unwrap()
    }

    fn mock_url(server: &MockServer) -> String {
        format!("{}/tokeninfo", server.uri())
    }

    #[tokio::test]
    async fn valid_token_with_allowed_aud_and_verified_email_passes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .and(query_param("access_token", "abc"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "aud": ALLOWED_CLIENT,
                "azp": ALLOWED_CLIENT,
                "sub": "12345",
                "email": "libra@example.com",
                "email_verified": "true",
                "scope": "openid email"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), Some("abc")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"libra@example.com");
    }

    #[tokio::test]
    async fn passthrough_when_no_client_ids_configured() {
        let cfg = GoogleOauthConfig::passthrough();
        let resp = call(app(cfg), None).await;
        // Pass-through → handler runs without AuthClaims → 500.
        // The key signal: NOT 401.
        assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_resolved_first_party_session_skips_tokeninfo() {
        // The `navigator` CLI's credential is not a Google token, so
        // handing it to tokeninfo would 401 a caller this deployment
        // itself authenticated. A session already in the extensions —
        // which only `inject_bearer_session` puts there, and only for a
        // blob whose HMAC verified — passes straight through.
        //
        // `.expect(0)` is the assertion that matters: the mock must never
        // be called.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let app = Router::new()
            .route("/protected", get(session_email))
            .route_layer(axum::middleware::from_fn_with_state(
                cfg,
                require_google_oauth,
            ));

        let mut session =
            crate::session::SessionData::fresh("lawyer@example.com", store::persons::Role::Lawyer);
        session.email = Some("lawyer@example.com".into());
        session.source = crate::session::SessionSource::Cli;

        let mut req = Request::builder()
            .uri("/protected")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(session);
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"lawyer@example.com");
    }

    #[tokio::test]
    async fn missing_bearer_is_unauthorized() {
        let server = MockServer::start().await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), None).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tokeninfo_400_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_token"))
            .expect(1)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), Some("bogus")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn aud_not_in_allowlist_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "aud": "999999999.apps.googleusercontent.com",
                "azp": "999999999.apps.googleusercontent.com",
                "sub": "x",
                "email": "x@example.com",
                "email_verified": "true"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), Some("abc")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn email_unverified_is_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "aud": ALLOWED_CLIENT,
                "sub": "x",
                "email": "x@example.com",
                "email_verified": "false"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), Some("abc")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_email_domain_is_forbidden() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "aud": ALLOWED_CLIENT,
                "sub": "x",
                "email": "intruder@evil.example",
                "email_verified": "true"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), Some("abc")).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn resolve_person_reads_real_tier_and_defaults_unknown_to_client() {
        use super::resolve_person;

        use store::persons::Role;

        // The call site reads `role` off the row and treats a missing row as
        // the least-privileged tier; assert that resolution, not the getter.
        async fn role_of(surreal: Option<&store::surreal::SurrealDb>, email: &str) -> Role {
            resolve_person(surreal, email)
                .await
                .map_or(Role::Client, |p| p.role)
        }

        let surreal = mem_surreal().await;
        for (email, role) in [
            ("lawyer@example.com", Role::Lawyer),
            ("cli@example.com", Role::Client),
        ] {
            store::persons::create(
                &surreal,
                &store::persons::NewPerson::with_role(email, email, role),
            )
            .await
            .unwrap();
        }

        assert_eq!(
            role_of(Some(&surreal), "lawyer@example.com").await,
            Role::Lawyer
        );
        assert_eq!(
            role_of(Some(&surreal), "LaWyEr@Example.com").await,
            Role::Lawyer,
            "a verified email whose casing differs from the stored row keeps its lawyer tier"
        );
        assert_eq!(
            role_of(Some(&surreal), "cli@example.com").await,
            Role::Client
        );
        // Unknown email and absent db both fall back to the least
        // privilege — never lawyer.
        assert_eq!(
            role_of(Some(&surreal), "nobody@example.com").await,
            Role::Client
        );
        assert_eq!(role_of(None, "anyone@example.com").await, Role::Client);
    }

    #[tokio::test]
    async fn aud_without_apps_googleusercontent_suffix_also_matches() {
        // Some flows return the bare numeric client_id; the
        // allowlist normalization should treat them as equivalent.
        let bare = "123456789012-abcdefghijklmnopqrstuvwxyzabcdef";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/tokeninfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "aud": bare,
                "azp": bare,
                "sub": "x",
                "email": "libra@example.com",
                "email_verified": "true"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let cfg =
            GoogleOauthConfig::for_test([ALLOWED_CLIENT], Some("example.com"), mock_url(&server));
        let resp = call(app(cfg), Some("abc")).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
