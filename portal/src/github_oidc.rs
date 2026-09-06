//! Verify GitHub Actions OIDC ID tokens.
//!
//! A Project repository's CI job presents the JWT GitHub minted for that run.
//! This module checks issuer, audience, and signature, then returns the
//! repository claims the seed-token mint uses to bind the session to one
//! Project.

use std::sync::Arc;

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;

use crate::auth::JwksDocument;

/// Claims GitHub Actions puts on an OIDC ID token.
#[derive(Debug, Clone, Deserialize)]
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
}

/// How a GitHub Actions OIDC JWT is verified.
#[derive(Clone)]
pub struct GitHubOidc {
    inner: Arc<GitHubOidcInner>,
}

enum GitHubOidcInner {
    /// Refuse every token. Tests and boots that do not mint CI sessions.
    Reject,
    /// Return these claims for any non-empty token. Integration tests.
    Fixed(GitHubActionsClaims),
    /// Verify RS256 against GitHub's published JWKS.
    Jwks { issuer: String, jwks_url: String },
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubOidcError {
    #[error("this deployment does not accept GitHub Actions OIDC tokens")]
    Rejected,
    #[error("GitHub Actions OIDC token is missing")]
    Missing,
    #[error("GitHub Actions OIDC token is invalid: {0}")]
    Invalid(String),
}

const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";
const GITHUB_ACTIONS_JWKS: &str = "https://token.actions.githubusercontent.com/.well-known/jwks";

impl GitHubOidc {
    /// Production verifier: GitHub's issuer and JWKS.
    #[must_use]
    pub fn github_actions() -> Self {
        Self {
            inner: Arc::new(GitHubOidcInner::Jwks {
                issuer: GITHUB_ACTIONS_ISSUER.to_string(),
                jwks_url: GITHUB_ACTIONS_JWKS.to_string(),
            }),
        }
    }

    /// Refuse every token.
    #[must_use]
    pub fn rejecting() -> Self {
        Self {
            inner: Arc::new(GitHubOidcInner::Reject),
        }
    }

    /// Return `claims` for any non-empty presented token.
    #[must_use]
    pub fn fixed(claims: GitHubActionsClaims) -> Self {
        Self {
            inner: Arc::new(GitHubOidcInner::Fixed(claims)),
        }
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
            GitHubOidcInner::Jwks { issuer, jwks_url } => {
                verify_jwks(token, expected_aud, issuer, jwks_url).await
            }
        }
    }
}

async fn verify_jwks(
    token: &str,
    expected_aud: &str,
    issuer: &str,
    jwks_url: &str,
) -> Result<GitHubActionsClaims, GitHubOidcError> {
    let header =
        decode_header(token).map_err(|error| GitHubOidcError::Invalid(error.to_string()))?;
    let kid = header
        .kid
        .ok_or_else(|| GitHubOidcError::Invalid("token header has no kid".into()))?;
    let doc: JwksDocument = reqwest::get(jwks_url)
        .await
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?
        .error_for_status()
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?
        .json()
        .await
        .map_err(|error| GitHubOidcError::Invalid(error.to_string()))?;
    let jwk = doc
        .keys
        .iter()
        .find(|key| key.kid.as_deref() == Some(kid.as_str()))
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

#[cfg(test)]
mod tests {
    use super::{GitHubActionsClaims, GitHubOidc, GitHubOidcError};

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
        }
    }
}
