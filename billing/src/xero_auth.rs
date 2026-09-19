//! Xero OAuth2 **client-credentials** authentication (server-to-server).
//!
//! Xero's "Custom Connection" app type uses the client-credentials grant:
//! there is no interactive consent and no refresh token — the app
//! exchanges its client id + secret directly for a short-lived access
//! token, exactly the no-human-in-the-loop posture a billing sync wants
//! (the Xero analogue of DocuSign JWT grant in [`crate::docusign_auth`]).
//!
//! A custom connection is bound to a **single** Xero organisation, so the
//! minted token already scopes to one tenant; the caller still passes the
//! `Xero-Tenant-Id` on each Accounting API call (see
//! [`crate::billing::XeroBillingProvider`]).
//!
//! The Basic-auth header builder is pure and unit-tested without a
//! network. [`XeroClientCredentials::mint`] is the one network call; the
//! live sandbox test drives it behind `XERO_SANDBOX_*` secrets and
//! self-skips when they are absent.

use base64::Engine;
use serde::Deserialize;

use crate::BillingError;

/// Xero's identity token endpoint for the client-credentials grant.
const DEFAULT_TOKEN_URL: &str = "https://identity.xero.com/connect/token";

/// Xero client-credentials access tokens live 30 minutes. Defaulted
/// conservatively in case a token response omits `expires_in`.
const DEFAULT_EXPIRES_IN: u64 = 1800;

/// Scopes a billing integration needs: create/read invoices, resolve (or
/// create) the billed contact, and **read** the IOLTA side — the pooled
/// trust bank accounts and the deposits, refunds, and withdrawals that move
/// through them. These are the **granular** scopes a Xero custom connection
/// grants today — the legacy parent `accounting.transactions` is not offered
/// in the connection scope picker, so requesting it fails token minting with
/// `invalid_scope`. `accounting.invoices` is exactly the scope an `ACCREC`
/// invoice POST needs; the two `.read` scopes are exactly what the IOLTA
/// mirror needs and no more, so a leaked token cannot move client funds.
/// `offline_access` is deliberately absent — client-credentials issues no
/// refresh token; the provider just re-mints. These scopes must also be
/// granted on the custom connection in the developer portal.
const DEFAULT_SCOPE: &str = "accounting.contacts accounting.invoices \
                             accounting.settings.read accounting.transactions.read";

/// Build the HTTP Basic `Authorization` header value the token request
/// carries (`Basic base64(client_id:client_secret)`). Pure — exposed so
/// the encoding is unit-tested without a network round-trip, and used
/// verbatim by [`XeroClientCredentials::mint`].
#[must_use]
pub fn basic_auth_header(client_id: &str, client_secret: &str) -> String {
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(format!("{client_id}:{client_secret}"));
    format!("Basic {encoded}")
}

/// Client-credentials auth config, env-driven. Holds the custom
/// connection's client id + secret, the token endpoint, and the scopes
/// to request.
pub struct XeroClientCredentials {
    client_id: String,
    client_secret: String,
    token_url: String,
    scope: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default = "default_expires_in")]
    expires_in: u64,
}

fn default_expires_in() -> u64 {
    DEFAULT_EXPIRES_IN
}

/// A freshly minted access token plus the lifetime Xero reported, so the
/// caller can cache it and re-mint before it expires.
#[derive(Debug, Clone)]
pub struct MintedToken {
    pub access_token: String,
    pub expires_in: u64,
}

impl XeroClientCredentials {
    /// Build from `XERO_SANDBOX_*` env (the demo-company custom
    /// connection). Returns `None` when either essential is absent, so
    /// the live sandbox test self-skips off a CI runner without secrets.
    #[must_use]
    pub fn from_sandbox_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Some(Self::new(
            get("XERO_SANDBOX_CLIENT_ID")?,
            get("XERO_SANDBOX_CLIENT_SECRET")?,
            get("XERO_SANDBOX_TOKEN_URL").unwrap_or_else(|| DEFAULT_TOKEN_URL.to_string()),
            get("XERO_SANDBOX_SCOPE").unwrap_or_else(|| DEFAULT_SCOPE.to_string()),
        ))
    }

    /// Build directly from parts. Used by [`Self::from_env`] and tests
    /// (which point `token_url` at a mock server).
    #[must_use]
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        token_url: impl Into<String>,
        scope: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token_url: token_url.into(),
            scope: scope.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Build from the canonical runtime env (the `XERO_*` scheme the app
    /// and `.env` use), returning `None` when the client-credentials
    /// essentials are absent so the provider can fall back to a static
    /// access token (or the stub).
    ///
    /// Required: `XERO_CLIENT_ID`, `XERO_CLIENT_SECRET`. Optional:
    /// `XERO_TOKEN_URL` (defaults to Xero's identity endpoint),
    /// `XERO_SCOPE` (defaults to the invoice + contact scopes).
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let get = |k: &str| std::env::var(k).ok().filter(|s| !s.is_empty());
        Some(Self::new(
            get("XERO_CLIENT_ID")?,
            get("XERO_CLIENT_SECRET")?,
            get("XERO_TOKEN_URL").unwrap_or_else(|| DEFAULT_TOKEN_URL.to_string()),
            get("XERO_SCOPE").unwrap_or_else(|| DEFAULT_SCOPE.to_string()),
        ))
    }

    /// Mint a short-lived Accounting API access token via the
    /// client-credentials grant, returning the token and the lifetime
    /// Xero reported. The one network call in this module.
    pub async fn mint(&self) -> Result<MintedToken, BillingError> {
        let resp = self
            .http
            .post(&self.token_url)
            .header(
                reqwest::header::AUTHORIZATION,
                basic_auth_header(&self.client_id, &self.client_secret),
            )
            .form(&[
                ("grant_type", "client_credentials"),
                ("scope", self.scope.as_str()),
            ])
            .send()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(BillingError::Provider(format!(
                "xero token endpoint responded {status}: {body}"
            )));
        }
        let token: TokenResponse = resp
            .json()
            .await
            .map_err(|e| BillingError::Provider(e.to_string()))?;
        Ok(MintedToken {
            access_token: token.access_token,
            expires_in: token.expires_in,
        })
    }

    /// Convenience wrapper returning just the access token — used by the
    /// live sandbox smoke test, which doesn't cache.
    pub async fn mint_access_token(&self) -> Result<String, BillingError> {
        Ok(self.mint().await?.access_token)
    }
}

#[cfg(test)]
mod tests {
    use super::{basic_auth_header, XeroClientCredentials, DEFAULT_EXPIRES_IN};
    use base64::Engine;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn basic_auth_header_is_base64_of_id_colon_secret() {
        let h = basic_auth_header("client-abc", "secret-xyz");
        let encoded = h.strip_prefix("Basic ").expect("Basic prefix");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("valid base64");
        assert_eq!(decoded, b"client-abc:secret-xyz");
    }

    #[tokio::test]
    async fn mint_posts_client_credentials_and_returns_token_and_lifetime() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/connect/token"))
            // The client id+secret ride in the Basic header, and the body
            // is the client-credentials grant with our scopes.
            .and(header(
                "authorization",
                basic_auth_header("ck", "cs").as_str(),
            ))
            .and(body_string_contains("grant_type=client_credentials"))
            .and(body_string_contains("accounting.invoices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "xero-tok",
                "token_type": "Bearer",
                "expires_in": 1800,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let auth = XeroClientCredentials::new(
            "ck",
            "cs",
            format!("{}/connect/token", server.uri()),
            "accounting.contacts accounting.invoices",
        );
        let minted = auth.mint().await.expect("mint succeeds against the mock");
        assert_eq!(minted.access_token, "xero-tok");
        assert_eq!(minted.expires_in, 1800);
    }

    #[tokio::test]
    async fn mint_defaults_lifetime_when_response_omits_expires_in() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/connect/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "no-exp",
            })))
            .mount(&server)
            .await;

        let auth = XeroClientCredentials::new(
            "ck",
            "cs",
            format!("{}/connect/token", server.uri()),
            "accounting.transactions",
        );
        let minted = auth.mint().await.expect("mint succeeds");
        assert_eq!(
            minted.expires_in, DEFAULT_EXPIRES_IN,
            "absent expires_in defaults to 30m"
        );
    }

    #[tokio::test]
    async fn mint_maps_non_2xx_to_provider_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/connect/token"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid_client"))
            .mount(&server)
            .await;

        let auth = XeroClientCredentials::new(
            "ck",
            "bad",
            format!("{}/connect/token", server.uri()),
            "accounting.transactions",
        );
        let err = auth.mint().await.expect_err("a 401 is a provider error");
        assert!(err.to_string().contains("401"), "status surfaces: {err}");
    }
}
