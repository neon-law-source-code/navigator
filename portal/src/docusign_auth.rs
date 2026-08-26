//! DocuSign JWT Grant authentication (server-to-server).
//!
//! The shipped [`crate::signature::DocuSignSignatureProvider`] takes a
//! static `DOCUSIGN_ACCESS_TOKEN`, which DocuSign expires in ~8 hours —
//! fine for a one-off manual smoke test, unusable for a nightly CI job
//! or for production. JWT Grant is the durable path: sign a short-lived
//! RSA JWT *assertion* with the firm's integration key + impersonated
//! user, exchange it at DocuSign's OAuth endpoint for an access token,
//! and use that token as the bearer for the eSignature REST API.
//!
//! The assertion builder is pure and takes `now` as a parameter so it is
//! deterministic to test without a clock or a network. The token
//! exchange ([`DocuSignJwtAuth::mint_access_token`]) is the one network
//! call; the live sandbox test drives it behind `DOCUSIGN_SANDBOX_*`
//! secrets.

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::signature::SignatureError;

/// DocuSign caps the JWT assertion lifetime at one hour.
const MAX_TTL_SECS: u64 = 3600;

/// The OAuth scope a signature-sending integration impersonates with.
const SCOPE: &str = "signature impersonation";

#[derive(Debug, Serialize)]
struct JwtClaims {
    /// Integration key (OAuth client id).
    iss: String,
    /// Impersonated user id (the firm's authorized DocuSign signer).
    sub: String,
    /// OAuth host, no scheme — `account-d.docusign.com` (demo) or
    /// `account.docusign.com` (prod).
    aud: String,
    iat: u64,
    exp: u64,
    scope: String,
}

/// Build a signed RS256 JWT assertion for DocuSign JWT Grant.
///
/// `now_secs` is unix seconds (a parameter so tests are deterministic);
/// the assertion is valid for `ttl_secs`, clamped to DocuSign's one-hour
/// maximum. `private_key_pem` is the RSA private key in PKCS#8/PKCS#1
/// PEM form.
pub fn build_jwt_assertion(
    integration_key: &str,
    user_id: &str,
    aud: &str,
    private_key_pem: &[u8],
    now_secs: u64,
    ttl_secs: u64,
) -> Result<String, jsonwebtoken::errors::Error> {
    let claims = JwtClaims {
        iss: integration_key.to_string(),
        sub: user_id.to_string(),
        aud: aud.to_string(),
        iat: now_secs,
        exp: now_secs + ttl_secs.min(MAX_TTL_SECS),
        scope: SCOPE.to_string(),
    };
    let key = EncodingKey::from_rsa_pem(private_key_pem)?;
    encode(&Header::new(Algorithm::RS256), &claims, &key)
}

/// JWT-grant auth config, env-driven. Holds the firm's integration key,
/// the impersonated user, the OAuth base, and the RSA private key.
pub struct DocuSignJwtAuth {
    integration_key: String,
    user_id: String,
    oauth_base: String,
    private_key_pem: Vec<u8>,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    /// Token lifetime in seconds (DocuSign returns ~3600). Defaulted
    /// conservatively in case a response omits it.
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    MAX_TTL_SECS
}

/// A freshly minted access token plus the lifetime DocuSign reported,
/// so the caller can cache it and re-mint before it expires.
#[derive(Debug, Clone)]
pub struct MintedToken {
    pub access_token: String,
    pub expires_in: u64,
}

/// Ensure an OAuth base carries an explicit scheme. DocuSign's prod
/// `DOCUSIGN_OAUTH_BASE` has been configured scheme-less
/// (`account.docusign.com`); without a scheme, [`DocuSignJwtAuth::mint`]
/// builds the relative URL `account.docusign.com/oauth/token`, which the
/// HTTP client rejects — the exact trap that bit the prod cutover. A
/// scheme-less value defaults to `https://`; anything already carrying a
/// scheme (including `http://` for test mock servers) passes through
/// untouched.
fn normalize_oauth_base(base: impl Into<String>) -> String {
    let base = base.into();
    if base.contains("://") {
        base
    } else {
        format!("https://{}", base.trim_start_matches('/'))
    }
}

impl DocuSignJwtAuth {
    /// Build from `DOCUSIGN_SANDBOX_*` env, defaulting the OAuth base to
    /// the demo host. Returns `None` when any required var is absent, so
    /// the live sandbox test self-skips off a CI runner without secrets.
    #[must_use]
    pub fn from_sandbox_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Some(Self {
            integration_key: get("DOCUSIGN_SANDBOX_INTEGRATION_KEY")?,
            user_id: get("DOCUSIGN_SANDBOX_USER_ID")?,
            oauth_base: normalize_oauth_base(
                get("DOCUSIGN_SANDBOX_OAUTH_BASE")
                    .unwrap_or_else(|| "https://account-d.docusign.com".to_string()),
            ),
            private_key_pem: get("DOCUSIGN_SANDBOX_RSA_KEY")?.into_bytes(),
            http: reqwest::Client::new(),
        })
    }

    /// Build directly from parts. `oauth_base` is the OAuth host URL —
    /// `https://account-d.docusign.com` for demo,
    /// `https://account.docusign.com` for production. A scheme-less value
    /// is defaulted to `https://` (see [`normalize_oauth_base`]). Used by
    /// [`Self::from_env`] and tests.
    #[must_use]
    pub fn new(
        integration_key: impl Into<String>,
        user_id: impl Into<String>,
        oauth_base: impl Into<String>,
        private_key_pem: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            integration_key: integration_key.into(),
            user_id: user_id.into(),
            oauth_base: normalize_oauth_base(oauth_base),
            private_key_pem: private_key_pem.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Build from the canonical runtime env (the `DOCUSIGN_*` scheme the
    /// app and `.env` use), returning `None` when the JWT-grant
    /// essentials are absent so the provider can fall back to a static
    /// access token.
    ///
    /// Required: `DOCUSIGN_INTEGRATION_KEY` (the app's Integration Key /
    /// OAuth client id), `DOCUSIGN_USER_ID` (the impersonated API user
    /// GUID), `DOCUSIGN_PRIVATE_KEY` (the RSA private-key PEM).
    ///
    /// `DOCUSIGN_OAUTH_BASE` defaults to the demo host here, which is
    /// correct for the sandbox and test lanes this constructor also serves.
    /// It is not a production fallback: `portal::config` rejects a
    /// production boot that declares DocuSign and omits the key, so the
    /// default below is unreachable from a guarded production process. A
    /// scheme-less value is upgraded to `https://` so a bare
    /// `account.docusign.com` still yields a valid token URL.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Some(Self::new(
            get("DOCUSIGN_INTEGRATION_KEY")?,
            get("DOCUSIGN_USER_ID")?,
            get("DOCUSIGN_OAUTH_BASE")
                .unwrap_or_else(|| "https://account-d.docusign.com".to_string()),
            get("DOCUSIGN_PRIVATE_KEY")?.into_bytes(),
        ))
    }

    /// The `aud` claim: the OAuth host without scheme or trailing slash.
    fn aud(&self) -> &str {
        self.oauth_base
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
    }

    /// Mint a short-lived eSignature access token via JWT grant,
    /// returning the token and the lifetime DocuSign reported. The one
    /// network call in this module. `now_secs` is unix seconds.
    pub async fn mint(&self, now_secs: u64) -> Result<MintedToken, SignatureError> {
        let assertion = build_jwt_assertion(
            &self.integration_key,
            &self.user_id,
            self.aud(),
            &self.private_key_pem,
            now_secs,
            MAX_TTL_SECS,
        )
        .map_err(|e| SignatureError::Provider(format!("jwt assertion: {e}")))?;

        let url = format!("{}/oauth/token", self.oauth_base.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
            .send()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SignatureError::Provider(format!(
                "docusign oauth responded {status}: {body}"
            )));
        }
        let token: TokenResponse = resp
            .json()
            .await
            .map_err(|e| SignatureError::Provider(e.to_string()))?;
        Ok(MintedToken {
            access_token: token.access_token,
            expires_in: token.expires_in,
        })
    }

    /// Convenience wrapper returning just the access token — used by the
    /// live sandbox smoke test, which doesn't cache.
    pub async fn mint_access_token(&self, now_secs: u64) -> Result<String, SignatureError> {
        Ok(self.mint(now_secs).await?.access_token)
    }
}

/// Throwaway 2048-bit RSA keypair for TESTS ONLY — generated at
/// authoring time, never used against a real DocuSign account. Exposed
/// `pub(crate)` so the provider tests in [`crate::signature`] can build
/// a JWT auth without duplicating the PEM.
#[cfg(test)]
pub(crate) const TEST_PRIV_PEM: &str = r"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQCahp278eYjAS3G
gqLwL3yKvtJwn26QehDYt84GqA58FkEAR202VZbUVkSCKa8HG30Lsy5BN7/CoP1o
7wl6rr+AV4sf18A1O5k7u6FGrBMSozgydmIYbAgKITuvc2Dm9EU707fmOQEdICuH
gyIBz+Am5P8g7BUPIVic7l7ghRNifo7rWH4u8aWlZIxARzDammTRZp844pnDG0DN
GsGE8DIiYTqlErsOxuNWIr4fPREGPJzGSyCjiURCtDfBbcr1FiITf8kB/UXJUaYw
ttToClGzW2jk4UE0QLeMhYXDRjGVqcTMhDzyYXL5riSWQ8vKHXYnFBFLzJMGTexJ
RbOtlNQvAgMBAAECggEACjKAUz2gicZ9+P/Nn9sKYB+SmeheLqjs1q2z1LWfaxSO
3+VWxtikFklxG5kuRIz4Vgl82m9C4iWnQ2xO1v/pgZ8v/lR0Xy7v1Zoeskq7DCZQ
Qug+tfeJxPKyJ8m4kdUkgnuzbZJtHo5tFkloOPAOYz1bvBZIQieEW6rRVltXJE81
I1q7yzRYYn4UqqlULAZLM35J2tMwAvCJt+uiVKevDzE9Y6Th/eyaZpRk4H3HFXgh
oke/iq5A8DwG+WWUYCh4wAQfZNsgx4y/61Icw4dEgM1rrWl73rXrkJeJEhxr+TQj
11yPyMhBD+wK0RSKXqsn8WyJLETcfQB8PDCgDnt9TQKBgQDQcyTK0h8f7zDk70Kw
ubmVC85WfOP6jQF6qgXGoZHOsPonlZSIbv6ocWL9ax/moQYha12/7DakKMDpKoSL
SDVcXYIrQJEtCewJ4DNX/nbTNb5Igp/mJYUBQpbmVh4F3GIfXjFHCJL13uxYqODM
Tr8oawhGbsYDEtxEzFRWpxIZ8wKBgQC9xnj8t16d+IKHW43grlJrVXlYUzNh6M+2
0YDBdCx53V9sghCQb9H/VaRtiMaFtKqueT22mXtaX2fV+nNtuSjlA862CSw6ry+o
ceWJQ/tWKAZxJJOT7jgXBPTZHv4yq+fHytu/P3dsyVIqBGlQmnuO4bGvXrIgwUyV
257X9AAP1QKBgQCIPVmkvmTdaGYam06JVzo2cjrwSDxxO8vlsk6IHn3AC+fUC23D
JliHG2TJoUR+ZmwtV5E0qVylOoWrX8C1kAJgVjWHs3GvcDa31bN5JbXgIdY2ajm8
IHWn9y/NaCfDSOFRAy1N8gqrbIIpCGe04RsLfbkw36HHzIHu7WWKJTQthQKBgQCv
cE3lAvf7fgPdcmwk68LR60C0wKXdu8Zasi8fqHB9cIOI4mzBuj4emGPbxvgQH0cy
6G5+4kDA+TYbAN+47dW6cdylOLGkxtN+G10hmrE9ot7htfigZzd/QFvCZP6GhZlO
gGDJ2rhi33KP2Wgq1cWn/0muYBK4aTqNx2x/I9jyyQKBgHOnJa898JNANFFXbDgq
6/gZwbraIG6kP9KO84UXI/+/5/skcKK4eXYybB/HzrC7AQVQdJkIyzDYNDSEsTS6
GOFZJe6RN11Wfwq853r+yFHFnUEOac78/2P3LbfEo71JV0vWJIaKJtFfYIpLgBjU
ZAUSQlrz0bVbicQo41Jgr+pA
-----END PRIVATE KEY-----
";

#[cfg(test)]
mod tests {
    use super::{build_jwt_assertion, Algorithm, DocuSignJwtAuth, TEST_PRIV_PEM as TEST_PRIV};
    use jsonwebtoken::{decode, DecodingKey, Validation};
    use serde::Deserialize;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_PUB: &str = r"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAmoadu/HmIwEtxoKi8C98
ir7ScJ9ukHoQ2LfOBqgOfBZBAEdtNlWW1FZEgimvBxt9C7MuQTe/wqD9aO8Jeq6/
gFeLH9fANTuZO7uhRqwTEqM4MnZiGGwICiE7r3Ng5vRFO9O35jkBHSArh4MiAc/g
JuT/IOwVDyFYnO5e4IUTYn6O61h+LvGlpWSMQEcw2ppk0WafOOKZwxtAzRrBhPAy
ImE6pRK7DsbjViK+Hz0RBjycxksgo4lEQrQ3wW3K9RYiE3/JAf1FyVGmMLbU6ApR
s1to5OFBNEC3jIWFw0YxlanEzIQ88mFy+a4klkPLyh12JxQRS8yTBk3sSUWzrZTU
LwIDAQAB
-----END PUBLIC KEY-----
";

    #[derive(Deserialize)]
    struct Claims {
        iss: String,
        sub: String,
        aud: String,
        scope: String,
        iat: u64,
        exp: u64,
    }

    fn decode_with_pub(jwt: &str, aud: &str) -> Claims {
        let mut v = Validation::new(Algorithm::RS256);
        v.set_audience(&[aud]);
        // The test signs with a fixed past `now`, so don't validate exp
        // against the real clock — we assert exp arithmetic ourselves.
        v.validate_exp = false;
        decode::<Claims>(
            jwt,
            &DecodingKey::from_rsa_pem(TEST_PUB.as_bytes()).unwrap(),
            &v,
        )
        .expect("assertion verifies under the matching public key")
        .claims
    }

    #[test]
    fn normalize_oauth_base_upgrades_scheme_less_to_https() {
        use super::normalize_oauth_base;
        // The prod trap: a bare host must gain a scheme so `mint` builds an
        // absolute `https://account.docusign.com/oauth/token`.
        assert_eq!(
            normalize_oauth_base("account.docusign.com"),
            "https://account.docusign.com"
        );
        assert_eq!(
            normalize_oauth_base("account-d.docusign.com"),
            "https://account-d.docusign.com"
        );
        // Already-schemed values pass through untouched — including the
        // `http://` mock-server URIs the wiremock tests pass to `new`.
        assert_eq!(
            normalize_oauth_base("https://account.docusign.com"),
            "https://account.docusign.com"
        );
        assert_eq!(
            normalize_oauth_base("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn new_upgrades_scheme_less_oauth_base() {
        // `new` (and therefore `from_env`) must apply the upgrade so the
        // `aud` claim and token URL are built from a real host.
        let auth = DocuSignJwtAuth::new("ik", "user", "account.docusign.com", b"pem".to_vec());
        assert_eq!(auth.aud(), "account.docusign.com");
        assert_eq!(auth.oauth_base, "https://account.docusign.com");
    }

    #[test]
    fn assertion_is_rs256_signed_with_the_grant_claims() {
        let jwt = build_jwt_assertion(
            "ik-123",
            "user-456",
            "account-d.docusign.com",
            TEST_PRIV.as_bytes(),
            1_000,
            3600,
        )
        .expect("builds");
        let c = decode_with_pub(&jwt, "account-d.docusign.com");
        assert_eq!(c.iss, "ik-123");
        assert_eq!(c.sub, "user-456");
        assert_eq!(c.aud, "account-d.docusign.com");
        assert_eq!(c.scope, "signature impersonation");
        assert_eq!(c.iat, 1_000);
        assert_eq!(c.exp, 4_600);
    }

    #[test]
    fn ttl_is_clamped_to_one_hour() {
        let jwt = build_jwt_assertion(
            "ik",
            "u",
            "account-d.docusign.com",
            TEST_PRIV.as_bytes(),
            0,
            99_999,
        )
        .expect("builds");
        let c = decode_with_pub(&jwt, "account-d.docusign.com");
        assert_eq!(c.exp, 3600, "exp must be clamped to now + 1h");
    }

    #[test]
    fn a_tampered_assertion_fails_verification() {
        let mut jwt = build_jwt_assertion(
            "ik",
            "u",
            "account-d.docusign.com",
            TEST_PRIV.as_bytes(),
            1_000,
            3600,
        )
        .expect("builds");
        // Flip a character in the signature segment.
        jwt.pop();
        jwt.push(if jwt.ends_with('A') { 'B' } else { 'A' });
        let mut v = Validation::new(Algorithm::RS256);
        v.set_audience(&["account-d.docusign.com"]);
        v.validate_exp = false;
        let result = decode::<Claims>(
            &jwt,
            &DecodingKey::from_rsa_pem(TEST_PUB.as_bytes()).unwrap(),
            &v,
        );
        assert!(result.is_err(), "tampered assertion must not verify");
    }

    #[tokio::test]
    async fn mint_returns_the_token_and_reported_lifetime() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "tok-xyz",
                "token_type": "Bearer",
                "expires_in": 3600,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let auth = DocuSignJwtAuth::new("ik", "user", server.uri(), TEST_PRIV.as_bytes().to_vec());
        let minted = auth.mint(0).await.expect("mint succeeds against the mock");
        assert_eq!(minted.access_token, "tok-xyz");
        assert_eq!(minted.expires_in, 3600);
    }

    #[tokio::test]
    async fn mint_defaults_lifetime_when_response_omits_expires_in() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "tok-no-exp",
            })))
            .mount(&server)
            .await;

        let auth = DocuSignJwtAuth::new("ik", "user", server.uri(), TEST_PRIV.as_bytes().to_vec());
        let minted = auth.mint(0).await.expect("mint succeeds");
        assert_eq!(minted.expires_in, 3600, "absent expires_in defaults to 1h");
    }
}
