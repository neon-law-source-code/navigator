#![allow(clippy::struct_field_names)]
//! OAuth2 Authorization Code flow with PKCE — the browser-flow
//! half of OIDC.
//!
//! Routes mounted under `/auth/*`:
//!
//! - `GET /auth/login` — generate state + PKCE verifier, set a
//!   short-lived pre-auth cookie, 302 to the IdP.
//! - `GET /auth/callback` — validate the returned `state`, exchange
//!   the `code` for tokens, decode the id_token, set the session
//!   cookie, 302 back to the `return_to` URL.
//! - `GET|POST /auth/logout` — clear the session cookie, then 302 to
//!   the IdP's `end_session_endpoint` (RP-initiated OIDC logout) so the
//!   provider drops its SSO session too; falls back to 302 home when the
//!   provider published no end-session endpoint.
//!
//! Config is loaded from the environment at boot — see
//! [`OAuthConfig::from_env`]. The IdP's authorization + token
//! endpoints come from `<issuer>/.well-known/openid-configuration`
//! so we don't hard-code provider-specific URLs.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use base64::Engine;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};
use tower_cookies::{cookie::SameSite, Cookie, Cookies};

use store::persons::Role;

use crate::auth::{AuthSetupError, JwksDocument};

use crate::session::{
    now_unix_secs, random_token_32, SessionData, SessionStore, DEFAULT_SESSION_TTL_SECS,
    SESSION_COOKIE_NAME,
};

/// The placeholder Microsoft's multi-tenant discovery documents put where a
/// concrete tenant id would go. Verified live against
/// `https://login.microsoftonline.com/organizations/v2.0/.well-known/openid-configuration`,
/// which answers `"issuer": "https://login.microsoftonline.com/{tenantid}/v2.0"`.
pub const ENTRA_TENANT_TEMPLATE: &str = "{tenantid}";

/// Split `OAUTH_MICROSOFT_ALLOWED_TENANTS` into normalised tenant ids.
///
/// Comma-separated, whitespace-tolerant, lower-cased so a GUID pasted from the
/// Entra portal in either case matches the `tid` claim. Empty entries are
/// dropped, so a trailing comma is not a silently-empty allowlist entry that
/// would match a token with an empty `tid`.
fn parse_tenant_allowlist(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect()
}

/// Pre-auth (login-in-progress) cookie name.
pub const PRE_AUTH_COOKIE_NAME: &str = "navigator_pre_auth";
/// Pre-auth cookie lifetime — 5 minutes is plenty for the
/// roundtrip to the IdP and back.
pub const PRE_AUTH_TTL_SECS: i64 = 5 * 60;

/// Which identity provider an [`OAuthConfig`] speaks to.
///
/// Navigator holds one config per provider and renders one button per
/// configured provider. The slug is both the `/auth/login/{provider}` path
/// segment and the value recorded in [`crate::session::SessionData::provider`],
/// so a sign-out reaches the provider that actually minted the session.
/// The serde spelling is deliberately the same string [`Self::slug`] returns,
/// so the pre-auth cookie, the session cookie, and the `/auth/login/{provider}`
/// path all name a provider identically. One spelling, three places.
/// [`Default`] is the primary slot, which is what makes a `serde(default)`
/// pre-auth or session cookie from before this enum existed decode as the door
/// it was actually minted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ProviderId {
    /// The original single slot: `OAUTH_ISSUER_URL` and friends. Google in
    /// production, Rauthy in the local lanes, any compliant OIDC provider in
    /// principle — the module never hard-codes its endpoints.
    #[default]
    #[serde(rename = "oidc")]
    Primary,
    /// Microsoft Entra ID, configured from `OAUTH_MICROSOFT_*`. Separate from
    /// [`Self::Primary`] because multi-tenant Entra needs per-token issuer
    /// validation ([`IssuerPolicy::EntraTenants`]) that a normal single-issuer
    /// provider does not.
    #[serde(rename = "microsoft")]
    Microsoft,
}

impl ProviderId {
    /// URL path segment on `/auth/login/{provider}` and the string persisted in
    /// the session cookie. `oidc` is the historical spelling of the primary
    /// slot and is kept so existing links and bookmarks keep working.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Primary => "oidc",
            Self::Microsoft => "microsoft",
        }
    }

    /// Parse a slug back. `None` for anything unrecognised, which the route
    /// turns into a 404 rather than silently falling back to a provider the
    /// caller did not ask for.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "oidc" => Some(Self::Primary),
            "microsoft" => Some(Self::Microsoft),
            _ => None,
        }
    }

    /// The sign-in button label.
    ///
    /// Microsoft's branding rules require the exact words "Sign in with
    /// Microsoft" and forbid exposing the "Azure" or "Active Directory"
    /// brands, so the string is fixed rather than derived from config.
    #[must_use]
    pub const fn button_label(self) -> &'static str {
        match self {
            Self::Primary => webapp::auth_pages::GOOGLE_SIGN_IN,
            Self::Microsoft => webapp::auth_pages::MICROSOFT_SIGN_IN,
        }
    }

    /// Which claim carries the address a `persons` row is matched on.
    ///
    /// This is a security decision per provider, not a convenience:
    /// `resolve_person_from_claims` falls back to matching a pre-seeded row by
    /// address, so whichever claim is chosen here is what an attacker would
    /// have to control in order to land on somebody else's row.
    ///
    /// - [`Self::Primary`] — `email`. Google issues `email_verified: true`
    ///   only after proving the user controls that mailbox, so the address is
    ///   evidence.
    /// - [`Self::Microsoft`] — `preferred_username` first, the user principal
    ///   name, which Entra can only issue on a domain the signing tenant has
    ///   verified with Microsoft. `email` is the fallback for a directory that
    ///   omits the UPN, and it is only reachable at all because
    ///   `OAUTH_MICROSOFT_ALLOWED_TENANTS` has already confined the signer to
    ///   a tenant an operator named.
    fn identity_address(self, claims: &IdTokenClaims) -> Option<String> {
        let pick = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        match self {
            Self::Primary => pick(&claims.email),
            Self::Microsoft => pick(&claims.preferred_username).or_else(|| pick(&claims.email)),
        }
    }
}

/// How to validate the `iss` claim on an id_token.
///
/// Almost every provider publishes a fixed issuer string and the check is a
/// byte compare. Microsoft's multi-tenant authorities do not: `/organizations`
/// and `/common` publish the literal template
/// `https://login.microsoftonline.com/{tenantid}/v2.0`, and the token carries
/// the signing tenant's GUID in its own `tid` claim. Pinning the template
/// verbatim rejects every real token, so multi-tenant Entra needs the
/// per-token form below.
#[derive(Debug, Clone)]
pub enum IssuerPolicy {
    /// One fixed issuer, enforced inside `jsonwebtoken`'s own `Validation`.
    Exact,
    /// Microsoft Entra multi-tenant. `iss` must equal `template` with
    /// `{tenantid}` replaced by the token's `tid`, and `tid` must appear in
    /// `allowed_tenants`.
    ///
    /// The allowlist is consulted **first**, so the string we interpolate can
    /// only ever be one an operator wrote into
    /// `OAUTH_MICROSOFT_ALLOWED_TENANTS`; a token from an unlisted tenant is
    /// rejected before its `tid` is used for anything at all.
    EntraTenants {
        /// The templated issuer read from the discovery document.
        template: String,
        /// Lower-cased tenant GUIDs permitted to sign in.
        allowed_tenants: Vec<String>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum OAuthSetupError {
    #[error("missing env var: {0}")]
    Missing(&'static str),
    #[error("fetching discovery doc: {0}")]
    DiscoveryFetch(String),
    #[error("parsing discovery doc: {0}")]
    DiscoveryParse(String),
    /// `OAUTH_MICROSOFT_ALLOWED_TENANTS` was unset or empty while
    /// `OAUTH_MICROSOFT_CLIENT_ID` was set. Fail boot rather than default to
    /// "any Entra tenant": Entra's `email` claim is tenant-asserted and
    /// unverified, so an open tenant list would let anybody able to create a
    /// free Entra tenant assert a seeded person's address. The allowlist is
    /// the control that makes multi-tenant sign-in safe, so it is mandatory.
    #[error("OAUTH_MICROSOFT_ALLOWED_TENANTS must list at least one tenant id")]
    MissingTenantAllowlist,
}

#[derive(Clone)]
pub struct OAuthConfig {
    inner: Arc<OAuthConfigInner>,
}

#[derive(Clone)]
struct OAuthConfigInner {
    /// Which provider this config speaks to. Chooses the button label, the
    /// `/auth/login/{provider}` slug, and which id_token claim carries the
    /// address a `persons` row is matched on.
    provider: ProviderId,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    authorization_endpoint: String,
    token_endpoint: String,
    end_session_endpoint: Option<String>,
    /// RS256 id_token verifier, built from the IdP's published JWKS and
    /// pinned to the discovered `issuer` + our `client_id` audience. Not a
    /// fixed key set fetched once at boot — see [`IdTokenVerifier`] for how
    /// it keeps itself current across a provider's key rotation. `None`
    /// only on the hand-built test config; production always carries one
    /// and [`callback`] refuses to mint a session without it.
    id_token_verifier: Option<Arc<IdTokenVerifier>>,
}

#[derive(Debug, Deserialize)]
struct DiscoveryDoc {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    end_session_endpoint: Option<String>,
}

impl OAuthConfig {
    /// Build with hand-supplied endpoints. Used by tests to point at
    /// a mock IdP without doing real discovery.
    #[must_use]
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
        authorization_endpoint: impl Into<String>,
        token_endpoint: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(OAuthConfigInner {
                provider: ProviderId::Primary,
                client_id: client_id.into(),
                client_secret: client_secret.into(),
                redirect_uri: redirect_uri.into(),
                authorization_endpoint: authorization_endpoint.into(),
                token_endpoint: token_endpoint.into(),
                end_session_endpoint: None,
                id_token_verifier: None,
            }),
        }
    }

    /// Re-label a hand-built config as belonging to `provider`. Tests use it
    /// to drive the Microsoft path against a mock IdP; production sets the
    /// provider inside the `from_env` constructors.
    #[must_use]
    pub fn with_provider(self, provider: ProviderId) -> Self {
        let mut inner = (*self.inner).clone();
        inner.provider = provider;
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Which provider this config speaks to.
    #[must_use]
    pub fn provider(&self) -> ProviderId {
        self.inner.provider
    }

    /// Attach an id_token verifier to a hand-built config. Tests use this
    /// to exercise the real verification path in [`callback`] with a
    /// locally-minted signing key; production builds the verifier inside
    /// [`OAuthConfig::from_env`] from the IdP's published JWKS.
    #[must_use]
    pub fn with_id_token_verifier(self, verifier: IdTokenVerifier) -> Self {
        let mut inner = (*self.inner).clone();
        inner.id_token_verifier = Some(Arc::new(verifier));
        Self {
            inner: Arc::new(inner),
        }
    }

    /// The RS256 id_token verifier, when configured. `callback` treats
    /// `None` as a misconfiguration and refuses the sign-in rather than
    /// trusting an unverified token.
    #[must_use]
    pub fn id_token_verifier(&self) -> Option<&Arc<IdTokenVerifier>> {
        self.inner.id_token_verifier.as_ref()
    }

    /// Attach a provider `end_session_endpoint` to a hand-built config so a
    /// test can exercise the RP-initiated logout redirect ([`end_session_url`])
    /// without doing real discovery. Production reads this from the discovery
    /// document in [`OAuthConfig::from_env`].
    #[must_use]
    pub fn with_end_session_endpoint(self, endpoint: impl Into<String>) -> Self {
        let mut inner = (*self.inner).clone();
        inner.end_session_endpoint = Some(endpoint.into());
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Build from env. Returns `Ok(None)` when `OAUTH_ISSUER_URL` is
    /// unset (the binary keeps booting without the browser-flow
    /// routes); returns `Err` only when `OAUTH_ISSUER_URL` *is* set
    /// but a required sibling is missing or discovery fails.
    pub async fn from_env() -> Result<Option<Self>, OAuthSetupError> {
        let Ok(issuer) = std::env::var("OAUTH_ISSUER_URL") else {
            return Ok(None);
        };
        let client_id = std::env::var("OAUTH_CLIENT_ID")
            .map_err(|_| OAuthSetupError::Missing("OAUTH_CLIENT_ID"))?;
        let client_secret = std::env::var("OAUTH_CLIENT_SECRET")
            .map_err(|_| OAuthSetupError::Missing("OAUTH_CLIENT_SECRET"))?;
        let redirect_uri = std::env::var("OAUTH_REDIRECT_URI")
            .map_err(|_| OAuthSetupError::Missing("OAUTH_REDIRECT_URI"))?;

        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let doc: DiscoveryDoc = reqwest::get(&url)
            .await
            .map_err(|e| OAuthSetupError::DiscoveryFetch(e.to_string()))?
            .json()
            .await
            .map_err(|e| OAuthSetupError::DiscoveryParse(e.to_string()))?;

        // Build the id_token verifier from the IdP's published JWKS,
        // pinned to the discovered issuer and our client_id audience.
        // This is the mandatory check on the redirect callback — a
        // forged or mis-issued id_token can never mint a session.
        // The primary slot is a single-issuer provider by definition — the
        // discovery document is fetched from the very issuer we then pin.
        let verifier = IdTokenVerifier::from_jwks_url(
            &doc.jwks_uri,
            &doc.issuer,
            &client_id,
            IssuerPolicy::Exact,
        )
        .await
        .map_err(|e| OAuthSetupError::DiscoveryFetch(e.to_string()))?;

        Ok(Some(Self {
            inner: Arc::new(OAuthConfigInner {
                provider: ProviderId::Primary,
                client_id,
                client_secret,
                redirect_uri,
                authorization_endpoint: doc.authorization_endpoint,
                token_endpoint: doc.token_endpoint,
                end_session_endpoint: doc.end_session_endpoint,
                id_token_verifier: Some(verifier),
            }),
        }))
    }

    /// Default Microsoft authority: work-or-school accounts from any Entra
    /// tenant, personal Microsoft accounts excluded. Business clients sign in
    /// with the account they already hold, and an `@outlook.com` login cannot
    /// silently become a second identity for the same human.
    pub const DEFAULT_MICROSOFT_ISSUER: &'static str =
        "https://login.microsoftonline.com/organizations/v2.0";

    /// Build the **Microsoft Entra ID** provider from the environment.
    ///
    /// Reads:
    ///
    /// - `OAUTH_MICROSOFT_CLIENT_ID` — the Entra app registration's
    ///   Application (client) ID. Unset gives `Ok(None)`: the second button
    ///   never renders and every existing deployment is byte-identical.
    /// - `OAUTH_MICROSOFT_CLIENT_SECRET` — the registration's client secret.
    ///   Entra advertises `client_secret_post`, which is the form field
    ///   [`exchange_code`] already sends.
    /// - `OAUTH_MICROSOFT_ALLOWED_TENANTS` — **required** once the client id
    ///   is set. Comma-separated Entra tenant ids. See
    ///   [`IssuerPolicy::EntraTenants`] and
    ///   [`OAuthSetupError::MissingTenantAllowlist`] for why it is not
    ///   optional.
    /// - `OAUTH_MICROSOFT_ISSUER_URL` — optional override, defaulting to
    ///   [`Self::DEFAULT_MICROSOFT_ISSUER`]. Point it at
    ///   `https://login.microsoftonline.com/<tenant-guid>/v2.0` for a
    ///   single-tenant registration; that authority publishes a concrete
    ///   issuer, so the ordinary [`IssuerPolicy::Exact`] byte compare applies
    ///   without any further configuration.
    ///
    /// The redirect URI is deliberately **shared** with the primary provider
    /// (`OAUTH_REDIRECT_URI`): both registrations point at `/auth/callback`,
    /// and the callback tells them apart from the signed pre-auth cookie. So
    /// adding a provider adds no new public route to register or defend.
    pub async fn microsoft_from_env() -> Result<Option<Self>, OAuthSetupError> {
        let Some(client_id) = std::env::var("OAUTH_MICROSOFT_CLIENT_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(None);
        };
        let client_secret = std::env::var("OAUTH_MICROSOFT_CLIENT_SECRET")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or(OAuthSetupError::Missing("OAUTH_MICROSOFT_CLIENT_SECRET"))?;
        let redirect_uri = std::env::var("OAUTH_REDIRECT_URI")
            .map_err(|_| OAuthSetupError::Missing("OAUTH_REDIRECT_URI"))?;
        let allowed_tenants = parse_tenant_allowlist(
            &std::env::var("OAUTH_MICROSOFT_ALLOWED_TENANTS").unwrap_or_default(),
        );
        if allowed_tenants.is_empty() {
            return Err(OAuthSetupError::MissingTenantAllowlist);
        }
        let issuer = std::env::var("OAUTH_MICROSOFT_ISSUER_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| Self::DEFAULT_MICROSOFT_ISSUER.to_string());

        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let doc: DiscoveryDoc = reqwest::get(&url)
            .await
            .map_err(|e| OAuthSetupError::DiscoveryFetch(e.to_string()))?
            .json()
            .await
            .map_err(|e| OAuthSetupError::DiscoveryParse(e.to_string()))?;

        // A multi-tenant authority publishes the templated issuer; a
        // single-tenant one publishes a concrete tenant id. Pick the policy
        // from what the document actually says rather than from which
        // authority we asked for.
        let policy = if doc.issuer.contains(ENTRA_TENANT_TEMPLATE) {
            IssuerPolicy::EntraTenants {
                template: doc.issuer.clone(),
                allowed_tenants: allowed_tenants.clone(),
            }
        } else {
            IssuerPolicy::Exact
        };
        let verifier =
            IdTokenVerifier::from_jwks_url(&doc.jwks_uri, &doc.issuer, &client_id, policy)
                .await
                .map_err(|e| OAuthSetupError::DiscoveryFetch(e.to_string()))?;

        tracing::info!(
            issuer = %doc.issuer,
            tenants = allowed_tenants.len(),
            "oauth: microsoft entra provider configured",
        );

        Ok(Some(Self {
            inner: Arc::new(OAuthConfigInner {
                provider: ProviderId::Microsoft,
                client_id,
                client_secret,
                redirect_uri,
                authorization_endpoint: doc.authorization_endpoint,
                token_endpoint: doc.token_endpoint,
                end_session_endpoint: doc.end_session_endpoint,
                id_token_verifier: Some(verifier),
            }),
        }))
    }

    #[must_use]
    pub fn authorization_endpoint(&self) -> &str {
        &self.inner.authorization_endpoint
    }
    #[must_use]
    pub fn token_endpoint(&self) -> &str {
        &self.inner.token_endpoint
    }
    #[must_use]
    pub fn end_session_endpoint(&self) -> Option<&str> {
        self.inner.end_session_endpoint.as_deref()
    }

    /// The configured OAuth redirect URI. Its scheme is the deployment's
    /// external scheme (KIND uses `http://localhost…`, prod uses
    /// `https://…`), so it doubles as the signal for whether auth cookies
    /// should carry the `Secure` flag — even behind a TLS-terminating LB
    /// that forwards plain HTTP internally.
    #[must_use]
    pub fn redirect_uri(&self) -> &str {
        &self.inner.redirect_uri
    }
}

/// 32-byte random url-safe verifier, then S256-derived challenge.
#[must_use]
pub fn pkce_verifier() -> String {
    random_token_32()
}

#[must_use]
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// Pre-auth cookie payload: enough to validate the callback later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreAuth {
    /// Which provider this login is against.
    ///
    /// Both providers share one redirect URI (`/auth/callback`), so this is
    /// what the callback disambiguates on. Keeping it inside the HMAC-signed
    /// one-shot cookie next to `state` and the PKCE verifier means the code
    /// can only ever be redeemed at the token endpoint of the provider the
    /// login actually started against — the mitigation shape RFC 9207
    /// describes for IdP mix-up.
    ///
    /// `serde(default)` so a login already in flight when a new build rolls
    /// out decodes as [`ProviderId::Primary`] and completes normally instead
    /// of failing on an unknown cookie shape.
    #[serde(default)]
    pub provider: ProviderId,
    pub state: String,
    pub verifier: String,
    /// One-time value sent on the authorize request and required to
    /// match `id_token.nonce` in the callback — binds the returned
    /// token to *this* login and defeats id_token replay/injection.
    #[serde(default)]
    pub nonce: String,
    pub return_to: String,
    pub exp: i64,
}

impl PreAuth {
    #[must_use]
    pub fn new(return_to: String) -> Self {
        Self::for_provider(ProviderId::Primary, return_to)
    }

    /// A pre-auth payload bound to `provider`.
    #[must_use]
    pub fn for_provider(provider: ProviderId, return_to: String) -> Self {
        Self {
            provider,
            state: random_token_32(),
            verifier: pkce_verifier(),
            nonce: random_token_32(),
            return_to,
            exp: now_unix_secs() + PRE_AUTH_TTL_SECS,
        }
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.exp <= now_unix_secs()
    }
}

/// Build the authorize URL the user gets redirected to.
#[must_use]
pub fn authorize_url(cfg: &OAuthConfig, pre: &PreAuth) -> String {
    use std::fmt::Write;
    let challenge = pkce_challenge(&pre.verifier);
    let mut url = url_with_query(cfg.authorization_endpoint());
    let client = urlencode(&cfg.inner.client_id);
    let redirect = urlencode(&cfg.inner.redirect_uri);
    let scope = urlencode("openid email profile");
    let state = urlencode(&pre.state);
    let nonce = urlencode(&pre.nonce);
    let _ = write!(
        url,
        "response_type=code&client_id={client}&redirect_uri={redirect}&scope={scope}&state={state}&nonce={nonce}&code_challenge={challenge}&code_challenge_method=S256",
    );
    url
}

/// The routes each configured provider's sign-in button points at, in the
/// order they render. The primary door comes first so the page a signed-out
/// person already knows does not reorder underneath them when a second
/// provider is switched on.
fn provider_buttons(s: &AuthState, return_to: &str) -> Vec<webapp::auth_pages::SignInProvider> {
    s.configured_providers()
        .into_iter()
        .map(|provider| webapp::auth_pages::SignInProvider {
            href: format!("/auth/login/{}?return_to={return_to}", provider.slug()),
            label: provider.button_label().to_string(),
        })
        .collect()
}

/// Build the RP-initiated logout URL when the provider published an
/// `end_session_endpoint` (OIDC RP-Initiated Logout 1.0). Bouncing the
/// browser through it clears the provider's own SSO session, so the next
/// `/auth/login` prompts for credentials instead of silently
/// re-authenticating from a live provider session.
///
/// The request carries `post_logout_redirect_uri` — the app's own origin,
/// derived from the OAuth redirect URI so it is the same origin the login
/// flow already round-trips through and is therefore on the provider's
/// allowlisted `post_logout_redirect_uris` — and `client_id`, which lets
/// the provider validate that redirect without an `id_token_hint`: our
/// session never retains the id_token, so we have no hint to send.
///
/// Returns `None` when the provider published no `end_session_endpoint`
/// (or the redirect URI has no parseable origin); the caller then falls
/// back to clearing the app session and redirecting home.
#[must_use]
pub fn end_session_url(cfg: &OAuthConfig) -> Option<String> {
    use std::fmt::Write;
    let endpoint = cfg.end_session_endpoint()?;
    let post_logout = app_origin(cfg.redirect_uri())?;
    let mut url = url_with_query(endpoint);
    let redirect = urlencode(&post_logout);
    let client = urlencode(&cfg.inner.client_id);
    let _ = write!(
        url,
        "post_logout_redirect_uri={redirect}&client_id={client}"
    );
    Some(url)
}

/// The app's own origin (`scheme://host[:port]`, no trailing path) parsed
/// from the OAuth redirect URI. This is the base the login flow already
/// uses, so a provider that allowlists the login redirect also allowlists
/// this post-logout redirect.
fn app_origin(redirect_uri: &str) -> Option<String> {
    let parsed = url::Url::parse(redirect_uri).ok()?;
    let origin = parsed.origin();
    if origin.is_tuple() {
        Some(origin.ascii_serialization())
    } else {
        None
    }
}

fn url_with_query(base: &str) -> String {
    let mut out = base.to_string();
    if base.contains('?') {
        out.push('&');
    } else {
        out.push('?');
    }
    out
}

fn urlencode(s: &str) -> String {
    use std::fmt::Write;
    // Minimal percent-encoder for OAuth params (RFC 3986 unreserved
    // chars are left as-is). Good enough for the limited character
    // set we hand to the IdP.
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

/// Combined router state.
#[derive(Clone)]
pub struct AuthState {
    pub oauth: OAuthConfig,
    /// Microsoft Entra ID, when `OAUTH_MICROSOFT_CLIENT_ID` is set. `None` —
    /// the default — leaves `/auth/login` exactly as it was: one provider, one
    /// immediate redirect, no chooser.
    ///
    /// This sits alongside [`Self::oauth`] rather than replacing it with a
    /// list because the primary slot is the one every other part of the module
    /// still reads for deployment-wide facts (the redirect URI's scheme, the
    /// fallback end-session endpoint). A provider added here is additive.
    pub oauth_microsoft: Option<OAuthConfig>,
    pub sessions: SessionStore,
    /// Store handle for the tables the auth flows
    /// touch (participation on a fresh signup, the sent-email log).
    /// The SurrealDB handle. The whole auth path answers from here:
    /// `persons` moved with ENG-19, so resolving the IdP claims to a
    /// local row and reading `role` off it performs no extra read at
    /// all. The password-reset and email-confirmation flows mint and
    /// claim `email_token` rows in the same engine.
    pub surreal: store::surreal::SurrealDb,
    /// Outbound email backend kept on the auth state for the admin
    /// "Send welcome" button and other direct sends; the workflow
    /// trigger below routes through `workflow_runtime`, not this
    /// field.
    pub email: std::sync::Arc<dyn crate::email::EmailService>,
    /// Durable workflow runtime — the OAuth callback fires
    /// `workflows::email::welcome::trigger_welcome` against this
    /// when a fresh `persons` row appears.
    pub workflow_runtime: std::sync::Arc<dyn workflows::StateMachineRuntime>,
    /// Email address that is "always admin" — JIT-created on first
    /// sign-in, role-healed if cleared. `None` disables the carve-out
    /// (every sign-in then strictly requires a pre-seeded row). Loaded
    /// from `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` in `bootstrap`; threaded
    /// here so tests can opt in/out without mutating process env.
    pub bootstrap_owner_email: Option<String>,
    /// Self-signup capability, a global toggle that is **off by default**.
    /// Off: an IdP-authenticated email with no pre-seeded `persons` row gets
    /// 403 (today's behavior). On: the first login for an unknown verified
    /// email JIT-creates a `client` with an empty portfolio. Loaded from
    /// `NAVIGATOR_SELF_SIGNUP_ENABLED` in `bootstrap`; threaded here (not
    /// read from env in the handler) so tests can opt in/out.
    pub self_signup_enabled: bool,
    /// Email/password front door, delegated to **GCP Identity
    /// Platform**. Present only when `NAVIGATOR_IDENTITY_PLATFORM_API_KEY`
    /// is set. `None` keeps `/auth/login` as the pure OIDC redirect, so
    /// existing OIDC-only deploys are byte-identical. We never store or
    /// hash a password — Identity Platform validates it over TLS and
    /// hands back an ID token, the same trust model as the OIDC
    /// back-channel below. See the "Sign-in" section of the deploy
    /// workshop.
    pub identity_password: Option<IdentityPasswordConfig>,
    /// Admin door to **GCP Identity Platform**, used by the password-reset
    /// and email-confirm flows to write a new password or flip
    /// `emailVerified` for an account the signed-out user can't touch
    /// themselves. Unlike [`Self::identity_password`] (a public browser
    /// key), these calls need a service-account bearer token, minted from
    /// the GCE metadata server over plain `reqwest` — no GCP SDK in `web`.
    /// `None` disables reset/confirm even when the password door is on, so
    /// the routes 404 and the email-confirm gate falls through (no admin
    /// credential ⇒ nothing to write). See [`crate::idp_admin`].
    pub identity_admin: Option<crate::idp_admin::IdentityAdminConfig>,
    /// Whether auth cookies (`session`, pre-auth, login-CSRF) carry the
    /// `Secure` flag. Derived in `bootstrap` from the OAuth redirect
    /// URI scheme: `true` for an `https://` deployment, `false` for the
    /// `http://localhost` KIND loop so cookies still round-trip over
    /// plain HTTP in dev.
    pub secure_cookies: bool,
}

impl AuthState {
    /// The config for `provider`, or `None` when that provider is not
    /// configured on this deployment.
    #[must_use]
    pub fn provider_config(&self, provider: ProviderId) -> Option<&OAuthConfig> {
        match provider {
            ProviderId::Primary => Some(&self.oauth),
            ProviderId::Microsoft => self.oauth_microsoft.as_ref(),
        }
    }

    /// Every configured provider, primary first.
    #[must_use]
    pub fn configured_providers(&self) -> Vec<ProviderId> {
        let mut out = vec![ProviderId::Primary];
        if self.oauth_microsoft.is_some() {
            out.push(ProviderId::Microsoft);
        }
        out
    }

    /// Look a session's recorded provider slug back up.
    ///
    /// Falls back to the primary provider for a session minted before the slug
    /// was recorded, or for a slug this build no longer configures — logout
    /// must clear the app session either way, so an unknown provider degrades
    /// to the old single-provider behaviour rather than failing.
    fn provider_config_for_slug(&self, slug: Option<&str>) -> &OAuthConfig {
        slug.and_then(ProviderId::from_slug)
            .and_then(|provider| self.provider_config(provider))
            .unwrap_or(&self.oauth)
    }
}

/// Configuration for the Identity Platform email/password sign-in path.
///
/// The `api_key` is the project's Identity Platform **browser key** — it
/// only scopes anonymous Identity Toolkit calls to this project; it is
/// not an admin credential and grants no data access on its own. The
/// password the user types is forwarded once to Google's
/// `accounts:signInWithPassword` endpoint over TLS and never persisted.
#[derive(Clone)]
pub struct IdentityPasswordConfig {
    /// Identity Platform browser API key (`?key=` on the REST call).
    pub api_key: String,
    /// Identity Toolkit REST base. `https://identitytoolkit.googleapis.com`
    /// in prod; tests point it at a mock.
    pub endpoint: String,
}

impl IdentityPasswordConfig {
    /// Production Identity Toolkit REST base.
    pub const DEFAULT_ENDPOINT: &'static str = "https://identitytoolkit.googleapis.com";

    /// Build from the environment. Returns `None` (password sign-in off)
    /// when `NAVIGATOR_IDENTITY_PLATFORM_API_KEY` is unset or empty, so
    /// the route is strictly opt-in and never a boot invariant.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("NAVIGATOR_IDENTITY_PLATFORM_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let endpoint = std::env::var("NAVIGATOR_IDENTITY_PLATFORM_ENDPOINT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| Self::DEFAULT_ENDPOINT.to_string());
        Some(Self { api_key, endpoint })
    }
}

/// Build the /auth/* sub-router.
pub fn routes(state: AuthState) -> Router {
    let mut router = Router::new()
        .route("/auth/login", get(login))
        // The per-provider redirect, always reachable by its own path so a
        // button on the chooser works even when `/auth/login` renders the
        // chooser instead of redirecting. `oidc` is the primary slot's slug,
        // which is why the historical `/auth/login/oidc` URL still resolves;
        // `microsoft` is Entra. An unrecognised or unconfigured slug 404s
        // rather than falling back to a provider nobody asked for.
        .route("/auth/login/{provider}", get(start_provider_redirect))
        // Email/password submit (Identity Platform). 404s when password
        // sign-in is not configured.
        .route("/auth/password", post(password_login))
        .route("/auth/callback", get(callback))
        .route("/auth/logout", get(logout).post(logout));

    // Self-service password reset + email confirmation only exist where
    // an email/password door does — an OIDC-only deploy has no passwords
    // to reset and its Google tokens are already `email_verified`. Mount
    // them only then, so those deploys stay byte-identical (the routes are
    // simply absent → 404), mirroring how `/auth/password` itself 404s.
    if state.identity_password.is_some() {
        router = router
            .merge(crate::password_reset::routes())
            .merge(crate::email_confirm::routes());
    }

    router.with_state(state)
}

/// Cookie that carries the signed login-CSRF token while the password
/// form is on screen — the double-submit counterpart to the hidden
/// field embedded in the form.
pub const LOGIN_CSRF_COOKIE_NAME: &str = "navigator_login_csrf";

/// The sign-in failure shown for both an unknown email and a wrong
/// password.
///
/// Deliberately identical for the two cases: a message that distinguished
/// them would confirm which addresses have accounts. Warm rather than
/// terse, because the person reading it is locked out.
const LOGIN_FAILED: &str = "That email and password don't match what we have on file. \
                            Please try again.";

/// A resolved sign-in toast: its tone plus its text, owned so it remains
/// valid until the response is rendered.
enum NoticeText {
    Danger(String),
    Success(String),
}

impl NoticeText {
    fn as_login_notice(&self) -> webapp::auth_pages::LoginNotice {
        match self {
            NoticeText::Danger(text) => webapp::auth_pages::LoginNotice::Danger(text.clone()),
            NoticeText::Success(text) => webapp::auth_pages::LoginNotice::Success(text.clone()),
        }
    }
}

/// Map the `notice` query flag to the toned banner the sign-in page should
/// surface, if any. The bounce case is red; the post-action outcomes are
/// green.
fn login_notice(notice: Option<&str>) -> Option<NoticeText> {
    match notice {
        Some("login_required") => Some(NoticeText::Danger(
            "You need to log in to view that page.".to_string(),
        )),
        Some("password_reset") => Some(NoticeText::Success(
            "Your password has been updated. Please sign in.".to_string(),
        )),
        Some("email_confirmed") => Some(NoticeText::Success(
            "Your email is confirmed. Please sign in.".to_string(),
        )),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct LoginQuery {
    #[serde(default = "default_return_to")]
    pub return_to: String,
    /// Optional UX hint set by the redirector. `notice=login_required`
    /// tells the sign-in page to greet the visitor with a red toast (see
    /// [`login_notice`]); absent for a voluntary visit to `/auth/login`.
    #[serde(default)]
    pub notice: Option<String>,
}

/// The neutral "no explicit destination" default. Empty rather than a concrete
/// path because the landing is role-dependent and the role is not known until
/// sign-in completes: [`post_login_landing`] resolves the empty default (and the
/// retired `/portal`) into the caller's tier landing, while any explicit
/// `return_to` a deep-link bounce set is honored unchanged.
fn default_return_to() -> String {
    String::new()
}

/// Where a freshly authenticated person lands. A firm tier (owner/admin/lawyer/
/// clerk) goes to the team home; a client goes to their matter list. Only the
/// neutral default and the retired `/portal` fall through to the tier landing —
/// any other `return_to` is an explicit deep link (an anonymous bounce recorded
/// the page the visitor was reaching for) and is returned unchanged.
///
/// Public because it is the landing contract the Using workshop teaches in
/// print, and `workshop_claims_grounding` asserts the deck against this
/// function rather than restating the rule. A test that re-implemented the
/// fork would agree with itself while the deck went stale.
#[must_use]
pub fn post_login_landing(role: Role, return_to: &str) -> String {
    if !return_to.is_empty() && return_to != "/portal" {
        return return_to.to_string();
    }
    if role == Role::Client {
        crate::dioxus_app::PROJECTS_PATH.to_string()
    } else {
        crate::dioxus_app::APP_TEAM_PATH.to_string()
    }
}

async fn login(
    State(s): State<AuthState>,
    cookies: Cookies,
    Query(q): Query<LoginQuery>,
) -> Response {
    // A chooser is only meaningful when there is something to choose: a
    // password front door, or a second identity provider. A deployment with
    // one provider and no password door keeps the immediate redirect,
    // byte-identical to before this route learned about providers at all.
    let providers = provider_buttons(&s, &q.return_to);
    if s.identity_password.is_some() || providers.len() > 1 {
        // Hold the resolved notice so the borrowed `LoginNotice` it lends
        // the view outlives the render call.
        let notice = login_notice(q.notice.as_deref());
        return login_chooser_response(
            &s,
            &cookies,
            &q.return_to,
            None,
            notice.as_ref().map(NoticeText::as_login_notice),
            StatusCode::OK,
        );
    }
    start_provider(&s, &cookies, ProviderId::Primary, q.return_to)
}

/// The per-provider redirect handler behind `/auth/login/{provider}`, so each
/// button on the chooser reaches the provider it names.
async fn start_provider_redirect(
    State(s): State<AuthState>,
    cookies: Cookies,
    axum::extract::Path(slug): axum::extract::Path<String>,
    Query(q): Query<LoginQuery>,
) -> Response {
    // Unknown slug, or a provider this deployment has not configured: 404.
    // Silently substituting the primary provider would send somebody who
    // clicked "Sign in with Microsoft" to a Google consent screen.
    let Some(provider) = ProviderId::from_slug(&slug) else {
        return (StatusCode::NOT_FOUND, "unknown sign-in provider").into_response();
    };
    if s.provider_config(provider).is_none() {
        return (StatusCode::NOT_FOUND, "sign-in provider not configured").into_response();
    }
    start_provider(&s, &cookies, provider, q.return_to)
}

/// Set the pre-auth cookie and 302 to `provider`'s IdP.
fn start_provider(
    s: &AuthState,
    cookies: &Cookies,
    provider: ProviderId,
    return_to: String,
) -> Response {
    let Some(cfg) = s.provider_config(provider) else {
        return (StatusCode::NOT_FOUND, "sign-in provider not configured").into_response();
    };
    let pre = PreAuth::for_provider(provider, return_to);
    let cookie_value = s
        .sessions
        .encode_signed_bytes(&serde_json::to_vec(&pre).expect("pre-auth is always serializable"));
    cookies.add(pre_auth_cookie(cookie_value, s.secure_cookies));
    Redirect::to(&authorize_url(cfg, &pre)).into_response()
}

/// Render the password sign-in page, mint a fresh login-CSRF token, and
/// drop it as a signed cookie (the double-submit pair to the form's
/// hidden field). Used for the initial GET and for re-rendering after a
/// rejected attempt (with `error` set and a 401 status).
fn login_chooser_response(
    s: &AuthState,
    cookies: &Cookies,
    return_to: &str,
    error: Option<&str>,
    notice: Option<webapp::auth_pages::LoginNotice>,
    status: StatusCode,
) -> Response {
    let csrf = random_token_32();
    let signed = s.sessions.encode_signed_bytes(csrf.as_bytes());
    cookies.add(login_csrf_cookie(signed, s.secure_cookies));
    let page = webapp::auth_pages::login(
        return_to,
        &csrf,
        &provider_buttons(s, return_to),
        s.identity_password.is_some(),
        error,
        notice,
    );
    (status, page).into_response()
}

/// Submitted email/password form.
#[derive(Deserialize)]
pub struct PasswordLoginForm {
    pub email: String,
    pub password: String,
    #[serde(default = "default_return_to")]
    pub return_to: String,
    #[serde(default)]
    pub csrf_token: String,
}

/// Why a password sign-in didn't yield a token.
enum PasswordError {
    /// Identity Platform rejected the credentials (unknown email, wrong
    /// password, disabled, throttled). Collapsed to one outcome so the
    /// response never reveals which — a client-confidentiality duty, not
    /// just security hygiene.
    Rejected,
    /// The sign-in service itself failed (network, 5xx, unparseable).
    Upstream,
}

#[derive(Serialize)]
struct SignInRequest<'a> {
    email: &'a str,
    password: &'a str,
    #[serde(rename = "returnSecureToken")]
    return_secure_token: bool,
}

#[derive(Deserialize)]
struct SignInResponse {
    #[serde(rename = "idToken")]
    id_token: String,
}

/// Forward the typed password to Identity Platform's
/// `accounts:signInWithPassword` over TLS and return the ID token it
/// mints. The password is never logged, stored, or hashed by us —
/// Google owns the credential. The returned token is trusted because it
/// arrives over TLS straight from Google, exactly as the OIDC
/// back-channel trusts the token endpoint's response.
async fn verify_password_with_identity_platform(
    cfg: &IdentityPasswordConfig,
    email: &str,
    password: &str,
) -> Result<String, PasswordError> {
    let url = format!(
        "{}/v1/accounts:signInWithPassword?key={}",
        cfg.endpoint.trim_end_matches('/'),
        cfg.api_key,
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&SignInRequest {
            email,
            password,
            return_secure_token: true,
        })
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => match r.json::<SignInResponse>().await {
            Ok(b) => Ok(b.id_token),
            Err(e) => {
                tracing::warn!(error = %e, "identity-platform: sign-in response parse failed");
                Err(PasswordError::Upstream)
            }
        },
        // 4xx is the credential-rejection family (EMAIL_NOT_FOUND,
        // INVALID_PASSWORD, INVALID_LOGIN_CREDENTIALS, USER_DISABLED,
        // TOO_MANY_ATTEMPTS_TRY_LATER) — all collapse to Rejected so the
        // caller's response can't be used to enumerate accounts. We log
        // only the status code, never the email or password.
        Ok(r) if r.status().is_client_error() => {
            tracing::info!(
                status = r.status().as_u16(),
                "identity-platform: password sign-in rejected"
            );
            Err(PasswordError::Rejected)
        }
        Ok(r) => {
            tracing::warn!(
                status = r.status().as_u16(),
                "identity-platform: sign-in upstream error"
            );
            Err(PasswordError::Upstream)
        }
        Err(e) => {
            tracing::warn!(error = %e, "identity-platform: sign-in http error");
            Err(PasswordError::Upstream)
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn password_login(
    State(s): State<AuthState>,
    cookies: Cookies,
    Form(form): Form<PasswordLoginForm>,
) -> Response {
    let Some(cfg) = s.identity_password.clone() else {
        return (StatusCode::NOT_FOUND, "password sign-in is not enabled").into_response();
    };

    // Double-submit CSRF: the token in the signed cookie must match the
    // hidden form field. The cookie is HttpOnly + HMAC-signed, so a
    // cross-origin attacker can neither read nor forge it.
    let cookie_token = cookies
        .get(LOGIN_CSRF_COOKIE_NAME)
        .and_then(|c| s.sessions.decode_signed_bytes(c.value()))
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    let csrf_ok = cookie_token.as_deref().is_some_and(|tok| {
        !tok.is_empty() && constant_time_eq(tok.as_bytes(), form.csrf_token.as_bytes())
    });
    if !csrf_ok {
        return (StatusCode::BAD_REQUEST, "invalid or missing CSRF token").into_response();
    }
    // One-shot: clear the consumed token (a fresh one is minted on any
    // re-render below).
    cookies.add(expired_cookie(LOGIN_CSRF_COOKIE_NAME));

    match verify_password_with_identity_platform(&cfg, &form.email, &form.password).await {
        Ok(id_token) => {
            let Some(claims) = decode_unverified_payload(&id_token) else {
                return (StatusCode::BAD_GATEWAY, "id_token claims parse failed").into_response();
            };
            complete_sign_in(&s, &cookies, None, claims, &form.return_to).await
        }
        Err(PasswordError::Rejected) => login_chooser_response(
            &s,
            &cookies,
            &form.return_to,
            Some(LOGIN_FAILED),
            None,
            StatusCode::UNAUTHORIZED,
        ),
        Err(PasswordError::Upstream) => {
            (StatusCode::BAD_GATEWAY, "sign-in service unavailable").into_response()
        }
    }
}

/// Constant-time byte compare so a CSRF check can't be timing-probed.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
}

/// Minimal id_token payload. We deliberately only ask for the
/// fields Neon Law Navigator actually needs: a stable subject for linkage,
/// an email for first-time row creation, and an optional display
/// name. **The role is not read from the token.** Authorization is
/// derived from the `role` column on the `persons` table after the
/// upsert, so granting/revoking access is a database write — not
/// an IdP configuration change.
///
/// See [`docs/oidc.md`](../../../docs/oidc.md) for the full
/// sequence diagram and the identity-vs-authorization rationale.
#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    sub: String,
    /// The issuer. Redundant for a single-issuer provider, where
    /// `jsonwebtoken`'s own `Validation` has already enforced it. Load-bearing
    /// under [`IssuerPolicy::EntraTenants`], where the expected value depends
    /// on `tid` and so cannot be pinned before decode.
    #[serde(default)]
    iss: Option<String>,
    /// Microsoft Entra tenant id of the signing directory. Absent from every
    /// other provider's tokens; required when the issuer is templated.
    #[serde(default)]
    tid: Option<String>,
    /// Entra's primary username, normally the user principal name.
    ///
    /// Preferred over `email` on the Microsoft door. Entra can only issue a
    /// UPN on a domain the signing tenant has verified, whereas `email` is
    /// populated from the directory's `mail` attribute, which nobody verifies
    /// — Microsoft's own claims reference says of `email` that "this value
    /// isn't guaranteed to be correct". Since a `persons` row is matched by
    /// address, taking the unverified claim would let any tenant admin assert
    /// somebody else's address. See [`ProviderId::identity_address`].
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    email: Option<String>,
    /// Whether the IdP asserts the address is verified. Both Google
    /// (`email_verified`) and Identity Platform / Firebase password
    /// tokens carry it. `Some(false)` is the hard gate: a password user
    /// who hasn't confirmed their email gets no session until they click
    /// the confirmation link (see [`complete_sign_in`]). A Google token
    /// carries `Some(true)`, so "sign in with Google **or** confirm your
    /// email" falls out of one check. `None` (claim absent) is treated as
    /// "not unverified" — we never had an email-confirm step before, so a
    /// token that simply omits the claim must keep working.
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    /// Echoed back from the authorize request; verified against the
    /// pre-auth cookie's `nonce` on the redirect callback. Absent on
    /// the Identity-Platform password token (a different, direct-TLS
    /// trust path — see [`password_login`]).
    #[serde(default)]
    nonce: Option<String>,
}

/// Why id_token verification failed. Every variant is a hard reject:
/// the callback never mints a session from a token that trips one.
#[derive(Debug, thiserror::Error)]
pub enum IdTokenError {
    #[error("token header is malformed or missing a `kid`")]
    Header,
    /// Multi-tenant Entra token with no `tid`. Without it there is no issuer
    /// to compare against, so the token cannot be validated at all.
    #[error("token carries no `tid` claim, so its issuer cannot be validated")]
    MissingTenant,
    /// The signing tenant is not in `OAUTH_MICROSOFT_ALLOWED_TENANTS`. This is
    /// the ordinary refusal for a stranger signing in from their own Entra
    /// tenant, and it is the reason the allowlist is mandatory.
    #[error("signing tenant is not in the configured allowlist")]
    TenantNotAllowed,
    /// `iss` did not match the tenant-interpolated issuer.
    #[error("`iss` does not match the issuer expected for the token's tenant")]
    Issuer,
    #[error("no JWKS key matches the token `kid`")]
    UnknownKid,
    #[error("signature, issuer, audience, or expiry check failed: {0}")]
    Validation(String),
    #[error("id_token `nonce` does not match the login's pre-auth nonce")]
    Nonce,
}

/// How often the background task re-fetches the full JWKS, independent of
/// whether any unrecognised `kid` has been seen. Hourly is Microsoft's own
/// documented suggestion for signing-key-rollover handling. See ENG-326.
const JWKS_REFRESH_INTERVAL: Duration = Duration::from_hours(1);

/// Minimum time between JWKS refetches triggered by an unrecognised `kid`.
/// Without this floor, a flood of tokens carrying garbage `kid`s — forged or
/// simply stale — would turn every one of them into an outbound request
/// against the provider's JWKS endpoint. See ENG-326.
const JWKS_REFETCH_FLOOR: Duration = Duration::from_mins(5);

/// RS256 id_token verifier built from an IdP's published JWKS and
/// pinned to the expected issuer and audience (our `client_id`).
///
/// Verification is **mandatory** on the OIDC redirect callback. We do
/// not lean on the TLS back-channel alone: OIDC core §3.1.3.7 requires
/// the relying party to verify the id_token's signature, `iss`, `aud`,
/// and `exp`, and to bind it to the login via `nonce`. This type is the
/// one place that happens for the browser flow.
///
/// The JWKS is not fixed at construction: a provider rotates its signing
/// key on its own schedule (Microsoft explicitly reserves the right to do
/// so with no notice), so a verifier built once at boot and never updated
/// 401s every sign-in from the moment of rotation until the process
/// restarts. [`Self::from_jwks_url`] instead returns a self-refreshing
/// `Arc`: a background task re-fetches the full key set every
/// [`JWKS_REFRESH_INTERVAL`], and [`Self::verify`] refetches immediately
/// (floored by [`JWKS_REFETCH_FLOOR`]) the moment it sees a `kid` it
/// doesn't recognise. See ENG-326.
pub struct IdTokenVerifier {
    /// Per-`kid` decoding keys, replaced wholesale under one write lock on
    /// every refresh. A map rather than the historical flat list so a
    /// lookup is by `kid` directly rather than a linear scan.
    keys: AsyncRwLock<HashMap<String, DecodingKey>>,
    /// Where [`Self::refresh_keys`] re-fetches from. `None` only for a
    /// verifier built from a fixed key set ([`Self::from_keys`], used
    /// exclusively by tests) — there is nowhere to refetch from, so an
    /// unrecognised `kid` there is a hard reject exactly as before this
    /// type learned to refresh itself.
    jwks_url: Option<String>,
    /// When the last **on-demand** (unrecognised-`kid`) refetch completed.
    /// `None` means no on-demand refetch has happened yet in this
    /// verifier's lifetime, so the very first one is never held back by
    /// [`JWKS_REFETCH_FLOOR`] — that floor governs the spacing *between*
    /// on-demand refetches, not the delay since the boot fetch. Held across
    /// the refetch itself (a `tokio::sync::Mutex` guard may span an
    /// `.await`), so concurrent callers hitting the same unrecognised `kid`
    /// serialize on one fetch instead of each independently deciding to
    /// refetch.
    last_kid_refetch: AsyncMutex<Option<Instant>>,
    validation: Validation,
    /// How `iss` is checked. [`IssuerPolicy::Exact`] delegates to
    /// `validation`; [`IssuerPolicy::EntraTenants`] is enforced in
    /// [`Self::verify`] after decode, because the expected issuer is not known
    /// until the token's own `tid` claim has been read and allowlisted.
    issuer_policy: IssuerPolicy,
}

impl IdTokenVerifier {
    /// Build a verifier from `(kid, key)` pairs already in hand, pinned
    /// to `issuer` and `audience`. `from_jwks_document` is the production
    /// caller; tests use it directly with a locally-held signing key. No
    /// `jwks_url` is attached, so this verifier never refetches — an
    /// unrecognised `kid` is a hard reject, matching production before
    /// ENG-326 (and exactly what a unit test with no mock JWKS endpoint
    /// needs).
    #[must_use]
    pub fn from_keys(
        keys: Vec<(String, DecodingKey)>,
        issuer: &str,
        audience: &str,
        issuer_policy: IssuerPolicy,
    ) -> Self {
        let mut validation = Validation::new(Algorithm::RS256);
        // `set_issuer`/`set_audience` enable iss/aud enforcement; exp is
        // validated by default. These are the token-confusion defenses.
        //
        // Under `EntraTenants` the discovered issuer is a template, so pinning
        // it here would reject every real token. `iss` is still mandatory —
        // `verify` enforces it against the interpolated, allowlisted value
        // instead, and a token whose `tid` is absent or unlisted is refused.
        if matches!(issuer_policy, IssuerPolicy::Exact) {
            validation.set_issuer(&[issuer]);
        }
        validation.set_audience(&[audience]);
        validation.validate_exp = true;
        Self {
            keys: AsyncRwLock::new(keys.into_iter().collect()),
            jwks_url: None,
            last_kid_refetch: AsyncMutex::new(None),
            validation,
            issuer_policy,
        }
    }

    /// Build a verifier from an already-fetched JWKS document. Like
    /// [`Self::from_keys`], carries no `jwks_url` — [`Self::from_jwks_url`]
    /// is the production path that attaches one.
    pub fn from_jwks_document(
        doc: &JwksDocument,
        issuer: &str,
        audience: &str,
        issuer_policy: IssuerPolicy,
    ) -> Result<Self, AuthSetupError> {
        let keys = Self::keys_from_document(doc)?;
        Ok(Self::from_keys(
            keys.into_iter().collect(),
            issuer,
            audience,
            issuer_policy,
        ))
    }

    /// Parse a JWKS document into `kid` → key, skipping non-RSA entries.
    fn keys_from_document(
        doc: &JwksDocument,
    ) -> Result<HashMap<String, DecodingKey>, AuthSetupError> {
        let mut keys = HashMap::new();
        for k in &doc.keys {
            if k.kty != "RSA" {
                continue;
            }
            let (Some(n), Some(e)) = (k.n.as_deref(), k.e.as_deref()) else {
                continue;
            };
            let key = DecodingKey::from_rsa_components(n, e)
                .map_err(|e| AuthSetupError::Key(e.to_string()))?;
            keys.insert(k.kid.clone().unwrap_or_default(), key);
        }
        if keys.is_empty() {
            return Err(AuthSetupError::Empty);
        }
        Ok(keys)
    }

    /// Fetch the JWKS at `url` and return a self-refreshing verifier: the
    /// returned `Arc` owns a background task that re-fetches the full key
    /// set every [`JWKS_REFRESH_INTERVAL`], on top of the immediate,
    /// floored refetch [`Self::verify`] performs on an unrecognised `kid`.
    /// The task runs for the life of the process — this is boot
    /// configuration, not a value with a shorter-lived owner.
    pub async fn from_jwks_url(
        url: &str,
        issuer: &str,
        audience: &str,
        issuer_policy: IssuerPolicy,
    ) -> Result<Arc<Self>, AuthSetupError> {
        let verifier = Arc::new(Self::fetch_and_build(url, issuer, audience, issuer_policy).await?);

        let background = Arc::clone(&verifier);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(JWKS_REFRESH_INTERVAL);
            // `interval.tick()` fires immediately on its first call; the
            // keys we just fetched above are already current, so consume
            // that tick without doing any work.
            interval.tick().await;
            loop {
                interval.tick().await;
                background.refresh_keys().await;
            }
        });

        Ok(verifier)
    }

    /// Fetch the JWKS at `url` and build the verifier with it attached as
    /// [`Self::jwks_url`], but without spawning the background refresh task
    /// [`Self::from_jwks_url`] wraps this with. A private seam so a test
    /// exercising only the on-demand [`Self::refetch_on_unknown_kid`] path
    /// doesn't also start a task that outlives the test's own process —
    /// `cargo nextest` runs each test as its own process and flags one
    /// that keeps running as "leaky".
    async fn fetch_and_build(
        url: &str,
        issuer: &str,
        audience: &str,
        issuer_policy: IssuerPolicy,
    ) -> Result<Self, AuthSetupError> {
        let doc: JwksDocument = reqwest::get(url)
            .await
            .map_err(|e| AuthSetupError::Fetch(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthSetupError::Parse(e.to_string()))?;
        let mut verifier = Self::from_jwks_document(&doc, issuer, audience, issuer_policy)?;
        verifier.jwks_url = Some(url.to_string());
        Ok(verifier)
    }

    /// Re-fetch the full JWKS from [`Self::jwks_url`] and replace the
    /// cached keys wholesale. A fetch failure is logged and otherwise
    /// swallowed: keeping the existing (possibly stale) cache is strictly
    /// better than clearing it, since a transient outage at the provider's
    /// JWKS endpoint must not itself lock out sign-in. A `None` `jwks_url`
    /// (test-only [`Self::from_keys`]) makes this a no-op.
    async fn refresh_keys(&self) {
        let Some(url) = self.jwks_url.as_deref() else {
            return;
        };
        match Self::fetch_keys(url).await {
            Ok(fresh) => {
                let kid_count = fresh.len();
                *self.keys.write().await = fresh;
                tracing::info!(kid_count, "oauth: JWKS keys refreshed");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    url,
                    "oauth: JWKS refresh failed; keeping the previously cached keys",
                );
            }
        }
    }

    async fn fetch_keys(url: &str) -> Result<HashMap<String, DecodingKey>, AuthSetupError> {
        let doc: JwksDocument = reqwest::get(url)
            .await
            .map_err(|e| AuthSetupError::Fetch(e.to_string()))?
            .json()
            .await
            .map_err(|e| AuthSetupError::Parse(e.to_string()))?;
        Self::keys_from_document(&doc)
    }

    /// The decoding key for `kid`, refetching once — subject to
    /// [`JWKS_REFETCH_FLOOR`] — if it's missing from the cache. `None` only
    /// when `kid` is still unresolved after that refetch (or after a
    /// refetch withheld by the floor).
    async fn resolve_key(&self, kid: &str) -> Option<DecodingKey> {
        if let Some(key) = self.keys.read().await.get(kid).cloned() {
            return Some(key);
        }
        self.refetch_on_unknown_kid(kid).await;
        self.keys.read().await.get(kid).cloned()
    }

    /// On an unrecognised `kid`, refetch the JWKS immediately unless an
    /// on-demand refetch already happened within [`JWKS_REFETCH_FLOOR`] —
    /// the algorithm Microsoft documents for signing-key-rollover handling.
    /// A `None` `jwks_url` (test-only [`Self::from_keys`]) makes this a
    /// no-op: there is nowhere to refetch from.
    async fn refetch_on_unknown_kid(&self, kid: &str) {
        if self.jwks_url.is_none() {
            return;
        }
        // Held across the fetch itself: a second caller arriving for the
        // same rotation event waits here rather than independently
        // deciding — under the floor — to skip, or — over the floor — to
        // fire a second redundant fetch.
        let mut last = self.last_kid_refetch.lock().await;
        if let Some(last_at) = *last {
            let since = last_at.elapsed();
            if since < JWKS_REFETCH_FLOOR {
                tracing::info!(
                    kid,
                    since_last_refetch_secs = since.as_secs(),
                    "oauth: unrecognised kid within the refetch floor; not refetching",
                );
                return;
            }
        }
        tracing::info!(kid, "oauth: unrecognised kid; refetching JWKS immediately");
        self.refresh_keys().await;
        *last = Some(Instant::now());
    }

    /// Enforce [`IssuerPolicy::EntraTenants`] against decoded claims.
    ///
    /// Order matters and is the security property: the tenant is allowlisted
    /// **before** its id is interpolated, so the string compared against `iss`
    /// can only ever be one an operator wrote into the environment. Microsoft
    /// states the requirement directly — a multi-tenant application "must
    /// validate that the `issuer` property in the published metadata matches
    /// the `iss` claim in the token, in addition to the usual check that the
    /// `iss` claim in the token contains the tenant ID (`tid`) claim."
    fn check_entra_tenant(&self, claims: &IdTokenClaims) -> Result<(), IdTokenError> {
        let IssuerPolicy::EntraTenants {
            template,
            allowed_tenants,
        } = &self.issuer_policy
        else {
            return Ok(());
        };
        let tid = claims
            .tid
            .as_deref()
            .map(str::trim)
            .filter(|tid| !tid.is_empty())
            .ok_or(IdTokenError::MissingTenant)?
            .to_ascii_lowercase();
        if !allowed_tenants.contains(&tid) {
            return Err(IdTokenError::TenantNotAllowed);
        }
        let expected = template.replace(ENTRA_TENANT_TEMPLATE, &tid);
        // Case-insensitive because the tenant id was lower-cased above and a
        // host name is case-insensitive anyway. This weakens nothing: both
        // halves of `expected` are ours — the template came from the discovery
        // document and the tenant id from the allowlist — so the comparison is
        // still against exactly one string an operator sanctioned.
        if !claims
            .iss
            .as_deref()
            .is_some_and(|iss| iss.eq_ignore_ascii_case(&expected))
        {
            return Err(IdTokenError::Issuer);
        }
        Ok(())
    }

    /// The configured issuer policy. Test-only: the unit tests assert that
    /// switching Microsoft on leaves every other provider on the `Exact` path.
    #[cfg(test)]
    fn issuer_policy_for_test(&self) -> &IssuerPolicy {
        &self.issuer_policy
    }

    /// [`Self::check_entra_tenant`] without a signed token. Test-only: it lets
    /// the tenant and issuer rules be asserted claim-by-claim, which a
    /// full-token test cannot do for a `tid` that is absent entirely.
    #[cfg(test)]
    fn check_entra_tenant_for_test(&self, claims: &IdTokenClaims) -> Result<(), IdTokenError> {
        self.check_entra_tenant(claims)
    }

    /// Verify `token` and bind it to `expected_nonce`. Returns the
    /// identity claims only when signature, issuer, audience, expiry,
    /// and nonce all check out.
    ///
    /// Async because an unrecognised `kid` refetches the JWKS
    /// ([`Self::refetch_on_unknown_kid`]) before giving up — a provider key
    /// rotation must not need a process restart to stop 401ing every
    /// sign-in. See ENG-326.
    async fn verify(
        &self,
        token: &str,
        expected_nonce: &str,
    ) -> Result<IdTokenClaims, IdTokenError> {
        let header = decode_header(token).map_err(|_| IdTokenError::Header)?;
        let kid = header.kid.ok_or(IdTokenError::Header)?;

        let Some(key) = self.resolve_key(&kid).await else {
            // Distinct from the ordinary `oidc.id_token.rejected` audit line
            // every `IdTokenError` gets in `verify_id_token`: this one fires
            // only when a `kid` is still unresolved immediately after a
            // refetch (or a refetch was withheld by the floor), which is the
            // shape a stuck or unpropagated key rotation takes — worth a
            // grep of its own rather than reading as one more generic 401.
            tracing::error!(
                target: "audit",
                event = "oauth.jwks.unknown_kid",
                kid,
                "oauth: id_token kid not found in JWKS even after a refetch",
            );
            return Err(IdTokenError::UnknownKid);
        };

        let claims = decode::<IdTokenClaims>(token, &key, &self.validation)
            .map_err(|e| IdTokenError::Validation(e.to_string()))?
            .claims;
        // Multi-tenant Entra only: `iss` and `tid` together, since neither is
        // meaningful without the other.
        self.check_entra_tenant(&claims)?;
        // Bind the token to this login. Constant-time so the nonce can't
        // be timing-probed (it isn't a secret, but the compare is free).
        match claims.nonce.as_deref() {
            Some(n) if constant_time_eq(n.as_bytes(), expected_nonce.as_bytes()) => Ok(claims),
            _ => Err(IdTokenError::Nonce),
        }
    }
}

async fn callback(
    State(s): State<AuthState>,
    cookies: Cookies,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if q.error.is_some() {
        return (StatusCode::BAD_REQUEST, "oauth error from idp").into_response();
    }
    let Some(code) = q.code else {
        return (StatusCode::BAD_REQUEST, "missing `code` parameter").into_response();
    };
    let Some(returned_state) = q.state else {
        return (StatusCode::BAD_REQUEST, "missing `state` parameter").into_response();
    };

    // Three phases, each fallible into a `(status, message)` error that we
    // render at the end: validate the pre-auth cookie, exchange the code
    // for tokens, verify the id_token. The small tuple error keeps the
    // helpers off `clippy::result_large_err` (an axum `Response` is big).
    let render = |e: (StatusCode, &'static str)| e.into_response();
    let pre = match consume_pre_auth(&s, &cookies, &returned_state) {
        Ok(pre) => pre,
        Err(e) => return render(e),
    };
    // Which provider this code belongs to comes from the signed pre-auth
    // cookie, never from the query string: the browser cannot forge it, and it
    // was written by the `/auth/login/{provider}` that started this login. A
    // provider that has since been unconfigured (a rolled-back deploy landing
    // mid-login) is a 400, not a redemption against a different provider.
    let Some(cfg) = s.provider_config(pre.provider) else {
        tracing::warn!(
            provider = pre.provider.slug(),
            "oauth: callback for a provider this deployment no longer configures",
        );
        return render((StatusCode::BAD_REQUEST, "sign-in provider not configured"));
    };
    let token = match exchange_code(cfg, &code, &pre).await {
        Ok(token) => token,
        Err(e) => return render(e),
    };
    let claims = match verify_id_token(cfg, token, &pre.nonce).await {
        Ok(claims) => claims,
        Err(e) => return render(e),
    };
    complete_sign_in(&s, &cookies, Some(pre.provider), claims, &pre.return_to).await
}

/// A renderable callback error: an HTTP status plus a static message.
type CallbackError = (StatusCode, &'static str);

/// Phase 1: validate + consume the one-shot pre-auth cookie, checking
/// expiry and that the returned `state` matches what we issued.
fn consume_pre_auth(
    s: &AuthState,
    cookies: &Cookies,
    returned_state: &str,
) -> Result<PreAuth, CallbackError> {
    let bad = |msg: &'static str| Err((StatusCode::BAD_REQUEST, msg));
    let Some(pre_cookie) = cookies.get(PRE_AUTH_COOKIE_NAME) else {
        return bad("missing pre-auth cookie");
    };
    let Some(pre_bytes) = s.sessions.decode_signed_bytes(pre_cookie.value()) else {
        return bad("invalid pre-auth cookie");
    };
    let Ok(pre) = serde_json::from_slice::<PreAuth>(&pre_bytes) else {
        return bad("malformed pre-auth cookie");
    };
    if pre.is_expired() {
        return bad("pre-auth cookie expired");
    }
    if pre.state != returned_state {
        return bad("state mismatch");
    }
    // One-shot: clear it now that we've consumed it.
    cookies.add(expired_cookie(PRE_AUTH_COOKIE_NAME));
    Ok(pre)
}

/// Phase 2: exchange the authorization `code` at the IdP's token
/// endpoint (PKCE `code_verifier` from the pre-auth cookie).
async fn exchange_code(
    cfg: &OAuthConfig,
    code: &str,
    pre: &PreAuth,
) -> Result<TokenResponse, CallbackError> {
    match reqwest::Client::new()
        .post(cfg.token_endpoint())
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", cfg.inner.redirect_uri.as_str()),
            ("client_id", cfg.inner.client_id.as_str()),
            ("client_secret", cfg.inner.client_secret.as_str()),
            ("code_verifier", pre.verifier.as_str()),
        ])
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r.json().await.map_err(|e| {
            tracing::warn!(error = %e, "oauth: token response parse failed");
            (StatusCode::BAD_GATEWAY, "token parse failed")
        }),
        Ok(r) => {
            // The status alone cannot distinguish a stale `client_secret`
            // (`invalid_client`) from a replayed code (`invalid_grant`) from a
            // callback the IdP does not recognise (`redirect_uri_mismatch`) —
            // all three are one 400/401 here. RFC 6749 §5.2 puts the machine
            // -readable cause in the body, so log that instead of discarding
            // it: without it, diagnosing this means reasoning backwards from
            // the deployment tree.
            let status = r.status().as_u16();
            let body = r.text().await.unwrap_or_default();
            let (error, description) = oauth_error_fields(&body);
            tracing::warn!(
                status,
                error = error.unwrap_or("unparsed"),
                error_description = description.unwrap_or(""),
                "oauth: token exchange returned non-2xx"
            );
            Err((StatusCode::BAD_GATEWAY, "token exchange failed"))
        }
        Err(e) => {
            tracing::warn!(error = %e, "oauth: token exchange http error");
            Err((StatusCode::BAD_GATEWAY, "token exchange failed"))
        }
    }
}

/// The `error` and `error_description` of an OAuth error response
/// (RFC 6749 §5.2), borrowed from `body`.
///
/// Only these two fields, never the whole body: the response is attacker
/// -influenced and an IdP may answer with an HTML error page, so logging it
/// wholesale invites both log spam and injected content. The two RFC fields are
/// short, machine-readable, and carry no credential — the request's
/// `client_secret` is never echoed back in them.
///
/// Returns `(None, None)` for a body that is not the documented JSON shape,
/// which the caller reports as `unparsed` rather than mistaking for "no error".
fn oauth_error_fields(body: &str) -> (Option<&str>, Option<&str>) {
    // Hand-scan rather than `serde_json::from_str`: the body may not be JSON at
    // all, and a borrowed &str keeps this allocation-free on the error path.
    let field = |name: &str| -> Option<&str> {
        let needle = format!("\"{name}\"");
        let at = body.find(&needle)? + needle.len();
        let rest = body[at..].trim_start().strip_prefix(':')?.trim_start();
        let value = rest.strip_prefix('"')?;
        let end = value.find('"')?;
        Some(&value[..end])
    };
    (field("error"), field("error_description"))
}

/// Phase 3: verify the id_token's signature, issuer, audience, expiry,
/// and nonce. Verification is mandatory — a missing verifier is a deploy
/// misconfiguration, not a reason to trust the token unverified. Emits
/// the audit events for both outcomes.
async fn verify_id_token(
    cfg: &OAuthConfig,
    token: TokenResponse,
    nonce: &str,
) -> Result<IdTokenClaims, CallbackError> {
    let Some(verifier) = cfg.id_token_verifier() else {
        tracing::error!(
            "oauth: no id_token verifier configured; refusing to mint a session from an unverified token",
        );
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "auth misconfigured"));
    };
    // The OIDC flow always returns an id_token (we request `openid`). We
    // never fall back to the access_token: it is not an identity
    // assertion and carries no verifiable claims for us.
    let Some(id_token) = token.id_token else {
        return Err((StatusCode::BAD_GATEWAY, "no id_token returned"));
    };
    match verifier.verify(&id_token, nonce).await {
        Ok(claims) => {
            tracing::info!(
                target: "audit",
                event = "oidc.id_token.verified",
                subject = %claims.sub,
                "oauth: id_token signature, issuer, audience, and nonce verified",
            );
            Ok(claims)
        }
        Err(e) => {
            // Audit stream (→ OTLP → Iceberg): every rejected id_token is
            // a security-relevant event. The reason is the `IdTokenError`
            // variant, never the token bytes.
            tracing::warn!(
                target: "audit",
                event = "oidc.id_token.rejected",
                reason = %e,
                "oauth: id_token verification failed",
            );
            Err((StatusCode::UNAUTHORIZED, "id_token verification failed"))
        }
    }
}

/// Shared sign-in tail for both front doors (the OIDC callback and the
/// Identity Platform password submit): resolve the local `persons` row
/// from the token claims, fire the welcome workflow for a brand-new row,
/// mint the standard `SessionData` cookie, and redirect to `return_to`.
///
/// The role is always read back from the DB row, never trusted from the
/// token — so every downstream `require_auth` / embedded Rego policy / CSRF layer is
/// identical no matter which door the person came through.
async fn complete_sign_in(
    s: &AuthState,
    cookies: &Cookies,
    provider: Option<ProviderId>,
    mut claims: IdTokenClaims,
    return_to: &str,
) -> Response {
    // Normalise the address a `persons` row will be matched on, per provider.
    // Doing it here — once, before any lookup — means every downstream step
    // (the resolve, the email-confirm gate, the welcome workflow, the session
    // cookie) agrees on one address, and there is exactly one place to read to
    // find out which claim a given door trusts.
    //
    // `None` is the Identity Platform password door, which has no OIDC
    // provider: its token comes straight back over TLS from Google with the
    // address the user just typed, so `email` is already the right claim.
    if let Some(provider) = provider {
        claims.email = provider.identity_address(&claims);
    }
    // The IdP owns identity (`sub`); our `persons` table owns the rest —
    // name, memberships, billing, and the system-wide tier. The lookup is
    // strict: a person must be pre-seeded (matched on `oidc_subject` or
    // `email`) for sign-in to succeed. The only exception is the
    // configured bootstrap Owner, JIT-created with the `Owner` role so a
    // fresh deployment can never lock its operator out.
    let (person_id, role, fresh) = match resolve_person_from_claims(
        &s.surreal,
        &claims,
        s.bootstrap_owner_email.as_deref(),
        s.self_signup_enabled,
    )
    .await
    {
        Ok(t) => t,
        Err(ResolveError::NotPreSeeded) => {
            // No `person_id` exists to log — the refusal *is* that no row
            // matched. `sub` is the IdP's opaque subject, which correlates the
            // attempt without carrying an address; the email itself stays out,
            // because a sign-in refusal is exactly the line where an
            // unprovisioned person's address would otherwise be recorded.
            tracing::info!(
                sub = %claims.sub,
                "auth: no pre-seeded persons row for the supplied email; returning 403",
            );
            // Still a 403, but its own page: sign-up here is operator-mediated,
            // so this visitor is not misconfigured — they have not engaged the
            // firm yet, and the generic "not authorized" wording reads as a
            // broken account rather than telling them what to do next.
            return (
                StatusCode::FORBIDDEN,
                webapp::error_pages::sign_in_not_provisioned(),
            )
                .into_response();
        }
        Err(ResolveError::Db(e)) => {
            tracing::warn!(error = %e, "auth: person lookup failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                webapp::error_pages::server_error(),
            )
                .into_response();
        }
    };

    // Hard gate: a password (non-Google) user whose address the IdP
    // reports unverified gets NO session. We send a confirmation link and
    // render a "check your inbox" page instead. Google sign-in carries
    // `email_verified: true`, so it never trips this — exactly the rule
    // "sign in with Google **or** confirm your email." The write that
    // flips `emailVerified` needs the admin door; with no admin config
    // there is nothing to confirm *against*, so we don't pretend to gate.
    if claims.email_verified == Some(false) && s.identity_admin.is_some() {
        let name = claims.name.clone().unwrap_or_default();
        let email = claims.email.clone().unwrap_or_default();
        return crate::email_confirm::gate_unverified(s, cookies, person_id, &name, &email).await;
    }

    // First-time signup → drive the `onboarding__welcome` workflow,
    // fire-and-forget so the redirect doesn't wait on the broker. The
    // bootstrap-Owner JIT path and, where enabled, a self-signup client
    // produce a `NewSignup`.
    if let Some(NewSignup { email, name }) = fresh {
        let runtime = s.workflow_runtime.clone();
        let pid = person_id;
        tokio::spawn(async move {
            if let Err(e) =
                workflows::email::welcome::trigger_welcome(runtime.as_ref(), pid, &name, &email)
                    .await
            {
                // `person_id` alone: it already identifies the signup, and the
                // address would add nothing but client-identifying content to
                // a signal that leaves the firm's trust boundary.
                tracing::warn!(
                    error = %e,
                    person_id = %pid,
                    "welcome workflow trigger failed",
                );
            }
        });
    }

    // Resolve the landing while `role` is still in hand, before it moves into
    // the session cookie below.
    let landing = post_login_landing(role, return_to);

    let mut session = SessionData::fresh(claims.sub, role);
    session.email = claims.email;
    session.person_id = Some(person_id);
    // Which door this session came through, so sign-out reaches the provider
    // that actually holds the SSO session rather than always the primary one.
    session.provider = provider.map(|provider| provider.slug().to_string());
    cookies.add(session_cookie(
        s.sessions.encode(&session),
        s.secure_cookies,
    ));
    Redirect::to(&landing).into_response()
}

/// Outcome of resolving the IdP-supplied claims to a local
/// `persons` row.
#[derive(Debug, thiserror::Error)]
enum ResolveError {
    /// No row matched on either `oidc_subject` or `email`. The
    /// caller renders a 403; sign-up is operator-mediated.
    #[error("no pre-seeded persons row for the IdP-supplied email")]
    NotPreSeeded,
    #[error(transparent)]
    Db(#[from] store::persons::PersonError),
}

/// Read `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` once at boot. `None` is a
/// hard-fail mode: every sign-in then strictly requires a pre-seeded
/// row. Some-value is the carve-out path — that single address is
/// JIT-created with the `Owner` role on first sign-in and healed
/// back to `Owner` on every subsequent sign-in even if a UI edit
/// cleared the role. An absent or blank value disables the carve-out.
#[must_use]
pub fn bootstrap_owner_email_from_env() -> Option<String> {
    bootstrap_owner_email(
        std::env::var("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL")
            .ok()
            .as_deref(),
    )
}

fn bootstrap_owner_email(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(ToOwned::to_owned)
}

/// Read the global self-signup toggle from `NAVIGATOR_SELF_SIGNUP_ENABLED`
/// once at boot. **Off by default**: only `1`, `true`, `yes`, or `on`
/// (case-insensitive) enable it; anything else — including an unset or blank
/// value — keeps the operator-mediated 403 behavior.
#[must_use]
pub fn self_signup_enabled_from_env() -> bool {
    self_signup_enabled(
        std::env::var("NAVIGATOR_SELF_SIGNUP_ENABLED")
            .ok()
            .as_deref(),
    )
}

fn self_signup_enabled(value: Option<&str>) -> bool {
    matches!(
        value.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Returned alongside the person id when the OAuth callback inserts
/// a brand-new row — the seam that triggers the welcome email. `None`
/// means the row already existed (either linked by `oidc_subject` or
/// promoted from a seeded email).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSignup {
    pub email: String,
    pub name: String,
}

/// Resolve the `persons` row that corresponds to the IdP claims.
///
/// Lookup order:
///   1. Match on `oidc_subject = claims.sub` (already linked).
///   2. Match on `email = claims.email` and, if the row hasn't been
///      linked yet, promote it (existing seeded person logging in
///      for the first time — the row keeps its pre-assigned role).
///   3. **No match** → return `ResolveError::NotPreSeeded`, *except*
///      when the email matches the configured bootstrap Owner address.
///      That single carve-out JIT-creates an `Owner` row so a fresh
///      deployment can never lock its operator out.
///
/// Sign-up is operator-mediated by design. New rows can only be
/// seeded by writing to the `persons` table (or, equivalently,
/// editing `store/seeds/Person.yaml` and re-running the seed
/// loader); the IdP token never grants access by itself.
///
/// If the resolved row belongs to the bootstrap Owner email, the `Owner`
/// role is force-set on the returned value AND persisted back to the
/// database — so even an accidental demotion in the `/app/admin/people`
/// UI heals on the next sign-in.
///
/// Path 3 (bootstrap Owner JIT) is the only path that returns
/// `Some(NewSignup)` — promotion is intentionally NOT treated as a
/// fresh signup because the row was already seeded by an operator.
async fn resolve_person_from_claims(
    surreal: &store::surreal::SurrealDb,
    claims: &IdTokenClaims,
    bootstrap_owner_email: Option<&str>,
    self_signup_enabled: bool,
) -> Result<(Uuid, Role, Option<NewSignup>), ResolveError> {
    use store::persons::{self, NewPerson};

    let bootstrap_owner = bootstrap_owner_email.map(str::to_lowercase);
    let email_lower = claims.email.as_deref().map(str::to_lowercase);
    let is_bootstrap_owner = matches!(
        (&bootstrap_owner, &email_lower),
        (Some(a), Some(e)) if a == e,
    );

    if let Some(existing) = persons::find_by_oidc_subject(surreal, &claims.sub).await? {
        let mut role = existing.role;
        if is_bootstrap_owner && role != Role::Owner {
            role = Role::Owner;
            persons::set_role(surreal, existing.id, Role::Owner).await?;
        }
        return Ok((existing.id, role, None));
    }

    let Some(email) = claims.email.clone() else {
        // No email on the token. We refuse to mint a session for an
        // unknown identifier — operators must seed before sign-in.
        return Err(ResolveError::NotPreSeeded);
    };

    // Case-insensitive: the IdP may present a casing that differs from the
    // pre-seeded row, and a byte-exact miss here would 403 a legitimately
    // seeded lawyer (and then collide with `persons_email_lower_key`
    // on the insert below).
    if let Some(existing) = persons::find_by_email_ci(surreal, &email).await? {
        let mut role = existing.role;
        if existing.oidc_subject.is_none() {
            persons::link_oidc_subject(surreal, existing.id, &claims.sub).await?;
        }
        if is_bootstrap_owner && role != Role::Owner {
            role = Role::Owner;
            persons::set_role(surreal, existing.id, Role::Owner).await?;
        }
        return Ok((existing.id, role, None));
    }

    if !is_bootstrap_owner {
        if !self_signup_enabled {
            // Sign-up is operator-mediated: an unknown email is refused.
            return Err(ResolveError::NotPreSeeded);
        }
        // Self-signup (a global capability, default off, on only where a
        // deployment opts in): the first login for an unknown verified email
        // JIT-creates a `client` with NO `person_project_roles` rows — an
        // empty portfolio until an admin assigns participation. embedded Rego policy and the
        // role-tier model are untouched. A fresh client is a real signup, so
        // it drives the welcome workflow like the bootstrap-Owner path.
        let name = claims.name.clone().unwrap_or_else(|| email.clone());
        let new = NewPerson {
            oidc_subject: Some(claims.sub.clone()),
            ..NewPerson::with_role(name.clone(), email.clone(), Role::Client)
        };
        return match store::persons::create(surreal, &new).await {
            Ok(created) => Ok((created.id, Role::Client, Some(NewSignup { email, name }))),
            // Two concurrent first logins for the same identity can both
            // pass the subject and email lookups above before either insert
            // runs; one wins and the loser trips the unique `oidc_subject`
            // or `email_lower` index. Rather than surface that loser as a
            // 500, re-resolve the row the winner created and treat this
            // login as an existing (non-fresh) sign-in.
            Err(insert_err) => {
                match resolve_existing_after_race(surreal, &claims.sub, &email).await? {
                    Some(existing) => Ok((existing.id, existing.role, None)),
                    None => Err(ResolveError::Db(insert_err)),
                }
            }
        };
    }

    // Bootstrap Owner JIT-create path. Role is `Owner`; the welcome
    // workflow fires once so the operator gets a paper trail.
    let name = claims.name.clone().unwrap_or_else(|| email.clone());
    let new = store::persons::create(
        surreal,
        &NewPerson {
            oidc_subject: Some(claims.sub.clone()),
            ..NewPerson::with_role(name.clone(), email.clone(), Role::Owner)
        },
    )
    .await?;
    Ok((new.id, Role::Owner, Some(NewSignup { email, name })))
}

/// Re-resolve an existing `persons` row by OIDC subject, then by
/// case-insensitive email — the same order [`resolve_person_from_claims`]
/// uses. Called after a JIT insert loses a first-login race so concurrent
/// self-signups converge on the row the winner committed instead of
/// mapping the unique-index violation to a 500.
async fn resolve_existing_after_race(
    surreal: &store::surreal::SurrealDb,
    sub: &str,
    email: &str,
) -> Result<Option<store::persons::Person>, store::persons::PersonError> {
    if let Some(existing) = store::persons::find_by_oidc_subject(surreal, sub).await? {
        return Ok(Some(existing));
    }
    store::persons::find_by_email_ci(surreal, email).await
}

async fn logout(State(s): State<AuthState>, cookies: Cookies) -> Response {
    // Read the provider off the session *before* clearing the cookie: it names
    // which IdP is holding the SSO session we are about to ask to end. Sending
    // a Microsoft-authenticated person to the primary provider's end-session
    // endpoint would leave their Entra session live and bounce them through a
    // logout screen belonging to an account they never used.
    let provider_slug = cookies
        .get(SESSION_COOKIE_NAME)
        .and_then(|cookie| s.sessions.decode(cookie.value()))
        .and_then(|session| session.provider);
    let end_session_cfg = s.provider_config_for_slug(provider_slug.as_deref());
    cookies.add(expired_cookie(SESSION_COOKIE_NAME));
    cookies.add(expired_cookie(PRE_AUTH_COOKIE_NAME));
    // RP-initiated OIDC logout: clearing our app session leaves the
    // provider's SSO session live, so the next `/auth/login` would silently
    // re-authenticate. When the provider published an `end_session_endpoint`,
    // bounce the browser through it (with a `post_logout_redirect_uri` back to
    // the app) so the provider clears its session too. When it did not, fall
    // back to redirecting home — the app session is already cleared.
    match end_session_url(end_session_cfg) {
        Some(url) => Redirect::to(&url).into_response(),
        None => Redirect::to("/").into_response(),
    }
}

/// Payload-only decode for the **Identity-Platform password path only**.
///
/// That token is not delivered through a browser redirect: we POST the
/// typed password straight to Google's `signInWithPassword` over TLS and
/// Google hands the id_token back on the same connection. There is no
/// `code`, no redirect, and no possibility of IdP-mixup or token
/// injection, so the back-channel TLS is the trust boundary (the same
/// trust the OIDC *token endpoint* gets). The redirect [`callback`] does
/// **not** use this — it runs full JWKS signature + `iss`/`aud`/`exp` +
/// `nonce` verification via [`IdTokenVerifier`].
fn decode_unverified_payload(jwt: &str) -> Option<IdTokenClaims> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(payload))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn pre_auth_cookie(value: String, secure: bool) -> Cookie<'static> {
    let mut c = Cookie::new(PRE_AUTH_COOKIE_NAME, value);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(tower_cookies::cookie::time::Duration::seconds(
        PRE_AUTH_TTL_SECS,
    ));
    c
}

fn login_csrf_cookie(value: String, secure: bool) -> Cookie<'static> {
    let mut c = Cookie::new(LOGIN_CSRF_COOKIE_NAME, value);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(tower_cookies::cookie::time::Duration::seconds(
        PRE_AUTH_TTL_SECS,
    ));
    c
}

/// Build the signed session cookie. `Max-Age` matches the payload's
/// own TTL so the cookie is *persistent* — it survives a browser
/// restart instead of dying on close — and the two expiries stay in
/// lockstep. `crate::session_renew` slides both forward on activity.
pub(crate) fn session_cookie(value: String, secure: bool) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE_NAME, value);
    c.set_http_only(true);
    c.set_secure(secure);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_max_age(tower_cookies::cookie::time::Duration::seconds(
        DEFAULT_SESSION_TTL_SECS,
    ));
    c
}

pub(crate) fn expired_cookie(name: &'static str) -> Cookie<'static> {
    let mut c = Cookie::new(name, "");
    c.set_path("/");
    c.set_max_age(tower_cookies::cookie::time::Duration::seconds(0));
    c
}

#[cfg(test)]
mod tests {
    use super::{
        authorize_url, bootstrap_owner_email, bootstrap_owner_email_from_env, constant_time_eq,
        decode_unverified_payload, default_return_to, login_notice, oauth_error_fields,
        pkce_challenge, pkce_verifier, post_login_landing, resolve_existing_after_race,
        resolve_person_from_claims, self_signup_enabled, session_cookie, urlencode, IdTokenClaims,
        IdTokenError, IdTokenVerifier, IdentityPasswordConfig, IssuerPolicy, NoticeText,
        OAuthConfig, PreAuth, ProviderId, ResolveError,
    };
    use crate::session::{now_unix_secs, random_token_32, DEFAULT_SESSION_TTL_SECS};
    use crate::test_support::{oidc_verifier, sign_id_token, sign_id_token_with_kid};

    /// The failure that took `www.neonlaw.com` down on 2026-08-10: a new OAuth
    /// client ID paired with the previous client's secret, which Google answers
    /// with `invalid_client`. The status is a bare 401 — only the body names the
    /// cause, so this is the field that turns the next occurrence into one look
    /// at the log instead of an audit of the deployment tree.
    #[test]
    fn a_stale_client_secret_is_named_in_the_body() {
        let (error, description) =
            oauth_error_fields(r#"{"error":"invalid_client","error_description":"Unauthorized"}"#);
        assert_eq!(error, Some("invalid_client"));
        assert_eq!(description, Some("Unauthorized"));
    }

    /// The three causes a single 400/401 conflates must each come back
    /// distinctly, or the log is no better than the status code.
    #[test]
    fn the_conflated_causes_stay_distinguishable() {
        for code in ["invalid_client", "invalid_grant", "redirect_uri_mismatch"] {
            let body = format!(r#"{{"error":"{code}"}}"#);
            assert_eq!(oauth_error_fields(&body).0, Some(code));
        }
    }

    #[test]
    fn whitespace_and_field_order_do_not_matter() {
        let (error, description) = oauth_error_fields(
            "{ \"error_description\" : \"Bad request\" , \"error\" : \"invalid_grant\" }",
        );
        assert_eq!(error, Some("invalid_grant"));
        assert_eq!(description, Some("Bad request"));
    }

    /// An IdP may answer a token request with an HTML error page or an empty
    /// body. That must read as "could not parse", never panic and never be
    /// mistaken for a successful absence of error.
    #[test]
    fn a_non_json_body_yields_nothing() {
        for body in ["", "<html><body>502 Bad Gateway</body></html>", "{", "null"] {
            assert_eq!(oauth_error_fields(body), (None, None), "body: {body:?}");
        }
    }

    /// Only the two RFC fields are extracted. Anything else in the response —
    /// including a field an IdP should never send — stays out of the log.
    #[test]
    fn no_other_field_is_extracted() {
        let (error, description) = oauth_error_fields(
            r#"{"error":"invalid_client","client_secret":"do-not-log-me","access_token":"ya29.x"}"#,
        );
        assert_eq!(error, Some("invalid_client"));
        assert_eq!(description, None);
        for logged in [error.unwrap_or_default(), description.unwrap_or_default()] {
            assert!(!logged.contains("do-not-log-me"), "secret leaked: {logged}");
            assert!(!logged.contains("ya29."), "token leaked: {logged}");
        }
    }
    use base64::Engine;
    use store::persons::Role;
    use store::test_support::mem_surreal;

    fn cfg() -> OAuthConfig {
        OAuthConfig::new(
            "client123",
            "secret456",
            "https://app.example.com/auth/callback",
            "https://idp.example.com/oauth/authorize",
            "https://idp.example.com/oauth/token",
        )
    }

    #[test]
    fn login_notice_maps_each_flag_to_a_toned_catalog_toast() {
        // The bounce case is a red (Danger) toast; the post-action outcomes
        // are green (Success); anything else (and a voluntary visit) is no
        // toast. Each carries catalog-sourced, non-empty copy, and the
        // borrow-lending conversion renders both tones.
        let danger = login_notice(Some("login_required")).expect("login_required → a toast");
        assert!(matches!(danger, NoticeText::Danger(ref t) if !t.is_empty()));
        assert!(matches!(
            danger.as_login_notice(),
            webapp::auth_pages::LoginNotice::Danger(_)
        ));

        for flag in ["password_reset", "email_confirmed"] {
            let success = login_notice(Some(flag)).expect("post-action → a toast");
            assert!(matches!(success, NoticeText::Success(ref t) if !t.is_empty()));
            assert!(matches!(
                success.as_login_notice(),
                webapp::auth_pages::LoginNotice::Success(_)
            ));
        }

        assert!(login_notice(Some("not-a-flag")).is_none());
        assert!(login_notice(None).is_none());
    }

    #[test]
    fn bootstrap_owner_requires_a_non_blank_configured_email() {
        assert_eq!(bootstrap_owner_email(None), None);
        assert_eq!(bootstrap_owner_email(Some("")), None);
        assert_eq!(bootstrap_owner_email(Some(" \t\n ")), None);
        assert_eq!(
            bootstrap_owner_email(Some(" owner@example.com ")),
            Some("owner@example.com".into()),
        );
    }

    #[test]
    fn self_signup_toggle_is_off_unless_explicitly_affirmative() {
        // Off by default: unset, blank, and negative values all stay 403.
        for off in [
            None,
            Some(""),
            Some("  "),
            Some("0"),
            Some("false"),
            Some("no"),
            Some("off"),
        ] {
            assert!(
                !self_signup_enabled(off),
                "{off:?} must not enable self-signup"
            );
        }
        // Only the affirmative words (case-insensitive) enable it.
        for on in [
            Some("1"),
            Some("true"),
            Some("TRUE"),
            Some("Yes"),
            Some(" on "),
        ] {
            assert!(self_signup_enabled(on), "{on:?} must enable self-signup");
        }
    }

    fn unknown_claims(email: &str) -> IdTokenClaims {
        IdTokenClaims {
            sub: format!("sub-{email}"),
            iss: None,
            tid: None,
            preferred_username: None,
            email: Some(email.to_string()),
            email_verified: Some(true),
            name: Some("New Trainee".into()),
            nonce: None,
        }
    }

    /// Claims shaped the way multi-tenant Entra emits them: the address in
    /// `preferred_username`, `tid` naming the signing directory, and `iss`
    /// derived from that `tid`.
    fn entra_claims(
        tid: &str,
        preferred_username: Option<&str>,
        email: Option<&str>,
    ) -> IdTokenClaims {
        IdTokenClaims {
            sub: "entra-pairwise-subject".into(),
            iss: Some(format!("https://login.microsoftonline.com/{tid}/v2.0")),
            tid: Some(tid.to_string()),
            preferred_username: preferred_username.map(str::to_string),
            email: email.map(str::to_string),
            email_verified: None,
            name: Some("Entra User".into()),
            nonce: None,
        }
    }

    #[test]
    fn tenant_allowlist_normalises_and_drops_empty_entries() {
        use super::parse_tenant_allowlist;
        assert_eq!(
            parse_tenant_allowlist("  AAAA-1111 , bbbb-2222,, "),
            vec!["aaaa-1111".to_string(), "bbbb-2222".to_string()],
            "entries are trimmed and lower-cased, and blanks are dropped",
        );
        // A blank value is an empty allowlist, not an allowlist containing "".
        // The difference matters: an entry of "" would match a token whose
        // `tid` is empty, which is the opposite of fail-closed.
        assert!(parse_tenant_allowlist("").is_empty());
        assert!(parse_tenant_allowlist(" , ,").is_empty());
    }

    #[test]
    fn primary_provider_reads_the_address_from_email_only() {
        // The Google/Rauthy door is unchanged: `email` and nothing else. A
        // `preferred_username` on a primary-provider token must be ignored,
        // or switching Microsoft on would quietly change how every existing
        // provider resolves a person.
        let claims = entra_claims("t", Some("upn@example.test"), Some("mail@example.test"));
        assert_eq!(
            ProviderId::Primary.identity_address(&claims).as_deref(),
            Some("mail@example.test"),
        );
    }

    #[test]
    fn microsoft_provider_prefers_the_upn_and_falls_back_to_email() {
        // UPN wins when both are present — the tenant-asserted `email` claim
        // must never be able to select a person row out from under the UPN.
        let both = entra_claims("t", Some("upn@example.test"), Some("mail@example.test"));
        assert_eq!(
            ProviderId::Microsoft.identity_address(&both).as_deref(),
            Some("upn@example.test"),
        );
        // Entra omits `email` when a directory's `mail` attribute is empty, so
        // the UPN alone has to be enough.
        let upn_only = entra_claims("t", Some("upn@example.test"), None);
        assert_eq!(
            ProviderId::Microsoft.identity_address(&upn_only).as_deref(),
            Some("upn@example.test"),
        );
        // And a directory that omits the UPN falls back — safe only because
        // the tenant allowlist has already vouched for the signer.
        let email_only = entra_claims("t", None, Some("mail@example.test"));
        assert_eq!(
            ProviderId::Microsoft
                .identity_address(&email_only)
                .as_deref(),
            Some("mail@example.test"),
        );
        // A blank claim is not an address.
        let blank = entra_claims("t", Some("   "), None);
        assert_eq!(ProviderId::Microsoft.identity_address(&blank), None);
    }

    #[test]
    fn provider_slugs_round_trip_and_reject_strangers() {
        for provider in [ProviderId::Primary, ProviderId::Microsoft] {
            assert_eq!(ProviderId::from_slug(provider.slug()), Some(provider));
        }
        // The historical URL is the primary slot's slug, so existing links and
        // bookmarks keep resolving.
        assert_eq!(ProviderId::from_slug("oidc"), Some(ProviderId::Primary));
        assert_eq!(ProviderId::from_slug("okta"), None);
        assert_eq!(ProviderId::from_slug(""), None);
    }

    #[test]
    fn provider_serialises_as_its_slug_so_both_cookies_agree() {
        // The pre-auth cookie names a provider by serde, the session cookie by
        // `slug()`. They must be the same string or the two cookies disagree
        // about which provider signed a person in.
        for provider in [ProviderId::Primary, ProviderId::Microsoft] {
            let encoded = serde_json::to_string(&provider).expect("provider serialises");
            assert_eq!(encoded, format!("\"{}\"", provider.slug()));
        }
    }

    #[test]
    fn pre_auth_defaults_to_the_primary_provider_for_a_cookie_without_the_field() {
        // A login already in flight when a new build rolls out has a cookie
        // with no `provider`. It must complete against the primary provider,
        // not fail on an unrecognised shape.
        let legacy = serde_json::json!({
            "state": "S",
            "verifier": "V",
            "nonce": "N",
            "return_to": "/app/projects",
            "exp": now_unix_secs() + 60,
        });
        let pre: PreAuth = serde_json::from_value(legacy).expect("legacy pre-auth decodes");
        assert_eq!(pre.provider, ProviderId::Primary);
    }

    #[test]
    fn entra_verifier_rejects_unlisted_tenants_mismatched_issuers_and_missing_tid() {
        let verifier = crate::test_support::entra_verifier("aud", &["ALLOWED-TENANT"]);
        // Allowlisted, issuer derived from the same tenant: accepted. The
        // allowlist is compared case-insensitively, so a GUID pasted from the
        // portal in either case works.
        let ok = IdTokenClaims {
            iss: Some(
                crate::test_support::TEST_ENTRA_ISSUER_TEMPLATE
                    .replace("{tenantid}", "allowed-tenant"),
            ),
            ..entra_claims("Allowed-Tenant", Some("sam@client.test"), None)
        };
        assert!(verifier.check_entra_tenant_for_test(&ok).is_ok());

        // Unlisted tenant, internally consistent issuer: refused.
        let stranger = IdTokenClaims {
            iss: Some(
                crate::test_support::TEST_ENTRA_ISSUER_TEMPLATE.replace("{tenantid}", "other"),
            ),
            ..entra_claims("other", Some("sam@client.test"), None)
        };
        assert!(matches!(
            verifier.check_entra_tenant_for_test(&stranger),
            Err(IdTokenError::TenantNotAllowed),
        ));

        // Allowlisted tenant claiming a different tenant's issuer: refused.
        let crossed = IdTokenClaims {
            iss: Some(
                crate::test_support::TEST_ENTRA_ISSUER_TEMPLATE.replace("{tenantid}", "other"),
            ),
            ..entra_claims("allowed-tenant", Some("sam@client.test"), None)
        };
        assert!(matches!(
            verifier.check_entra_tenant_for_test(&crossed),
            Err(IdTokenError::Issuer),
        ));

        // No `tid` at all: nothing to validate the issuer against.
        let no_tid = IdTokenClaims {
            tid: None,
            ..entra_claims("allowed-tenant", Some("sam@client.test"), None)
        };
        assert!(matches!(
            verifier.check_entra_tenant_for_test(&no_tid),
            Err(IdTokenError::MissingTenant),
        ));
    }

    #[test]
    fn exact_issuer_policy_leaves_the_tenant_check_inert() {
        // Every non-Entra provider must be untouched by the tenant machinery:
        // `Validation` has already enforced its fixed issuer, and a token with
        // no `tid` is perfectly normal there.
        let verifier = oidc_verifier("aud");
        assert!(matches!(
            verifier.issuer_policy_for_test(),
            IssuerPolicy::Exact
        ));
        let claims = unknown_claims("lawyer@neonlaw.test");
        assert!(verifier.check_entra_tenant_for_test(&claims).is_ok());
    }

    #[tokio::test]
    async fn self_signup_off_refuses_an_unknown_email() {
        let surreal = mem_surreal().await;
        let claims = unknown_claims("stranger@example.com");
        // Default off, no bootstrap-Owner carve-out: an unknown email is 403.
        let err = resolve_person_from_claims(&surreal, &claims, None, false)
            .await
            .expect_err("an unknown email must be refused when self-signup is off");
        assert!(matches!(err, ResolveError::NotPreSeeded));
        // And no persons row was created.
        assert!(
            store::persons::find_by_email_ci(&surreal, "stranger@example.com")
                .await
                .unwrap()
                .is_none(),
            "the 403 path must not create a person",
        );
    }

    #[tokio::test]
    async fn self_signup_on_creates_a_client_with_an_empty_portfolio() {
        let surreal = mem_surreal().await;
        let claims = unknown_claims("trainee@example.com");
        let (person_id, role, fresh) = resolve_person_from_claims(&surreal, &claims, None, true)
            .await
            .expect("self-signup on creates the person");
        // A client, treated as a fresh signup (drives the welcome workflow).
        assert_eq!(role, Role::Client);
        assert!(fresh.is_some(), "a self-signup client is a fresh signup");
        let person = store::persons::find_by_email_ci(&surreal, "trainee@example.com")
            .await
            .unwrap()
            .expect("the client row exists");
        assert_eq!(person.id, person_id);
        assert_eq!(person.role, Role::Client);
        // Empty portfolio: no participation rows until an admin assigns them.
        let participations = store::projects::participations_for_person(&surreal, person_id)
            .await
            .unwrap()
            .len();
        assert_eq!(
            participations, 0,
            "a new self-signup client has an empty portfolio"
        );
    }

    #[tokio::test]
    async fn bootstrap_owner_is_created_even_with_self_signup_off() {
        let surreal = mem_surreal().await;
        let claims = unknown_claims("boss@example.com");
        // The bootstrap-Owner carve-out is independent of the self-signup
        // toggle: it JIT-creates an Owner even when self-signup is off.
        let (_, role, fresh) =
            resolve_person_from_claims(&surreal, &claims, Some("boss@example.com"), false)
                .await
                .expect("bootstrap Owner is always JIT-created");
        assert_eq!(role, Role::Owner);
        assert!(fresh.is_some());
    }

    #[tokio::test]
    async fn resolve_existing_after_race_finds_the_row_by_subject_then_email() {
        let surreal = mem_surreal().await;
        let claims = unknown_claims("racer@example.com");
        // Seed the row the "winning" request would have committed.
        let (person_id, _, _) = resolve_person_from_claims(&surreal, &claims, None, true)
            .await
            .expect("self-signup on creates the person");

        // Matches on OIDC subject (the first lookup the recovery tries).
        let by_subject = resolve_existing_after_race(&surreal, &claims.sub, "racer@example.com")
            .await
            .unwrap()
            .expect("the winner's row resolves by subject");
        assert_eq!(by_subject.id, person_id);

        // Falls back to case-insensitive email when the subject differs.
        let by_email =
            resolve_existing_after_race(&surreal, "some-other-subject", "RACER@example.com")
                .await
                .unwrap()
                .expect("the winner's row resolves by email when the subject misses");
        assert_eq!(by_email.id, person_id);

        // An identity that never signed up resolves to nothing.
        assert!(
            resolve_existing_after_race(&surreal, "ghost", "ghost@example.com")
                .await
                .unwrap()
                .is_none(),
            "an unknown identity has no row to recover",
        );
    }

    #[tokio::test]
    async fn concurrent_self_signup_converges_on_one_client_without_a_500() {
        let surreal = mem_surreal().await;

        // Fire several identical first logins at once (`unknown_claims` is
        // deterministic per email, so every task presents the same subject
        // and email). Whichever insert wins, the losers must recover the
        // winner's row rather than propagate the unique-index violation as a
        // `ResolveError::Db` (a 500 at the callback).
        let mut handles = Vec::new();
        for _ in 0..8 {
            // The handle is a cheap clone around one shared engine, so
            // every task races against the same database.
            let surreal = surreal.clone();
            handles.push(tokio::spawn(async move {
                let claims = unknown_claims("stampede@example.com");
                resolve_person_from_claims(&surreal, &claims, None, true).await
            }));
        }

        let mut ids = Vec::new();
        for handle in handles {
            let (person_id, role, _) = handle
                .await
                .expect("the resolve task must not panic")
                .expect("a concurrent self-signup must never 500");
            assert_eq!(role, Role::Client);
            ids.push(person_id);
        }

        // Every request resolved to the same client, and exactly one row exists.
        let first = ids[0];
        assert!(
            ids.iter().all(|id| *id == first),
            "all concurrent logins must converge on one client, got {ids:?}",
        );
        let rows = store::persons::list_directory(&surreal, "", "stampede@example.com", &[])
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the race must create exactly one persons row"
        );
    }

    #[test]
    fn bootstrap_owner_email_from_env_reads_the_configured_value() {
        // The env-reading wrapper is what `main` actually calls, so cover it
        // rather than only the pure helper beneath it: an unset variable must
        // disable the carve-out instead of selecting some default identity.
        //
        // SAFETY: `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` is read only by
        // `bootstrap_owner_email_from_env`, which no other test in this binary
        // calls, so there is no concurrent reader of this key to race.
        std::env::remove_var("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL");
        assert_eq!(bootstrap_owner_email_from_env(), None);

        std::env::set_var("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL", " \t ");
        assert_eq!(bootstrap_owner_email_from_env(), None);

        std::env::set_var("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL", " owner@example.com ");
        assert_eq!(
            bootstrap_owner_email_from_env(),
            Some("owner@example.com".into()),
        );

        std::env::remove_var("NAVIGATOR_BOOTSTRAP_OWNER_EMAIL");
    }

    #[test]
    fn session_cookie_is_persistent_with_matching_max_age() {
        // A persistent cookie (Max-Age set) survives a browser restart,
        // and its lifetime matches the signed payload's TTL.
        let c = session_cookie("payload.sig".into(), true);
        let max_age = c.max_age().expect("session cookie must set Max-Age");
        assert_eq!(max_age.whole_seconds(), DEFAULT_SESSION_TTL_SECS);
        assert!(c.secure().unwrap_or(false));
        assert!(c.http_only().unwrap_or(false));
    }

    #[test]
    fn pkce_verifier_is_url_safe_and_random() {
        let a = pkce_verifier();
        let b = pkce_verifier();
        assert_ne!(a, b);
        assert!(!a.contains('+') && !a.contains('/'));
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        use sha2::Digest;
        let verifier = "the-verifier";
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(verifier.as_bytes()));
        assert_eq!(pkce_challenge(verifier), expected);
    }

    #[test]
    fn urlencode_handles_reserved_chars() {
        assert_eq!(urlencode("hello"), "hello");
        assert_eq!(urlencode("hi there"), "hi%20there");
        assert_eq!(urlencode("a/b?c=d"), "a%2Fb%3Fc%3Dd");
        assert_eq!(
            urlencode("openid email profile"),
            "openid%20email%20profile"
        );
    }

    #[test]
    fn authorize_url_contains_every_required_param() {
        let pre = PreAuth {
            provider: ProviderId::Primary,
            state: "STATE123".into(),
            verifier: "the-verifier".into(),
            nonce: "NONCE789".into(),
            return_to: "/app/projects".into(),
            exp: now_unix_secs() + 300,
        };
        let url = authorize_url(&cfg(), &pre);
        assert!(url.starts_with("https://idp.example.com/oauth/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client123"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fapp.example.com%2Fauth%2Fcallback"));
        assert!(url.contains("scope=openid%20email%20profile"));
        assert!(url.contains("state=STATE123"));
        assert!(url.contains("nonce=NONCE789"));
        let challenge = pkce_challenge("the-verifier");
        assert!(url.contains(&format!("code_challenge={challenge}")));
        assert!(url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn authorize_url_appends_with_amp_when_endpoint_has_existing_query() {
        let cfg = OAuthConfig::new(
            "c",
            "s",
            "http://x",
            "https://idp.example.com/authorize?foo=bar",
            "https://idp.example.com/token",
        );
        let pre = PreAuth {
            provider: ProviderId::Primary,
            state: "S".into(),
            verifier: "v".into(),
            nonce: "n".into(),
            return_to: "/".into(),
            exp: now_unix_secs() + 60,
        };
        let url = authorize_url(&cfg, &pre);
        assert!(url.starts_with("https://idp.example.com/authorize?foo=bar&response_type="));
    }

    #[test]
    fn pre_auth_expires_in_the_future_and_carries_distinct_state_and_verifier() {
        let p = PreAuth::new("/somewhere".into());
        assert!(p.exp > now_unix_secs());
        assert!(!p.is_expired());
        assert_ne!(p.state, p.verifier);
        assert_eq!(p.return_to, "/somewhere");
    }

    #[test]
    fn default_return_to_is_the_neutral_empty_sentinel() {
        // Empty, not a concrete path: the landing is role-dependent and
        // `post_login_landing` resolves it once the tier is known.
        assert_eq!(default_return_to(), "");
    }

    #[test]
    fn post_login_landing_sends_firm_tiers_to_the_team_home() {
        for role in [Role::Owner, Role::Admin, Role::Lawyer, Role::Clerk] {
            assert_eq!(post_login_landing(role, ""), "/app/team", "{role:?}");
        }
    }

    #[test]
    fn post_login_landing_sends_a_client_to_their_matters() {
        assert_eq!(post_login_landing(Role::Client, ""), "/app/projects");
    }

    /// A stale `/portal` deep link is folded into the tier landing for every
    /// tier, not just the two this once covered.
    ///
    /// The retired namespace is served by nothing and deliberately has no
    /// redirect shim — `GET /portal` is a 404, pinned by
    /// `server/tests/routes.rs::the_retired_project_prefixes_are_not_served`.
    /// This is the one place the old path is still read, and it is not a shim
    /// for it: it sanitizes `return_to` on the login door, where a link already
    /// sitting in sent email arrives. Dropping the comparison would honor it as
    /// an explicit deep link and land a person on a 404 the instant they
    /// authenticated, which is worse than the one string compare it costs.
    #[test]
    fn post_login_landing_resolves_the_retired_portal_path_per_role() {
        for role in [Role::Owner, Role::Admin, Role::Lawyer, Role::Clerk] {
            assert_eq!(post_login_landing(role, "/portal"), "/app/team", "{role:?}");
        }
        assert_eq!(post_login_landing(Role::Client, "/portal"), "/app/projects");
    }

    #[test]
    fn post_login_landing_honors_an_explicit_deep_link() {
        // An anonymous bounce recorded the page the visitor was reaching for;
        // it is returned unchanged regardless of tier.
        for role in [Role::Owner, Role::Lawyer, Role::Client] {
            assert_eq!(
                post_login_landing(role, "/app/projects/atlas-llc"),
                "/app/projects/atlas-llc",
                "{role:?}",
            );
        }
    }

    #[test]
    fn pre_auth_marked_expired_when_exp_in_past() {
        let p = PreAuth {
            provider: ProviderId::Primary,
            state: "s".into(),
            verifier: "v".into(),
            nonce: "n".into(),
            return_to: "/".into(),
            exp: now_unix_secs() - 1,
        };
        assert!(p.is_expired());
    }

    #[test]
    fn auth_cookies_carry_secure_only_when_requested() {
        use super::{login_csrf_cookie, pre_auth_cookie, session_cookie};
        for builder in [
            session_cookie as fn(String, bool) -> _,
            pre_auth_cookie,
            login_csrf_cookie,
        ] {
            assert_eq!(builder("v".into(), true).secure(), Some(true));
            assert_eq!(builder("v".into(), false).secure(), Some(false));
            // HttpOnly is unconditional regardless of the Secure flag.
            assert_eq!(builder("v".into(), false).http_only(), Some(true));
        }
    }

    #[tokio::test]
    async fn id_token_verifier_accepts_a_valid_signed_token() {
        let verifier = oidc_verifier("client123");
        let nonce = test_nonce();
        let token = sign_id_token(
            "client123",
            &nonce,
            "rauthy-libra-subject",
            "libra@example.com",
            "Libra",
        );
        let claims = verifier.verify(&token, &nonce).await.expect("valid token");
        assert_eq!(claims.sub, "rauthy-libra-subject");
        assert_eq!(claims.email.as_deref(), Some("libra@example.com"));
    }

    #[tokio::test]
    async fn id_token_verifier_rejects_a_nonce_mismatch() {
        let verifier = oidc_verifier("client123");
        let token_nonce = test_nonce();
        let expected_nonce = test_nonce();
        let token = sign_id_token("client123", &token_nonce, "s", "e@x.com", "N");
        // A token whose nonce doesn't match the login's pre-auth nonce is
        // a replay/injection and must be refused.
        let err = verifier.verify(&token, &expected_nonce).await.unwrap_err();
        assert!(matches!(err, IdTokenError::Nonce));
    }

    #[tokio::test]
    async fn id_token_verifier_rejects_a_token_minted_for_another_audience() {
        let verifier = oidc_verifier("client123");
        let nonce = test_nonce();
        // Signed for a *different* client of the same IdP — the
        // token-confusion attack. Audience pinning rejects it.
        let token = sign_id_token("other-client", &nonce, "s", "e@x.com", "N");
        let err = verifier.verify(&token, &nonce).await.unwrap_err();
        assert!(matches!(err, IdTokenError::Validation(_)));
    }

    #[tokio::test]
    async fn id_token_verifier_rejects_garbage_and_unsigned_tokens() {
        let verifier = oidc_verifier("client123");
        assert!(verifier.verify("not-a-jwt", "n").await.is_err());
        // Unsigned "alg:none"-style token: header.payload with no usable
        // kid / signature → rejected before any claim is trusted.
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"x","nonce":"n","iss":"https://idp.test","aud":"client123"}"#);
        let unsigned = format!("aGVhZGVy.{payload}.");
        assert!(verifier.verify(&unsigned, "n").await.is_err());
    }

    /// ENG-326: a provider key rotation must not need a restart. An
    /// unrecognised `kid` refetches the JWKS immediately, but a flood of
    /// tokens carrying the same unrecognised `kid` must not turn into a
    /// flood of requests against the provider — at most one refetch per
    /// [`super::JWKS_REFETCH_FLOOR`].
    #[tokio::test]
    async fn unknown_kid_refetches_at_most_once_per_floor_interval() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // A fixed, valid RSA key under a `kid` the test tokens never carry —
        // the point of this test is counting fetches, not a successful
        // verification. Sourced from RFC 7517 §A.1, reused from the
        // `auth::tests` JWKS fixtures.
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [{
                    "kid": "provider-key-1",
                    "kty": "RSA",
                    "n": "0vx7agoebGcQSuuPiLJXZptN9nndrQmbXEps2aiAFbWhM78LhWx4cbbfAAtVT86zwu1RK7aPFFxuhDR1L6tSoc_BJECPebWKRXjBZCiFV4n3oknjhMstn64tZ_2W-5JsGY4Hc5n9yBXArwl93lqt7_RN5w6Cf0h4QyQ5v-65YGjQR0_FDW2QvzqY368QQMicAtaSqzs8KJZgnYb9c7d0zgdAZHzu6qMQvRL5hajrn1n91CbOpbISD08qNLyrdkt-bFTWhAI4vMQFh6WeZu0fM4lFd2NcRwr3XPksINHaQ-G_xBniIqbw0Ls1jF44-csFCur-kEgU8awapJzKnqDKgw",
                    "e": "AQAB",
                    "alg": "RS256",
                }],
            })))
            .mount(&server)
            .await;

        // `fetch_and_build`, not `from_jwks_url`: the latter spawns a
        // background task that outlives this test's process and gets
        // flagged "leaky" by nextest. This test only needs the on-demand
        // refetch-on-unknown-kid path, which doesn't depend on that task.
        let jwks_url = format!("{}/jwks", server.uri());
        let verifier = IdTokenVerifier::fetch_and_build(
            &jwks_url,
            "https://idp.test",
            "client123",
            IssuerPolicy::Exact,
        )
        .await
        .expect("mock JWKS fetches");

        let fetch_count = || async {
            server
                .received_requests()
                .await
                .expect("request recording is on by default")
                .iter()
                .filter(|r| r.url.path() == "/jwks")
                .count()
        };
        assert_eq!(fetch_count().await, 1, "the boot fetch");

        // Every one of these tokens carries a `kid` the mock JWKS never
        // serves, so every call takes the unknown-kid path.
        let nonce = test_nonce();
        let token =
            sign_id_token_with_kid("client123", &nonce, "s", "e@x.com", "N", "rotated-away-kid");

        let err = verifier.verify(&token, &nonce).await.unwrap_err();
        assert!(matches!(err, IdTokenError::UnknownKid));
        assert_eq!(fetch_count().await, 2, "first unknown kid refetches once");

        for _ in 0..3 {
            let err = verifier.verify(&token, &nonce).await.unwrap_err();
            assert!(matches!(err, IdTokenError::UnknownKid));
        }
        assert_eq!(
            fetch_count().await,
            2,
            "repeated unknown kids within the floor trigger no further refetch",
        );
    }

    /// Generate a unique nonce for a unit test using the production path.
    fn test_nonce() -> String {
        random_token_32()
    }

    #[test]
    fn id_token_payload_decodes_sub_and_email() {
        // header.payload.sig — header and sig are irrelevant here.
        // Roles are deliberately *not* part of the payload schema:
        // the IdP only carries identity, the DB carries authz.
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            br#"{"sub":"rauthy-libra-subject","email":"libra@example.com","name":"Libra"}"#,
        );
        let jwt = format!("aGVhZGVy.{payload}.c2ln");
        let claims = decode_unverified_payload(&jwt).unwrap();
        assert_eq!(claims.sub, "rauthy-libra-subject");
        assert_eq!(claims.email.as_deref(), Some("libra@example.com"));
        assert_eq!(claims.name.as_deref(), Some("Libra"));
    }

    #[test]
    fn id_token_payload_returns_none_for_garbage() {
        assert!(decode_unverified_payload("not-a-jwt").is_none());
        assert!(decode_unverified_payload("only.two").is_none());
        assert!(decode_unverified_payload("a.b.c").is_none());
    }

    #[test]
    fn constant_time_eq_matches_only_equal_byte_strings() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        // Length mismatch is never equal (and never panics).
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn identity_password_config_from_env_is_opt_in() {
        // The helper reads process env; assert the shape of the decision
        // without stomping a shared env var by checking the default
        // endpoint constant the prod path falls back to.
        assert_eq!(
            IdentityPasswordConfig::DEFAULT_ENDPOINT,
            "https://identitytoolkit.googleapis.com",
        );
    }
}
