//! HTTP-server configuration.
//!
//! The store's own connection contract lives in the `store` crate so
//! non-`web` consumers (`cli`, `mcp`) can open one without pulling in
//! the HTTP stack. This module owns only what's HTTP-specific.
//!
//! | Variable             | Default              | Purpose                                       |
//! |----------------------|----------------------|-----------------------------------------------|
//! | `PORT`               | `3001`               | TCP port to bind.                             |
//!
//! `from_lookup` is the testable seam: the production `from_env`
//! is a thin wrapper that delegates to `std::env::var`.

use store::{
    deployment::{
        applicable_web_requirements, ci_harness_enabled, harness_relaxations_apply,
        CREDENTIAL_ENVIRONMENT,
    },
    DeploymentEnvironment, DeploymentEnvironmentError,
};
use thiserror::Error;

/// Top-level application configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    pub port: u16,
    pub environment: DeploymentEnvironment,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("PORT must be a u16, got `{0}`")]
    BadPort(String),
    #[error(transparent)]
    Environment(#[from] DeploymentEnvironmentError),
}

/// Failures from [`enforce_deployment_invariants`]. Carries the full list
/// of violations so operators can fix the deploy in one pass instead
/// of redeploy-fix-redeploy roulette.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{environment} invariants violated:\n  - {}", violations.join("\n  - "))]
pub struct DeploymentInvariantError {
    pub environment: &'static str,
    pub violations: Vec<String>,
}

pub const DEFAULT_PORT: u16 = 3001;

impl AppConfig {
    /// Build an `AppConfig` from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Build an `AppConfig` from any `key -> Option<value>` lookup.
    /// This is the seam tests use to avoid mutating process env vars.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(get: F) -> Result<Self, ConfigError> {
        let port = match get("PORT") {
            None => DEFAULT_PORT,
            Some(raw) => raw.parse().map_err(|_| ConfigError::BadPort(raw))?,
        };
        let environment = DeploymentEnvironment::from_lookup(&get)?;
        Ok(AppConfig { port, environment })
    }
}

/// Enforce environment invariants on the running binary before it
/// starts serving traffic. A passthrough embedded Rego policy client, an in-memory
/// workflow runtime, or a filesystem storage backend silently weaken
/// policy / durability / persistence — crash with a structured error
/// instead.
///
/// Production retains hosted persistence and real integration requirements.
/// The dev profile permits disposable service wiring but requires real
/// non-production integrations unless the explicit CI harness is enabled.
#[allow(clippy::too_many_lines)] // a flat checklist; splitting it hurts readability
pub fn enforce_deployment_invariants<F: Fn(&str) -> Option<String>>(
    environment: DeploymentEnvironment,
    get: F,
) -> Result<(), DeploymentInvariantError> {
    let mut violations: Vec<String> = Vec::new();
    for requirement in applicable_web_requirements(environment, &get) {
        let satisfied = requirement.any_of.iter().any(|alternative| {
            alternative
                .iter()
                .all(|key| get(key).is_some_and(|value| !value.is_empty()))
        });
        if !satisfied {
            let alternatives = requirement
                .any_of
                .iter()
                .map(|alternative| alternative.join(" + "))
                .collect::<Vec<_>>()
                .join(" or ");
            violations.push(format!("{alternatives} must be set"));
        }
    }
    // The HMAC key that signs every browser session cookie AND every
    // `navigator site login` CLI bearer. If unset, `portal::SessionStore` falls
    // back to a random key minted fresh on each boot (see `main.rs`), so
    // every pod restart / rollout silently invalidates every active
    // session and forces all users to sign in again. Must also carry the
    // >=32 bytes of entropy the cookie design assumes.
    match get("SESSION_SECRET") {
        Some(s) if s.len() >= 32 => {}
        Some(_) => violations.push(
            "SESSION_SECRET must be at least 32 bytes (a shorter key weakens \
             the HMAC that signs every session cookie + CLI bearer token)"
                .into(),
        ),
        None => violations.push(
            "SESSION_SECRET must be set (otherwise SessionStore falls back to a \
             random per-boot key, so every pod restart / rollout invalidates \
             every active session and forces all users to sign in again)"
                .into(),
        ),
    }
    if get("OIDC_DISABLED")
        .as_deref()
        .is_some_and(|v| v == "true" || v == "1")
    {
        violations.push(
            "OIDC_DISABLED must not be `true`/`1` (it turns the bearer-token \
             verifier on /mcp + /api into an open pass-through)"
                .into(),
        );
    }
    // The asset origin flows into raw `<style>` `@font-face` `url('…')`
    // blocks and the CSP `img-src`/`font-src` directives. Validate it once
    // here so a malformed value crashes at boot instead of every call site
    // having to re-escape it — a well-formed origin cannot inject anywhere.
    // Unset/blank is valid (callers fall back to the `/public` mount), so
    // only a present-and-malformed value is a violation.
    if let Some(base) = get("NAVIGATOR_ASSET_BASE_URL") {
        if let Err(err) = views::assets::validate_asset_base_url(&base) {
            violations.push(format!("NAVIGATOR_ASSET_BASE_URL is invalid: {err}"));
        }
    }
    match environment {
        DeploymentEnvironment::Production => {
            if get("NAVIGATOR_STORAGE_BACKEND").is_some_and(|value| value != "gcs") {
                violations.push("NAVIGATOR_STORAGE_BACKEND must be `gcs` in production".into());
            }
            if get("NAVIGATOR_STORAGE_ENDPOINT").is_some_and(|value| !value.is_empty()) {
                violations.push(
                    "NAVIGATOR_STORAGE_ENDPOINT must be unset in production (hosted GCS cannot use an emulator or S3 endpoint)"
                        .into(),
                );
            }
            if ci_harness_enabled(&get) {
                violations.push("NAVIGATOR_CI_HARNESS must not be enabled in production".into());
            }
        }
        DeploymentEnvironment::Dev => {
            if !ci_harness_enabled(&get)
                && get("NAVIGATOR_STORAGE_BACKEND").as_deref() == Some("gcs")
                && get("NAVIGATOR_STORAGE_ENDPOINT").is_some_and(|value| !value.is_empty())
            {
                violations.push(
                    "NAVIGATOR_STORAGE_ENDPOINT must be unset for dev-profile GCS with Workload Identity"
                        .into(),
                );
            }
        }
    }

    // Scoped to the dev profile through the same predicate the requirements
    // table uses, so one flag keeps one meaning.
    //
    // Testing the raw flag here made every check below conditional on
    // `NAVIGATOR_CI_HARNESS` in *any* profile. That was not reachable — the
    // production arm above rejects the flag outright, so such a boot already
    // failed — but it left these checks depending on that neighbouring guard
    // rather than on their own evidence. Among them is the rejection of
    // DocuSign's demo OAuth host, which is what keeps a production deployment
    // from authenticating against demo and sending envelopes that carry no
    // legal weight. A check that load-bearing should not be switchable off as
    // a side effect of relaxing an unrelated flag.
    if !harness_relaxations_apply(environment, &get) {
        if get("NAVIGATOR_EMAIL_BACKEND").is_some_and(|value| value != "sendgrid") {
            violations.push("NAVIGATOR_EMAIL_BACKEND must be exactly `sendgrid`".into());
        }
        let expected = environment.as_str();
        if get(CREDENTIAL_ENVIRONMENT).is_some_and(|value| value != expected) {
            violations.push(format!(
                "{CREDENTIAL_ENVIRONMENT} must be exactly `{expected}` so credentials cannot cross deployment environments"
            ));
        }
        let docusign_base = get("DOCUSIGN_BASE_URL").unwrap_or_default();
        let oauth_base = get("DOCUSIGN_OAUTH_BASE").unwrap_or_default();
        match environment {
            DeploymentEnvironment::Dev => {
                if !docusign_base.is_empty()
                    && !docusign_base.starts_with("https://demo.docusign.net/")
                {
                    violations.push(
                        "DOCUSIGN_BASE_URL must use https://demo.docusign.net/ in the dev profile"
                            .into(),
                    );
                }
                if !oauth_base.is_empty() && oauth_base != "https://account-d.docusign.com" {
                    violations.push(
                        "DOCUSIGN_OAUTH_BASE must use https://account-d.docusign.com in the dev profile"
                            .into(),
                    );
                }
            }
            DeploymentEnvironment::Production => {
                if !docusign_base.is_empty() && docusign_base.contains("demo.docusign.net") {
                    violations.push("DOCUSIGN_BASE_URL must not use the DocuSign demo environment in production".into());
                }
                if oauth_base.contains("account-d.docusign.com") {
                    violations.push("DOCUSIGN_OAUTH_BASE must not use the DocuSign demo environment in production".into());
                }
                if get("DOCUSIGN_INTEGRATION_KEY").is_some_and(|value| !value.is_empty())
                    && oauth_base.is_empty()
                {
                    violations.push(
                        "DOCUSIGN_OAUTH_BASE must be set explicitly for production JWT credentials"
                            .into(),
                    );
                }
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(DeploymentInvariantError {
            environment: environment.as_str(),
            violations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{enforce_deployment_invariants, AppConfig, ConfigError};
    use cloud::workspace::NAVIGATOR_GITHUB_ORG;
    use std::collections::HashMap;
    use store::DeploymentEnvironment;

    fn production_invariants<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<(), super::DeploymentInvariantError> {
        enforce_deployment_invariants(DeploymentEnvironment::Production, get)
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    /// A 32-byte (32-char ASCII) `SESSION_SECRET` for the happy-path
    /// invariant tests — long enough to clear the length check.
    const SECRET32: &str = "0123456789abcdef0123456789abcdef";

    /// The synthetic organization every fixture in this module configures.
    ///
    /// Which organization a deployment's own automation lives in is
    /// *configuration*, so no real organization name is a constant or a fixture
    /// value here.
    const AN_ORGANIZATION: &str = "an-organization";

    #[test]
    fn port_is_parsed_from_env() {
        let cfg = AppConfig::from_lookup(lookup(&[("PORT", "8080")])).unwrap();
        assert_eq!(cfg.port, 8080);
    }

    #[test]
    fn invalid_port_is_an_error() {
        let err = AppConfig::from_lookup(lookup(&[("PORT", "not-a-number")])).unwrap_err();
        assert_eq!(err, ConfigError::BadPort("not-a-number".into()));
    }

    #[test]
    fn port_zero_is_accepted_for_dynamic_binding() {
        let cfg = AppConfig::from_lookup(lookup(&[("PORT", "0")])).unwrap();
        assert_eq!(cfg.port, 0);
    }

    #[test]
    fn prod_invariants_pass_when_all_set() {
        let result = production_invariants(lookup(&[
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "gcs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_FROM_EMAIL", "staging@example.com"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_INBOUND_SECRET", "secret"),
            ("SENDGRID_EVENTS_SECRET", "secret"),
            ("SENDGRID_EVENTS_PUBLIC_KEY", "base64-spki"),
            ("DOCUSIGN_HMAC_KEY", "hmac-secret"),
            ("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"),
            ("DOCUSIGN_OAUTH_BASE", "https://account.docusign.com"),
            ("DOCUSIGN_SIGNER_EMAIL", "signer@example.com"),
            ("DOCUSIGN_ACCOUNT_ID", "account"),
            ("DOCUSIGN_ACCESS_TOKEN", "token"),
            ("DOCUSIGN_WEBHOOK_SECRET", "webhook"),
            ("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "production"),
            ("SESSION_SECRET", SECRET32),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            ("NAVIGATOR_GITHUB_APP_ID", "123456"),
            ("NAVIGATOR_GITHUB_APP_PRIVATE_KEY", "test-pem"),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", "whsec"),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                "neon-law-source-code/navigator",
            ),
            ("NAVIGATOR_GITHUB_APP_LOGIN", "navigator-nightwatch[bot]"),
            ("RESTATE_INGRESS_URL", "https://ingress.restate.cloud:8080"),
            ("RESTATE_AUTH_TOKEN", "key_test"),
            ("GOOGLE_OAUTH_CLIENT_IDS", "123.apps.googleusercontent.com"),
            ("NAVIGATOR_SURREAL_ENDPOINT", "wss://example.surreal.cloud"),
            ("NAVIGATOR_SURREAL_NAMESPACE", "navigator"),
            ("NAVIGATOR_SURREAL_DATABASE", "navigator"),
            ("NAVIGATOR_SURREAL_USER", "admin"),
            ("NAVIGATOR_SURREAL_PASSWORD", "secret"),
        ]));
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn prod_invariants_require_the_google_oauth_client_allowlist() {
        // Unset, `google_oauth` degrades to a pass-through and
        // `mcp_principal` never injects a `Principal` — so AIDA's
        // project-scope check on `aida_create_notation` silently stops
        // running. Fail at boot rather than fail open per request.
        let base = [
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "gcs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_FROM_EMAIL", "staging@example.com"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_INBOUND_SECRET", "secret"),
            ("SENDGRID_EVENTS_SECRET", "secret"),
            ("SENDGRID_EVENTS_PUBLIC_KEY", "base64-spki"),
            ("DOCUSIGN_HMAC_KEY", "hmac-secret"),
            ("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"),
            ("DOCUSIGN_OAUTH_BASE", "https://account.docusign.com"),
            ("DOCUSIGN_SIGNER_EMAIL", "signer@example.com"),
            ("DOCUSIGN_ACCOUNT_ID", "account"),
            ("DOCUSIGN_ACCESS_TOKEN", "token"),
            ("DOCUSIGN_WEBHOOK_SECRET", "webhook"),
            ("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "production"),
            ("SESSION_SECRET", SECRET32),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            ("NAVIGATOR_GITHUB_APP_ID", "123456"),
            ("NAVIGATOR_GITHUB_APP_PRIVATE_KEY", "test-pem"),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", "whsec"),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                "neon-law-source-code/navigator",
            ),
            ("NAVIGATOR_GITHUB_APP_LOGIN", "navigator-nightwatch[bot]"),
            ("RESTATE_INGRESS_URL", "https://ingress.restate.cloud:8080"),
            ("RESTATE_AUTH_TOKEN", "key_test"),
            ("NAVIGATOR_SURREAL_ENDPOINT", "wss://example.surreal.cloud"),
            ("NAVIGATOR_SURREAL_NAMESPACE", "navigator"),
            ("NAVIGATOR_SURREAL_DATABASE", "navigator"),
            ("NAVIGATOR_SURREAL_USER", "admin"),
            ("NAVIGATOR_SURREAL_PASSWORD", "secret"),
        ];
        let err = production_invariants(lookup(&base)).unwrap_err();
        assert_eq!(err.violations.len(), 1);
        assert!(
            err.violations[0].starts_with("GOOGLE_OAUTH_CLIENT_IDS"),
            "expected the OAuth allowlist violation, got: {}",
            err.violations[0]
        );

        let mut with_ids = base.to_vec();
        with_ids.push(("GOOGLE_OAUTH_CLIENT_IDS", "123.apps.googleusercontent.com"));
        assert!(production_invariants(lookup(&with_ids)).is_ok());
    }

    #[test]
    fn only_the_automation_home_requires_github_receiver_credentials() {
        let receiver_keys = [
            "NAVIGATOR_GITHUB_WEBHOOK_SECRET",
            "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
            "NAVIGATOR_GITHUB_APP_LOGIN",
            "RESTATE_INGRESS_URL",
            "RESTATE_AUTH_TOKEN",
        ];
        let without_receiver = |project_id: &'static str| {
            let mut pairs = full_with_jwks();
            pairs.retain(|(key, _)| !receiver_keys.contains(key));
            pairs.push(("NAVIGATOR_GCP_PROJECT_ID", project_id));
            pairs
        };

        assert!(
            production_invariants(lookup(&without_receiver("neon-law"))).is_ok(),
            "a tenant deployment must not need the singleton receiver credentials"
        );

        let err = production_invariants(lookup(&without_receiver("neon-law-stg"))).unwrap_err();
        assert_eq!(err.violations.len(), 1);
        assert!(
            err.violations[0].starts_with("NAVIGATOR_GITHUB_WEBHOOK_SECRET"),
            "the automation home must fail closed without receiver credentials: {err:?}"
        );
    }

    #[test]
    fn prod_invariants_collect_every_missing_var_at_once() {
        // Operators should not have to fix one var, redeploy, fix
        // the next. Every missing var must surface in a single error.
        //
        // The fixture declares `DOCUSIGN_BASE_URL` and nothing else about
        // DocuSign, because that is the deployment this test is about: one
        // that means to sign and has configured it incompletely. DocuSign is
        // trigger-gated on that key, so a deployment omitting it declines the
        // integration outright and is owed no DocuSign diagnostic at all —
        // asserted in `store::deployment`.
        let err = production_invariants(|key| match key {
            "NAVIGATOR_GCP_PROJECT_ID" => {
                Some(store::deployment::GITHUB_AUTOMATION_HOME_PROJECT.to_string())
            }
            "DOCUSIGN_BASE_URL" => Some("https://na4.docusign.net/restapi".to_string()),
            _ => None,
        })
        .unwrap_err();
        // The exact set can grow as a deployment invariant is added; the
        // assertions below pin the required diagnostics without making this
        // aggregate count brittle across feature profiles.
        assert!(err.violations.len() >= 20);
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("GOOGLE_OAUTH_CLIENT_IDS")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_GITHUB_WEBHOOK_SECRET")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_GITHUB_ORG")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("SESSION_SECRET")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("DOCUSIGN_HMAC_KEY")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("SENDGRID_EVENTS_PUBLIC_KEY")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("RESTATE_BROKER_URL")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_CLAMD_ADDR")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_STORAGE_BACKEND")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("SENDGRID_API_KEY")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("SENDGRID_INBOUND_SECRET")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("SENDGRID_EVENTS_SECRET")));
    }

    /// The full happy-set plus the JWKS bearer path: audience + issuer
    /// pinned, OIDC not disabled.
    fn full_with_jwks() -> Vec<(&'static str, &'static str)> {
        vec![
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "gcs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_FROM_EMAIL", "staging@example.com"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_INBOUND_SECRET", "secret"),
            ("SENDGRID_EVENTS_SECRET", "secret"),
            ("SENDGRID_EVENTS_PUBLIC_KEY", "base64-spki"),
            ("DOCUSIGN_HMAC_KEY", "hmac-secret"),
            ("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"),
            ("DOCUSIGN_OAUTH_BASE", "https://account.docusign.com"),
            ("DOCUSIGN_SIGNER_EMAIL", "signer@example.com"),
            ("DOCUSIGN_ACCOUNT_ID", "account"),
            ("DOCUSIGN_ACCESS_TOKEN", "token"),
            ("DOCUSIGN_WEBHOOK_SECRET", "webhook"),
            ("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "production"),
            ("SESSION_SECRET", SECRET32),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            ("NAVIGATOR_GITHUB_APP_ID", "123456"),
            ("NAVIGATOR_GITHUB_APP_PRIVATE_KEY", "test-pem"),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", "whsec"),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                "neon-law-source-code/navigator",
            ),
            ("NAVIGATOR_GITHUB_APP_LOGIN", "navigator-nightwatch[bot]"),
            ("RESTATE_INGRESS_URL", "https://ingress.restate.cloud:8080"),
            ("RESTATE_AUTH_TOKEN", "key_test"),
            ("OIDC_JWKS_URL", "https://idp/jwks"),
            ("OIDC_AUDIENCE", "navigator-web"),
            ("OIDC_ISSUER", "https://idp"),
            ("GOOGLE_OAUTH_CLIENT_IDS", "123.apps.googleusercontent.com"),
            // The SurrealDB coordinates. `web` fails closed without the
            // endpoint, so a production-shaped fixture carries all three the
            // way a real deployment's `config.toml` does.
            (
                "NAVIGATOR_SURREAL_ENDPOINT",
                "wss://example-instance.aws-usw2.surreal.cloud",
            ),
            ("NAVIGATOR_SURREAL_NAMESPACE", "navigator"),
            ("NAVIGATOR_SURREAL_DATABASE", "navigator"),
            ("NAVIGATOR_SURREAL_USER", "admin"),
            ("NAVIGATOR_SURREAL_PASSWORD", "secret"),
        ]
    }

    #[test]
    fn oidc_disabled_true_is_rejected() {
        let mut pairs = full_with_jwks();
        pairs.push(("OIDC_DISABLED", "true"));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("OIDC_DISABLED")));
    }

    #[test]
    fn jwks_path_requires_audience_and_issuer() {
        // JWKS set but neither audience nor issuer → two violations.
        let err = production_invariants(lookup(&[
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "gcs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_FROM_EMAIL", "staging@example.com"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_INBOUND_SECRET", "secret"),
            ("SENDGRID_EVENTS_SECRET", "secret"),
            ("SENDGRID_EVENTS_PUBLIC_KEY", "base64-spki"),
            ("DOCUSIGN_HMAC_KEY", "hmac-secret"),
            ("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"),
            ("DOCUSIGN_OAUTH_BASE", "https://account.docusign.com"),
            ("DOCUSIGN_SIGNER_EMAIL", "signer@example.com"),
            ("DOCUSIGN_ACCOUNT_ID", "account"),
            ("DOCUSIGN_ACCESS_TOKEN", "token"),
            ("DOCUSIGN_WEBHOOK_SECRET", "webhook"),
            ("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "production"),
            ("SESSION_SECRET", SECRET32),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            ("NAVIGATOR_GITHUB_APP_ID", "123456"),
            ("NAVIGATOR_GITHUB_APP_PRIVATE_KEY", "test-pem"),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", "whsec"),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                "neon-law-source-code/navigator",
            ),
            ("NAVIGATOR_GITHUB_APP_LOGIN", "navigator-nightwatch[bot]"),
            ("RESTATE_INGRESS_URL", "https://ingress.restate.cloud:8080"),
            ("RESTATE_AUTH_TOKEN", "key_test"),
            ("OIDC_JWKS_URL", "https://idp/jwks"),
        ]))
        .unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("OIDC_AUDIENCE")));
        assert!(err.violations.iter().any(|v| v.starts_with("OIDC_ISSUER")));
    }

    #[test]
    fn jwks_path_passes_with_audience_and_issuer() {
        assert!(production_invariants(lookup(&full_with_jwks())).is_ok());
    }

    #[test]
    fn a_malformed_asset_base_url_is_a_violation() {
        // A present-but-hostile asset origin (the `</style>` breakout #493
        // escaped) must fail fast at boot, next to the other invariants.
        let mut pairs = full_with_jwks();
        pairs.push((
            "NAVIGATOR_ASSET_BASE_URL",
            "https://evil.test/x'</style><script>",
        ));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_ASSET_BASE_URL")));
    }

    #[test]
    fn a_well_formed_asset_base_url_passes() {
        // The real production shape (an absolute bucket origin) is accepted,
        // so the guard does not reject a legitimate deploy.
        let mut pairs = full_with_jwks();
        pairs.push((
            "NAVIGATOR_ASSET_BASE_URL",
            "https://storage.example.test/navigator-assets",
        ));
        assert!(production_invariants(lookup(&pairs)).is_ok());
    }

    #[test]
    fn prod_invariants_reject_filesystem_backend() {
        let err = production_invariants(lookup(&[
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "fs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_FROM_EMAIL", "staging@example.com"),
            ("SENDGRID_API_KEY", "SG.test"),
            ("SENDGRID_INBOUND_SECRET", "secret"),
            ("SENDGRID_EVENTS_SECRET", "secret"),
            ("SENDGRID_EVENTS_PUBLIC_KEY", "base64-spki"),
            ("DOCUSIGN_HMAC_KEY", "hmac-secret"),
            ("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"),
            ("DOCUSIGN_OAUTH_BASE", "https://account.docusign.com"),
            ("DOCUSIGN_SIGNER_EMAIL", "signer@example.com"),
            ("DOCUSIGN_ACCOUNT_ID", "account"),
            ("DOCUSIGN_ACCESS_TOKEN", "token"),
            ("DOCUSIGN_WEBHOOK_SECRET", "webhook"),
            ("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "production"),
            ("SESSION_SECRET", SECRET32),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            ("NAVIGATOR_GITHUB_APP_ID", "123456"),
            ("NAVIGATOR_GITHUB_APP_PRIVATE_KEY", "test-pem"),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", "whsec"),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                "neon-law-source-code/navigator",
            ),
            ("NAVIGATOR_GITHUB_APP_LOGIN", "navigator-nightwatch[bot]"),
            ("RESTATE_INGRESS_URL", "https://ingress.restate.cloud:8080"),
            ("RESTATE_AUTH_TOKEN", "key_test"),
            ("GOOGLE_OAUTH_CLIENT_IDS", "123.apps.googleusercontent.com"),
            ("NAVIGATOR_SURREAL_ENDPOINT", "wss://example.surreal.cloud"),
            ("NAVIGATOR_SURREAL_NAMESPACE", "navigator"),
            ("NAVIGATOR_SURREAL_DATABASE", "navigator"),
            ("NAVIGATOR_SURREAL_USER", "admin"),
            ("NAVIGATOR_SURREAL_PASSWORD", "secret"),
        ]))
        .unwrap_err();
        assert_eq!(err.violations.len(), 1);
        assert!(err.violations[0].contains("must be `gcs` in production"));
    }

    #[test]
    fn session_secret_shorter_than_32_bytes_is_rejected() {
        let mut pairs = full_with_jwks();
        // Replace the happy-path 32-byte secret with a too-short one.
        pairs.retain(|(k, _)| *k != "SESSION_SECRET");
        pairs.push(("SESSION_SECRET", "too-short"));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert_eq!(err.violations.len(), 1);
        assert!(err.violations[0].starts_with("SESSION_SECRET"));
        assert!(err.violations[0].contains("at least 32 bytes"));
    }

    fn dev_pairs() -> Vec<(&'static str, &'static str)> {
        vec![
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "gcs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("SLACK_BOT_TOKEN", "xoxb-test"),
            ("NAVIGATOR_EMAIL_BACKEND", "sendgrid"),
            ("SENDGRID_API_KEY", "SG.dev"),
            ("SENDGRID_FROM_EMAIL", "dev@example.com"),
            ("SENDGRID_INBOUND_SECRET", "secret"),
            ("SENDGRID_EVENTS_SECRET", "secret"),
            ("SENDGRID_EVENTS_PUBLIC_KEY", "base64-spki"),
            ("DOCUSIGN_BASE_URL", "https://demo.docusign.net/restapi"),
            ("DOCUSIGN_OAUTH_BASE", "https://account-d.docusign.com"),
            ("DOCUSIGN_SIGNER_EMAIL", "signer@example.com"),
            ("DOCUSIGN_ACCOUNT_ID", "account"),
            ("DOCUSIGN_ACCESS_TOKEN", "token"),
            ("DOCUSIGN_HMAC_KEY", "hmac"),
            ("DOCUSIGN_WEBHOOK_SECRET", "webhook"),
            ("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "dev"),
            ("SESSION_SECRET", SECRET32),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            ("NAVIGATOR_GITHUB_APP_ID", "123456"),
            ("NAVIGATOR_GITHUB_APP_PRIVATE_KEY", "test-pem"),
            ("NAVIGATOR_GITHUB_WEBHOOK_SECRET", "whsec"),
            (
                "NAVIGATOR_GITHUB_CANONICAL_REPOSITORY",
                "neon-law-source-code/navigator",
            ),
            ("NAVIGATOR_GITHUB_APP_LOGIN", "navigator-nightwatch[bot]"),
            ("RESTATE_INGRESS_URL", "https://ingress.restate.cloud:8080"),
            ("RESTATE_AUTH_TOKEN", "key_test"),
            ("GOOGLE_OAUTH_CLIENT_IDS", "123.apps.googleusercontent.com"),
            ("NAVIGATOR_SURREAL_ENDPOINT", "wss://example.surreal.cloud"),
            ("NAVIGATOR_SURREAL_NAMESPACE", "navigator"),
            ("NAVIGATOR_SURREAL_DATABASE", "navigator"),
            ("NAVIGATOR_SURREAL_USER", "admin"),
            ("NAVIGATOR_SURREAL_PASSWORD", "secret"),
        ]
    }

    #[test]
    fn dev_requires_demo_credentials_and_rejects_production_credentials() {
        assert!(
            enforce_deployment_invariants(DeploymentEnvironment::Dev, lookup(&dev_pairs())).is_ok()
        );

        let mut crossed = dev_pairs();
        crossed.retain(|(key, _)| *key != "NAVIGATOR_CREDENTIAL_ENVIRONMENT");
        crossed.push(("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "production"));
        let err = enforce_deployment_invariants(DeploymentEnvironment::Dev, lookup(&crossed))
            .unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|violation| violation.starts_with("NAVIGATOR_CREDENTIAL_ENVIRONMENT")));
    }

    #[test]
    fn production_rejects_dev_credentials_and_docusign_demo() {
        let mut pairs = full_with_jwks();
        pairs.retain(|(key, _)| {
            !matches!(
                *key,
                "NAVIGATOR_CREDENTIAL_ENVIRONMENT" | "DOCUSIGN_BASE_URL"
            )
        });
        pairs.push(("NAVIGATOR_CREDENTIAL_ENVIRONMENT", "dev"));
        pairs.push(("DOCUSIGN_BASE_URL", "https://demo.docusign.net/restapi"));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|violation| violation.starts_with("NAVIGATOR_CREDENTIAL_ENVIRONMENT")));
        assert!(err
            .violations
            .iter()
            .any(|violation| violation.starts_with("DOCUSIGN_BASE_URL")));
    }

    #[test]
    fn only_explicit_dev_ci_harness_may_use_fakes() {
        let harness = [
            ("RESTATE_BROKER_URL", "http://restate:9070"),
            ("NAVIGATOR_CLAMD_ADDR", "clamav:3310"),
            ("NAVIGATOR_STORAGE_BACKEND", "gcs"),
            ("NAVIGATOR_APPLICATIONS_BUCKET", "proj-applications"),
            ("SESSION_SECRET", SECRET32),
            // Not integration-tier: `web` cannot boot without a Surreal
            // handle, so the harness supplies the coordinates exactly as
            // `.github/workflows/ci.yml` does for the server-mode lane.
            ("NAVIGATOR_SURREAL_ENDPOINT", "ws://127.0.0.1:8000"),
            ("NAVIGATOR_SURREAL_NAMESPACE", "navigator"),
            ("NAVIGATOR_SURREAL_DATABASE", "navigator"),
            ("NAVIGATOR_SURREAL_USER", "admin"),
            ("NAVIGATOR_SURREAL_PASSWORD", "secret"),
            ("NAVIGATOR_CI_HARNESS", "1"),
        ];
        assert!(
            enforce_deployment_invariants(DeploymentEnvironment::Dev, lookup(&harness)).is_ok()
        );
        assert!(
            enforce_deployment_invariants(DeploymentEnvironment::Production, lookup(&harness))
                .is_err()
        );
    }

    #[test]
    fn the_docusign_demo_rejection_does_not_depend_on_the_harness_guard() {
        // Defense in depth, not a reachable bypass: production + the harness
        // flag is already its own violation a few lines up, so this boot fails
        // either way. What is asserted here is *independence* — the DocuSign
        // demo-host rejection must fire on its own evidence rather than rely
        // on that neighbouring guard staying in place. Relaxing
        // `NAVIGATOR_CI_HARNESS` handling for some future production-shaped
        // smoke test would otherwise switch this check off as a side effect,
        // with nothing failing to say so.
        let mut pairs = full_with_jwks();
        pairs.push(("NAVIGATOR_CI_HARNESS", "1"));
        pairs.retain(|(key, _)| *key != "DOCUSIGN_OAUTH_BASE");
        pairs.push(("DOCUSIGN_OAUTH_BASE", "https://account-d.docusign.com"));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert!(
            err.violations
                .iter()
                .any(|violation| violation.starts_with("DOCUSIGN_OAUTH_BASE")),
            "the demo OAuth host must be rejected on its own evidence, got: {:?}",
            err.violations
        );
    }

    fn dev_invariants<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<(), super::DeploymentInvariantError> {
        enforce_deployment_invariants(DeploymentEnvironment::Dev, get)
    }

    #[test]
    fn production_rejects_a_storage_endpoint() {
        // Hosted GCS in production must not point at an emulator/S3 endpoint.
        let mut pairs = full_with_jwks();
        pairs.push(("NAVIGATOR_STORAGE_ENDPOINT", "http://test-storage:4443"));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_STORAGE_ENDPOINT")));
    }

    #[test]
    fn dev_gcs_rejects_a_storage_endpoint() {
        // The cloud staging lane runs the dev profile on real GCS through
        // Workload Identity.
        let mut pairs = dev_pairs();
        pairs.push(("NAVIGATOR_STORAGE_ENDPOINT", "http://test-storage:4443"));
        let err = dev_invariants(lookup(&pairs)).unwrap_err();
        assert!(err.violations.iter().any(
            |v| v.starts_with("NAVIGATOR_STORAGE_ENDPOINT") && v.contains("Workload Identity")
        ));
    }

    #[test]
    fn non_harness_requires_sendgrid_email_backend() {
        let mut pairs = full_with_jwks();
        pairs.retain(|(k, _)| *k != "NAVIGATOR_EMAIL_BACKEND");
        pairs.push(("NAVIGATOR_EMAIL_BACKEND", "capturing"));
        let err = production_invariants(lookup(&pairs)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("NAVIGATOR_EMAIL_BACKEND")));
    }

    #[test]
    fn dev_docusign_must_use_demo_endpoints() {
        let mut pairs = dev_pairs();
        pairs.retain(|(k, _)| *k != "DOCUSIGN_BASE_URL");
        pairs.push(("DOCUSIGN_BASE_URL", "https://na4.docusign.net/restapi"));
        pairs.push(("DOCUSIGN_OAUTH_BASE", "https://account.docusign.com"));
        let err = dev_invariants(lookup(&pairs)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("DOCUSIGN_BASE_URL")));
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("DOCUSIGN_OAUTH_BASE")));
    }

    #[test]
    fn production_docusign_rejects_demo_oauth_and_requires_jwt_oauth_base() {
        // A demo OAuth base is rejected in production.
        let mut demo = full_with_jwks();
        demo.push(("DOCUSIGN_OAUTH_BASE", "https://account-d.docusign.com"));
        let err = production_invariants(lookup(&demo)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.starts_with("DOCUSIGN_OAUTH_BASE") && v.contains("demo environment")));

        // JWT credentials (an integration key) with no OAuth base are rejected.
        // The base has to be taken back out: a fixture that declares DocuSign
        // now carries it, because `WEB_REQUIREMENTS` demands it once
        // `DOCUSIGN_BASE_URL` declares the integration.
        let mut jwt = full_with_jwks();
        jwt.retain(|(key, _)| *key != "DOCUSIGN_OAUTH_BASE");
        jwt.push(("DOCUSIGN_INTEGRATION_KEY", "integration-key"));
        let err = production_invariants(lookup(&jwt)).unwrap_err();
        assert!(err
            .violations
            .iter()
            .any(|v| v.contains("DOCUSIGN_OAUTH_BASE must be set explicitly")));
    }
}
