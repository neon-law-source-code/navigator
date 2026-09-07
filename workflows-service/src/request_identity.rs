use restate_sdk::endpoint::Builder;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

pub const RESTATE_IDENTITY_KEY: &str = "RESTATE_IDENTITY_KEY";

/// Pins `jsonwebtoken`'s process-level `CryptoProvider` to `rust_crypto`.
///
/// `store`'s `surrealdb-core` dependency compiles `jsonwebtoken` 10.4.0 with
/// its `aws_lc_rs` feature on every native target, unconditionally — this
/// crate's own `restate-jwt` dependency (feeding `restate-sdk-shared-core`)
/// compiles the same crate version with `rust_crypto`. Cargo unifies both
/// into the one `jsonwebtoken` instance this binary links, so
/// `CryptoProvider::from_crate_features` sees two providers and panics the
/// first time a real request is verified (ENG-550) instead of never being
/// ambiguous. Neither dependency exposes a feature to turn the other back
/// off, so the ambiguity must be resolved at runtime: call this once, before
/// the server starts accepting requests, to make the choice deterministic
/// regardless of what Cargo resolves.
pub fn install_crypto_provider() {
    let _ = restate_jwt::crypto::rust_crypto::DEFAULT_PROVIDER.install_default();
}

#[derive(Serialize, Deserialize)]
struct HealthCheckClaims {
    exp: u64,
}

/// Round-trips a throwaway HMAC-signed token through the exact
/// `jsonwebtoken` sign/verify path Restate Cloud's request-identity
/// signatures use, proving the process-level `CryptoProvider` pinned by
/// [`install_crypto_provider`] is still deterministic instead of ambiguous
/// (ENG-550). `catch_unwind` turns a regression back into a `bool` a health
/// probe can act on, rather than a panic that would also take the probe
/// down (ENG-551).
#[must_use]
pub fn crypto_provider_is_healthy() -> bool {
    std::panic::catch_unwind(|| {
        const SECRET: &[u8] = b"workflows-service-health-check";
        let exp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_secs()
            + 60;
        let token = restate_jwt::encode(
            &restate_jwt::Header::new(restate_jwt::Algorithm::HS256),
            &HealthCheckClaims { exp },
            &restate_jwt::EncodingKey::from_secret(SECRET),
        )
        .expect("health-check token signs");
        restate_jwt::decode::<HealthCheckClaims>(
            &token,
            &restate_jwt::DecodingKey::from_secret(SECRET),
            &restate_jwt::Validation::new(restate_jwt::Algorithm::HS256),
        )
        .is_ok()
    })
    .unwrap_or(false)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IdentityConfigError {
    #[error("{RESTATE_IDENTITY_KEY} must be set for the production Restate Cloud worker")]
    Missing,
    #[error("invalid {RESTATE_IDENTITY_KEY}: {0}")]
    Invalid(String),
}

pub fn apply_identity_key<F: Fn(&str) -> Option<String>>(
    builder: Builder,
    environment: store::DeploymentEnvironment,
    get: F,
) -> Result<Builder, IdentityConfigError> {
    if environment == store::DeploymentEnvironment::Dev {
        return Ok(builder);
    }

    let key = get(RESTATE_IDENTITY_KEY)
        .filter(|key| !key.trim().is_empty())
        .ok_or(IdentityConfigError::Missing)?;
    builder
        .identity_key(&key)
        .map_err(|error| IdentityConfigError::Invalid(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        apply_identity_key, crypto_provider_is_healthy, install_crypto_provider,
        IdentityConfigError,
    };
    use axum::body::Body;
    use axum::http::Request;
    use ed25519_dalek::{pkcs8::EncodePrivateKey, SigningKey};
    use restate_jwt::{EncodingKey, Header};
    use restate_sdk::endpoint::Endpoint;
    use serde::Serialize;
    use std::time::{SystemTime, UNIX_EPOCH};

    const INVOKE_PATH: &str = "/invoke/foo/bar";

    #[derive(Serialize)]
    struct Claims<'a> {
        aud: &'a str,
        exp: u64,
        iat: u64,
        nbf: u64,
    }

    fn endpoint_with_key(key: Option<&str>) -> Endpoint {
        let builder = apply_identity_key(
            Endpoint::builder(),
            store::DeploymentEnvironment::Production,
            |_| key.map(str::to_owned),
        )
        .expect("test identity key is valid");
        builder.build()
    }

    fn response_status(endpoint: &Endpoint, request: Request<Body>) -> u16 {
        endpoint.handle(request).status().as_u16()
    }

    fn signed_request() -> (String, String) {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let identity_key = format!(
            "publickeyv1_{}",
            bs58::encode(signing_key.verifying_key().to_bytes()).into_string()
        );
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_secs();
        let claims = Claims {
            aud: INVOKE_PATH,
            exp: now + 60,
            iat: now,
            nbf: now.saturating_sub(60),
        };
        let mut header = Header::new(restate_jwt::Algorithm::EdDSA);
        header.typ = Some("JWT".into());
        header.kid = Some(identity_key.clone());
        let private_key = signing_key
            .to_pkcs8_der()
            .expect("test signing key encodes as PKCS#8");
        let token = restate_jwt::encode(
            &header,
            &claims,
            &EncodingKey::from_ed_der(private_key.as_bytes()),
        )
        .expect("test token signs");
        (identity_key, token)
    }

    fn signed_http_request(token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(INVOKE_PATH)
            .header("content-type", "application/vnd.restate.invocation.v5")
            .header("x-restate-signature-scheme", "v1")
            .header("x-restate-jwt-v1", token)
            .body(Body::empty())
            .expect("test request has valid headers")
    }

    #[test]
    fn production_requires_an_identity_key() {
        let Err(error) = apply_identity_key(
            Endpoint::builder(),
            store::DeploymentEnvironment::Production,
            |_| None,
        ) else {
            panic!("production must not boot without request identity");
        };
        assert_eq!(error, IdentityConfigError::Missing);
    }

    #[test]
    fn dev_remains_keyless() {
        let endpoint = apply_identity_key(
            Endpoint::builder(),
            store::DeploymentEnvironment::Dev,
            |_| None,
        )
        .expect("dev may omit request identity")
        .build();
        assert_eq!(
            response_status(
                &endpoint,
                Request::builder()
                    .method("POST")
                    .uri(INVOKE_PATH)
                    .header("content-type", "application/vnd.restate.invocation.v5")
                    .body(Body::empty())
                    .expect("test request has a valid path")
            ),
            404
        );
    }

    #[test]
    fn production_endpoint_accepts_only_a_valid_identity_signature() {
        install_crypto_provider();
        let (identity_key, token) = signed_request();
        let endpoint = endpoint_with_key(Some(&identity_key));

        assert_eq!(
            response_status(
                &endpoint,
                Request::builder()
                    .method("POST")
                    .uri(INVOKE_PATH)
                    .header("content-type", "application/vnd.restate.invocation.v5")
                    .body(Body::empty())
                    .expect("unsigned test request has a valid path")
            ),
            401
        );
        assert_eq!(
            response_status(&endpoint, signed_http_request(&token[..token.len() - 1])),
            401
        );
        assert_eq!(response_status(&endpoint, signed_http_request(&token)), 404);
    }

    #[test]
    fn crypto_provider_health_check_passes_once_the_provider_is_pinned() {
        install_crypto_provider();
        assert!(crypto_provider_is_healthy());
    }
}
