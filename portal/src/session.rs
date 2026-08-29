//! Server-side session-cookie store.
//!
//! Sessions are stateless: the entire payload (subject, expiry,
//! role, CSRF token) lives in a single HMAC-signed cookie. The
//! server doesn't keep any per-session state, so horizontal scaling
//! "just works" — every node that shares `SESSION_SECRET` accepts
//! every cookie.
//!
//! Cookie format: `<base64url(json{...})>.<base64url(hmac-sha256)>`.
//! Tamper attempts (changing the payload, forging the signature)
//! fail constant-time verification via the `hmac` crate.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use base64::Engine;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use store::persons::Role;

/// HTTP cookie name carrying the signed session payload.
pub const SESSION_COOKIE_NAME: &str = "navigator_session";

/// Default session lifetime — 8 hours.
pub const DEFAULT_SESSION_TTL_SECS: i64 = 8 * 60 * 60;

/// CLI bearer-token lifetime — 1 hour. Much tighter than the browser
/// session: a CLI token is a portable file credential
/// (`~/.navigator.json`), so a leak should expire fast. Bounds the
/// blast radius until granular server-side revocation lands.
pub const CLI_SESSION_TTL_SECS: i64 = 60 * 60;

/// Which front door minted a session — a browser cookie or a portable
/// CLI bearer token. Lets audit logs tell them apart and lets policy
/// treat a file-backed CLI credential differently from a cookie.
/// Defaults to [`SessionSource::Browser`] so tokens minted before this
/// field existed still decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    #[default]
    Browser,
    Cli,
}

/// Original admin actor retained while the session acts as a client.
///
/// This mirrors OAuth token-exchange actor semantics: authorization checks
/// see the effective top-level subject, while the application can still
/// identify the admin who initiated the impersonation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Impersonation {
    pub actor_sub: String,
    #[serde(default)]
    pub actor_email: Option<String>,
    #[serde(default)]
    pub actor_person_id: Option<Uuid>,
    pub target_name: String,
    pub target_email: String,
}

/// Payload encoded into the session cookie. Signed HMAC-SHA256 by
/// [`SessionStore::encode`] / decoded by [`SessionStore::decode`].
///
/// `role` is the DB-sourced system-wide tier — read from
/// `persons.role` at OIDC callback time, **not** from any token
/// claim. See [`docs/access-model.md`](../../../docs/access-model.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionData {
    /// OIDC `sub` claim — the IdP's stable identifier for the
    /// person. Rauthy / Google / etc. each use their own format.
    pub sub: String,
    /// Email from the OIDC `email` claim. Optional because some
    /// IdPs withhold it without explicit `scope=email`.
    #[serde(default)]
    pub email: Option<String>,
    /// `persons.id` from our own database — the local row we
    /// upserted after the OIDC handshake. `None` only briefly
    /// during boot-time tests that bypass the callback.
    #[serde(default)]
    pub person_id: Option<Uuid>,
    /// Unix epoch seconds — cookies past this time are rejected.
    pub exp: i64,
    /// System-wide tier read from `persons.role`.
    pub role: Role,
    /// Per-session CSRF token, embedded in every admin form.
    pub csrf_token: String,
    /// Which front door minted this session. `Browser` for the OIDC /
    /// password cookie flow, `Cli` for a `navigator site login` bearer token.
    #[serde(default)]
    pub source: SessionSource,
    /// Slug of the OIDC provider that authenticated this session
    /// (`portal::oauth::ProviderId::slug`), or `None` for a door with no
    /// upstream provider — the Identity Platform password form, a CLI bearer.
    ///
    /// Read on sign-out so RP-initiated logout ends the SSO session at the
    /// provider that actually holds it. Stored as the slug rather than the
    /// enum so this module keeps no dependency on the OAuth layer, and
    /// `serde(default)` so sessions minted before the field existed keep
    /// decoding — they simply fall back to the primary provider.
    #[serde(default)]
    pub provider: Option<String>,
    /// Present only when an admin has switched the browser session into a
    /// client lens. The session's `sub`/`email`/`person_id`/`role` are the
    /// effective client; this field preserves the actor for the banner and
    /// for restoring the admin session.
    #[serde(default)]
    pub impersonation: Option<Impersonation>,
}

impl SessionData {
    /// True when `exp` has passed.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.exp <= now_unix()
    }

    /// Convenience: build a session that expires in `DEFAULT_SESSION_TTL_SECS`.
    #[must_use]
    pub fn fresh(sub: impl Into<String>, role: Role) -> Self {
        Self {
            sub: sub.into(),
            email: None,
            person_id: None,
            exp: now_unix() + DEFAULT_SESSION_TTL_SECS,
            role,
            csrf_token: random_token_32(),
            source: SessionSource::Browser,
            provider: None,
            impersonation: None,
        }
    }
}

/// HMAC-signing store. Cheap to clone (key is `Arc`-wrapped).
#[derive(Clone)]
pub struct SessionStore {
    key: Arc<Vec<u8>>,
}

impl SessionStore {
    #[must_use]
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self {
            key: Arc::new(key.into()),
        }
    }

    /// Build from `SESSION_SECRET` env var. Returns `None` if unset
    /// so the binary can decide whether to fail boot or fall back to
    /// a disabled session backend.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        std::env::var("SESSION_SECRET").ok().map(Self::new)
    }

    /// Sign + base64-encode arbitrary bytes into a cookie value
    /// string. Use for the pre-auth (OAuth state + PKCE verifier)
    /// cookie too.
    #[must_use]
    pub fn encode_signed_bytes(&self, payload: &[u8]) -> String {
        let body = b64().encode(payload);
        let sig = b64().encode(self.sign(body.as_bytes()));
        format!("{body}.{sig}")
    }

    /// Decode + constant-time-verify a signed cookie value, returning
    /// the original bytes. `None` on any tamper / format error.
    #[must_use]
    pub fn decode_signed_bytes(&self, raw: &str) -> Option<Vec<u8>> {
        let (body, sig_b64) = raw.rsplit_once('.')?;
        let expected_sig = b64().decode(sig_b64).ok()?;
        if !self.verify(body.as_bytes(), &expected_sig) {
            return None;
        }
        b64().decode(body).ok()
    }

    /// Sign + base64-encode `data` into a cookie value string.
    #[must_use]
    pub fn encode(&self, data: &SessionData) -> String {
        self.encode_signed_bytes(
            &serde_json::to_vec(data).expect("session data is always serializable"),
        )
    }

    /// Decode + verify a cookie value. Returns `None` on:
    /// missing separator, bad base64, bad signature (constant-time),
    /// malformed JSON, or expired payload.
    #[must_use]
    pub fn decode(&self, raw: &str) -> Option<SessionData> {
        let bytes = self.decode_signed_bytes(raw)?;
        let data: SessionData = serde_json::from_slice(&bytes).ok()?;
        if data.is_expired() {
            return None;
        }
        Some(data)
    }

    fn sign(&self, payload: &[u8]) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts any key");
        mac.update(payload);
        mac.finalize().into_bytes().to_vec()
    }

    fn verify(&self, payload: &[u8], expected_sig: &[u8]) -> bool {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("HMAC accepts any key");
        mac.update(payload);
        // `verify_slice` is constant-time.
        mac.verify_slice(expected_sig).is_ok()
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn now_unix() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is past epoch")
            .as_secs(),
    )
    .expect("clock fits in i64")
}

/// 32 random bytes, url-safe base64 encoded. Used for CSRF tokens
/// and OAuth state values.
#[must_use]
pub fn random_token_32() -> String {
    let buf: [u8; 32] = rand::random();
    b64().encode(buf)
}

/// Current unix epoch seconds. Public so the oauth module's
/// pre-auth cookie can compute its own expiry.
#[must_use]
pub fn now_unix_secs() -> i64 {
    now_unix()
}

#[cfg(test)]
mod tests {
    use super::{
        now_unix, random_token_32, Role, SessionData, SessionSource, SessionStore,
        DEFAULT_SESSION_TTL_SECS,
    };

    fn store() -> SessionStore {
        SessionStore::new("test-key-not-for-production")
    }

    #[test]
    fn fresh_session_has_expiry_in_the_future() {
        let s = SessionData::fresh("nick@neonlaw.com", Role::Admin);
        assert!(s.exp > now_unix());
        assert!(s.exp <= now_unix() + DEFAULT_SESSION_TTL_SECS);
        assert!(!s.is_expired());
        assert!(!s.csrf_token.is_empty());
        // A browser session by default; the CLI mint overrides this.
        assert_eq!(s.source, SessionSource::Browser);
    }

    #[test]
    fn source_round_trips_through_the_signed_store() {
        let store = store();
        let mut s = SessionData::fresh("x", Role::Admin);
        s.source = SessionSource::Cli;
        let decoded = store.decode(&store.encode(&s)).expect("valid token");
        assert_eq!(decoded.source, SessionSource::Cli);
    }

    #[test]
    fn legacy_payload_without_source_decodes_as_browser() {
        // A token minted before `source` existed has no such JSON field;
        // serde's default must heal it to Browser rather than failing.
        let mut v = serde_json::to_value(SessionData::fresh("x", Role::Client)).unwrap();
        v.as_object_mut().unwrap().remove("source");
        let s: SessionData = serde_json::from_value(v).unwrap();
        assert_eq!(s.source, SessionSource::Browser);
    }

    #[test]
    fn encode_then_decode_round_trips_exactly() {
        let store = store();
        let original = SessionData::fresh("nick@neonlaw.com", Role::Admin);
        let cookie = store.encode(&original);
        let decoded = store.decode(&cookie).expect("decode should succeed");
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_rejects_payload_tamper() {
        let store = store();
        let cookie = store.encode(&SessionData::fresh("libra", Role::Client));
        // Flip one byte in the body half.
        let (body, sig) = cookie.split_once('.').unwrap();
        let tampered_body: String = body
            .chars()
            .enumerate()
            .map(|(i, c)| if i == 0 { 'X' } else { c })
            .collect();
        let tampered = format!("{tampered_body}.{sig}");
        assert!(store.decode(&tampered).is_none());
    }

    #[test]
    fn decode_rejects_signature_tamper() {
        let store = store();
        let cookie = store.encode(&SessionData::fresh("libra", Role::Client));
        let (body, _) = cookie.split_once('.').unwrap();
        let forged = format!("{body}.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(store.decode(&forged).is_none());
    }

    #[test]
    fn decode_rejects_cookie_signed_by_a_different_key() {
        let cookie = SessionStore::new("key-a").encode(&SessionData::fresh("libra", Role::Client));
        assert!(SessionStore::new("key-b").decode(&cookie).is_none());
    }

    #[test]
    fn decode_rejects_expired_session() {
        let store = store();
        let mut s = SessionData::fresh("libra", Role::Client);
        s.exp = now_unix() - 60; // expired 60s ago
        let cookie = store.encode(&s);
        assert!(store.decode(&cookie).is_none());
    }

    #[test]
    fn decode_rejects_garbage_input() {
        let store = store();
        assert!(store.decode("").is_none());
        assert!(store.decode("not-a-cookie").is_none());
        assert!(store.decode("only-one-part").is_none());
        assert!(store.decode("a.b.c").is_none());
    }

    #[test]
    fn random_tokens_are_unique_and_url_safe() {
        let a = random_token_32();
        let b = random_token_32();
        assert_ne!(a, b);
        // url-safe base64 of 32 bytes (no padding) = 43 chars.
        assert_eq!(a.len(), 43);
        // No `+` or `/`.
        assert!(!a.contains('+') && !a.contains('/'));
    }
}
