//! Shared test scaffolding: a canonical [`AppState`] builder.
//!
//! Every web integration test (and the `features` BDD runner) needs an
//! `AppState` to build a router, and they used to each inline the full
//! ~30-field literal — so adding one field meant editing ~25 files. This
//! builder is the single source of those defaults: a test takes
//! [`app_state`] and overrides only the fields it cares about via struct
//! update syntax:
//!
//! ```ignore
//! let state = AppState {
//!     auth: AuthConfig::new(true, Some(claims)),
//!     oauth: Some(cfg),
//!     ..portal::test_support::app_state(surreal.clone()).await
//! };
//! ```
//!
//! It is always compiled (not feature-gated) so both the in-crate
//! integration tests and the downstream `features` crate can reach it
//! without a self dev-dependency; every type it touches
//! (`StubSignatureProvider`, `CapturingEmail`, the passthrough policy /
//! google-oauth configs, the empty indices) already ships in the binary
//! as a production fallback, so it adds no real surface.

use std::sync::Arc;

use crate::{AppState, AuthConfig, CanonicalHost, SessionStore, WorkshopIndex};

/// The session signing key the test builder uses. Stable so encoded
/// cookies round-trip across a test's requests.
pub const TEST_SESSION_KEY: &str = "test-session-key-not-for-production";

/// An `Authorization` header value that satisfies
/// [`crate::auth::require_session`] as `role`.
///
/// It is the real CLI credential, not a stand-in: the `navigator` CLI presents
/// the same HMAC-signed [`crate::SessionData`] blob the browser holds in its
/// cookie, so a test that drives `/lawyer` or `/app` with this header takes
/// the identical path through the session boundary, embedded Rego policy, and the handlers that
/// production does. Signed with [`TEST_SESSION_KEY`], which
/// [`app_state`] hands to the router's [`crate::SessionStore`].
#[doc(hidden)]
#[must_use]
pub fn bearer_header(role: store::persons::Role) -> String {
    let sessions = SessionStore::new(TEST_SESSION_KEY);
    format!(
        "Bearer {}",
        sessions.encode(&crate::SessionData::fresh("test-bearer-subject", role))
    )
}

/// [`bearer_header`] for the firm lens — the common case for a test that
/// drives the `/lawyer` surface over the CLI credential.
#[doc(hidden)]
#[must_use]
pub fn lawyer_bearer_header() -> String {
    bearer_header(store::persons::Role::Lawyer)
}

/// Build an [`AppState`] wired with dev/test defaults: in-memory runtimes,
/// passthrough policy + google-oauth, stub signature + billing providers,
/// a capturing email backend, filesystem storage in a temp dir, and empty
/// content indices. The caller supplies the `db` (one schema per test via
/// `store::test_support::pg`) and overrides any field through struct
/// update syntax — see the module docs.
/// A private in-memory SurrealDB with the schema applied.
///
/// Deliberately *not* `store::surreal::test_support::mem()`, which is
/// the same thing behind `store`'s `test-support` feature. This module
/// is always compiled (see the module docs), so reaching for that
/// feature would put it — and `testcontainers` with it — in the build
/// graph of every `web` binary, which is exactly what `store`'s
/// `[features]` comment says the gate exists to prevent.
///
/// `connect`, `SurrealConfig`, and `schema::apply` are ungated public
/// API and `kv-mem` is a non-optional `store` dependency, so this costs
/// nothing extra to build.
pub async fn embedded_surreal() -> store::surreal::SurrealDb {
    let db = store::surreal::connect(&store::surreal::SurrealConfig {
        // Named, never defaulted — `store::surreal` refuses to guess an
        // endpoint, and a test is just another caller that says which
        // engine it means.
        endpoint: "mem://".into(),
        namespace: "navigator".into(),
        database: "navigator".into(),
        auth: store::surreal::SurrealAuth::Anonymous,
    })
    .await
    .expect("start an embedded SurrealDB engine");
    store::schema::apply(&db)
        .await
        .expect("apply the Surreal schema to a fresh embedded engine");
    db
}

/// The caller supplies **both** stores.
///
/// The Surreal handle is an argument rather than something this builder
/// mints, and that is load-bearing: `persons` lives in that engine
/// (#1093; ENG-19), so a test that seeded a person into some *other*
/// embedded engine would drive a router that cannot see them — a silent
/// empty read rather than a failure. Taking the handle makes the store
/// the test seeds and the store the router reads the same one by
/// construction. `store::test_support::engines()` hands out the pair.
#[doc(hidden)]
pub async fn app_state(surreal: store::surreal::SurrealDb) -> AppState {
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-web-test-storage"))
            .await
            .unwrap(),
    );
    AppState {
        brand_bundle: None,
        surreal,
        workshops: WorkshopIndex::empty(),
        docs: crate::DocsIndex::empty(),
        blog: crate::BlogIndex::empty(),
        auth: AuthConfig::new(true, None),
        google_oauth: crate::google_oauth::GoogleOauthConfig::passthrough(),
        rate_limit: crate::rate_limit::RateLimit::disabled(),
        canonical_host: CanonicalHost::new(None),
        portal_only: crate::PortalOnly::default(),
        sessions: SessionStore::new(TEST_SESSION_KEY),
        oauth: None,
        oauth_microsoft: None,
        // One shared root for both lanes, mirroring dev/KIND. A test
        // that overrides `storage` and drives a form fill must override
        // `assets_storage` (and stage blanks — see [`stage_blank_forms`])
        // on the same root.
        assets_storage: storage.clone(),
        // The Project-application bundle lane shares the one fs root here,
        // mirroring dev/KIND. A `project_portal` test seeds a bundle under
        // `<code>/portal/` on this same handle.
        applications_storage: storage.clone(),
        forms_registry: Arc::new(forms::registry().expect("forms registry loads")),
        storage,
        policy: crate::policy::PolicyClient::passthrough(),
        workflow_runtime: Arc::new(workflows::InMemoryRuntime::new()),
        questionnaire_runtime: Arc::new(workflows::InMemoryRuntime::new()),
        signature_provider: Arc::new(crate::signature::StubSignatureProvider::new()),
        billing_provider: Arc::new(crate::billing::StubBillingProvider::new()),
        contract_reviewer: Arc::new(crate::contract_review::StubContractReviewer),
        esignature_webhook_secret: None,
        esignature_hmac_key: None,
        email: Arc::new(crate::email::CapturingEmail::new()),
        attachment_scanner: Arc::new(crate::attachment_scanner::FakeAttachmentScanner::clean()),
        inbound_email_secret: None,
        email_events_secret: None,
        sendgrid_events_public_key: None,
        bootstrap_owner_email: None,
        self_signup_enabled: false,
        identity_password: None,
        identity_admin: None,
        a2a_router: None,
    }
}

/// Like [`app_state`], but with an explicit Project-applications bucket so a
/// `project_portal` test can publish a bundle into the exact handle the router
/// streams from. Each test passes its own [`cloud::FsStorage`] rooted at a
/// unique directory, so one test's bundle can never leak into another's.
#[doc(hidden)]
pub async fn app_state_with_applications(
    surreal: store::surreal::SurrealDb,
    applications: Arc<dyn cloud::StorageService>,
) -> AppState {
    let mut state = app_state(surreal).await;
    state.applications_storage = applications;
    state
}

/// A private applications bucket backed by a unique temporary directory, so a
/// `project_portal` test publishes into a store no other test shares.
#[doc(hidden)]
pub async fn empty_applications_bucket() -> Arc<dyn cloud::StorageService> {
    let root =
        std::env::temp_dir().join(format!("navigator-portal-bundle-{}", uuid::Uuid::new_v4()));
    Arc::new(
        cloud::FsStorage::new(root)
            .await
            .expect("a temp-dir applications bucket"),
    )
}

/// Publish one bundle object at `<code>/portal/<path>` in `applications`,
/// mirroring what the publish workflow does — flat objects under the Project's
/// `portal/` prefix, which is exactly where `project_portal::serve` reads them.
#[doc(hidden)]
pub async fn publish_portal_object(
    applications: &Arc<dyn cloud::StorageService>,
    code: &str,
    path: &str,
    content_type: &str,
    bytes: &[u8],
) {
    applications
        .put(
            &format!("{code}/{}/{path}", cloud::workspace::PORTAL_MOUNT_SEGMENT),
            bytes,
            content_type,
        )
        .await
        .expect("publish a portal bundle object");
}

/// Stage a synthetic blank for every registry form in `storage` (at each
/// form's `object_path`) and return a registry whose `.sha256` pins match
/// the staged bytes.
///
/// The canonical blanks live only in the public assets bucket, pinned by
/// the repo's `.sha256` files — bytes an offline test cannot have. This
/// helper builds a genuinely fillable stand-in from the form's own
/// field-layer mirror — `.fields.toml` rules (a text widget per mapped
/// field; a checkbox where the rule is `checked_when`-shaped), or, for a
/// re-authored form, its `.fields` manifest (a radio group with the
/// notation's `choices:` as on-states for `custom_single_choice__*`
/// names; a text widget otherwise) — so the full pull → verify → fill →
/// flatten pipeline runs against the storage seam. The synthetic blanks
/// are deterministic, so they are built once per process (`OnceLock`);
/// the pin strings leak (`Box::leak`) exactly once to satisfy
/// `FormMeta`'s `&'static` fields.
///
/// # Panics
///
/// Panics on any staging failure — test scaffolding fails loudly.
pub async fn stage_blank_forms(storage: &dyn cloud::StorageService) -> Arc<Vec<forms::FormMeta>> {
    static STAGED: std::sync::OnceLock<Vec<(forms::FormMeta, Vec<u8>)>> =
        std::sync::OnceLock::new();
    let staged = STAGED.get_or_init(|| {
        forms::registry()
            .expect("forms registry loads")
            .into_iter()
            .map(|form| {
                let specs = synthetic_field_specs(&form);
                let bytes = pdf::blank_acroform_with(&specs);
                let pin: &'static str = Box::leak(forms::sha256_hex(&bytes).into_boxed_str());
                (
                    forms::FormMeta {
                        sha256_pin: pin,
                        ..form
                    },
                    bytes,
                )
            })
            .collect()
    });
    for (form, bytes) in staged {
        storage
            .put(form.object_path, bytes, "application/pdf")
            .await
            .expect("stage synthetic blank");
    }
    Arc::new(staged.iter().map(|(form, _)| form.clone()).collect())
}

/// The widget shapes for one form's synthetic blank, from its
/// re-authored `.fields` manifest.
fn synthetic_field_specs(form: &forms::FormMeta) -> Vec<pdf::FieldSpec> {
    let manifest = forms::manifest(form.code).expect("form has a manifest");
    let choices = notation_choices(form.object_path);
    manifest
        .iter()
        .map(|name| {
            let role = name.strip_prefix("custom_single_choice__");
            match role.and_then(|r| choices.get(r)) {
                Some(options) => pdf::FieldSpec::Radio {
                    name: (*name).to_string(),
                    options: options.clone(),
                },
                None => pdf::FieldSpec::Text {
                    name: (*name).to_string(),
                },
            }
        })
        .collect()
}

/// The sibling notation's `custom_questions:` options — the on-state
/// vocabulary a re-authored radio group carries, keyed by the custom
/// question's `__<key>` and read from its nested `choices`.
fn notation_choices(object_path: &str) -> std::collections::BTreeMap<String, Vec<String>> {
    #[derive(serde::Deserialize)]
    struct Fm {
        #[serde(default)]
        custom_questions: std::collections::BTreeMap<String, CustomQuestion>,
    }
    #[derive(serde::Deserialize)]
    struct CustomQuestion {
        #[serde(default)]
        choices: std::collections::BTreeMap<String, String>,
    }
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("templates")
        .join(object_path.replace(".pdf", ".md"));
    let contents = std::fs::read_to_string(&path).expect("sibling notation");
    let fm = contents
        .strip_prefix("---\n")
        .and_then(|rest| rest.find("\n---").map(|end| &rest[..end]))
        .expect("notation frontmatter");
    let fm: Fm = serde_yaml::from_str(fm).expect("notation frontmatter parses");
    fm.custom_questions
        .into_iter()
        .filter(|(_, q)| !q.choices.is_empty())
        .map(|(role, q)| (role, q.choices.into_keys().collect()))
        .collect()
}

// --- OIDC id_token test crypto -------------------------------------------
//
// The OIDC redirect callback now does full RS256 signature + `iss`/`aud`/
// `exp` + `nonce` verification, so integration tests can no longer mint an
// unsigned token. These helpers sign a real id_token with a throwaway test
// keypair and build the matching [`IdTokenVerifier`], so every callback
// test exercises the production verification path. The keypair is a
// generated 2048-bit RSA pair, never used against a real IdP.

use jsonwebtoken::{encode, Algorithm, DecodingKey, EncodingKey, Header};

use crate::oauth::{IdTokenVerifier, IssuerPolicy, OAuthConfig, ProviderId};

/// Issuer the test verifier pins and the test tokens claim.
pub const TEST_OIDC_ISSUER: &str = "https://idp.test";
/// `kid` on the test signing key and in signed tokens' headers.
pub const TEST_OIDC_KID: &str = "test-oidc-key-1";

const TEST_OIDC_PRIV_PEM: &str = r"-----BEGIN PRIVATE KEY-----
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

const TEST_OIDC_PUB_PEM: &str = r"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAmoadu/HmIwEtxoKi8C98
ir7ScJ9ukHoQ2LfOBqgOfBZBAEdtNlWW1FZEgimvBxt9C7MuQTe/wqD9aO8Jeq6/
gFeLH9fANTuZO7uhRqwTEqM4MnZiGGwICiE7r3Ng5vRFO9O35jkBHSArh4MiAc/g
JuT/IOwVDyFYnO5e4IUTYn6O61h+LvGlpWSMQEcw2ppk0WafOOKZwxtAzRrBhPAy
ImE6pRK7DsbjViK+Hz0RBjycxksgo4lEQrQ3wW3K9RYiE3/JAf1FyVGmMLbU6ApR
s1to5OFBNEC3jIWFw0YxlanEzIQ88mFy+a4klkPLyh12JxQRS8yTBk3sSUWzrZTU
LwIDAQAB
-----END PUBLIC KEY-----
";

/// An [`IdTokenVerifier`] over the test keypair, pinned to
/// [`TEST_OIDC_ISSUER`] and the given `audience` (the OAuth `client_id`).
#[must_use]
pub fn oidc_verifier(audience: &str) -> IdTokenVerifier {
    let key = DecodingKey::from_rsa_pem(TEST_OIDC_PUB_PEM.as_bytes())
        .expect("test OIDC public key parses");
    IdTokenVerifier::from_keys(
        vec![(TEST_OIDC_KID.to_string(), key)],
        TEST_OIDC_ISSUER,
        audience,
        IssuerPolicy::Exact,
    )
}

/// The templated issuer Microsoft's multi-tenant authorities publish, with the
/// test host substituted for `login.microsoftonline.com` so the whole Entra
/// path can be exercised against a local signing key.
pub const TEST_ENTRA_ISSUER_TEMPLATE: &str = "https://entra.test/{tenantid}/v2.0";

/// An [`IdTokenVerifier`] configured exactly as multi-tenant Entra requires:
/// no fixed issuer, a per-token issuer interpolated from `tid`, and a tenant
/// allowlist. `allowed_tenants` is what an operator would put in
/// `OAUTH_MICROSOFT_ALLOWED_TENANTS`.
#[must_use]
pub fn entra_verifier(audience: &str, allowed_tenants: &[&str]) -> IdTokenVerifier {
    let key = DecodingKey::from_rsa_pem(TEST_OIDC_PUB_PEM.as_bytes())
        .expect("test OIDC public key parses");
    IdTokenVerifier::from_keys(
        vec![(TEST_OIDC_KID.to_string(), key)],
        TEST_ENTRA_ISSUER_TEMPLATE,
        audience,
        IssuerPolicy::EntraTenants {
            template: TEST_ENTRA_ISSUER_TEMPLATE.to_string(),
            allowed_tenants: allowed_tenants
                .iter()
                .map(|tenant| (*tenant).to_ascii_lowercase())
                .collect(),
        },
    )
}

/// An [`OAuthConfig`] standing in for the Microsoft door: labelled
/// [`ProviderId::Microsoft`] so the callback picks the Entra claim rules, and
/// carrying an [`entra_verifier`] so the templated-issuer path is the one
/// under test.
#[must_use]
pub fn microsoft_oauth_config(
    cfg: OAuthConfig,
    client_id: &str,
    allowed_tenants: &[&str],
) -> OAuthConfig {
    cfg.with_provider(ProviderId::Microsoft)
        .with_id_token_verifier(entra_verifier(client_id, allowed_tenants))
}

/// Wrap an [`OAuthConfig`] with a test id_token verifier pinned to
/// `client_id` so the callback's verification path is exercised end to end.
#[must_use]
pub fn oauth_config_with_verifier(cfg: OAuthConfig, client_id: &str) -> OAuthConfig {
    cfg.with_id_token_verifier(oidc_verifier(client_id))
}

#[derive(serde::Serialize)]
struct TestIdTokenClaims<'a> {
    sub: &'a str,
    email: &'a str,
    name: &'a str,
    nonce: &'a str,
    iss: &'a str,
    aud: &'a str,
    exp: i64,
}

/// An Entra-shaped id_token payload: `tid` and `preferred_username` present,
/// and `email` optional, because Entra populates `email` from the directory's
/// `mail` attribute and omits the claim when that attribute is empty.
#[derive(serde::Serialize)]
struct TestEntraIdTokenClaims<'a> {
    sub: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preferred_username: Option<&'a str>,
    name: &'a str,
    nonce: &'a str,
    iss: &'a str,
    tid: &'a str,
    aud: &'a str,
    exp: i64,
}

/// Sign an Entra-shaped id_token with the test key.
///
/// `tid` is both the tenant claim and the value interpolated into
/// [`TEST_ENTRA_ISSUER_TEMPLATE`] to build `iss`, which is what a real
/// multi-tenant token looks like. `iss_override` forces a mismatched issuer so
/// a test can prove the per-tenant issuer check actually bites.
#[must_use]
pub fn sign_entra_id_token(
    aud: &str,
    nonce: &str,
    sub: &str,
    tid: &str,
    preferred_username: Option<&str>,
    email: Option<&str>,
    iss_override: Option<&str>,
) -> String {
    let derived = TEST_ENTRA_ISSUER_TEMPLATE.replace("{tenantid}", tid);
    let claims = TestEntraIdTokenClaims {
        sub,
        email,
        preferred_username,
        name: "Entra User",
        nonce,
        iss: iss_override.unwrap_or(derived.as_str()),
        tid,
        aud,
        exp: crate::session::now_unix_secs() + 300,
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(TEST_OIDC_KID.to_string());
    let key = EncodingKey::from_rsa_pem(TEST_OIDC_PRIV_PEM.as_bytes())
        .expect("test OIDC private key parses");
    encode(&header, &claims, &key).expect("sign test entra id_token")
}

/// Sign a valid RS256 id_token with the test key. `aud` must equal the
/// `client_id` the verifier is pinned to and `nonce` must match the
/// login's pre-auth nonce, or [`IdTokenVerifier::verify`] rejects it.
#[must_use]
pub fn sign_id_token(aud: &str, nonce: &str, sub: &str, email: &str, name: &str) -> String {
    sign_id_token_with_kid(aud, nonce, sub, email, name, TEST_OIDC_KID)
}

/// [`sign_id_token`] with an explicit `kid`, rather than the fixed
/// [`TEST_OIDC_KID`] every verifier under test is pinned to. Exists so a
/// test can mint a token whose `kid` a verifier's cached JWKS will never
/// recognise — e.g. to exercise [`IdTokenVerifier`]'s refetch-on-unknown-
/// `kid` path without needing a signing key that actually matches what a
/// mock JWKS endpoint serves.
#[must_use]
pub fn sign_id_token_with_kid(
    aud: &str,
    nonce: &str,
    sub: &str,
    email: &str,
    name: &str,
    kid: &str,
) -> String {
    let claims = TestIdTokenClaims {
        sub,
        email,
        name,
        nonce,
        iss: TEST_OIDC_ISSUER,
        aud,
        exp: crate::session::now_unix_secs() + 300,
    };
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let key = EncodingKey::from_rsa_pem(TEST_OIDC_PRIV_PEM.as_bytes())
        .expect("test OIDC private key parses");
    encode(&header, &claims, &key).expect("sign test id_token")
}
