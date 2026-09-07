//! Exercises Restate request-identity verification with a *real* signature,
//! in its own process (a separate binary from every other test in this
//! crate). Unit tests in `request_identity.rs` share one process, so once any
//! of them calls `CryptoProvider::install_default()` it stays installed for
//! every test that runs after it — masking the exact ambiguity this file
//! exists to catch. `store`'s `surrealdb-core` dependency compiles
//! `jsonwebtoken` with the `aws_lc_rs` feature; `restate-jwt` (this crate's
//! own `jsonwebtoken` dependency, feeding `restate-sdk-shared-core`) compiles
//! it with `rust_crypto`. Cargo unifies both into the one `jsonwebtoken`
//! instance workflows-service links, so `CryptoProvider::from_crate_features`
//! can't pick a default and panics the first time a real request is
//! verified (ENG-550).

use axum::body::Body;
use axum::http::Request;
use ed25519_dalek::pkcs8::EncodePrivateKey;
use ed25519_dalek::SigningKey;
use restate_jwt::{EncodingKey, Header};
use restate_sdk::endpoint::Endpoint;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use workflows_service::request_identity::{apply_identity_key, install_crypto_provider};

const INVOKE_PATH: &str = "/invoke/foo/bar";

#[derive(Serialize)]
struct Claims<'a> {
    aud: &'a str,
    exp: u64,
    iat: u64,
    nbf: u64,
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

/// Reproduces ENG-550, and proves `main()`'s fix for it: mirroring the
/// `install_crypto_provider()` call `main()` makes before the server starts
/// makes the same signature verification that used to panic (both
/// `rust_crypto` and `aws_lc_rs` are compiled into the one shared
/// `jsonwebtoken` instance) succeed deterministically instead.
#[test]
fn production_endpoint_verifies_a_real_signature_without_a_preinstalled_provider() {
    install_crypto_provider();
    let (identity_key, token) = signed_request();
    let endpoint = apply_identity_key(
        Endpoint::builder(),
        store::DeploymentEnvironment::Production,
        |_| Some(identity_key.clone()),
    )
    .expect("test identity key is valid")
    .build();

    let request = Request::builder()
        .method("POST")
        .uri(INVOKE_PATH)
        .header("content-type", "application/vnd.restate.invocation.v5")
        .header("x-restate-signature-scheme", "v1")
        .header("x-restate-jwt-v1", &token)
        .body(Body::empty())
        .expect("signed test request has valid headers");

    // A verified-but-unbound-handler request 404s; the point of this
    // assertion is reaching it at all instead of panicking first.
    assert_eq!(endpoint.handle(request).status().as_u16(), 404);
}
