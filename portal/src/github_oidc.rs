//! Verify GitHub Actions OIDC ID tokens.
//!
//! A Project repository's CI job presents the JWT GitHub minted for that run.
//! This module checks issuer, audience, and signature, then returns the
//! repository claims the seed-token mint uses to bind the session to one
//! Project. JWKS documents are cached and refreshed; a token's `jti` is
//! spent on first successful verify so the mint door cannot replay it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::auth::JwksDocument;
use crate::session::now_unix_secs;

/// Claims GitHub Actions puts on an OIDC ID token.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitHubActionsClaims {
    /// `repo:owner/name:ref:refs/heads/main`
    pub sub: String,
    pub iss: String,
    /// `owner/name`
    pub repository: String,
    pub repository_owner: String,
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub event_name: String,
    /// Unique token id; required so the mint door is single-use.
    #[serde(default)]
    pub jti: String,
    /// Unix expiry of the GitHub JWT.
    #[serde(default)]
    pub exp: i64,
}

/// How a GitHub Actions OIDC JWT is verified.
#[derive(Clone)]
pub struct GitHubOidc {
    inner: Arc<GitHubOidcInner>,
    spent_jtis: Arc<Mutex<HashMap<String, i64>>>,
}

enum GitHubOidcInner {
    /// Refuse every token. Tests and boots that do not mint CI sessions.
    Reject,
    /// Return these claims for any non-empty token. Integration tests.
    Fixed(GitHubActionsClaims),
    /// Verify RS256 against GitHub's published JWKS.
    Jwks {
        issuer: String,
        jwks_url: String,
        cache: Arc<AsyncMutex<Option<(Instant, JwksDocument)>>>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubOidcError {
    #[error("this deployment does not accept GitHub Actions OIDC tokens")]
    Rejected,
    #[error("GitHub Actions OIDC token is missing")]
    Missing,
    #[error("GitHub Actions OIDC token is invalid: {0}")]
    Invalid(String),
    #[error("GitHub Actions OIDC token has already been exchanged")]
    Replayed,
}

const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";
const GITHUB_ACTIONS_JWKS: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";
const JWKS_TTL: Duration = Duration::from_secs(3600);

impl GitHubOidc {
    fn with_inner(inner: GitHubOidcInner) -> Self {
        Self {
            inner: Arc::new(inner),
            spent_jtis: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Production verifier: GitHub's issuer and JWKS.
    #[must_use]
    pub fn github_actions() -> Self {
        Self::jwks(GITHUB_ACTIONS_ISSUER, GITHUB_ACTIONS_JWKS)
    }

    /// Verify RS256 against the given issuer and JWKS URL. Tests pin this at a
    /// mock origin; production uses [`Self::github_actions`].
    #[must_use]
    pub fn jwks(issuer: impl Into<String>, jwks_url: impl Into<String>) -> Self {
        Self::with_inner(GitHubOidcInner::Jwks {
            issuer: issuer.into(),
            jwks_url: jwks_url.into(),
            cache: Arc::new(AsyncMutex::new(None)),
        })
    }

    /// Refuse every token.
    #[must_use]
    pub fn rejecting() -> Self {
        Self::with_inner(GitHubOidcInner::Reject)
    }

    /// Return `claims` for any non-empty presented token.
    #[must_use]
    pub fn fixed(claims: GitHubActionsClaims) -> Self {
        Self::with_inner(GitHubOidcInner::Fixed(claims))
    }

    /// Verify `token` was minted for `expected_aud`.
    pub async fn verify(
        &self,
        token: &str,
        expected_aud: &str,
    ) -> Result<GitHubActionsClaims, GitHubOidcError> {
        if token.trim().is_empty() {
            return Err(GitHubOidcError::Missing);
        }
        match &*self.inner {
            GitHubOidcInner::Reject => Err(GitHubOidcError::Rejected),
            GitHubOidcInner::Fixed(claims) => Ok(claims.clone()),
            GitHubOidcInner::Jwks {
                issuer,
                jwks_url,
                cache,
            } => verify_jwks(token, expected_aud, issuer, jwks_url, cache).await,
        }
    }

    /// Mark this GitHub JWT's `jti` spent until `exp`. A second exchange of
    /// the same token on this process is [`GitHubOidcError::Replayed`]. The
    /// set is shared across clones of this verifier so one `AppState` honors
    /// single-use for every replica of the mint door in the process.
    pub fn spend_jti(&self, jti: &str, exp: i64) -> Result<(), GitHubOidcError> {
        let jti = jti.trim();
        if jti.is_empty() {
            return Err(GitHubOidcError::Invalid("token has no jti".into()));
        }
        let now = now_unix_secs();
        let mut spent = self
            .spent_jtis
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        spent.retain(|_, until| *until > now);
        if spent.get(jti).is_some_and(|until| *until > now) {
            return Err(GitHubOidcError::Replayed);
        }
        spent.insert(jti.to_string(), exp.max(now + 1));
        Ok(())
    }
}

async fn verify_jwks(
    token: &str,
    expected_aud: &str,
    issuer: &str,
    jwks_url: &str,
    cache: &AsyncMutex<Option<(Instant, JwksDocument)>>,
) -> Result<GitHubActionsClaims, GitHubOidcError> {
    let header =
        decode_header(token).map_err(|error| GitHubOidcError::Invalid(error.to_string()))?;
    let kid = header
        .kid
        .ok_or_else(|| GitHubOidcError::Invalid("token header has no kid".into()))?;
    let doc = load_jwks(jwks_url, cache, false).await?;
    let claims = decode_with_jwks(token, expected_aud, issuer, &kid, &doc);
    if let Ok(claims) = claims {
        Ok(claims)
    } else {
        let doc = load_jwks(jwks_url, cache, true).await?;
        decode_with_jwks(token, expected_aud, issuer, &kid, &doc)
    }
}

async fn load_jwks(
    jwks_url: &str,
    cache: &AsyncMutex<Option<(Instant, JwksDocument)>>,
    force: bool,
) -> Result<JwksDocument, GitHubOidcError> {
    if !force {
        let guard = cache.lock().await;
        if let Some((fetched_at, doc)) = guard.as_ref() {
            if fetched_at.elapsed() < JWKS_TTL {
                return Ok(doc.clone());
            }
        }
    }
    let doc: JwksDocument = reqwest::get(jwks_url)
        .await
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?
        .error_for_status()
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?
        .json()
        .await
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?;
    *cache.lock().await = Some((Instant::now(), doc.clone()));
    Ok(doc)
}

fn decode_with_jwks(
    token: &str,
    expected_aud: &str,
    issuer: &str,
    kid: &str,
    doc: &JwksDocument,
) -> Result<GitHubActionsClaims, GitHubOidcError> {
    let jwk = doc
        .keys
        .iter()
        .find(|key| key.kid.as_deref() == Some(kid))
        .ok_or_else(|| GitHubOidcError::Invalid(format!("no JWKS entry for kid {kid}")))?;
    let n = jwk
        .n
        .as_deref()
        .ok_or_else(|| GitHubOidcError::Invalid("JWKS key is missing n".into()))?;
    let e = jwk
        .e
        .as_deref()
        .ok_or_else(|| GitHubOidcError::Invalid("JWKS key is missing e".into()))?;
    let key = DecodingKey::from_rsa_components(n, e)
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[expected_aud]);
    validation.set_issuer(&[issuer]);
    let token = decode::<GitHubActionsClaims>(token, &key, &validation)
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?;
    Ok(token.claims)
}

impl Default for GitHubActionsClaims {
    fn default() -> Self {
        Self {
            sub: String::new(),
            iss: GITHUB_ACTIONS_ISSUER.to_string(),
            repository: String::new(),
            repository_owner: String::new(),
            git_ref: "refs/heads/main".to_string(),
            event_name: "push".to_string(),
            jti: String::new(),
            exp: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        decode_with_jwks, GitHubActionsClaims, GitHubOidc, GitHubOidcError, GITHUB_ACTIONS_ISSUER,
    };
    use crate::auth::{JwksDocument, JwksKey};
    use crate::session::now_unix_secs;
    use crate::test_support::{sign_rs256_claims, test_oidc_jwks_rsa, TEST_OIDC_KID};
    use serde::Serialize;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[derive(Serialize)]
    struct SignedGithubClaims<'a> {
        sub: &'a str,
        iss: &'a str,
        aud: &'a str,
        repository: &'a str,
        repository_owner: &'a str,
        #[serde(rename = "ref")]
        git_ref: &'a str,
        event_name: &'a str,
        jti: &'a str,
        exp: i64,
    }

    fn signed_claims<'a>(issuer: &'a str, audience: &'a str) -> SignedGithubClaims<'a> {
        SignedGithubClaims {
            sub: "repo:neon-law-source-code/navigator:ref:refs/heads/main",
            iss: issuer,
            aud: audience,
            repository: "neon-law-source-code/navigator",
            repository_owner: "neon-law-source-code",
            git_ref: "refs/heads/main",
            event_name: "pull_request",
            jti: "jti-1",
            exp: now_unix_secs() + 300,
        }
    }

    fn sample_claims(aud_iss: &str) -> GitHubActionsClaims {
        GitHubActionsClaims {
            sub: "repo:neon-law-source-code/navigator:ref:refs/heads/main".into(),
            iss: aud_iss.to_string(),
            repository: "neon-law-source-code/navigator".into(),
            repository_owner: "neon-law-source-code".into(),
            git_ref: "refs/heads/main".into(),
            event_name: "pull_request".into(),
            jti: "jti-1".into(),
            exp: now_unix_secs() + 300,
        }
    }

    #[tokio::test]
    async fn rejecting_refuses_every_token() {
        let err = GitHubOidc::rejecting()
            .verify("anything", "https://staging.neonlaw.com")
            .await
            .unwrap_err();
        assert!(matches!(err, GitHubOidcError::Rejected));
    }

    #[tokio::test]
    async fn an_empty_token_is_missing() {
        let err = GitHubOidc::fixed(GitHubActionsClaims::default())
            .verify("  ", "https://staging.neonlaw.com")
            .await
            .unwrap_err();
        assert!(matches!(err, GitHubOidcError::Missing));
    }

    #[tokio::test]
    async fn github_actions_verifier_still_rejects_a_blank_token() {
        let err = GitHubOidc::github_actions()
            .verify("   ", "https://staging.neonlaw.com")
            .await
            .unwrap_err();
        assert!(matches!(err, GitHubOidcError::Missing));
    }

    #[tokio::test]
    async fn fixed_verifier_returns_the_configured_claims() {
        let claims = sample_claims(GITHUB_ACTIONS_ISSUER);
        let got = GitHubOidc::fixed(claims.clone())
            .verify("token", "https://staging.neonlaw.com")
            .await
            .expect("fixed verifier accepts any non-empty token");
        assert_eq!(got.repository, claims.repository);
        assert_eq!(got.jti, claims.jti);
    }

    #[test]
    fn a_spent_jti_cannot_be_exchanged_again() {
        let oidc = GitHubOidc::fixed(GitHubActionsClaims::default());
        oidc.spend_jti("once", 4_000_000_000).unwrap();
        let err = oidc.spend_jti("once", 4_000_000_000).unwrap_err();
        assert!(matches!(err, GitHubOidcError::Replayed));
    }

    #[test]
    fn a_blank_jti_is_invalid() {
        let oidc = GitHubOidc::fixed(GitHubActionsClaims::default());
        let err = oidc.spend_jti("  ", 4_000_000_000).unwrap_err();
        assert!(matches!(err, GitHubOidcError::Invalid(_)));
    }

    #[test]
    fn decode_with_jwks_reports_missing_kid_n_and_e() {
        let token = sign_rs256_claims(
            TEST_OIDC_KID,
            &signed_claims(GITHUB_ACTIONS_ISSUER, "https://staging.neonlaw.com"),
        );
        let no_kid = JwksDocument {
            keys: vec![JwksKey {
                kid: Some("other".into()),
                ..test_oidc_jwks_rsa()
            }],
        };
        let err = decode_with_jwks(
            &token,
            "https://staging.neonlaw.com",
            GITHUB_ACTIONS_ISSUER,
            TEST_OIDC_KID,
            &no_kid,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no JWKS entry"));

        let mut missing_n = test_oidc_jwks_rsa();
        missing_n.n = None;
        let err = decode_with_jwks(
            &token,
            "https://staging.neonlaw.com",
            GITHUB_ACTIONS_ISSUER,
            TEST_OIDC_KID,
            &JwksDocument {
                keys: vec![missing_n],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing n"));

        let mut missing_e = test_oidc_jwks_rsa();
        missing_e.e = None;
        let err = decode_with_jwks(
            &token,
            "https://staging.neonlaw.com",
            GITHUB_ACTIONS_ISSUER,
            TEST_OIDC_KID,
            &JwksDocument {
                keys: vec![missing_e],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing e"));
    }

    #[tokio::test]
    async fn jwks_verifier_accepts_a_signed_github_actions_token() {
        let server = MockServer::start().await;
        let issuer = "https://token.actions.test";
        let audience = "https://staging.neonlaw.com";
        Mock::given(method("GET"))
            .and(path("/.well-known/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [test_oidc_jwks_rsa()]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let claims = signed_claims(issuer, audience);
        let token = sign_rs256_claims(TEST_OIDC_KID, &claims);
        let oidc = GitHubOidc::jwks(issuer, format!("{}/.well-known/jwks", server.uri()));
        let got = oidc.verify(&token, audience).await.expect("valid token");
        assert_eq!(got.repository, claims.repository);
        assert_eq!(got.event_name, "pull_request");

        // Cache hit: a second verify must not fetch JWKS again (mock expect(1)).
        oidc.verify(&token, audience)
            .await
            .expect("cached JWKS still verifies");
    }

    #[tokio::test]
    async fn jwks_verifier_refetches_when_the_cached_kid_is_unknown() {
        let server = MockServer::start().await;
        let issuer = "https://token.actions.test";
        let audience = "https://staging.neonlaw.com";
        Mock::given(method("GET"))
            .and(path("/.well-known/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [{
                    "kid": "stale",
                    "kty": "RSA",
                    "n": test_oidc_jwks_rsa().n,
                    "e": "AQAB"
                }]
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [test_oidc_jwks_rsa()]
            })))
            .mount(&server)
            .await;

        let claims = signed_claims(issuer, audience);
        let token = sign_rs256_claims(TEST_OIDC_KID, &claims);
        let oidc = GitHubOidc::jwks(issuer, format!("{}/.well-known/jwks", server.uri()));
        let got = oidc
            .verify(&token, audience)
            .await
            .expect("refetch finds the kid");
        assert_eq!(got.jti, "jti-1");
    }

    #[tokio::test]
    async fn jwks_verifier_rejects_a_header_without_kid() {
        let oidc = GitHubOidc::jwks(GITHUB_ACTIONS_ISSUER, "http://127.0.0.1/jwks");
        let err = oidc
            .verify("not-a-jwt", "https://staging.neonlaw.com")
            .await
            .unwrap_err();
        assert!(matches!(err, GitHubOidcError::Invalid(_)));
    }
}
