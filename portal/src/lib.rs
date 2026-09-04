#![allow(
    clippy::doc_markdown,
    clippy::result_large_err,
    clippy::unused_async_trait_impl
)]
// The OpenAPI document in `openapi.rs` is one large `serde_json::json!`
// literal; each documented path/schema nests it deeper, so the default 128
// recursion limit no longer expands it. Raise the ceiling for the whole crate.
#![recursion_limit = "256"]
//! Neon Law Navigator web server library.
//!
//! Exposes [`bootstrap`] so a brand binary and the integration tests that
//! exercise it compose the exact same router — there is no second definition
//! of the route table in tests.
//!
//! This crate owns the authenticated application and the anonymous
//! protocol ingress. The public face lives in the brand crate that serves it
//! (`neon`), which is what keeps a page's copy next to the binary that
//! publishes it.

#[cfg(test)]
pub(crate) mod test_tracing {
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::Interest;
    use tracing::{Event, Metadata, Subscriber};

    /// A globally-installed subscriber that claims interest in every callsite
    /// but records nothing itself.
    ///
    /// `tracing` computes and caches each callsite's interest from the
    /// *globally registered* dispatchers only — a thread-local `set_default`
    /// (how the capture tests install their subscriber) is invisible to that
    /// cache. With no global dispatcher, a callsite first seen — or rebuilt —
    /// while the global default is `NoSubscriber` caches as `Interest::never()`,
    /// and the event a capturing test is asserting on is dropped before its
    /// per-event `enabled` check ever runs against the thread-local subscriber.
    /// Under `cargo test -j4` with coverage instrumentation that ordering
    /// happens intermittently, so the capture flakes. Returning
    /// `Interest::sometimes()` from a globally-registered dispatcher keeps every
    /// callsite deferring to the current dispatcher, so the thread-local capture
    /// is consulted per event; `enabled` returns `false` so this global itself
    /// records nothing on threads without a capturing default.
    struct AlwaysInterested;

    impl Subscriber for AlwaysInterested {
        fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
            Interest::sometimes()
        }
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            false
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    static INSTALL: std::sync::Once = std::sync::Once::new();

    /// Installs [`AlwaysInterested`] as the process-global default the first time
    /// any capture test runs, so callsite interest can never cache as `never`.
    /// Idempotent, and a no-op if some other global default is already set.
    pub(crate) fn ensure_callsite_interest() {
        INSTALL.call_once(|| {
            let _ = tracing::subscriber::set_global_default(AlwaysInterested);
        });
    }
}

pub mod a2a;
pub mod admin;
pub mod admin_csv;
pub mod agent_router;
pub mod api;
pub mod api_audit;
pub mod attachment_scanner;
pub mod audit_fields;
pub mod auth;
pub mod blog;
pub mod brand_fonts;
pub mod cron_schedules;
// The billing-provider seam moved to the `billing` crate so the
// worker-side `billing-workflows` can share it. Re-exported here so
// existing `portal::billing` / `portal::xero_auth` paths keep resolving.
pub use billing;
pub use billing::xero_auth;
pub mod admin_contract_reviews;
pub mod admin_playbooks;
pub mod canonical_host;
pub mod chatwoot;
pub mod clauses;
pub mod cli_auth;
pub mod config;
pub mod content_loader;
pub mod contract_review;
pub mod contract_review_walk;
pub mod conversation;
pub mod csrf;
pub mod dioxus_app;
pub mod docs;
pub mod documents;
pub mod docusign_auth;
pub mod email;
pub mod email_confirm;
pub mod email_events;
pub mod email_threads;
pub mod esign_view;
pub mod esignature_webhook;
pub mod expunge;
pub mod expunge_request_route;
pub mod expunge_route;
pub mod google_oauth;
pub mod gov_forms;
pub mod hosting;
pub mod idp_admin;
pub mod inbound_email;
pub mod intake;
pub mod marketing;
pub mod matter_documents;
pub mod mcp_principal;
pub mod oauth;
pub mod openapi;
pub mod password_reset;
pub mod people_commands;
pub mod people_list_answer;
pub mod policy;
pub mod portal_only;
pub mod project_documents;
pub mod project_export;
pub mod project_notation;
pub mod project_portal;
pub mod rate_limit;
pub mod retainer_walk;
pub mod review;
pub mod session;
pub mod session_renew;
pub mod signature;
pub mod signature_render;
pub mod template_api;
pub mod template_gallery;
mod template_paths;
pub mod tenant;
/// Shared test scaffolding (the canonical `AppState` builder). Always
/// compiled so both the integration tests and the `features` crate can
/// use it; see the module docs.
pub mod test_support;
pub mod visitor_analytics;
pub mod webhook_auth;
pub mod welcome;
pub mod workshops;

pub use oauth::{AuthState as OAuthState, OAuthConfig};
pub use session::{SessionData, SessionSource, SessionStore};

pub use canonical_host::CanonicalHost;
pub use portal_only::PortalOnly;

pub use auth::{AuthClaims, AuthConfig};
pub use blog::{BlogIndex, BlogPost};
pub use config::{AppConfig, ConfigError};
pub use docs::{Doc, DocsIndex};
pub use marketing::MarketingDoc;
// The A2A confirmation gate looks the *approver* up in `persons`, so a
// test that drives the gate must inject the same `Principal` the auth
// middleware produces in prod. Re-export it so the BDD suite can build
// one without depending on `mcp` directly.
pub use mcp::Principal;
pub use workshops::{WorkshopIndex, WorkshopMaterial};

use std::path::Path;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, FromRef, Path as AxumPath, State};
use axum::http::{header, HeaderName, HeaderValue, Method, Request, StatusCode, Uri};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

/// Header name used for the per-request correlation ID. Lowercase per
/// the HTTP/2 convention; `SetRequestIdLayer` adds the header if the
/// client did not send one, and `PropagateRequestIdLayer` mirrors it
/// onto the response.
const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// `Cache-Control` for `/public/` static assets. One hour is the
/// conservative default until we add content-hashed filenames; bump
/// to `immutable` once asset paths are fingerprinted.
const STATIC_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("public, max-age=3600");
const NOSNIFF: HeaderValue = HeaderValue::from_static("nosniff");

/// `Strict-Transport-Security` value — two years with
/// `includeSubDomains` and `preload`, making the site eligible for
/// the HSTS preload list. Safe because every public entry point
/// terminates TLS at the GCP HTTPS LB before reaching the pod (see
/// `cloud/README.md`).
const HSTS_VALUE: HeaderValue =
    HeaderValue::from_static("max-age=63072000; includeSubDomains; preload");

/// `Content-Security-Policy` header value. All JS/CSS is vendored under
/// the same-origin `/public` mount (no CDN), so `script-src 'self'` is
/// achievable and there are no inline `<script>` tags to allow.
/// `style-src` keeps `'unsafe-inline'` because the templates use
/// inline `style` attributes; everything else is locked to same-origin.
/// `object-src` and `frame-ancestors 'none'` kill plugin and
/// clickjacking vectors (the latter matching `X-Frame-Options: DENY`),
/// and `form-action 'self'` stops a reflected form from posting
/// credentials cross-origin. Applied with `if_not_present`, so the
/// Swagger UI route keeps its own looser CSP.
///
/// The one cross-origin asset origin carries responsive photos, licensed
/// webfonts, and slide video through `views::assets::asset_url`. When
/// `NAVIGATOR_ASSET_BASE_URL` is absolute, [`asset_csp_origin`] adds it to
/// `img-src`, `font-src`, and `media-src`. `media-src` is named explicitly
/// rather than left to fall back to `default-src 'self'`, which would play
/// a clip locally and block the same clip from the bucket in production.
/// Scripts and styles stay `'self'`; only
/// passive presentation assets leave the app origin. The Dioxus render route
/// (issue #641) replaces this policy with its own per-response one — a nonce
/// for its inline hydration scripts, `'wasm-unsafe-eval'` for the client
/// bundle, and, on a public page of a deployment that names a support-chat
/// inbox, that installation's origin on four directives
/// ([`crate::chatwoot`]). Those allowances live there rather than here
/// precisely so they never widen this policy: every route that is not a Dioxus
/// render — the JSON API, the redirects, the static mounts — keeps scripts
/// same-origin with no host source at all, and a deployment carrying no widget
/// keeps that on the rendered pages too.
fn csp_value() -> HeaderValue {
    let asset_extra = asset_csp_origin()
        .map(|origin| format!(" {origin}"))
        .unwrap_or_default();
    let csp = format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; \
         frame-ancestors 'none'; img-src 'self' data:{asset_extra}; \
         font-src 'self'{asset_extra}; media-src 'self'{asset_extra}; \
         style-src 'self' 'unsafe-inline'; script-src 'self'; form-action 'self'"
    );
    HeaderValue::from_str(&csp).expect("CSP is built from a fixed template and an ASCII URL origin")
}

/// The `scheme://host[:port]` origin of `NAVIGATOR_ASSET_BASE_URL`, for
/// inclusion in the CSP asset directives — or `None` when the base is the
/// same-origin `/public` default (or any relative path). A CSP host-source is
/// an origin, not a path, so the bucket sub-path is dropped.
fn asset_csp_origin() -> Option<String> {
    csp_asset_origin_from(&std::env::var("NAVIGATOR_ASSET_BASE_URL").ok()?)
}

/// Pure core of [`asset_csp_origin`], split out so tests exercise
/// every base form without stomping the process-wide env var (which
/// would race the parallel test runner).
pub(crate) fn csp_asset_origin_from(base: &str) -> Option<String> {
    let base = base.trim();
    let (scheme, rest) = base
        .strip_prefix("https://")
        .map(|r| ("https://", r))
        .or_else(|| base.strip_prefix("http://").map(|r| ("http://", r)))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}{authority}"))
}

/// Directory the `web` binary serves under `/public/` by default —
/// the crate-bundled `public/` folder. Set `NAVIGATOR_PUBLIC_DIR`
/// at runtime to override.
pub const DEFAULT_PUBLIC_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../server/public");

/// Root for the bundled workshop materials. Override with
/// `NAVIGATOR_WORKSHOPS_DIR`.
pub const DEFAULT_WORKSHOPS_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../server/content/workshops");

/// Root for the bundled blog posts served at `/blog`. Override with
/// `NAVIGATOR_BLOG_DIR`.
pub const DEFAULT_BLOG_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../server/content/blog");

/// Shared router state. `Clone`-cheap — every field is `Arc`-backed
/// or wraps one.
#[derive(Clone)]
pub struct AppState {
    /// Read-only deployment identity bundle, validated before the router is
    /// constructed. It is separate from object-storage data.
    pub brand_bundle: Option<views::brand_bundle::BrandBundle>,
    /// The SurrealDB handle — the store. A process that cannot reach this
    /// engine cannot serve anything.
    pub surreal: store::surreal::SurrealDb,
    pub workshops: WorkshopIndex,
    /// Workspace docs published at `/docs/{slug}`, baked from the
    /// `docs/` tree at compile time. See [`docs`].
    pub docs: DocsIndex,
    /// Firm blog posts served at `/blog`, loaded at boot from a
    /// directory of dated `.md` files. See [`blog`].
    pub blog: BlogIndex,
    pub auth: AuthConfig,
    /// Google OAuth access-token validator for `/mcp`. Pass-through
    /// when `GOOGLE_OAUTH_CLIENT_IDS` is unset (KIND / local dev).
    pub google_oauth: google_oauth::GoogleOauthConfig,
    /// Per-IP request limiter for the abuse-sensitive endpoints
    /// (`/auth/*`, `/mcp`, `/app/api/aida/rpc`). Disabled in tests/dev;
    /// `RateLimit::from_env` enables it in production.
    pub rate_limit: rate_limit::RateLimit,
    pub canonical_host: CanonicalHost,
    /// White-label "portal-only" mode. When enabled, the public
    /// marketing surface is not mounted and `/` redirects to
    /// `/app/projects`. Disabled by default. Sourced from
    /// the mounted brand manifest. See [`portal_only`].
    pub portal_only: PortalOnly,
    pub sessions: SessionStore,
    pub oauth: Option<OAuthConfig>,
    /// Microsoft Entra ID as a **second** browser sign-in provider, alongside
    /// (never instead of) [`Self::oauth`]. `None` — the default, and every
    /// deployment that does not set `OAUTH_MICROSOFT_CLIENT_ID` — leaves the
    /// login page exactly as it was.
    ///
    /// The `/auth/*` router only mounts when [`Self::oauth`] is set, so the
    /// primary slot stays the deployment's anchor: this is an additional door,
    /// not a replacement one. Google keeps issuing its own `sub`, so no
    /// existing `persons.oidc_subject` is invalidated by switching it on.
    pub oauth_microsoft: Option<OAuthConfig>,
    /// Object storage backend (filesystem in dev, Google Cloud
    /// Storage in production via the `cloud` crate).
    pub storage: std::sync::Arc<dyn cloud::StorageService>,
    /// Public-assets object storage (`cloud::assets_from_env`) — the
    /// lane blank government forms are pulled from at fill and download
    /// time, verified against their repo `.sha256` pins. A distinct
    /// bucket from `storage` in production; the same root in dev/KIND.
    pub assets_storage: std::sync::Arc<dyn cloud::StorageService>,
    /// Project-application object storage (`cloud::applications_from_env`) —
    /// the private, per-deployment `<project>-applications` bucket each
    /// Project's published client-portal bundle lives in, streamed through
    /// Axum at `/app/projects/{code}/portal`. A distinct bucket from
    /// `storage` in production; the same root in dev/KIND.
    pub applications_storage: std::sync::Arc<dyn cloud::StorageService>,
    /// Vendored-forms registry (`forms::registry()` in production; a
    /// test harness swaps in entries pinned to synthetic staged blanks).
    pub forms_registry: std::sync::Arc<Vec<forms::FormMeta>>,
    /// Embedded Rego policy decision client.
    pub policy: policy::PolicyClient,
    /// Durable runtime for both timelines (workflow + questionnaire).
    /// In-memory in dev/tests; the `RestateRuntime` adapter is wired
    /// in production. Callers pick the timeline by passing
    /// [`workflows::MachineKind`] explicitly.
    pub workflow_runtime: Arc<dyn workflows::StateMachineRuntime>,
    /// Same `Arc` as `workflow_runtime` (the two timelines share one
    /// runtime instance keyed by `(MachineKind, notation_id)`). Kept
    /// as a separate field for now so call sites that drive only the
    /// questionnaire don't pretend to own the workflow side.
    pub questionnaire_runtime: Arc<dyn workflows::StateMachineRuntime>,
    /// Pluggable signature provider. The stub is the default; a
    /// real provider (DocuSign, Dropbox Sign) drops in behind the
    /// same trait.
    pub signature_provider: Arc<dyn signature::SignatureProvider>,
    /// Inbound-contract deviation reviewer. Selected like
    /// [`bootstrap`]'s A2A router: [`contract_review::GeminiContractReviewer`]
    /// (Vertex) when `NAVIGATOR_GCP_PROJECT_ID` is set, else the
    /// deterministic [`contract_review::StubContractReviewer`] (KIND /
    /// tests). The `analysis__contract_deviations` step runs this web-side
    /// — the worker has no LLM access.
    pub contract_reviewer: Arc<dyn contract_review::ContractReviewer>,
    /// Pluggable billing provider. The stub is the default; the real
    /// `XeroBillingProvider` drops in behind the same trait when the
    /// `XERO_*` env is configured. No `web` handler raises an invoice
    /// through it — accounting originates in Xero, and the nightly
    /// `ReconcileInvoices` workflow only folds paid-status back.
    pub billing_provider: Arc<dyn billing::BillingProvider>,
    /// Coarse path secret the e-signature provider must include in its
    /// completion-webhook URL (`/webhook/esignature/{secret}`). Same
    /// `None`-accepts-any-token dev posture as `inbound_email_secret`;
    /// loaded from `DOCUSIGN_WEBHOOK_SECRET`. Defense-in-depth — the
    /// real gate is `esignature_hmac_key`. See [`esignature_webhook`].
    pub esignature_webhook_secret: Option<String>,
    /// Shared HMAC-SHA256 key the e-signature webhook verifies over the
    /// raw request body before advancing workflow state. `None` in
    /// dev/tests skips verification; required in production via
    /// `enforce_deployment_invariants`. Loaded from `DOCUSIGN_HMAC_KEY`.
    pub esignature_hmac_key: Option<String>,
    /// Outbound email backend. `CapturingEmail` in dev/tests so
    /// outbound mail never escapes the host; `SendGridEmail` (wrapped
    /// in `RetryingEmail`) in production. Selected by
    /// [`email::from_env`].
    pub email: Arc<dyn email::EmailService>,
    /// Fail-closed malware scanner for inbound attachment bytes. Production
    /// and staging use `clamd`; ordinary tests inject a deterministic fake.
    pub attachment_scanner: Arc<dyn attachment_scanner::AttachmentScanner>,
    /// Shared secret SendGrid Inbound Parse must include in the
    /// webhook URL path. `None` in dev/tests (the route accepts any
    /// path token); required in production via
    /// `enforce_deployment_invariants`. Loaded from `SENDGRID_INBOUND_SECRET`.
    pub inbound_email_secret: Option<String>,
    /// Shared secret SendGrid's Event Webhook must include in the
    /// delivery-event URL path (`/webhook/email-events/{secret}`). Same
    /// `None`-accepts-any-token dev posture as `inbound_email_secret`;
    /// loaded from `SENDGRID_EVENTS_SECRET`. See [`email_events`].
    pub email_events_secret: Option<String>,
    /// SendGrid's "Signed Event Webhook" verification key — a
    /// base64-encoded DER `SubjectPublicKeyInfo` for the ECDSA/P-256
    /// public key SendGrid issues. When set, the Event Webhook verifies
    /// each delivery-event POST's signature over `timestamp || body`
    /// (the real payload-level gate; the path secret is only coarse).
    /// `None` in dev/tests skips it; required in production via
    /// `enforce_deployment_invariants`. Loaded from `SENDGRID_EVENTS_PUBLIC_KEY`.
    pub sendgrid_events_public_key: Option<String>,
    /// Email that is always granted the `admin` role on sign-in and
    /// JIT-created when missing. `None` disables the carve-out, so
    /// every sign-in then strictly requires a pre-seeded `persons`
    /// row. Sourced from `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL` in production;
    /// tests set it explicitly so the suite can run in parallel
    /// without env-var stomping.
    pub bootstrap_owner_email: Option<String>,
    /// Global self-signup capability, **off by default**. Off: an unknown
    /// verified email gets 403 (operator-mediated onboarding). On: the first
    /// login for an unknown email JIT-creates a `client` with an empty
    /// portfolio. Sourced from `NAVIGATOR_SELF_SIGNUP_ENABLED`; injected into
    /// `oauth::AuthState` so the handler never reads env. See #738.
    pub self_signup_enabled: bool,
    /// Opt-in email/password front door, delegated to GCP Identity
    /// Platform. `None` (the default) keeps `/auth/login` a pure OIDC
    /// redirect. Sourced from `NAVIGATOR_IDENTITY_PLATFORM_API_KEY` in
    /// production; tests inject a mock-endpoint config directly so the
    /// password path can be exercised without touching process env.
    pub identity_password: Option<oauth::IdentityPasswordConfig>,
    /// Opt-in admin door to GCP Identity Platform, backing the
    /// password-reset and email-confirm flows (they write a new password
    /// or flip `emailVerified` for a signed-out user). `None` unless
    /// `NAVIGATOR_GCP_PROJECT_ID` is set; tests inject a mock-endpoint
    /// config directly. See [`idp_admin::IdentityAdminConfig`].
    pub identity_admin: Option<idp_admin::IdentityAdminConfig>,
    /// Optional override for the A2A natural-language router. `None` in
    /// production and KIND — [`bootstrap`] then selects
    /// [`agent_router::GeminiRouter`] (when `NAVIGATOR_GCP_PROJECT_ID`
    /// is set) or [`agent_router::NullRouter`]. Tests inject a scripted
    /// [`agent_router::AgentRouter`] here to drive the agentic loop
    /// deterministically — exercising the loop, the real tools, and the
    /// real email side-effects — without a live LLM.
    pub a2a_router: Option<Arc<dyn agent_router::AgentRouter>>,
}

impl FromRef<AppState> for store::surreal::SurrealDb {
    fn from_ref(s: &AppState) -> Self {
        s.surreal.clone()
    }
}

impl FromRef<AppState> for WorkshopIndex {
    fn from_ref(s: &AppState) -> Self {
        s.workshops.clone()
    }
}

impl FromRef<AppState> for DocsIndex {
    fn from_ref(s: &AppState) -> Self {
        s.docs.clone()
    }
}

impl FromRef<AppState> for BlogIndex {
    fn from_ref(s: &AppState) -> Self {
        s.blog.clone()
    }
}

impl FromRef<AppState> for SessionStore {
    fn from_ref(s: &AppState) -> Self {
        s.sessions.clone()
    }
}

impl FromRef<AppState> for CanonicalHost {
    fn from_ref(s: &AppState) -> Self {
        s.canonical_host.clone()
    }
}

impl FromRef<AppState> for Arc<dyn email::EmailService> {
    fn from_ref(s: &AppState) -> Self {
        s.email.clone()
    }
}

impl FromRef<AppState> for Arc<dyn cloud::StorageService> {
    fn from_ref(s: &AppState) -> Self {
        s.storage.clone()
    }
}

/// Axum extractor that produces an [`views::AuthState`] for any
/// handler that wants to render the auth-aware header. The session
/// cookie (if present and unexpired) preserves the caller's tier;
/// everything else — missing cookie, bad signature, expired payload —
/// yields `Anonymous`. Inserting this extractor on a public-page
/// handler is what lights up the Portal / Lawyer / Admin / Sign out
/// utility links in the layout.
pub struct MaybeAuth(pub views::AuthState);

impl<S> axum::extract::FromRequestParts<S> for MaybeAuth
where
    S: Send + Sync,
    SessionStore: FromRef<S>,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        let sessions = SessionStore::from_ref(state);
        let session =
            if let Ok(cookies) = tower_cookies::Cookies::from_request_parts(parts, state).await {
                cookies
                    .get(session::SESSION_COOKIE_NAME)
                    .and_then(|c| sessions.decode(c.value()))
                    .filter(|s| !s.is_expired())
            } else {
                None
            };
        let auth = session.map_or(views::AuthState::Anonymous, |s| {
            auth_state_for_session_data(&s)
        });
        Ok(MaybeAuth(auth))
    }
}

pub(crate) fn auth_state_for_session_data(session: &session::SessionData) -> views::AuthState {
    if let Some(i) = &session.impersonation {
        return views::AuthState::Impersonating {
            target_name: i.target_name.clone(),
            target_email: i.target_email.clone(),
            csrf_token: session.csrf_token.clone(),
        };
    }
    match session.role {
        store::persons::Role::Owner => views::AuthState::Owner,
        store::persons::Role::Admin => views::AuthState::Admin,
        store::persons::Role::Lawyer => views::AuthState::Lawyer,
        store::persons::Role::Clerk => views::AuthState::Clerk,
        store::persons::Role::Client => views::AuthState::Authenticated,
    }
}

#[cfg(test)]
mod auth_state_tests {
    use super::auth_state_for_session_data;
    use crate::session::SessionData;
    use store::persons::Role;

    #[test]
    fn session_roles_map_to_their_nav_capabilities() {
        let cases = [
            (Role::Owner, views::AuthState::Owner, true, true),
            (Role::Admin, views::AuthState::Admin, true, true),
            (Role::Lawyer, views::AuthState::Lawyer, true, false),
            (Role::Clerk, views::AuthState::Clerk, false, false),
            (Role::Client, views::AuthState::Authenticated, false, false),
        ];

        for (role, expected, lawyer_tier, admin) in cases {
            let session = SessionData::fresh("subject", role);
            let auth = auth_state_for_session_data(&session);
            assert_eq!(auth, expected, "{role:?} should preserve its nav tier");
            assert_eq!(auth.is_lawyer_tier(), lawyer_tier, "{role:?} lawyer-tier");
            assert_eq!(auth.is_admin(), admin, "{role:?} admin");
        }
    }
}

/// Compose `router` behind Navigator's single anonymous-access boundary.
///
/// [`auth::inject_bearer_session`] sits outermost so the `navigator` CLI's
/// signed credential resolves into a `SessionData` before
/// [`auth::require_session`] looks for one; a router that already carries its
/// own bearer layer simply finds the session resolved and passes through.
///
/// Both use `route_layer`, so the boundary runs only on a *matched* route: an
/// unknown path still reaches [`fallback_not_found`] instead of being handed a
/// login redirect that implies the route exists.
pub fn session_boundary<S>(
    router: Router<S>,
    sessions: &SessionStore,
    auth: &AuthConfig,
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .route_layer(axum::middleware::from_fn_with_state(
            (sessions.clone(), auth.clone()),
            crate::auth::require_session,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            sessions.clone(),
            crate::auth::inject_bearer_session,
        ))
}

/// Wrap one brand-supplied router in the application's session and policy
/// gate: policy innermost, then the session requirement, then bearer
/// resolution outermost.
///
/// A brand crate publishes pages; deciding who may read one is the
/// application's. A gated page rides the brand's `public_dioxus` slot — which
/// [`bootstrap`] mounts outside [`session_boundary`] — so the layers have to
/// travel with the router itself. This function is how they do that without a
/// brand naming [`auth`] or [`policy`], which is what would let one brand's
/// idea of "signed in" drift from the other's.
///
/// Layers are applied with `route_layer`, so an unknown path still reaches
/// [`fallback_not_found`] rather than a login redirect implying the route
/// exists.
pub fn gated(state: &AppState, router: Router) -> Router {
    router
        .route_layer(axum::middleware::from_fn_with_state(
            (state.sessions.clone(), state.policy.clone()),
            crate::policy::require_policy,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            (state.sessions.clone(), state.auth.clone()),
            crate::auth::require_session,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.sessions.clone(),
            crate::auth::inject_bearer_session,
        ))
}

/// Composition 1 — the complete anonymous allowlist the shared application
/// owns.
///
/// Nothing here renders Navigator content to a human. `/health` and `/readyz`
/// are the Kubernetes probes; `/version` is the deploy-identity probe. The
/// webhook receivers
/// authenticate their sender by signature or path secret rather than by
/// session, the DocuSign consent callback is the provider's return leg of an
/// admin-initiated consent grant, and `/assets/*` exposes only the deployment's
/// dedicated marketing-assets bucket through the GKE workload identity. The
/// documents, exports, and logs buckets have no anonymous route.
///
/// A route belongs here only if an anonymous caller *must* reach it. Adding
/// one is a deliberate widening of the boundary and fails
/// `portal/tests/router_contract.rs` until the contract records it.
fn public_ingress_routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/readyz", get(readyz))
        .route("/version", get(version))
        .route("/assets/{*key}", get(public_asset))
        .route(
            "/webhook/sendgrid/inbound/{secret}",
            axum::routing::post(inbound_email::webhook).layer(DefaultBodyLimit::max(
                inbound_email::MAX_INBOUND_MESSAGE_BYTES,
            )),
        )
        .route(
            "/webhook/email-events/{secret}",
            axum::routing::post(email_events::webhook),
        )
        .route(
            "/webhook/esignature/{secret}",
            axum::routing::post(esignature_webhook::webhook),
        )
        .route("/docusign/consent-callback", get(docusign_consent_callback))
}

fn public_asset_key_is_safe(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('/')
        && !key.contains('\\')
        && !key.chars().any(char::is_control)
        && key
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

async fn public_asset(State(state): State<AppState>, AxumPath(key): AxumPath<String>) -> Response {
    if !public_asset_key_is_safe(&key) {
        return StatusCode::NOT_FOUND.into_response();
    }

    match state.assets_storage.get(&key).await {
        Ok(object) => {
            let content_type = HeaderValue::from_str(&object.content_type)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            let mut response = object.bytes.into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_TYPE, content_type);
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, STATIC_CACHE_CONTROL);
            response
                .headers_mut()
                .insert(header::X_CONTENT_TYPE_OPTIONS, NOSNIFF);
            response
        }
        Err(cloud::StorageError::NotFound(_)) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            tracing::error!(error = %error, asset_key = %key, "public asset read failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Composition 2 — shared human-facing Navigator routes that carry no state
/// of their own beyond [`AppState`].
///
/// These describe or reference the application: the workspace documentation and
/// the notation gallery with its raw markdown. They
/// are Navigator tools, not a brand's public pages, so they compose behind
/// [`session_boundary`]. A host that wants a public excerpt republishes it as
/// its own page. The rest of composition 2 — `/app`,
/// the JSON API, the API documentation, and the Dioxus pages — has
/// its own router state and joins the boundary in [`bootstrap`].
fn shared_human_routes() -> Router<AppState> {
    Router::new().route("/app/api/templates/{*path}", get(api_template_raw))
}

/// The reusable host bootstrap. `portal` owns the authenticated application and
/// the anonymous protocol/operational ingress; the host supplies its own public
/// pages as a `Router<AppState>`. This function is the single expression of
/// #730's "hosts publish; portal authenticates" invariant — every host binary
/// composes through it, so the anonymous-access boundary cannot drift between
/// hosts.
///
/// `host_public` mounts *outside* [`session_boundary`]: those routes are the
/// brand's public surface. The shared human tools, the JSON API, and every
/// application router still compose behind the boundary below.
///
/// # Errors
///
/// Returns [`MountError`] when a declared brand route is equal to or beneath a
/// Navigator-owned prefix.
#[allow(clippy::too_many_lines)]
pub fn bootstrap(
    state: AppState,
    public_dir: &Path,
    host_public: Router<AppState>,
    host_paths: &[&str],
    host_dioxus: Vec<Router>,
) -> Result<Router, MountError> {
    validate_host_paths(host_paths)?;
    let branding = state
        .brand_bundle
        .as_ref()
        .map_or(&views::brand::DEFAULT_BRANDING, |bundle| {
            views::brand::Branding::from_manifest(&bundle.manifest)
        });
    let brand_bundle = state.brand_bundle.clone();
    let static_files = tower::ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            STATIC_CACHE_CONTROL,
        ))
        .service(ServeDir::new(public_dir));
    // JSON API uses only `Db` as state; merge it under the same root.
    // Every gated `/app/api/*` data route runs behind `require_policy` so embedded Rego policy
    // enforces the OIDC requirement uniformly with `/app/*`.
    //
    // `require_csrf` sits innermost (closest to the handler, so it runs
    // AFTER `inject_bearer_session` has resolved any bearer credential):
    // a cookie-authenticated JSON write must carry a valid CSRF token,
    // while a bearer write stays exempt — the same credential-keyed rule
    // the `/app` and `/app/lawyer` form routers apply. Without it a
    // cookie-authenticated `hx-post` of JSON to a mutating `/app/api/*` route
    // would bypass CSRF entirely.
    let api = session_boundary(
        api::routes()
            .with_state(api::ApiState {
                surreal: state.surreal.clone(),
                email: state.email.clone(),
                bootstrap_owner_email: state.bootstrap_owner_email.clone(),
                bootstrap_company: admin::bootstrap_company_from_env(),
                questionnaire_runtime: state.questionnaire_runtime.clone(),
                storage: state.storage.clone(),
                workflow_runtime: state.workflow_runtime.clone(),
                assets_storage: state.assets_storage.clone(),
                forms_registry: state.forms_registry.clone(),
                signature_provider: state.signature_provider.clone(),
                contract_reviewer: state.contract_reviewer.clone(),
            })
            .layer(axum::middleware::from_fn_with_state(
                (state.sessions.clone(), crate::csrf::CsrfMode::Strict),
                crate::csrf::require_csrf,
            ))
            .route_layer(axum::middleware::from_fn_with_state(
                (state.sessions.clone(), state.policy.clone()),
                crate::policy::require_policy,
            ))
            // Outside the policy gate, so a refused request is audited too —
            // see `api_audit` for why the denied attempt is the one worth
            // recording. Inside the session boundary, so the session is
            // already resolved and the event can name a person.
            .route_layer(axum::middleware::from_fn(
                crate::api_audit::audit_api_request,
            )),
        &state.sessions,
        &state.auth,
    );
    // API documentation — the Swagger UI shell (at `/app/api` and its
    // shorter public-footer alias `/api`) and the OpenAPI document at
    // `/app/api/openapi.json`. Unlike `api::routes()` above, these mount with
    // no session boundary and no `require_policy` at all: the reference is
    // public, precisely so a reader never needs a session just to see what
    // the API looks like. What stays gated is the API `api::routes()`
    // describes, not the description of it — `/app/api/people` and every
    // other operation still runs the full session + policy stack above, so
    // "Try it out" against a real operation still needs a session, and
    // `crate::policy::swagger_ui_unauthenticated` is what that failure
    // renders (a friendly 401 rather than a redirect an XHR can't follow).
    let api_docs = api::doc_routes();
    let admin_state = admin::AdminState {
        surreal: state.surreal.clone(),
        workflow_runtime: state.workflow_runtime.clone(),
        signature_provider: state.signature_provider.clone(),
        retainer_intake_questionnaire: workflows::retainer_intake_questionnaire(),
        questionnaire_runtime: state.questionnaire_runtime.clone(),
        storage: state.storage.clone(),
        assets_storage: state.assets_storage.clone(),
        forms_registry: state.forms_registry.clone(),
        email: state.email.clone(),
        billing_provider: state.billing_provider.clone(),
        contract_reviewer: state.contract_reviewer.clone(),
        bootstrap_owner_email: state.bootstrap_owner_email.clone(),
        bootstrap_company: admin::bootstrap_company_from_env(),
        sessions: state.sessions.clone(),
        secure_cookies: secure_cookies(&state),
    };
    // #956 Phase 4: the client self-serve intake page renders through Dioxus at
    // /app/projects/{project_code}/intake/{notation_id}. Its pre-layer resolves the
    // current step (which needs `workflows`, so it cannot happen in `webapp`)
    // and owns the 404; the save `POST` on the same path stays on the handler
    // below, which now redirects back here with an `?error=` flash.
    let dioxus_client_intake = dioxus_app::client_intake_router(
        admin_state.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the per-notation clause editor renders through Dioxus at
    // /app/lawyer/notations/{id}/clauses. Its pre-layer keeps answering the
    // `?format=json` list the `navigator retainer clause list` CLI reads; every
    // mutation stays on the handlers below, which already redirect back here.
    let dioxus_clause_editor = dioxus_app::clause_editor_router(
        admin_state.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the workspace documentation renders through Dioxus at
    // /docs and /docs/{slug}. Its pre-layer resolves the doc from the
    // compiled-in DocsIndex and owns the canonicalizing redirects and the
    // unknown-slug 404.
    let dioxus_docs_index = dioxus_app::docs_router(
        dioxus_app::DOCS_PATH,
        Some(dioxus_app::DOCS_INDEX_SLUG),
        state.docs.clone(),
    );
    let dioxus_doc = dioxus_app::docs_router(dioxus_app::DOC_PATH, None, state.docs.clone());
    // The same documentation, a second door: inside the authenticated
    // application, wearing the app chrome, for the tiers that operate
    // Navigator. The public mount above is unchanged — this adds a reader, it
    // does not move one.
    let dioxus_app_docs_index = dioxus_app::app_docs_router(
        dioxus_app::APP_DOCS_PATH,
        Some(dioxus_app::DOCS_INDEX_SLUG),
        state.docs.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_app_doc = dioxus_app::app_docs_router(
        dioxus_app::APP_DOC_PATH,
        None,
        state.docs.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_app_team = dioxus_app::app_team_router(
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_app_brands = dioxus_app::app_brands_router(
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the template gallery renders through Dioxus at /templates
    // and /templates/{*path}. The detail pre-layer keeps owning the alias and
    // kebab redirects, the `/download` raw markdown, and the not-curated 404.
    let dioxus_template_gallery =
        dioxus_app::template_gallery_router(webapp::template_gallery::GalleryContent {
            title: "Template gallery".to_string(),
            cards: template_gallery::gallery()
                .iter()
                .map(dioxus_app::template_card)
                .collect(),
        });
    let dioxus_template_entry = dioxus_app::template_entry_router();
    // #956 Phase 4: the lawyer questionnaire walker step renders through Dioxus
    // at /app/lawyer/notations/{id}/step. Its pre-layer resolves the step from the
    // questionnaire runtime (which needs `workflows`, so it cannot happen in
    // `webapp`) and keeps answering the `?format=json` surface the
    // site intake walks.
    let dioxus_walker_step = dioxus_app::walker_step_router(
        admin_state.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the notation review-and-send screen renders through Dioxus
    // at /app/lawyer/notations/{id}/review. Its pre-layer assembles the document
    // (which needs `workflows` and storage, so it cannot happen in `webapp`)
    // and keeps answering the `?format=json` status surface the
    // `navigator notation status` CLI reads. Approve / send / request-changes
    // stay POSTs below and now redirect back here.
    let dioxus_intake_review = dioxus_app::intake_review_router(
        admin_state.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the lawyer re-ask screen renders through Dioxus at
    // /app/lawyer/notations/{id}/reask. Its pre-layer resolves the flagged set and
    // owns both the 404 and the bounce to review when nothing is parked; the
    // save POST stays on the handler below.
    let dioxus_reask = dioxus_app::reask_router(
        admin_state.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let admin = session_boundary(
        admin::routes(
            admin_state,
            state.auth.clone(),
            state.sessions.clone(),
            state.policy.clone(),
        ),
        &state.sessions,
        &state.auth,
    );
    // MCP rides on the same Pod / host as the public site, served at
    // `POST /mcp`. The layer stack (outermost first):
    //
    //   1. google_oauth::require_google_oauth — prod: validates the
    //      Google OAuth access token Gemini Enterprise sends as
    //      Bearer via tokeninfo, populates AuthClaims. Pass-through
    //      when GOOGLE_OAUTH_CLIENT_IDS is unset (KIND / local dev).
    //      Replaces the earlier IAP layer; IAP couldn't parse the
    //      opaque ya29.* tokens Gemini Enterprise actually sends.
    //   2. require_auth — KIND: validates Bearer JWT. In prod the
    //      Google-OAuth layer already populated AuthClaims so this
    //      short-circuits.
    //   3. require_policy — embedded Rego policy decision; same as /app.
    //
    // CSRF is intentionally NOT in the chain — JSON-RPC clients send
    // a Bearer token, not a session cookie.
    let mut mcp_state =
        mcp::McpState::new(state.surreal.clone(), state.questionnaire_runtime.clone());
    // Object storage is always available to the MCP tools — the
    // questionnaire walker reads template bodies from blob storage, so a
    // non-bundled template's spec can still be parsed.
    mcp_state.storage = Some(state.storage.clone());
    // The same mailer the JSON API routes hold — `LoggingEmail`-wrapped,
    // so `aida_send_welcome_email` writes the `sent_emails` audit row the
    // API door writes. Injecting it here is what lets the agent door go
    // through the shared command instead of the Restate trigger (ENG-317).
    mcp_state.email = Some(state.email.clone());
    let mcp = mcp::build_router(mcp_state.clone())
        .route_layer(axum::middleware::from_fn_with_state(
            state.google_oauth.clone(),
            crate::mcp_principal::inject_principal,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            (state.sessions.clone(), state.policy.clone()),
            crate::policy::require_policy,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.auth.clone(),
            crate::auth::require_auth,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            state.google_oauth.clone().with_db(state.surreal.clone()),
            crate::google_oauth::require_google_oauth,
        ))
        // Above the auth chain, so every layer below sees a resolved
        // session: the `navigator` CLI's own bearer, the same layer the
        // A2A rpc route already carries.
        //
        // Without it `/mcp` has no identity to scope a read by. The CLI's
        // credential is the HMAC-signed `SessionData` blob `cli_auth`
        // mints — not a JWT and not a Google access token — so
        // `require_auth` found nothing to validate and `inject_principal`
        // found no session to read an email from. Every read then
        // answered as the deployment rather than as the person signed in.
        //
        // Resolving it here is not a widening: `SessionStore::decode`
        // accepts only a blob this deployment signed, carrying the role
        // and expiry it minted at an authenticated OIDC login.
        .route_layer(axum::middleware::from_fn_with_state(
            state.sessions.clone(),
            crate::auth::inject_bearer_session,
        ))
        // Outermost: shed an over-budget IP with 429 before any auth or
        // tokeninfo work runs.
        .route_layer(axum::middleware::from_fn_with_state(
            state.rate_limit.clone(),
            crate::rate_limit::enforce,
        ));
    // A2A surface — the agent card at `/app/api/aida.json` and JSON-RPC
    // at `/app/api/aida/rpc`, the latter behind the same auth stack as
    // `/mcp`. Both are private, like every path under `/app/api`: the
    // card composes behind `session_boundary` below, so an anonymous
    // fetch gets the unauthenticated protocol document rather than the
    // transport and security schemes. Self-service A2A registration is
    // the deliberate cost — see the module docs on `a2a` for why the one
    // client this serves does not need it.
    //
    // The natural-language router maps free-form messages
    // (`message/send` without `metadata.skill`) onto a skill via
    // Vertex AI Gemini Flash. Pod's GSA needs `roles/aiplatform.user`
    // for Workload Identity to fetch a token. When
    // `NAVIGATOR_GCP_PROJECT_ID` is unset (KIND / local dev), falls
    // back to `NullRouter` which returns a helpful Task explaining
    // the `metadata.skill` backdoor.
    let router: Arc<dyn agent_router::AgentRouter> =
        if let Some(injected) = state.a2a_router.clone() {
            tracing::info!("a2a router: injected override (test harness)");
            injected
        } else if let Some(gemini) = agent_router::GeminiRouter::from_env() {
            tracing::info!("a2a router: Vertex AI Gemini Flash");
            Arc::new(gemini)
        } else {
            tracing::info!("a2a router: NullRouter (set NAVIGATOR_GCP_PROJECT_ID to enable)");
            Arc::new(agent_router::NullRouter)
        };
    let a2a_state = a2a::A2aState {
        mcp: mcp_state,
        canonical_host: state.canonical_host.clone(),
        router,
        pending: a2a::PendingConfirmations::new(),
    };
    let (a2a_card, a2a_rpc) = a2a::build_routers(
        a2a_state,
        state.google_oauth.clone(),
        state.auth.clone(),
        state.sessions.clone(),
        state.policy.clone(),
    );
    // The card needs a session like the rest of `/app/api`. It takes the
    // boundary alone rather than the RPC endpoint's full stack: reading
    // the card is not a tool call, so `require_google_oauth` and the
    // policy gate would be answering a question the card never asks.
    let a2a_card = session_boundary(a2a_card, &state.sessions, &state.auth);
    // Rate-limit the JSON-RPC endpoint. The card stays unlimited: it is a
    // static, per-request-cheap document behind the session boundary, and
    // a signed-in client re-reading it is not an abuse shape.
    let a2a_rpc = a2a_rpc.layer(axum::middleware::from_fn_with_state(
        state.rate_limit.clone(),
        crate::rate_limit::enforce,
    ));
    // Loopback-OAuth endpoints for the `navigator` CLI. `/auth/cli/start`
    // mints a CLI bearer from the browser session; `/auth/cli/whoami`
    // echoes the bearer caller's identity. Both live under the
    // private-mode-exempt `/auth/*` prefix.
    let cli_auth = cli_auth::routes(state.sessions.clone());
    let host_layer = axum::middleware::from_fn_with_state(
        state.canonical_host.clone(),
        canonical_host::resolve_brand_and_enforce_host,
    );
    // Browser-flow login routes only mount when OAUTH_* is configured;
    // otherwise the bearer-token path remains the only auth surface.
    let bootstrap_owner = state.bootstrap_owner_email.clone();
    let identity_password = state.identity_password.clone();
    let identity_admin = state.identity_admin.clone();
    let oauth_routes = state.oauth.as_ref().map(|oauth| {
        oauth::routes(oauth::AuthState {
            oauth: oauth.clone(),
            // Second provider, when configured. Additive: absent, the chooser
            // does not appear and `/auth/login` redirects straight to the
            // primary IdP exactly as before.
            oauth_microsoft: state.oauth_microsoft.clone(),
            sessions: state.sessions.clone(),
            surreal: state.surreal.clone(),
            email: state.email.clone(),
            workflow_runtime: state.workflow_runtime.clone(),
            bootstrap_owner_email: bootstrap_owner.clone(),
            // Global self-signup toggle (default off), injected from AppState
            // so the callback never reads env. See #738.
            self_signup_enabled: state.self_signup_enabled,
            // Opt-in email/password front door via GCP Identity Platform;
            // `None` (the default) keeps `/auth/login` a pure OIDC redirect.
            // Threaded from `AppState` (not read from env here) so tests can
            // inject a mock endpoint without mutating process env.
            identity_password: identity_password.clone(),
            // Opt-in admin door for the password-reset / email-confirm
            // flows; threaded from `AppState` for the same reason.
            identity_admin: identity_admin.clone(),
            // `Secure` auth cookies whenever the deployment's external
            // scheme is HTTPS (prod), off for the `http://localhost` KIND
            // loop so cookies still round-trip in dev. The redirect URI
            // carries the external scheme even behind a TLS-terminating LB.
            secure_cookies: oauth.redirect_uri().starts_with("https://"),
        })
        // Throttle the credential endpoints (login, password submit,
        // callback) per IP — the brute-force / credential-stuffing target.
        .layer(axum::middleware::from_fn_with_state(
            state.rate_limit.clone(),
            crate::rate_limit::enforce,
        ))
    });

    // Sliding session renewal state, captured before the move. `secure`
    // mirrors the auth router's cookie posture: `Secure` whenever the
    // external scheme is HTTPS (prod), off for the `http://localhost`
    // KIND loop. No OAuth configured ⇒ no browser sessions to renew.
    let session_renew = session_renew::RenewState {
        sessions: state.sessions.clone(),
        secure: state
            .oauth
            .as_ref()
            .is_some_and(|o| o.redirect_uri().starts_with("https://")),
    };

    // Composition 1 — the explicit anonymous allowlist. Composition 2 — the
    // shared human surface behind the one session boundary. Composition 3 —
    // `host_public`, the brand host's own public pages, mounted outside the
    // boundary. Every other Navigator route joins composition 2 below.
    let router = public_ingress_routes()
        .merge(session_boundary(
            // The raw-template read is an `/app/api` endpoint, so it takes the
            // same policy gate and audit layer as the operation router rather
            // than resting on the session boundary alone. Its rule is
            // deliberately the permissive one — any authenticated person, which
            // is what the notation gallery and `/templates` already admit — but
            // it is now *stated* rather than implied by the absence of a layer.
            shared_human_routes()
                .route_layer(axum::middleware::from_fn_with_state(
                    (state.sessions.clone(), state.policy.clone()),
                    crate::policy::require_policy,
                ))
                .route_layer(axum::middleware::from_fn(
                    crate::api_audit::audit_api_request,
                )),
            &state.sessions,
            &state.auth,
        ))
        .merge(host_public);

    let visitor_analytics_state =
        visitor_analytics::VisitorAnalyticsState::new(state.surreal.clone());
    // #641 Phase 3 (admin cluster): the lawyer entity-types directory renders
    // through Dioxus, replacing the read view. Same auth + embedded Rego policy gate.
    let dioxus_entity_types = dioxus_app::entity_types_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the expunge-request queue renders through
    // Dioxus — the first row-action page; its actions post to the existing
    // authorize/deny handlers via native forms carrying the session CSRF token.
    let dioxus_expunge_queue = dioxus_app::expunge_queue_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the admin governed-expunge confirmation + result renders
    // through Dioxus at `/app/lawyer/documents/{doc_id}/expunge`. Its form posts to
    // the unchanged `POST` handler, which redirects back here with `?record=`
    // or `?error=`; the loader is admin-only and 404s every other tier.
    let dioxus_expunge_document = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_DOCUMENT_EXPUNGE_PATH,
        webapp::expunge_document::AdminExpungeDocument,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the attorney contract-review screen renders through Dioxus
    // at `/app/lawyer/contract-reviews/{id}`. Its per-finding, summary, approve, and
    // reject forms post to the unchanged handlers on deeper paths.
    let dioxus_contract_review = dioxus_app::contract_review_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the "add entity" create form renders through
    // Dioxus — the first CRUD create form, on the shared `FormCard` + CSRF page
    // router. It posts to the unchanged `/app/admin/entities` create handler.
    let dioxus_entity_new = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_ENTITY_NEW_PATH,
        webapp::entity_new::LawyerEntityNew,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the "start a retainer walk" form — the lawyer on-ramp that
    // opens a matter — renders through Dioxus. It posts to the unchanged
    // `POST /app/lawyer/retainers/new`, which now redirects a refusal back here with
    // an `?error=` flash instead of re-rendering the form itself.
    let dioxus_retainer_start = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_RETAINER_NEW_PATH,
        webapp::retainer_start::LawyerRetainerStart,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the "edit entity" form renders through
    // Dioxus — the first CRUD edit form (a `FormCard` prefilled from the record
    // by its `{id}`). It posts to the unchanged `POST /app/admin/entities/{id}`.
    let dioxus_entity_edit = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_ENTITY_EDIT_PATH,
        webapp::entity_edit::LawyerEntityEdit,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the entities list renders through Dioxus — a
    // sortable table with per-row edit/delete actions. `POST /app/admin/entities`
    // (create) stays on the admin router; axum merges the same-path methods.
    let dioxus_entity_list = dioxus_app::entity_list_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the contract-negotiation playbooks cluster renders through
    // Dioxus — the sortable listing plus the create and edit-positions forms.
    // `POST /app/admin/playbooks` (create) and `POST /app/admin/playbooks/{id}` (update)
    // stay on `admin_playbooks`; axum merges the same-path methods. Both now
    // redirect a refusal back to the form with an `?error=` flash and the
    // rejected positions text instead of re-rendering inline.
    // #956 Phase 4: the lawyer workbench at `/app/lawyer` renders through Dioxus —
    // the project KPI overview, the calendar placeholder, and the
    // administrative directory. Person-scoped like the lawyer projects list, so
    // the counts and the matter list are the caller's workload.
    let dioxus_lawyer_dashboard = dioxus_app::lawyer_dashboard_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_harvard_outline = dioxus_app::harvard_outline_router(
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_notation_outline = dioxus_app::notation_outline_router(
        state.surreal.clone(),
        state.storage.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_playbooks = dioxus_app::sortable_admin_listing_router(
        dioxus_app::LAWYER_PLAYBOOKS_PATH,
        webapp::playbooks::LawyerPlaybookList,
        dioxus_app::LAWYER_PLAYBOOKS_SORT,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_playbook_new = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_PLAYBOOK_NEW_PATH,
        webapp::playbooks::LawyerPlaybookNew,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_playbook_edit = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_PLAYBOOK_EDIT_PATH,
        webapp::playbooks::LawyerPlaybookEdit,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the cron-schedule reference renders through Dioxus at
    // `/app/admin/schedules`. Each row's "Run now" is a native CSRF-carrying `POST`
    // to `cron_schedules`, which redirects back here with a `?notice=` flash —
    // already post/redirect/get, so nothing about the write path changes.
    let dioxus_schedules = dioxus_app::csrf_page_router(
        dioxus_app::LAWYER_SCHEDULES_PATH,
        webapp::schedules::LawyerSchedules,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4: the visitor-analytics dashboard renders through Dioxus at
    // `/app/admin/analytics`. Read-only, so it rides the plain listing router; the
    // admin-only gate lives in the loader, which commits a real 403.
    let dioxus_analytics = dioxus_app::admin_listing_router(
        dioxus_app::ADMIN_ANALYTICS_PATH,
        webapp::analytics::AdminAnalytics,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the admin "add person" form renders through
    // Dioxus — an admin-only create form on the shared FormCard + CSRF page
    // router. It posts to the native `POST /app/admin/people` create handler.
    let dioxus_admin_people_new = dioxus_app::csrf_page_router(
        dioxus_app::ADMIN_PEOPLE_NEW_PATH,
        webapp::admin_people_new::AdminPeopleNew,
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // ENG-81: the matter list renders through Dioxus at /app/projects — one
    // mount for every tier. The firm gets the sortable matter directory with
    // lifecycle badges; a client gets the KPI + project cards. Which one is
    // decided from the caller's role, not from the prefix they typed.
    // `POST /app/projects` (create) stays on the router.
    let dioxus_projects = dioxus_app::projects_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4 (app cluster): the blank government-forms index renders
    // through Dioxus at /app/forms. The download route stays on Axum.
    let dioxus_app_forms = dioxus_app::app_forms_router(
        state.surreal.clone(),
        &state.forms_registry,
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // ENG-81: the single matter renders through Dioxus at GET /app/projects/{code}
    // — one mount for every tier. The firm gets the workbench (header + DRIs +
    // missing-retainer notice + forge repo link + participation ledger +
    // document uploader + close-matter control); a client gets service,
    // invoice, notations with per-PDF download links, documents, and the
    // review surface. The object-store handle lets the client loader probe
    // which of each notation's PDFs exist, and the forge URL is injected for
    // the firm loader. The POST (edit-save) and every mutation route stay on
    // Axum; axum merges the same-path methods.
    // One Project's client portal at `/app/projects/{code}/portal`, a distinct
    // path shape from the matter show page above it.
    let project_portal = project_portal::router(
        state.surreal.clone(),
        state.applications_storage.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    let dioxus_project_detail = dioxus_app::project_detail_router(
        state.surreal.clone(),
        state.storage.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4 (projects cluster): the lawyer project forms render through
    // Dioxus — matter-open at `/app/projects/new` (with its two inline
    // "New entity" / "New client" creates, formerly HTMX-swapped Bootstrap
    // modals), the descriptive edit at `/app/projects/{project_code}/edit`, and the
    // admin-only participation add/edit forms. Every write stays on its existing
    // native `POST` handler; axum merges the same-path methods.
    let dioxus_lawyer_project_forms = dioxus_app::lawyer_project_forms_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #956 Phase 4 (projects cluster): one filed document's provenance page
    // renders through Dioxus under both lenses. The lens is pinned by the mount,
    // and the loader re-applies the cross-project and client-visibility guards;
    // the `…/download` routes stay on Axum.
    let dioxus_project_documents = dioxus_app::project_document_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (projects cluster): the matter conversation page renders through
    // Dioxus at GET /app/projects/{project_code}/conversation — one mount, the tier picks the
    // lens-scoped thread + a plain-textarea composer. The
    // `POST …/conversation/messages` stays on its existing Axum handler (it
    // already redirects a native form — PRG).
    let dioxus_conversation = dioxus_app::conversation_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3: the comment-only client document-review page renders through
    // Dioxus at /app/projects/{project_code}/review/{doc_id}; the comment data API
    // (`…/comments` GET/POST, driven by the document-review custom element)
    // stays on the Axum data API.
    let dioxus_review = dioxus_app::review_router(
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the admin console people list renders through
    // Dioxus at /app/admin/people — the sortable directory with a per-row
    // Edit/Delete/Impersonate action column. `POST /app/admin/people` (create) stays
    // on the router; axum merges the same-path methods.
    let dioxus_admin_people = dioxus_app::admin_people_router(
        state.bootstrap_owner_email.clone(),
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the admin console person show/edit page
    // renders through Dioxus — the prefilled edit form (name/email/role + the
    // read-only legal-name parts) plus the welcome/impersonate actions, mounted
    // at `/app/admin/people/{id}` and its `/edit` alias. Its native-form update,
    // welcome, and impersonate actions post to the admin router; axum merges
    // the same-path methods.
    let dioxus_admin_person_show = dioxus_app::admin_person_show_router(
        state.bootstrap_owner_email.clone(),
        state.surreal.clone(),
        state.sessions.clone(),
        state.policy.clone(),
        state.auth.clone(),
    );
    // #641 Phase 3 (admin cluster): the generic read-only admin listings render
    // through Dioxus via the shared `admin_listing` scaffold, replacing the
    // `render_listing` pages. Each carries the same auth + embedded Rego policy gate.
    // Each generic read-only admin listing (#641 Phase 3) mounts through the same
    // shared factory with the same auth + embedded Rego policy gate; only its path and component
    // differ. The macro keeps the state clones from drowning the list.
    macro_rules! listing_router {
        ($path:expr, $component:expr) => {
            dioxus_app::admin_listing_router(
                $path,
                $component,
                state.surreal.clone(),
                state.sessions.clone(),
                state.policy.clone(),
                state.auth.clone(),
            )
        };
    }
    // #956 Phase 4: the sortable read-only listings. They differ from the fixed
    // ones only by advertising a `?sort=`, so the allowed set travels to the
    // route's pre-handler and to the page's header anchors from one place.
    let dioxus_sortable_listings = [
        dioxus_app::sortable_admin_listing_router(
            dioxus_app::LAWYER_TEMPLATES_PATH,
            webapp::admin_listings::LawyerTemplates,
            &["code", "title", "respondent_type"],
            state.surreal.clone(),
            state.sessions.clone(),
            state.policy.clone(),
            state.auth.clone(),
        ),
        dioxus_app::sortable_admin_listing_router(
            dioxus_app::LAWYER_QUESTIONS_PATH,
            webapp::admin_listings::LawyerQuestions,
            &["code", "answer_type"],
            state.surreal.clone(),
            state.sessions.clone(),
            state.policy.clone(),
            state.auth.clone(),
        ),
        // ENG-221: the Owner/Admin matter directory at `/app/admin/projects` —
        // every matter's code, name, status, and accountable lawyer, reached
        // without a participation row on any of them. Sortable, so it rides the
        // same seam `/app/admin/people` and `/app/admin/analytics` do; the admin-tier
        // gate lives in the loader, which commits a real 403.
        dioxus_app::sortable_admin_listing_router(
            dioxus_app::ADMIN_MATTER_DIRECTORY_PATH,
            webapp::matter_directory::AdminMatterDirectory,
            dioxus_app::ADMIN_MATTER_DIRECTORY_SORT,
            state.surreal.clone(),
            state.sessions.clone(),
            state.policy.clone(),
            state.auth.clone(),
        ),
    ];
    let dioxus_admin_listings = [
        listing_router!(
            dioxus_app::LAWYER_JURISDICTIONS_PATH,
            webapp::admin_listings::LawyerJurisdictions
        ),
        listing_router!(
            dioxus_app::LAWYER_GIT_REPOSITORIES_PATH,
            webapp::admin_listings::LawyerGitRepositories
        ),
        listing_router!(
            dioxus_app::LAWYER_PERSON_ENTITY_ROLES_PATH,
            webapp::admin_listings::LawyerPersonEntityRoles
        ),
        listing_router!(
            dioxus_app::LAWYER_NOTATIONS_PATH,
            webapp::admin_listings::LawyerNotations
        ),
        listing_router!(
            dioxus_app::LAWYER_ANSWERS_PATH,
            webapp::admin_listings::LawyerAnswers
        ),
        listing_router!(
            dioxus_app::LAWYER_ADDRESSES_PATH,
            webapp::admin_listings::LawyerAddresses
        ),
        listing_router!(
            dioxus_app::LAWYER_ASSETS_PATH,
            webapp::admin_listings::LawyerAssets
        ),
        listing_router!(
            dioxus_app::LAWYER_PERSON_PROJECT_ROLES_PATH,
            webapp::admin_listings::LawyerPersonProjectRoles
        ),
        listing_router!(
            dioxus_app::LAWYER_DISCLOSURES_PATH,
            webapp::admin_listings::LawyerDisclosures
        ),
        listing_router!(
            dioxus_app::LAWYER_RELATIONSHIP_LOGS_PATH,
            webapp::admin_listings::LawyerRelationshipLogs
        ),
        listing_router!(
            dioxus_app::LAWYER_MAILROOMS_PATH,
            webapp::admin_listings::LawyerMailrooms
        ),
        listing_router!(
            dioxus_app::LAWYER_LETTERS_PATH,
            webapp::admin_listings::LawyerLetters
        ),
        listing_router!(
            dioxus_app::LAWYER_EMAIL_LOG_PATH,
            webapp::admin_listings::LawyerEmailLog
        ),
        // The letter-detail page is a single record (a `{id}` path param), not a
        // listing, but it mounts through the same gated-component factory.
        listing_router!(
            dioxus_app::LAWYER_LETTER_DETAIL_PATH,
            webapp::letter_detail::LawyerLetterDetail
        ),
        // The `/admin` console hub (#956 Phase 4) is a link table rather than a
        // listing, but it needs the same auth + policy + viewer-role stack, so
        // it mounts through the same factory. Its component re-checks for admin
        // and commits the `403` the `admin_gate` returned.
        listing_router!(
            dioxus_app::ADMIN_LANDING_PATH,
            webapp::admin_landing::AdminLandingEntry
        ),
    ];
    // Captured before `state` is consumed below; every remaining protected
    // composition is merged behind this same boundary.
    let boundary_sessions = state.sessions.clone();
    let boundary_auth = state.auth.clone();
    let mut router = mount_brand_assets(router, brand_bundle.as_ref())
        .nest_service("/public", static_files)
        .with_state(state)
        .merge(api)
        .merge(api_docs)
        .merge(admin)
        .merge(mcp)
        .merge(a2a_card)
        .merge(a2a_rpc)
        .merge(cli_auth);
    if let Some(oauth) = oauth_routes {
        router = router.merge(oauth);
    }
    // Phase 0 of the Dioxus adoption (issue #641): the `webapp` component
    // renders at `/dioxus-demo`, hydrated by a same-origin wasm bundle. The
    // sub-router owns only that page plus the static bundle paths — never the
    // global fallback — so every route above is unchanged. Absent a built
    // bundle (the default in unit tests and un-built deploys) this is a no-op.
    if let Some(dioxus_router) = dioxus_app::router() {
        router = router.merge(session_boundary(
            dioxus_router,
            &boundary_sessions,
            &boundary_auth,
        ));
    }
    // Each of these renders through Dioxus and is mounted unconditionally so it
    // server-side renders even without a client bundle, gated by its own auth +
    // embedded Rego policy layers.
    for dioxus_router in [
        // The lawyer entity-types directory (#641 Phase 3) renders through
        // Dioxus at `/app/admin/entity-types`, replacing the read view.
        dioxus_entity_types,
        dioxus_expunge_queue,
        dioxus_expunge_document,
        dioxus_contract_review,
        dioxus_entity_new,
        // The "start a retainer walk" form (#956 Phase 4) renders through
        // Dioxus at `/app/lawyer/retainers/new`; the `POST` on the same path stays
        // on the existing handler, which axum merges with this GET.
        dioxus_retainer_start,
        dioxus_entity_edit,
        dioxus_entity_list,
        // The contract-negotiation playbooks cluster (#956 Phase 4) renders
        // through Dioxus at `/app/admin/playbooks`, `/app/admin/playbooks/new`, and
        // `/app/admin/playbooks/{id}/edit`; the two write `POST`s stay on
        // `admin_playbooks`, which axum merges onto the same paths.
        // The lawyer workbench (#956 Phase 4) renders through Dioxus at `/app/lawyer`,
        // replacing the dashboard.
        dioxus_lawyer_dashboard,
        dioxus_harvard_outline,
        dioxus_notation_outline,
        dioxus_playbooks,
        dioxus_playbook_new,
        dioxus_playbook_edit,
        // The cron-schedule reference and the visitor-analytics dashboard
        // (#956 Phase 4) render through Dioxus at `/app/admin/schedules` and
        // `/app/admin/analytics`; the manual-run `POST`s stay on `cron_schedules`.
        dioxus_schedules,
        dioxus_analytics,
        dioxus_admin_people,
        dioxus_app_forms,
        // ENG-81: the matter list and the single matter each render through one
        // Dioxus mount, at `/app/projects` and `/app/projects/{code}`. The lens
        // comes from the caller's role.
        dioxus_projects,
        dioxus_project_detail,
        // The lawyer project forms (#956 Phase 4) render through Dioxus —
        // matter-open, the descriptive edit, and the participation add/edit.
        dioxus_lawyer_project_forms,
        // One filed document's provenance page renders through Dioxus at
        // `/app/projects/{project_code}/documents/{doc_id}`; the tier picks the lens.
        dioxus_project_documents,
        // The matter conversation page renders through Dioxus at
        // `/app/projects/{project_code}/conversation`; the tier picks the lens.
        dioxus_conversation,
        // The comment-only client document-review page (#641 Phase 3) renders
        // through Dioxus at `/app/projects/{project_code}/review/{doc_id}`.
        dioxus_review,
        // One Project's client portal at `/app/projects/{code}/portal`. It
        // crosses the same login boundary as every other `/app` page, and then
        // authorizes through Project participation itself, because that gate is
        // per-Project rather than per-tier. The route resolves and authorizes;
        // it serves nothing yet, because no bundle is published anywhere to
        // serve (see the module doc).
        project_portal,
        // The client self-serve intake page (#956 Phase 4) renders through
        // Dioxus at `/app/projects/{project_code}/intake/{notation_id}`.
        dioxus_client_intake,
        // The lawyer walker step (#956 Phase 4) renders through Dioxus at
        // `/app/lawyer/notations/{id}/step`.
        dioxus_walker_step,
        // The notation review-and-send screen (#956 Phase 4) renders through
        // Dioxus at `/app/lawyer/notations/{id}/review`.
        dioxus_intake_review,
        // The lawyer re-ask screen (#956 Phase 4) renders through Dioxus at
        // `/app/lawyer/notations/{id}/reask`.
        dioxus_reask,
        dioxus_admin_people_new,
        dioxus_admin_person_show,
        // The supervised Clerk surface (#956 Phase 4) renders through Dioxus at
        // The per-notation clause editor (#956 Phase 4) renders through Dioxus
        // at `/app/lawyer/notations/{id}/clauses`.
        dioxus_clause_editor,
        dioxus_app_docs_index,
        dioxus_app_doc,
        dioxus_app_team,
        dioxus_app_brands,
        dioxus_template_gallery,
        dioxus_template_entry,
    ] {
        router = router.merge(session_boundary(
            dioxus_router,
            &boundary_sessions,
            &boundary_auth,
        ));
    }
    // The two sortable read-only listings (#956 Phase 4) — the template catalog
    // and the questions directory — mount through the same scaffold as the
    // fixed-order ones, plus a pre-handler that 400s an unadvertised `?sort=`.
    for sortable in dioxus_sortable_listings {
        router = router.merge(session_boundary(
            sortable,
            &boundary_sessions,
            &boundary_auth,
        ));
    }
    // The generic read-only admin listings (#641 Phase 3) render through Dioxus,
    // each replacing its `render_listing` route.
    for listing_router in dioxus_admin_listings {
        router = router.merge(session_boundary(
            listing_router,
            &boundary_sessions,
            &boundary_auth,
        ));
    }
    // The living design system at `/design` is a public reference surface. It
    // mounts OUTSIDE `session_boundary`, which would `303` an anonymous reader
    // to login, and carries `inject_optional_session` so a signed-in caller
    // still gets the authenticated nav rather than the anonymous one — the same
    // anonymous treatment the host's marketing pages get below. It is not in
    // `host_dioxus` because that list is firm-host-only and the gallery is a
    // shared Navigator tool that must answer on both hosts.
    //
    // `/docs` and `/docs/{slug}` mount the same way, and for the same reason:
    // the workspace documentation is the manual for software anyone can clone.
    // It sat behind the session boundary while the source was closed, which put
    // a login door in front of the one document that explains how to run what is
    // now public — the argument that already un-gated the Navigator classes.
    // `/app/docs` is untouched: it is the second, role-restricted door to the
    // same index wearing the application chrome, and it stays gated because it
    // is part of the authenticated surface, not because the documents are.
    for public_router in [dioxus_app::design_router(), dioxus_docs_index, dioxus_doc] {
        router = router.merge(
            public_router.route_layer(axum::middleware::from_fn_with_state(
                boundary_sessions.clone(),
                crate::auth::inject_optional_session,
            )),
        );
    }
    // The host's own public Dioxus SSR pages (#730 PR6) — the firm host's
    // ported marketing pages. Unlike the built-in Dioxus routes, these are
    // anonymous marketing pages, so they mount OUTSIDE `session_boundary`
    // (which would `303` an anonymous reader to login) — the same anonymous
    // treatment `host_public` gives the firm pages. They still ride the shared
    // cookie/host/security-header layer stack applied below, and 404 on a host
    // that passes none.
    //
    // `inject_optional_session` resolves the session cookie into the
    // `SessionData` extension without gating the route (#807): these pages skip
    // `session_boundary`, so without it their auth-aware header resolver would
    // never see a signed-in caller and would always render the anonymous nav. It
    // runs outside each router's own layers (`CookieManagerLayer` is applied
    // outermost below), so the cookie is available and the extension is present
    // before `inject_public_utility` reads it.
    for host_router in host_dioxus {
        router = router.merge(
            host_router.route_layer(axum::middleware::from_fn_with_state(
                boundary_sessions.clone(),
                crate::auth::inject_optional_session,
            )),
        );
    }
    let router = router.fallback(fallback_not_found);
    // Layer ordering (outermost first — `Router::layer` wraps the
    // chain so the LAST `.layer(...)` runs first on the request and
    // last on the response):
    //
    //   request  → request-id → trace → security-headers → propagate-id
    //            → trailing-slash → host → brand → cookies → renew → handler
    //   response ← request-id ← trace ← security-headers ← propagate-id
    //            ← trailing-slash ← host ← brand ← cookies ← renew ← handler
    //
    // We want the request-id assigned BEFORE the trace span opens
    // (so the span carries the id) and the security headers applied
    // to every response (including 3xx redirects from the host
    // layer), so they sit on the outside of the cookie + host pair.
    // `redirect_trailing_slash` sits just inside them and outside the
    // host/cookie/session-renew work, since a redirect needs none of
    // that — but it still wants the same request-id and security
    // headers every other response gets.
    // `scope_branding` sits just inside `host_layer`: the host layer is what
    // resolves the request's `views::brand::BrandKey` and stashes it as a
    // request extension, so branding can only be scoped correctly once that
    // has already run.
    // Session renewal sits *inside* the cookie manager so the cookie it
    // re-issues is serialized into the response's `Set-Cookie` header.
    Ok(router
        .layer(axum::middleware::from_fn_with_state(
            session_renew,
            session_renew::renew_session,
        ))
        .layer(tower_cookies::CookieManagerLayer::new())
        .layer(axum::middleware::from_fn_with_state(
            branding,
            scope_branding,
        ))
        .layer(host_layer)
        .layer(axum::middleware::from_fn(redirect_trailing_slash))
        .layer(PropagateRequestIdLayer::new(X_REQUEST_ID))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            csp_value(),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("strict-transport-security"),
            HSTS_VALUE,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn_with_state(
            visitor_analytics_state,
            visitor_analytics::count_public_visit,
        ))
        .layer(SetRequestIdLayer::new(X_REQUEST_ID, MakeRequestUuid)))
}

/// `true` when `path` is the Project client-portal subtree —
/// `/app/projects/{code}/portal` and everything beneath it.
///
/// There a trailing slash is a route distinction the published bundle's own
/// base URL depends on (see [`project_portal`]), not an accidental link
/// variant, so [`trailing_slash_redirect_target`] must never touch it.
fn is_project_portal_subtree(path: &str) -> bool {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    matches!(
        (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next()
        ),
        (Some("app"), Some("projects"), Some(_code), Some("portal"))
    )
}

/// The `Location` a trailing-slash request should redirect to, or `None`
/// when the request must route normally.
///
/// Restricted to `GET`/`HEAD`: a redirect need not preserve a request body,
/// and no route in this router registers a `POST` path ending in `/`, so
/// widening this to every method would let a client silently retry a write
/// as a `GET` against a path nothing actually serves that way. Excludes
/// [`is_project_portal_subtree`], where the slash is meaningful rather than
/// incidental. The query string, if any, survives onto the target.
fn trailing_slash_redirect_target(method: &Method, uri: &Uri) -> Option<String> {
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    let path = uri.path();
    if path.len() <= 1 || !path.ends_with('/') || is_project_portal_subtree(path) {
        return None;
    }
    let stripped = path.trim_end_matches('/');
    let stripped = if stripped.is_empty() { "/" } else { stripped };
    Some(match uri.query() {
        Some(query) => format!("{stripped}?{query}"),
        None => stripped.to_string(),
    })
}

/// Redirect a `GET`/`HEAD` request for a registered path's trailing-slash
/// variant to the canonical, slash-free path.
///
/// A `301` — the conventional slash-normalization status — matching the
/// existing single-route precedent in [`project_portal::redirect_to_slash`],
/// since a `GET`/`HEAD`-only redirect need not preserve a method.
async fn redirect_trailing_slash(req: Request<axum::body::Body>, next: Next) -> Response {
    if let Some(target) = trailing_slash_redirect_target(req.method(), req.uri()) {
        if let Ok(location) = HeaderValue::from_str(&target) {
            return (
                StatusCode::MOVED_PERMANENTLY,
                [(header::LOCATION, location)],
            )
                .into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod trailing_slash_tests {
    use super::{is_project_portal_subtree, trailing_slash_redirect_target};
    use axum::http::{Method, Uri};

    fn target(method: &Method, uri: &str) -> Option<String> {
        trailing_slash_redirect_target(method, &uri.parse::<Uri>().unwrap())
    }

    #[test]
    fn strips_a_trailing_slash_from_a_get() {
        assert_eq!(
            target(&Method::GET, "/app/projects/"),
            Some("/app/projects".to_string())
        );
    }

    #[test]
    fn strips_a_trailing_slash_from_a_head() {
        assert_eq!(target(&Method::HEAD, "/docs/"), Some("/docs".to_string()));
    }

    #[test]
    fn preserves_the_query_string() {
        assert_eq!(
            target(&Method::GET, "/app/projects/?sort=name"),
            Some("/app/projects?sort=name".to_string())
        );
    }

    #[test]
    fn leaves_a_slash_free_path_alone() {
        assert_eq!(target(&Method::GET, "/app/projects"), None);
    }

    #[test]
    fn leaves_the_bare_root_alone() {
        assert_eq!(target(&Method::GET, "/"), None);
    }

    #[test]
    fn leaves_a_post_alone() {
        assert_eq!(target(&Method::POST, "/app/projects/"), None);
    }

    #[test]
    fn leaves_the_project_portal_root_alone() {
        assert_eq!(target(&Method::GET, "/app/projects/acme/portal/"), None);
    }

    #[test]
    fn leaves_a_project_portal_asset_alone() {
        assert_eq!(
            target(&Method::GET, "/app/projects/acme/portal/engagement/"),
            None
        );
    }

    #[test]
    fn portal_subtree_detection_requires_all_four_segments() {
        assert!(!is_project_portal_subtree("/app/projects/acme"));
        assert!(!is_project_portal_subtree("/app/projects/acme/"));
        assert!(is_project_portal_subtree("/app/projects/acme/portal"));
        assert!(is_project_portal_subtree("/app/projects/acme/portal/"));
        assert!(is_project_portal_subtree(
            "/app/projects/acme/portal/assets/app.js"
        ));
    }
}

/// The `presentations`-category certificate POST. The static route already
/// fixes the category, so this hands the shared handler the pair it expects.
async fn catalog_presentation_certificate_submit(
    state: State<AppState>,
    cookies: tower_cookies::Cookies,
    AxumPath(slug): AxumPath<String>,
    form: axum::extract::Form<CertificateForm>,
) -> axum::response::Response {
    catalog_certificate_submit(
        state,
        cookies,
        AxumPath(("presentations".to_string(), slug)),
        form,
    )
    .await
    .into_response()
}

/// The `presentations` certificate request — the one write on the *public*
/// material surface, and the only part of it that is not a Dioxus page router.
///
/// Brand-mounted rather than composed into [`bootstrap`] like its workshop
/// twin below, and deliberately so: only the host that publishes the talks
/// publishes the certificate they lead to. Composing it into the shared
/// application would put `/presentations/{slug}/certificate` on a host that
/// mounts no talk it could lead back to.
///
/// POST-only; a stray GET lands the reader back on the light table where the
/// form lives.
pub fn catalog_presentation_command_routes() -> Router<AppState> {
    Router::new().route(
        dioxus_app::PRESENTATION_CERTIFICATE_PATH,
        axum::routing::post(catalog_presentation_certificate_submit).get(
            |AxumPath(slug): AxumPath<String>| async move {
                axum::response::Redirect::to(&format!("/presentations/{slug}/slides"))
            },
        ),
    )
}

/// The workshop certificate request — the one write on the Catalog material
/// surface, and the only part of it that is not a Dioxus page router.
///
/// It carries the same policy gate the workshop pages carry, so a firm-side
/// reader can ask for their certificate while a `client` cannot, and the
/// session boundary in front of it turns an anonymous caller away first.
///
/// Brand-mounted like its `presentations` twin above, and for the same reason:
/// only the host that publishes the classes publishes the certificate they
/// lead to. Returned already stated and already gated — through the same
/// [`gated`] stack every other firm-side page rides — so the brand crate can push it
/// into its router list beside the class pages themselves.
pub fn catalog_workshop_command_routes(state: &AppState) -> Router {
    gated(
        state,
        Router::new()
            .route(
                dioxus_app::WORKSHOP_CERTIFICATE_PATH,
                axum::routing::post(catalog_workshop_certificate_submit).get(
                    |AxumPath(slug): AxumPath<String>| async move {
                        axum::response::Redirect::to(&format!("/workshops/{slug}/slides"))
                    },
                ),
            )
            .with_state(state.clone()),
    )
}

/// The `workshops`-category certificate POST. The static route already fixes
/// the category, so this hands the shared handler the pair it expects.
async fn catalog_workshop_certificate_submit(
    state: State<AppState>,
    cookies: tower_cookies::Cookies,
    AxumPath(slug): AxumPath<String>,
    form: axum::extract::Form<CertificateForm>,
) -> axum::response::Response {
    catalog_certificate_submit(
        state,
        cookies,
        AxumPath(("workshops".to_string(), slug)),
        form,
    )
    .await
    .into_response()
}

/// Host-level legal and crawler documents (`/privacy`, `/terms`,
/// `/robots.txt`, `/sitemap.xml`, `/llms.txt`).
///
/// These are host-specific, not brand-content: each host serves its own copy.
/// `sitemap` and `llms` name the pages *this* host serves, and the legal
/// documents carry *this* deployment's own privacy and terms text — hardcoding
/// one shared list is how a host would come to advertise another's pages as
/// its own. See [`SitemapPaths`] and [`LlmsTxtDocument`].
pub fn host_crawler_and_legal_routes(
    sitemap: SitemapPaths,
    llms: LlmsTxtDocument,
) -> Router<AppState> {
    // `/privacy` and `/terms` are served by the Dioxus SSR port: each brand
    // builds its own pair with `dioxus_app::legal_dioxus_routers`, mounted
    // alongside this table.
    Router::new()
        .route("/robots.txt", get(robots_txt))
        .route(
            "/sitemap.xml",
            get(
                move |State(state): State<AppState>, request: axum::extract::Request| async move {
                    sitemap_xml(&state, sitemap, &request)
                },
            ),
        )
        .route(
            "/llms.txt",
            get(
                move |State(state): State<AppState>, request: axum::extract::Request| async move {
                    llms_txt(&state, &request, llms)
                },
            ),
        )
}

/// Paths a brand host must reserve for the shared portal application.
///
/// Axum rejects a duplicate method/path when routers are merged; it cannot
/// express "the portal wins" by merge order. [`mount`] therefore validates a
/// host's declared paths before constructing the merged router, preventing a
/// host from accidentally shadowing an application surface.
pub const RESERVED_PATH_PREFIXES: &[&str] = &[
    "/health",
    "/readyz",
    "/version",
    "/assets",
    "/webhook",
    "/docusign",
    "/public",
    "/dioxus-demo",
    "/app",
    "/auth",
    "/mcp",
    "/docs",
    "/api",
];

/// A host tried to register a portal-owned path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountError {
    host_path: String,
}

impl std::fmt::Display for MountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "host route {} is reserved for the Navigator portal",
            self.host_path
        )
    }
}

impl std::error::Error for MountError {}

fn is_reserved_host_path(path: &str) -> bool {
    RESERVED_PATH_PREFIXES.iter().any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|remainder| remainder.starts_with('/'))
    })
}

fn validate_host_paths(host_paths: &[&str]) -> Result<(), MountError> {
    if let Some(host_path) = host_paths.iter().find(|path| is_reserved_host_path(path)) {
        return Err(MountError {
            host_path: (*host_path).to_owned(),
        });
    }
    Ok(())
}

/// Mount the shared application into a brand host after validating its route
/// declaration.
///
/// `host_paths` must name every route the host registers. This explicit list
/// is intentional: Axum's [`Router`] does not expose registered paths for
/// inspection, and allowing its duplicate-route panic would turn a host
/// configuration error into an opaque boot failure.
pub fn mount(state: AppState, host: Router, host_paths: &[&str]) -> Result<Router, MountError> {
    validate_host_paths(host_paths)?;
    Ok(host.merge(router(state)))
}

/// Build Navigator's mountable application router using its bundled public
/// asset directory.
///
/// The constructor always enables portal-only mode: it exposes the shared
/// application and generic docs, while a brand host owns the public marketing
/// surface. That is the tenant shape, so it composes
/// [`tenant::public_routes`] rather than restating a second bare-host
/// redirect that could drift from it.
pub fn router(mut state: AppState) -> Router {
    state.portal_only = PortalOnly::new(true);
    bootstrap(
        state,
        Path::new(DEFAULT_PUBLIC_DIR),
        tenant::public_routes(),
        &["/"],
        Vec::new(),
    )
    .expect("the tenant root redirect does not collide with Navigator")
}

fn mount_brand_assets(
    mut router: Router<AppState>,
    bundle: Option<&views::brand_bundle::BrandBundle>,
) -> Router<AppState> {
    let Some(bundle) = bundle else { return router };
    let assets = &bundle.manifest.assets;
    for (route, file) in [
        ("/public/brand/firm-logo.svg", assets.firm_logo.as_ref()),
        (
            "/public/brand/firm-logo.png",
            assets.firm_logo_raster.as_ref(),
        ),
    ] {
        if let Some(file) = file {
            router = router.route_service(route, ServeFile::new(bundle.directory.join(file)));
        }
    }
    for (public_path, file) in &assets.static_files {
        let route = format!("/public/brand/static/{public_path}");
        router = router.route_service(&route, ServeFile::new(bundle.directory.join(file)));
    }
    router
}

/// Scope the request's resolved brand for the life of the request. `state`
/// is this deployment's own default branding (built-in, or a mounted
/// white-label manifest); `host_layer` (which runs just outside this layer)
/// resolves which [`views::brand::BrandKey`] the request's `Host:` header
/// names and stashes it as a request extension. A request that reaches this
/// layer with no such extension — a router built without `host_layer`, as
/// most direct view/unit tests are — scopes the default key's branding, which
/// is `state` itself.
async fn scope_branding(
    State(default_branding): State<&'static views::brand::Branding>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let branding = request
        .extensions()
        .get::<views::brand::BrandKey>()
        .copied()
        .unwrap_or_default()
        .resolve_branding(default_branding);
    views::brand::scope(branding, next.run(request)).await
}

/// Build the kebab-case redirect target for a file-backed asset route
/// when any path segment is in the legacy underscore form, or `None`
/// when every segment is already canonical.
///
/// Borrowing the JSON:API member-name convention, every public asset URL
/// uses hyphens (see [`views::slug`]); this powers the permanent
/// redirect that lands a `…_…` link on its canonical `…-…` home, shared
/// by the blog, template, and docs routes so the rule can't drift apart.
fn kebab_redirect_path(segments: &[&str]) -> Option<String> {
    if segments.iter().any(|s| views::slug::needs_redirect(s)) {
        let path = segments
            .iter()
            .map(|s| views::slug::to_url(s))
            .collect::<Vec<_>>()
            .join("/");
        Some(format!("/{path}"))
    } else {
        None
    }
}

/// `GET /app/api/templates/*path` — the raw template markdown,
/// served inline as `text/markdown`. Unlike `/templates/.../download`
/// (the curated gallery, attachment-dispositioned), this serves any
/// `confidential: false` template under `templates/` so the README's
/// `templates/**/*.md` links resolve on the site. Confidential
/// templates and unknown paths 404.
async fn api_template_raw(AxumPath(path): AxumPath<String>) -> impl IntoResponse {
    let path = template_api::legacy_alias(&path).unwrap_or(&path);
    let redirect_segments = ["api", "templates", path].join("/");
    let redirect_parts: Vec<&str> = redirect_segments.split('/').collect();
    if let Some(to) = kebab_redirect_path(&redirect_parts) {
        return axum::response::Redirect::permanent(&to).into_response();
    }
    match template_api::find_raw_path(path) {
        Some(raw) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            )],
            raw,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "template not found\n").into_response(),
    }
}

/// `GET /docusign/consent-callback` — the ceremonial landing for the
/// DocuSign JWT-grant one-time consent click.
///
/// JWT grant never sends an authorization `code` back, so this URI exists
/// only so the `Allow` button has somewhere to redirect; the page is
/// purely informational. It is registered as the app's Redirect URI (see
/// [`docs/docusign-esignature.md`](../docs/docusign-esignature.md)),
/// deliberately distinct from the OIDC `/auth/callback`, and is exempt
/// from the private-mode gate so the operator lands on a confirmation
/// rather than a login bounce or a 404.
async fn docusign_consent_callback() -> axum::response::Html<String> {
    webapp::docusign_consent::render()
}

/// Route prefix for one material category. Workshops and presentations each
/// own a top-level path.
pub(crate) fn catalog_material_base(category: &str) -> String {
    format!("/{category}")
}
/// Build the table of contents shared by the overview and every step.
/// Build a material's Dioxus hub content — the outline grouped by chapter,
/// plus the start / slides / markdown affordances.
pub(crate) fn material_content(m: &WorkshopMaterial) -> webapp::catalog_material::MaterialContent {
    let base = catalog_material_base(&m.category);
    let chapters: Vec<webapp::catalog_material::MaterialChapter> = m
        .chapters
        .iter()
        .enumerate()
        .map(
            |(chapter_index, chapter)| webapp::catalog_material::MaterialChapter {
                number: chapter_index + 1,
                title: chapter.title.clone(),
                preamble_html: chapter.preamble_html.clone(),
                steps: (chapter.section_start..chapter.section_start + chapter.section_count)
                    .map(|section_index| webapp::catalog_material::MaterialStep {
                        number: section_index + 1,
                        title: m.sections[section_index].title.clone(),
                        href: format!("{base}/{}/step/{}", m.slug, section_index + 1),
                    })
                    .collect(),
            },
        )
        .collect();
    let start_href = chapters
        .iter()
        .find_map(|chapter| chapter.steps.first())
        .map(|step| step.href.clone());
    webapp::catalog_material::MaterialContent {
        title: m.title.clone(),
        description: m.description.clone(),
        intro_html: m.intro_html.clone(),
        body_html: m.body_html.clone(),
        chapters,
        start_href,
        slides_href: format!("{base}/{}/slides", m.slug),
        md_href: format!("{base}/{}.md", m.slug),
    }
}

/// Build a material's light-table content. `csrf_token` is minted per request
/// by the router's pre-layer, so it arrives from there rather than from the
/// material.
pub(crate) fn light_table_content(
    m: &WorkshopMaterial,
    csrf_token: String,
) -> webapp::catalog_slides::LightTableContent {
    let base = catalog_material_base(&m.category);
    let chapters: Vec<webapp::catalog_slides::SlideChapter> = m
        .chapters
        .iter()
        .enumerate()
        .map(
            |(chapter_index, chapter)| webapp::catalog_slides::SlideChapter {
                number: chapter_index + 1,
                title: chapter.title.clone(),
                slides: (chapter.section_start..chapter.section_start + chapter.section_count)
                    .map(|section_index| webapp::catalog_slides::SlideThumb {
                        number: section_index + 1,
                        title: m.sections[section_index].title.clone(),
                        body_html: m.sections[section_index].body_html.clone(),
                        href: format!("{base}/{}/step/{}", m.slug, section_index + 1),
                    })
                    .collect(),
            },
        )
        .collect();
    webapp::catalog_slides::LightTableContent {
        workshop_title: m.title.clone(),
        slug: m.slug.clone(),
        material_href: format!("{base}/{}", m.slug),
        chapters,
        total: m.sections.len(),
        certificate_action: format!("{base}/{}/certificate", m.slug),
        csrf_token,
    }
}

/// Build one classroom step (`…/{slug}/step/{n}`). `step` is 1-based; `None`
/// means the material has no such section and the caller must 404.
pub(crate) fn step_content(
    m: &WorkshopMaterial,
    step: usize,
) -> Option<webapp::catalog_step::StepContent> {
    let index = step.checked_sub(1)?;
    let section = m.sections.get(index)?;
    let base = catalog_material_base(&m.category);
    let material_href = format!("{base}/{}", m.slug);
    let total = m.sections.len();
    let step_href = |n: usize| format!("{material_href}/step/{n}");

    let chapters: Vec<webapp::catalog_step::StepMenuChapter> = m
        .chapters
        .iter()
        .enumerate()
        .map(
            |(chapter_index, chapter)| webapp::catalog_step::StepMenuChapter {
                number: chapter_index + 1,
                title: chapter.title.clone(),
                entries: (chapter.section_start..chapter.section_start + chapter.section_count)
                    .map(|section_index| webapp::catalog_step::StepMenuEntry {
                        number: section_index + 1,
                        title: m.sections[section_index].title.clone(),
                        href: step_href(section_index + 1),
                        current: section_index == index,
                    })
                    .collect(),
            },
        )
        .collect();
    // Which chapter this section belongs to, by the same section-range walk the
    // menu above uses.
    let current_chapter = chapters
        .iter()
        .find(|chapter| chapter.entries.iter().any(|entry| entry.current));

    Some(webapp::catalog_step::StepContent {
        workshop_title: m.title.clone(),
        slug: m.slug.clone(),
        title: section.title.clone(),
        body_html: section.body_html.clone(),
        notes_html: section.notes_html.clone(),
        number: step,
        total,
        chapter_number: current_chapter.map_or(0, |chapter| chapter.number),
        chapter_title: current_chapter.map_or_else(String::new, |chapter| chapter.title.clone()),
        chapter_total: chapters.len(),
        percent: (step * 100).checked_div(total).unwrap_or(0),
        prev_href: (step > 1).then(|| step_href(step - 1)),
        next_href: (step < total).then(|| step_href(step + 1)),
        slides_href: format!("{material_href}/slides"),
        display_href: format!("{material_href}/display/{step}"),
        material_href,
        chapters,
    })
}

/// Build one projected slide (`…/{slug}/display/{n}`). Same 1-based addressing
/// and out-of-range rule as [`step_content`].
pub(crate) fn display_content(
    m: &WorkshopMaterial,
    step: usize,
) -> Option<webapp::catalog_display::DisplayContent> {
    let section = step.checked_sub(1).and_then(|i| m.sections.get(i))?;
    let base = catalog_material_base(&m.category);
    let material_href = format!("{base}/{}", m.slug);
    Some(webapp::catalog_display::DisplayContent {
        workshop_title: m.title.clone(),
        title: section.title.clone(),
        body_html: section.body_html.clone(),
        prev_href: (step > 1).then(|| format!("{material_href}/display/{}", step - 1)),
        next_href: (step < m.sections.len())
            .then(|| format!("{material_href}/display/{}", step + 1)),
        step_href: format!("{material_href}/step/{step}"),
    })
}

/// Build the certificate confirmation (`…/{slug}/certificate/sent`).
pub(crate) fn certificate_sent_content(
    m: &WorkshopMaterial,
) -> webapp::catalog_certificate_sent::CertificateSentContent {
    webapp::catalog_certificate_sent::CertificateSentContent {
        workshop_title: m.title.clone(),
        material_href: format!("{}/{}", catalog_material_base(&m.category), m.slug),
    }
}

/// Serve a raw Markdown document as `text/markdown` — the
/// machine-readable twin of a stepped-content page. LLM crawlers and the
/// on-page "View as Markdown" link both fetch this; it is the one
/// canonical source for the corpus, so the HTML view never embeds the
/// raw markdown itself.
pub(crate) fn markdown_response_for(raw: &str) -> axum::response::Response {
    markdown_response(raw).into_response()
}

fn markdown_response(raw: &str) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/markdown; charset=utf-8"),
        )],
        raw.to_owned(),
    )
}

/// Resolve the absolute base URL (`scheme://authority`) for links in
/// machine-readable artifacts. Prefers `CANONICAL_HOST`; falls back to
/// the request `Host` header in dev. Mirrors the A2A agent card's
/// authority resolution so every absolute URL the site advertises uses
/// the same host, with no hard-coded domain (OSS forks get their own).
fn resolve_base_url(canonical_host: &CanonicalHost, headers: &axum::http::HeaderMap) -> String {
    let request_host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| host.split(':').next().unwrap_or(host))
        .filter(|host| !host.is_empty());
    if let Some(host) = request_host {
        if views::brand::registered_brand_key(host).is_some() {
            let scheme = scheme_for_authority(host);
            return format!("{scheme}://{host}");
        }
    }
    resolve_crawler_base_url(canonical_host)
}

fn scheme_for_authority(authority: &str) -> &'static str {
    if authority.starts_with("localhost")
        || authority.starts_with("127.0.0.1")
        || authority.starts_with("0.0.0.0")
    {
        "http"
    } else {
        "https"
    }
}

fn resolve_crawler_base_url(canonical_host: &CanonicalHost) -> String {
    let authority = canonical_host.host().unwrap_or("www.example.com");
    let scheme = scheme_for_authority(authority);
    format!("{scheme}://{authority}")
}

fn text_response(content_type: &'static str, body: String) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
        body,
    )
}

/// The `Disallow` list every host serves, whatever brand it wears.
///
/// It names what sits behind the session boundary, so the policy names those
/// paths rather than sending crawlers at a login redirect. That is the shared
/// Navigator application rather than any brand's marketing, and every host
/// mounts the same application, so every host disallows the same set. A
/// brand's anonymous surface — its marketing pages and the talks beneath
/// `/presentations/` — is deliberately absent from this list, and so is a
/// retired URL, which answers `410` rather than needing a crawler policy.
///
/// `/workshops` left the list when the Navigator classes became public. The
/// sitemap advertises them now, and a path a host both advertises and forbids
/// is a contradiction a crawler settles by not fetching the page.
const CRAWLER_DISALLOW_BLOCK: &str = "\
User-agent: *
Disallow: /app
Disallow: /admin
Disallow: /auth
Disallow: /mcp
Disallow: /docs
Disallow: /design
Disallow: /templates
";

/// `/robots.txt` — the host's crawler policy. Its own public marketing and
/// blog pages are crawlable. The sitemap URL is absolute so crawlers discover
/// the canonical host even in forks.
async fn robots_txt(
    State(canonical_host): State<CanonicalHost>,
    request: axum::extract::Request,
) -> impl IntoResponse {
    let base = resolve_base_url(&canonical_host, request.headers());
    let body = format!("{CRAWLER_DISALLOW_BLOCK}\nSitemap: {base}/sitemap.xml\n");
    text_response("text/plain; charset=utf-8", body)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn percent_encode_sitemap_path(path: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(char::from(byte));
            }
            _ => write!(&mut out, "%{byte:02X}").expect("write to string"),
        }
    }
    out
}

/// The anonymous pages one brand invites a crawler to read.
///
/// [`host_crawler_and_legal_routes`] is shared by every brand host, so the
/// sitemap cannot be: a single hardcoded list advertises one brand's pages on
/// another brand's host, and each of those entries is a `404` for the crawler
/// that follows it. Each brand supplies its own set instead, and
/// [`host_crawler_and_legal_routes`] carries it into the handler.
///
/// It takes [`AppState`] because a brand's crawlable surface includes its
/// content-backed pages — the firm's posts and its talks — which are whatever
/// was loaded at boot rather than a constant.
///
/// Every path a brand returns must be anonymously readable on that brand's
/// host. Its declared path table is the wider claim: it lists gated pages too,
/// and a sitemap entry pointing at a login redirect is worse than no entry at
/// all.
pub type SitemapPaths = fn(&AppState, views::brand::BrandKey) -> std::collections::BTreeSet<String>;

/// The host-owned public GET surfaces this router publishes: the documents
/// every brand serves, plus the brand's own anonymous pages.
///
/// Only host pages appear: the shared Navigator tools that used to be listed
/// here — `/docs`, `/templates`, `/design` — are authenticated
/// now, and a sitemap entry pointing at a login redirect is worse than no
/// entry at all.
fn sitemap_paths(
    state: &AppState,
    brand_paths: SitemapPaths,
    key: views::brand::BrandKey,
) -> std::collections::BTreeSet<String> {
    let mut paths = std::collections::BTreeSet::new();
    if state.portal_only.enabled() {
        return paths;
    }
    // The three documents this table and its Dioxus twin publish identically
    // on every host, so they are the sitemap's shared half.
    paths.extend([
        "/privacy".to_string(),
        "/terms".to_string(),
        "/llms.txt".to_string(),
    ]);
    paths.extend(brand_paths(state, key));
    paths
}

fn render_sitemap_xml(base: &str, paths: &std::collections::BTreeSet<String>) -> String {
    use std::fmt::Write as _;

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    let base = base.trim_end_matches('/');
    for path in paths {
        let loc = xml_escape(&format!("{base}{}", percent_encode_sitemap_path(path)));
        writeln!(&mut out, "  <url><loc>{loc}</loc></url>").expect("write to string");
    }
    out.push_str("</urlset>\n");
    out
}

/// `/sitemap.xml` — absolute canonical URLs for the public GET surfaces the
/// serving brand mounts. Content-backed pages (the firm's posts and talks) are
/// read from `AppState`, so the sitemap follows the content loaded at boot.
fn request_brand_key(request: &axum::extract::Request) -> views::brand::BrandKey {
    request
        .extensions()
        .get::<views::brand::BrandKey>()
        .copied()
        .unwrap_or_default()
}

fn sitemap_xml(
    state: &AppState,
    brand_paths: SitemapPaths,
    request: &axum::extract::Request,
) -> Response {
    let base = resolve_base_url(&state.canonical_host, request.headers());
    let key = request_brand_key(request);
    let body = render_sitemap_xml(&base, &sitemap_paths(state, brand_paths, key));
    text_response("application/xml; charset=utf-8", body).into_response()
}

#[cfg(test)]
mod crawler_discovery_tests {
    use super::{render_llms_txt, render_sitemap_xml, resolve_crawler_base_url};
    use crate::{CanonicalHost, LlmsTxt, LlmsTxtLink, LlmsTxtSection};
    use std::collections::BTreeSet;

    #[test]
    fn crawler_base_defaults_to_deployment_neutral_host() {
        assert_eq!(
            resolve_crawler_base_url(&CanonicalHost::new(None)),
            "https://www.example.com"
        );
        assert_eq!(
            resolve_crawler_base_url(&CanonicalHost::new(Some("www.neonlaw.com".into()))),
            "https://www.neonlaw.com"
        );
        assert_eq!(
            resolve_crawler_base_url(&CanonicalHost::new(Some("localhost:3001".into()))),
            "http://localhost:3001"
        );
        assert_eq!(
            resolve_crawler_base_url(&CanonicalHost::new(Some("127.0.0.1:3001".into()))),
            "http://127.0.0.1:3001"
        );
        assert_eq!(
            resolve_crawler_base_url(&CanonicalHost::new(Some("0.0.0.0:3001".into()))),
            "http://0.0.0.0:3001"
        );
    }

    #[test]
    fn sitemap_xml_percent_encodes_url_path_before_xml_escaping() {
        let xml = render_sitemap_xml(
            "https://www.neonlaw.com",
            &BTreeSet::from(["/blog/a name & symbol".to_string()]),
        );
        assert!(xml.contains("<loc>https://www.neonlaw.com/blog/a%20name%20%26%20symbol</loc>"));
        assert!(!xml.contains("a name"));
    }

    /// The brand names the site and the pages; this half renders them into the
    /// llmstxt.org shape, over an absolute base the brand never sees.
    #[test]
    fn llms_txt_renders_the_brands_pages_under_the_hosts_own_base_url() {
        let document = LlmsTxt {
            title: "Acme Law".to_string(),
            summary: "What Acme does.".to_string(),
            pages: vec![
                LlmsTxtLink {
                    title: "Acme Law".to_string(),
                    path: "/".to_string(),
                    description: "The practice.".to_string(),
                },
                LlmsTxtLink {
                    title: "Writing".to_string(),
                    path: "/blog".to_string(),
                    description: "Posts.".to_string(),
                },
            ],
            sections: Vec::new(),
        };
        // A trailing slash on the base must not double up on the path's own.
        let out = render_llms_txt("https://www.example.com/", &document);
        assert!(out.starts_with("# Acme Law\n\n> What Acme does.\n\n"));
        assert!(out.contains("- [Acme Law](https://www.example.com/): The practice.\n"));
        assert!(out.contains("- [Writing](https://www.example.com/blog): Posts.\n"));
        // Both hosts mount the same application, so the notes about it are the
        // shared half and arrive without the brand supplying them.
        assert!(out.contains("Nothing is legal advice without a signed retainer"));
        assert!(out.contains("`{{placeholders}}`"));
    }

    /// A section with nothing in it is a heading a crawler reads as a promise.
    #[test]
    fn llms_txt_omits_a_section_with_no_documents() {
        let mut document = LlmsTxt {
            title: "Acme Law".to_string(),
            summary: "What Acme does.".to_string(),
            pages: Vec::new(),
            sections: vec![LlmsTxtSection {
                heading: "Corpus".to_string(),
                links: Vec::new(),
            }],
        };
        let out = render_llms_txt("https://www.example.com", &document);
        assert!(!out.contains("## Corpus"), "{out}");
        assert!(!out.contains("## Pages"), "{out}");

        document.sections[0].links.push(LlmsTxtLink {
            title: "A Talk".to_string(),
            path: "/presentations/a-talk.md".to_string(),
            description: "What it covers.".to_string(),
        });
        let out = render_llms_txt("https://www.example.com", &document);
        assert!(out.contains(
            "## Corpus\n\n- [A Talk](https://www.example.com/presentations/a-talk.md): What it \
             covers.\n"
        ));
    }
}

/// One curated link in `/llms.txt`.
///
/// `path` is host-relative for the same reason [`SitemapPaths`] yields paths:
/// the absolute URL is derived once, from `CANONICAL_HOST`, so a brand cannot
/// hardcode a domain and a fork advertises its own with no edits.
pub struct LlmsTxtLink {
    /// The link text — what the document is called.
    pub title: String,
    /// The host-relative path, leading slash included.
    pub path: String,
    /// One line on what a crawler finds there.
    pub description: String,
}

/// A named group of links below `## Pages` — a brand's Markdown corpus.
pub struct LlmsTxtSection {
    /// The `##` heading. Rendered only when `links` is non-empty, so a host
    /// with nothing to index carries no empty section.
    pub heading: String,
    /// The documents beneath it.
    pub links: Vec<LlmsTxtLink>,
}

/// The brand half of `/llms.txt`: which site a crawler has reached, and the
/// documents it is invited to read there.
pub struct LlmsTxt {
    /// The document's H1 — the site serving this host, not the application
    /// underneath it.
    pub title: String,
    /// The one-line `>` summary beneath the H1.
    pub summary: String,
    /// The `## Pages` links: the brand's own anonymous pages.
    pub pages: Vec<LlmsTxtLink>,
    /// Corpus sections below `## Pages`, such as the firm's talks.
    pub sections: Vec<LlmsTxtSection>,
}

/// The `/llms.txt` document one brand publishes.
///
/// [`host_crawler_and_legal_routes`] is shared by every brand host, so this
/// document cannot be: one hardcoded page list would open every host with one
/// brand's name and send an LLM crawler at pages the others do not serve. Each
/// brand supplies its own instead, exactly as it supplies its own
/// [`SitemapPaths`].
///
/// It takes [`AppState`] because a brand's crawlable corpus includes its
/// content-backed documents — its talks — which are whatever was loaded at boot
/// rather than a constant.
///
/// Every path a brand returns must be anonymously readable on that brand's
/// host: advertising a login redirect or a 404 as a crawlable document is
/// worse than advertising nothing.
pub type LlmsTxtDocument = fn(&AppState, views::brand::BrandKey) -> LlmsTxt;

/// The notes every host publishes, whatever brand it wears.
///
/// The brand half of the document changes with the host; this half does not.
/// Both hosts mount the same Navigator application, so how an agent should
/// work with it — what a Template is, what a Notation is, where to ground a
/// questionnaire — reads identically on either domain.
const LLMS_TXT_NOTES: &str = "\
Important notes:
- Nothing is legal advice without a signed retainer for an active project.
- A Template is a markdown file with YAML frontmatter, `questionnaire:`, `workflow:`, and `{{placeholders}}`.
- A Notation is a running instance of a Template, bound to a matter and respondent.
- When writing notation, ground questionnaire states and placeholders in the Navigator glossary before inventing `custom_*` fields.
- Use the Navigator CLI to validate templates, render documents, walk intake, fill forms, and download generated packets.
";

fn render_llms_txt(base: &str, document: &LlmsTxt) -> String {
    use std::fmt::Write as _;

    let base = base.trim_end_matches('/');
    let mut out = format!(
        "# {}\n\n> {}\n\n{LLMS_TXT_NOTES}",
        document.title, document.summary
    );

    let mut write_links = |heading: &str, links: &[LlmsTxtLink]| {
        if links.is_empty() {
            return;
        }
        let _ = write!(out, "\n## {heading}\n\n");
        for link in links {
            let _ = writeln!(
                out,
                "- [{}]({base}{}): {}",
                link.title, link.path, link.description
            );
        }
    };

    write_links("Pages", &document.pages);
    for section in &document.sections {
        write_links(&section.heading, &section.links);
    }
    out
}

/// `/llms.txt` — the machine-readable corpus index in the
/// [llmstxt.org](https://llmstxt.org) convention: an H1, a one-line
/// summary, then one bullet per Markdown document the site serves so an
/// LLM crawler discovers every `.md` twin from a single file instead of
/// scraping rendered HTML. URLs are absolute and derived from
/// `CANONICAL_HOST` (see [`resolve_base_url`]), so a fork advertises its
/// own domain with no edits.
///
/// The serving brand names the pages (see [`LlmsTxtDocument`]); this renders
/// them into the shared document shape.
fn llms_txt(
    state: &AppState,
    request: &axum::extract::Request,
    document: LlmsTxtDocument,
) -> Response {
    let base = resolve_base_url(&state.canonical_host, request.headers());
    let key = request_brand_key(request);
    markdown_response(&render_llms_txt(&base, &document(state, key))).into_response()
}

/// Dedicated double-submit CSRF cookie for the workshop certificate form.
/// Distinct from `ACCOUNT_CSRF_COOKIE_NAME` (password reset / email-confirm)
/// so opening a workshop light table never clobbers an in-flight account
/// recovery in another tab, and vice versa.
pub(crate) const WORKSHOP_CERT_CSRF_COOKIE_NAME: &str = "navigator_workshop_cert_csrf";

/// Whether cookies should carry the `Secure` flag — true when the OAuth
/// redirect URI is HTTPS (prod), false in plain-HTTP local dev. Mirrors
/// how `AuthState::secure_cookies` is derived, for handlers that hold the
/// full `AppState` instead of an `AuthState`.
///
/// Public because a brand crate composing its own material routers needs the
/// same answer the application uses; deriving it a second time in a brand is
/// how the two drift.
#[must_use]
pub fn secure_cookies(app: &AppState) -> bool {
    app.oauth
        .as_ref()
        .is_some_and(|o| o.redirect_uri().starts_with("https://"))
}

/// Dedicated double-submit CSRF cookie for the presentation control bar.
/// Distinct from the certificate/register cookies so opening the present
/// page never clobbers an in-flight certificate request in another tab.
/// The light-table grid for one workshop: every slide as a thumbnail.
/// Mints a double-submit CSRF token for the certificate form embedded on
/// the page (revealed client-side once every slide has been viewed).
/// Form body for the certificate request (`POST …/{slug}/certificate`).
#[derive(serde::Deserialize)]
struct CertificateForm {
    name: String,
    email: String,
    #[serde(default)]
    csrf_token: String,
}

/// `POST …/{slug}/certificate` — a student who has worked through every
/// slide asks for their completion certificate. Validates the
/// double-submit CSRF token, then dispatches the durable
/// `workshop__certificate` workflow (which renders the PDF and emails it
/// from the firm's address). Completion is client-trusted
/// (localStorage, no telemetry), so this endpoint can't verify the slides
/// were actually viewed — it's an educational courtesy, not a credential.
async fn catalog_certificate_submit(
    State(app): State<AppState>,
    cookies: tower_cookies::Cookies,
    AxumPath((category, slug)): AxumPath<(String, String)>,
    axum::extract::Form(form): axum::extract::Form<CertificateForm>,
) -> impl IntoResponse {
    let Some(m) = app.workshops.find_in_category(&category, &slug) else {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    };
    if !crate::password_reset::verify_csrf_with(
        &app.sessions,
        &cookies,
        &form.csrf_token,
        WORKSHOP_CERT_CSRF_COOKIE_NAME,
    ) {
        return (StatusCode::BAD_REQUEST, "invalid or missing CSRF token").into_response();
    }
    cookies.add(crate::oauth::expired_cookie(WORKSHOP_CERT_CSRF_COOKIE_NAME));

    let name = form.name.trim();
    let email = form.email.trim();
    // Server-side bounds mirror the form's maxlength, so a client that
    // bypasses the HTML constraint can't feed a multi-megabyte name into
    // the Typst renderer or an oversized address to SendGrid.
    if name.is_empty() || name.len() > 120 || !email.contains('@') || email.len() > 254 {
        return (
            StatusCode::BAD_REQUEST,
            "Please enter your name and a valid email address.",
        )
            .into_response();
    }

    // The issue date is stamped here (web), so it rides the Restate signal
    // value and a replay reuses it deterministically — the worker never
    // reads the clock.
    let issued = chrono::Utc::now().format("%B %-d, %Y").to_string();
    // A fresh key per request: each certificate is its own ephemeral
    // workflow invocation.
    let key = uuid::Uuid::new_v4();
    let runtime = app.workflow_runtime.clone();
    if let Err(e) = workflows::email::certificate::trigger_certificate(
        runtime.as_ref(),
        key,
        name,
        email,
        &m.title,
        &issued,
    )
    .await
    {
        // Logged, never surfaced — the reply is the same neutral page so
        // the endpoint isn't an address-enumeration oracle. Instrument the
        // workshop + outcome only, never the recipient (trust boundary).
        tracing::warn!(error = %e, workshop = %m.slug, "certificate dispatch failed");
    }
    // Post/redirect/get: the confirmation is its own route, so a reload
    // re-renders it instead of dispatching a second certificate.
    axum::response::Redirect::to(&format!(
        "{}/{}/certificate/sent",
        catalog_material_base(&m.category),
        m.slug
    ))
    .into_response()
}

/// `GET /version` — report the release of the build that is actually
/// running, so an operator/CI/AIDA/browser can confirm which release prod
/// is on without shelling into a (shell-less) distroless pod.
///
/// The headline field is `release`: the `YY.M.D` Artifact Registry tag the
/// daily `deploy.yml` published, baked into the image as
/// `NAVIGATOR_RELEASE_TAG`. An image is pulled from the `ghcr` hub
/// by that dated tag, so `release` is what a `ship` rolls onto and what an
/// operator pins — it is the deploy's identity. The git fields stay alongside it for traceability:
/// `images/Containerfile.web` turns the `GIT_SHA`/`BUILD_TIME` build-args
/// (set by CI to the released commit) into `NAVIGATOR_GIT_SHA` /
/// `NAVIGATOR_BUILD_TIME`. All three are baked into the image bytes, so
/// they cannot drift from what was deployed. A local `cargo run` honestly
/// reports `"unknown"` (no env var, no build-arg).
///
/// Public, unauthenticated, exempt from the private-mode gate — it is an
/// ops/health-class endpoint like `/health` and `/readyz`.
async fn version() -> impl IntoResponse {
    let release = std::env::var("NAVIGATOR_RELEASE_TAG").unwrap_or_else(|_| "unknown".into());
    let commit_full = std::env::var("NAVIGATOR_GIT_SHA").unwrap_or_else(|_| "unknown".into());
    // The short SHA is the load-bearing field. Derive it from the full
    // one (first 7 chars) so the two can never disagree.
    let commit = if commit_full == "unknown" {
        "unknown".to_string()
    } else {
        commit_full.chars().take(7).collect()
    };
    let built = std::env::var("NAVIGATOR_BUILD_TIME").unwrap_or_else(|_| "unknown".into());
    axum::Json(serde_json::json!({
        "release": release,
        "commit": commit,
        "commit_full": commit_full,
        "built": built,
        "crate_version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn health(State(surreal): State<store::surreal::SurrealDb>) -> impl IntoResponse {
    match store::surreal::ping(&surreal).await {
        Ok(()) => (
            StatusCode::OK,
            "ok\nNothing here is legal advice without a signed retainer.",
        ),
        Err(e) => {
            tracing::warn!(error = %e, "health: store ping failed");
            (StatusCode::SERVICE_UNAVAILABLE, "store unavailable")
        }
    }
}

/// Readiness probe: the pod is ready to take traffic when its database is
/// reachable. The authorization policy is compiled at boot, so it is not a
/// runtime network dependency.
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let mut failures: Vec<String> = Vec::new();
    if let Err(e) = store::surreal::ping(&state.surreal).await {
        failures.push(format!("surreal: {e}"));
    }
    if failures.is_empty() {
        (StatusCode::OK, "ready").into_response()
    } else {
        let body = failures.join("\n");
        tracing::warn!(failure_count = failures.len(), "readyz: degraded");
        (StatusCode::SERVICE_UNAVAILABLE, body).into_response()
    }
}

/// Router-level fallback for paths that no other handler matched.
/// HTML clients (browsers, anything not under `/app/api` or `/mcp`) get
/// the styled 404 page; API/JSON-RPC clients get a tiny JSON body.
async fn fallback_not_found(req: axum::extract::Request) -> impl IntoResponse {
    let path = req.uri().path();
    if wants_json(path) {
        (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "not_found" })),
        )
            .into_response()
    } else {
        (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response()
    }
}

/// `true` when the request should get a machine-readable error body
/// rather than the HTML chrome. The two non-HTML surfaces this server
/// hosts are `/app/api/*` (JSON listings + the OpenAPI document) and
/// `/mcp` (MCP JSON-RPC). Everything else — including `/app/*` HTML
/// pages and the `/auth/*` flows — gets the styled error page.
///
/// `/app/api` exactly is the one path under the prefix that does *not*
/// want JSON: it is the Swagger UI shell, an HTML page a browser lands
/// on, so an anonymous visitor there belongs in the login door rather
/// than holding a JSON error body.
#[must_use]
pub fn wants_json(path: &str) -> bool {
    path.starts_with("/app/api/") || path.starts_with("/mcp/") || path == "/mcp"
}

#[cfg(test)]
mod version_tests {
    use super::version;
    use axum::body::Body;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    /// `GET /version` answers 200 with a JSON body, reflects the baked
    /// `NAVIGATOR_RELEASE_TAG` + `NAVIGATOR_GIT_SHA` when present (short =
    /// first 7 of the full SHA), and falls back to `"unknown"` when they
    /// are unset. The handler needs no `State`, so a one-route router
    /// exercises the real route wiring without standing up a DB. Both
    /// cases run in one test, sequentially, because the env var is
    /// process-global.
    #[tokio::test]
    async fn version_reports_baked_sha_or_unknown() {
        async fn get_version_json() -> serde_json::Value {
            let app = Router::new().route("/version", get(version));
            let resp = app
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/version")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), axum::http::StatusCode::OK);
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice(&bytes).unwrap()
        }

        // SAFETY: single-threaded within this test; no other code reads
        // NAVIGATOR_GIT_SHA, so there is no concurrent reader to race.
        std::env::set_var("NAVIGATOR_RELEASE_TAG", "26.6.23");
        std::env::set_var(
            "NAVIGATOR_GIT_SHA",
            "ef143cba1fdd299c0f57f99eddb7806df5464b68",
        );
        std::env::set_var("NAVIGATOR_BUILD_TIME", "2026-06-11T17:01:25-07:00");
        let v = get_version_json().await;
        assert_eq!(v["release"], "26.6.23");
        assert_eq!(v["commit"], "ef143cb");
        assert_eq!(v["commit_full"], "ef143cba1fdd299c0f57f99eddb7806df5464b68");
        assert_eq!(v["built"], "2026-06-11T17:01:25-07:00");
        assert!(v["crate_version"].is_string());

        std::env::remove_var("NAVIGATOR_RELEASE_TAG");
        std::env::remove_var("NAVIGATOR_GIT_SHA");
        std::env::remove_var("NAVIGATOR_BUILD_TIME");
        let v = get_version_json().await;
        assert_eq!(v["release"], "unknown");
        assert_eq!(v["commit"], "unknown");
        assert_eq!(v["commit_full"], "unknown");
        assert_eq!(v["built"], "unknown");
    }
}

#[cfg(test)]
mod csp_tests {
    use super::{csp_asset_origin_from, csp_value};

    /// An absolute `https`/`http` asset base contributes its
    /// `scheme://host` origin (the bucket sub-path is dropped — a CSP
    /// host-source is an origin, not a path). A relative base (the
    /// `/public` default) or junk contributes nothing, since `'self'`
    /// already covers same-origin photos.
    #[test]
    fn asset_origin_is_the_scheme_and_host_only() {
        assert_eq!(
            csp_asset_origin_from("https://storage.googleapis.com/my-proj-assets"),
            Some("https://storage.googleapis.com".to_string()),
        );
        assert_eq!(
            csp_asset_origin_from("https://cdn.example.com"),
            Some("https://cdn.example.com".to_string()),
        );
        assert_eq!(
            csp_asset_origin_from("  http://localhost:8080/assets/  "),
            Some("http://localhost:8080".to_string()),
        );
        assert_eq!(csp_asset_origin_from("/public"), None);
        assert_eq!(csp_asset_origin_from(""), None);
        assert_eq!(csp_asset_origin_from("https://"), None);
    }

    /// With no `NAVIGATOR_ASSET_BASE_URL` the CSP stays same-origin.
    /// Setting it to a bucket widens passive asset directives only. The env
    /// var is process-global, so this test owns it.
    #[test]
    fn csp_value_widens_presentation_asset_sources_for_the_asset_host() {
        // SAFETY: single-threaded within this test; the only readers of
        // NAVIGATOR_ASSET_BASE_URL are these helpers, run sequentially here.
        std::env::remove_var("NAVIGATOR_ASSET_BASE_URL");
        let csp = csp_value();
        let csp = csp.to_str().unwrap().to_string();
        assert!(csp.contains("img-src 'self' data:;"), "got: {csp}");
        assert!(csp.contains("font-src 'self';"), "got: {csp}");
        // `media-src` is named rather than left to fall back to `default-src`,
        // so slide video is governed explicitly on both origins.
        assert!(csp.contains("media-src 'self';"), "got: {csp}");
        // The site-wide policy stays strictly same-origin for scripts — no CDN
        // host and no wasm allowance; the Dioxus route carries its own CSP.
        assert!(csp.contains("script-src 'self';"), "got: {csp}");
        assert!(
            !csp.contains("wasm-unsafe-eval"),
            "wasm stays route-scoped: {csp}"
        );
        // The support-chat origin is route-scoped for the same reason. This
        // policy governs the JSON API, the redirects, and the static mounts —
        // none of which render a widget, and none of which should name its
        // installation as a script source.
        assert!(
            !csp.contains("chatwoot") && !csp.contains("connect-src"),
            "the widget origin stays route-scoped: {csp}"
        );
        assert!(!csp.contains("script-src 'self' http"), "got: {csp}");
        assert!(!csp.contains("googleapis"), "got: {csp}");

        std::env::set_var(
            "NAVIGATOR_ASSET_BASE_URL",
            "https://storage.googleapis.com/my-proj-assets",
        );
        let csp = csp_value();
        let csp = csp.to_str().unwrap().to_string();
        assert!(
            csp.contains("img-src 'self' data: https://storage.googleapis.com;"),
            "asset host must widen img-src: {csp}",
        );
        assert!(
            csp.contains("font-src 'self' https://storage.googleapis.com;"),
            "asset host must widen font-src: {csp}",
        );
        // Without this a slide's clip plays from the local `/public` mount and
        // is blocked from the bucket in production — the exact failure the
        // named directive exists to prevent.
        assert!(
            csp.contains("media-src 'self' https://storage.googleapis.com;"),
            "asset host must widen media-src: {csp}",
        );
        // Code never leaves the origin even when presentation assets do.
        assert!(csp.contains("script-src 'self'"), "got: {csp}");
        assert!(!csp.contains("script-src 'self' https"), "got: {csp}");
        std::env::remove_var("NAVIGATOR_ASSET_BASE_URL");
    }
}
