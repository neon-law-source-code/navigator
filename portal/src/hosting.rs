//! Shared host runtime assembly and run loop for Navigator's brand binaries.
//!
//! Every brand binary boots the same Navigator application: same database,
//! storage, providers, and `AppState`. What differs is the [`Brand`] it
//! declares — its telemetry name and the public surface it publishes — so a
//! brand crate is a `Brand` value and nothing else. [`build_from_env`]
//! assembles the runtime and [`run`] owns the boot: telemetry, that assembly,
//! composition through [`crate::bootstrap`], bind, serve, and drain.
//!
//! Both halves live here because they are one boot sequence. Splitting the
//! bind/serve tail into a separate crate would put a fourth dependency between
//! a brand crate and the application it mounts, and every brand main would
//! re-derive the ordering that `run` now states once.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use axum::Router;

use crate::{AppConfig, AppState};

/// Re-exported for brand crates: composing a public surface means naming
/// `Router`, and the thin-crate rule keeps a brand from depending on `axum`
/// directly. A brand takes it from the application it mounts, like every other
/// type it needs.
pub use axum::Router as PublicRouter;

/// Re-exported so a brand crate composes its whole [`Brand`] from `portal`
/// alone, without taking a `store` dependency for one enum.
pub use store::seed::BrandSeed;

/// The assembled shared host runtime.
///
/// Carries the configured [`AppState`], the resolved static-asset directory,
/// the deployment [`AppConfig`] (for the bind port), and the database handle
/// the binary closes at graceful shutdown.
pub struct HostRuntime {
    pub config: AppConfig,
    pub state: AppState,
    pub public_dir: PathBuf,
}

/// Build the shared Navigator host runtime from the process environment:
/// resolve config, connect + migrate + seed the database, configure object
/// storage and content, and assemble [`AppState`].
///
/// The caller owns telemetry init (a per-binary service name) and the
/// bind/serve loop; this function owns everything in between so the three brand
/// binaries cannot drift in how they assemble the application.
///
/// `brand_seed` names whose seeds this boot applies. It is [`Brand::seed`] for
/// a real binary; a caller assembling a runtime directly picks the brand whose
/// data it means to carry.
#[allow(clippy::too_many_lines)]
pub async fn build_from_env(brand_seed: store::seed::BrandSeed) -> anyhow::Result<HostRuntime> {
    let cfg = crate::AppConfig::from_env().context("loading AppConfig")?;
    // Branding is Neon Law by default; NAVIGATOR_CUSTOM_BRANDING opts into a
    // mounted white-label bundle (fail closed when set but absent).
    let brand_bundle = views::brand_bundle::BrandBundle::from_env()
        .context("resolving brand bundle from NAVIGATOR_CUSTOM_BRANDING")?;
    let branding = brand_bundle
        .as_ref()
        .map_or(&views::brand::DEFAULT_BRANDING, |bundle| {
            views::brand::Branding::from_manifest(&bundle.manifest)
        });
    views::brand::install_process_branding(branding)
        .map_err(anyhow::Error::msg)
        .context("installing process branding")?;
    tracing::info!(custom = brand_bundle.is_some(), "brand bundle configured");
    tracing::info!(
        environment = cfg.environment.as_str(),
        "configured deployment"
    );

    // Fail loud if a production deploy lacks Restate or GCS —
    // each silently degrades into a dev-only fallback that would lose
    // durability, allow-all every request, or persist client files
    // on a node-local filesystem.
    crate::config::enforce_deployment_invariants(cfg.environment, |k| std::env::var(k).ok())
        .context("deployment environment invariants")?;

    // The second store. Its schema is applied rather than migrated —
    // one idempotent `DEFINE` file converged on every boot, which is
    // what `store::schema::apply` documents itself for. There is no
    // fallback: `SurrealConfig::from_env` fails closed, so a missing
    // endpoint stops the boot here instead of surfacing later as a
    // password-reset link that cannot be minted.
    let surreal = store::surreal::connect_from_env()
        .await
        .context("connecting SurrealDB")?;
    store::schema::apply(&surreal)
        .await
        .context("applying the SurrealDB schema")?;
    tracing::info!(
        version = store::schema::SCHEMA_VERSION,
        "surreal schema applied"
    );

    // Object storage is created before the seed because template bodies
    // are now seeded into blob storage (not an inline column).
    let storage = cloud::from_env()
        .await
        .context("configuring object storage")?;
    tracing::info!("object storage configured");

    // The canonical seed writes template bodies as blobs to object storage.
    // In KIND the web pod can start before Garage is reachable, so
    // wait for the store to answer a probe before seeding — otherwise the
    // first seed fails on a connection error and the pod crash-loops (with a
    // growing backoff) until the dependency is up. That crash-loop window is
    // the root of the KIND e2e flake. The `fs` backend answers instantly, so
    // this is a no-op for local/`fs` dev.
    cloud::wait_until_ready(&storage, Duration::from_mins(1))
        .await
        .context("waiting for object storage to become ready")?;
    tracing::info!("object storage ready");

    // Public-assets lane: blank government forms are pulled from here at
    // fill/download time and verified against their repo `.sha256` pins.
    // A distinct bucket in prod (`NAVIGATOR_ASSETS_BUCKET`); the same
    // root as `storage` for the fs backend and single-bucket KIND.
    let assets_storage = cloud::assets_from_env()
        .await
        .context("configuring public-assets object storage")?;
    tracing::info!("assets object storage configured");

    // Applications lane: each Project's published client-portal bundle is
    // streamed from here at `/app/projects/{code}/portal`. A distinct
    // private bucket in prod (`NAVIGATOR_APPLICATIONS_BUCKET`); the same
    // root as `storage` for the fs backend and single-bucket KIND.
    let applications_storage = cloud::applications_from_env()
        .await
        .context("configuring Project-applications object storage")?;
    tracing::info!("applications object storage configured");

    // Every boot applies the canonical seed and the booting brand's own
    // seeds — both reach production. A `dev` boot additionally applies the
    // sample-matter fixture, which never does.
    let seed_report =
        store::seed::seed_environment(&surreal, &storage, cfg.environment, brand_seed)
            .await
            .context("seeding environment fixtures")?;
    tracing::info!(
        environment = cfg.environment.as_str(),
        brand = brand_seed.as_str(),
        summary = %seed_report.summary(),
        "seed applied"
    );

    let public_dir = std::env::var("NAVIGATOR_PUBLIC_DIR")
        .map_or_else(|_| PathBuf::from(crate::DEFAULT_PUBLIC_DIR), PathBuf::from);
    tracing::info!(?public_dir, "serving static assets");

    // Point `dioxus-server` at the built client bundle (issue #641). The
    // `navigator dev build-webapp` subcommand and `images/Containerfile.web`
    // build it into `<public_dir>/dioxus`; an explicit `DIOXUS_PUBLIC_PATH`
    // (set by tests or a custom layout) always wins. When neither the env nor
    // the default directory carries an `index.html`, the Dioxus demo page stays
    // absent (see `crate::dioxus_app`).
    if std::env::var_os("DIOXUS_PUBLIC_PATH").is_none() {
        let dioxus_dir = public_dir.join("dioxus");
        if dioxus_dir.join("index.html").is_file() {
            // Safe: set once at startup before any router build or worker
            // thread reads it.
            std::env::set_var("DIOXUS_PUBLIC_PATH", &dioxus_dir);
            tracing::info!(?dioxus_dir, "serving Dioxus client bundle");
        }
    }

    let workshops_dir = std::env::var("NAVIGATOR_WORKSHOPS_DIR").map_or_else(
        |_| PathBuf::from(crate::DEFAULT_WORKSHOPS_DIR),
        PathBuf::from,
    );
    let workshop_materials = crate::workshops::loader::load_navigator(&workshops_dir)
        .context("loading workshop content")?;
    tracing::info!(
        count = workshop_materials.len(),
        ?workshops_dir,
        "loaded workshop materials"
    );
    let workshops = crate::WorkshopIndex::new(workshop_materials);

    let blog_dir = std::env::var("NAVIGATOR_BLOG_DIR")
        .map_or_else(|_| PathBuf::from(crate::DEFAULT_BLOG_DIR), PathBuf::from);
    let blog = crate::blog::load_dir(&blog_dir).context("loading blog posts")?;
    tracing::info!(count = blog.posts().len(), ?blog_dir, "loaded blog posts");

    let auth = crate::AuthConfig::from_env().await;
    tracing::info!(enforced = auth.is_enforced(), "auth configured");

    let google_oauth = crate::google_oauth::GoogleOauthConfig::from_env();
    tracing::info!(
        enforced = google_oauth.is_enforced(),
        "google_oauth configured"
    );

    let canonical_host = crate::CanonicalHost::from_env();
    tracing::info!(
        enforced = canonical_host.is_enforced(),
        "canonical host configured"
    );

    let portal_only = crate::PortalOnly::new(
        brand_bundle
            .as_ref()
            .is_some_and(|bundle| bundle.manifest.portal_only),
    );
    tracing::info!(
        enabled = portal_only.enabled(),
        "portal-only mode configured"
    );

    let sessions = crate::SessionStore::from_env()
        .unwrap_or_else(|| crate::SessionStore::new(crate::session::random_token_32()));
    let oauth = crate::OAuthConfig::from_env()
        .await
        .context("loading OAuth config")?;
    tracing::info!(
        enabled = oauth.is_some(),
        "oauth (Authorization Code + PKCE) configured"
    );

    let policy =
        crate::policy::PolicyClient::embedded().context("compiling embedded Rego policy")?;
    tracing::info!("policy client configured");

    // Real DocuSign provider when the env is configured; otherwise the
    // stub (KIND / local dev). The inbound completion webhook
    // (`crate::esignature_webhook`) closes the loop in both cases — the
    // stub's synthetic ids still persist + correlate.
    let signature_provider: std::sync::Arc<dyn crate::signature::SignatureProvider> =
        crate::signature::DocuSignSignatureProvider::from_env().map_or_else(
            || {
                tracing::info!("signature provider: StubSignatureProvider (DOCUSIGN_* unset)");
                std::sync::Arc::new(crate::signature::StubSignatureProvider::new())
                    as std::sync::Arc<dyn crate::signature::SignatureProvider>
            },
            |ds| {
                tracing::info!("signature provider: DocuSignSignatureProvider");
                std::sync::Arc::new(ds)
            },
        );

    // Real Xero provider when the env is configured; otherwise the stub
    // (KIND / local dev), so a fork boots and self-tests without a Xero
    // custom connection. Mirrors the signature-provider wiring above.
    let billing_provider: std::sync::Arc<dyn crate::billing::BillingProvider> =
        crate::billing::XeroBillingProvider::from_env().map_or_else(
            || {
                tracing::info!("billing provider: StubBillingProvider (XERO_* unset)");
                std::sync::Arc::new(crate::billing::StubBillingProvider::new())
                    as std::sync::Arc<dyn crate::billing::BillingProvider>
            },
            |xero| {
                tracing::info!("billing provider: XeroBillingProvider");
                std::sync::Arc::new(xero)
            },
        );

    let esignature_webhook_secret = std::env::var("DOCUSIGN_WEBHOOK_SECRET")
        .ok()
        .filter(|s| !s.is_empty());
    let esignature_hmac_key = std::env::var("DOCUSIGN_HMAC_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    tracing::info!(
        path_secret = esignature_webhook_secret.is_some(),
        hmac_key = esignature_hmac_key.is_some(),
        "e-signature webhook auth configured"
    );

    let email = crate::email::from_env(surreal.clone()).context("loading email config")?;
    tracing::info!("email backend configured");
    let clamd_addr = std::env::var("NAVIGATOR_CLAMD_ADDR")
        .ok()
        .filter(|value| !value.is_empty())
        .context("NAVIGATOR_CLAMD_ADDR must name the fail-closed attachment scanner")?;
    let attachment_scanner: std::sync::Arc<dyn crate::attachment_scanner::AttachmentScanner> =
        std::sync::Arc::new(crate::attachment_scanner::ClamdAttachmentScanner::new(
            clamd_addr,
        ));
    tracing::info!("attachment scanner configured");

    // Runtime selection: if `RESTATE_BROKER_URL` is set in the
    // environment, the `web` binary talks to the in-cluster
    // `workflows-service` worker through Restate. Otherwise we fall
    // back to the in-process `InMemoryRuntime` *wrapped in
    // `DispatchingRuntime`* — without that wrap the local dev binary
    // never fires the welcome email (the `email_send__*` step has no
    // worker to consume it).
    //
    // Both implement `StateMachineRuntime`; the workflow and
    // questionnaire timelines share a single runtime instance keyed
    // by `(MachineKind, notation_id)`.
    let (workflow_runtime, questionnaire_runtime): (
        std::sync::Arc<dyn workflows::StateMachineRuntime>,
        std::sync::Arc<dyn workflows::StateMachineRuntime>,
    ) = if std::env::var("RESTATE_BROKER_URL").is_ok() {
        let rt = std::sync::Arc::new(workflows::RestateRuntime::from_env());
        tracing::info!("runtime: RestateRuntime (RESTATE_BROKER_URL is set)");
        (rt.clone(), rt)
    } else {
        let inner: std::sync::Arc<dyn workflows::StateMachineRuntime> =
            std::sync::Arc::new(workflows::InMemoryRuntime::new());
        let workflow = std::sync::Arc::new(
            workflows::DispatchingRuntime::new(inner.clone(), email.clone(), storage.clone())
                .with_store(surreal.clone()),
        );
        tracing::info!(
            "runtime: DispatchingRuntime<InMemoryRuntime> (RESTATE_BROKER_URL unset; dispatches \
             email_send__* steps in-process through the EmailService)"
        );
        (workflow, inner)
    };

    let inbound_email_secret = std::env::var("SENDGRID_INBOUND_SECRET")
        .ok()
        .filter(|s| !s.is_empty());
    tracing::info!(
        configured = inbound_email_secret.is_some(),
        "inbound email webhook secret configured"
    );

    let email_events_secret = std::env::var("SENDGRID_EVENTS_SECRET")
        .ok()
        .filter(|s| !s.is_empty());
    tracing::info!(
        configured = email_events_secret.is_some(),
        "email events webhook secret configured"
    );

    let sendgrid_events_public_key = std::env::var("SENDGRID_EVENTS_PUBLIC_KEY")
        .ok()
        .filter(|s| !s.is_empty());
    tracing::info!(
        configured = sendgrid_events_public_key.is_some(),
        "email events webhook signature verification configured"
    );

    let state = crate::AppState {
        brand_bundle,
        surreal,
        workshops,
        docs: crate::docs::loader::bundled(),
        blog,
        auth,
        google_oauth,
        rate_limit: crate::rate_limit::RateLimit::from_env(),
        canonical_host,
        portal_only,
        sessions,
        oauth,
        storage,
        assets_storage,
        applications_storage,
        forms_registry: std::sync::Arc::new(
            forms::registry().context("loading the vendored forms registry")?,
        ),
        policy,
        workflow_runtime,
        questionnaire_runtime,
        signature_provider,
        billing_provider,
        // Inbound-contract reviewer: Vertex Gemini when configured, else
        // the deterministic stub — selected here exactly like the A2A
        // router (chosen inside `bootstrap`). The
        // `analysis__contract_deviations` step is web-driven; the worker
        // has no LLM access.
        contract_reviewer: crate::contract_review::GeminiContractReviewer::from_env().map_or_else(
            || -> std::sync::Arc<dyn crate::contract_review::ContractReviewer> {
                std::sync::Arc::new(crate::contract_review::StubContractReviewer)
            },
            |r| std::sync::Arc::new(r),
        ),
        esignature_webhook_secret,
        esignature_hmac_key,
        email,
        attachment_scanner,
        inbound_email_secret,
        email_events_secret,
        sendgrid_events_public_key,
        bootstrap_owner_email: crate::oauth::bootstrap_owner_email_from_env(),
        self_signup_enabled: crate::oauth::self_signup_enabled_from_env(),
        // Opt-in email/password sign-in via GCP Identity Platform; `None`
        // unless `NAVIGATOR_IDENTITY_PLATFORM_API_KEY` is set.
        identity_password: crate::oauth::IdentityPasswordConfig::from_env(),
        // Opt-in admin door (password reset + email confirm); `None`
        // unless `NAVIGATOR_GCP_PROJECT_ID` is set.
        identity_admin: crate::idp_admin::IdentityAdminConfig::from_env(),
        // Production picks the router from env inside `bootstrap`
        // (Gemini when configured, else Null); no override here.
        a2a_router: None,
    };

    Ok(HostRuntime {
        config: cfg,
        state,
        public_dir,
    })
}

/// Resolve when the process receives `SIGINT` (Ctrl-C) or `SIGTERM`.
///
/// Kubernetes sends `SIGTERM` at pod termination; every host binary awaits this
/// so `axum::serve`'s graceful shutdown drains in-flight requests and flushes
/// batched telemetry before exit.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = term => {}
    }
}

/// Builds a brand's public Dioxus SSR routers once the runtime exists.
///
/// A closure rather than a plain value because the content-backed pages read
/// [`AppState`], which does not exist until [`build_from_env`] has run.
pub type PublicDioxusRouters = Box<dyn FnOnce(&AppState) -> Vec<Router> + Send>;

/// One public face, declared by the brand binary that serves it.
///
/// This is the whole of a brand crate: a name to report telemetry under and
/// the two halves of its public surface. Everything else — the database, the
/// authenticated application, the JSON API, the anonymous-access boundary —
/// comes from the application crate, identically for every brand, so a brand
/// cannot fork authorization by construction.
pub struct Brand {
    /// The brand's short key (`neon`). Labels the boot log,
    /// so a deploy names the face it serves in its first lines.
    pub key: &'static str,
    /// Which brand's own seeds this deployment applies. Unlike the
    /// sample-matter fixture, this layer reaches production, so
    /// it carries the data one brand owns and the other must not — the
    /// Firm's postal identities against the Foundation's.
    pub seed: store::seed::BrandSeed,
    /// The telemetry `service.name` this deployment reports under. It names
    /// the deployment rather than the process, so a trace says which face it
    /// came from.
    pub service_name: &'static str,
    /// Whether this host publishes no public marketing surface at all — the
    /// white-label tenant shape, which lights [`crate::PortalOnly`].
    pub portal_only: bool,
    /// The brand's public route table, mounted outside the session boundary.
    pub public_routes: Router<AppState>,
    /// Every path the brand registers across its public and Dioxus
    /// routers. [`crate::bootstrap`] rejects any entry at or below a
    /// Navigator-owned prefix before Axum composes the routers.
    pub public_paths: &'static [&'static str],
    /// The brand's public Dioxus SSR routers. A brand that supplies none here
    /// 404s its own home page whenever its real pages are Dioxus ports.
    pub public_dioxus: PublicDioxusRouters,
}

/// Boot one brand: telemetry, the shared runtime, that brand's composition,
/// bind, serve, drain.
///
/// This is the entire body of a brand binary's `main`. The ordering it fixes
/// is load-bearing — `.env` before any environment read, telemetry before the
/// first `tracing` call, and the Dioxus routers built from `state` before
/// `state` moves into [`crate::bootstrap`] — and stating it once is what keeps
/// three brand binaries from drifting the way the pre-#860 host pair did.
///
/// # Errors
///
/// Propagates a failure to assemble the runtime (configuration, database,
/// storage, content) or to bind and serve the configured port.
pub async fn run(brand: Brand) -> anyhow::Result<()> {
    // Load `.env` / `.devx/env` before any env read; in cluster the
    // environment is injected and both calls are no-ops.
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_path(".devx/env");

    // One observability seam per binary, named for the deployment.
    let telemetry_guard = telemetry::init(brand.service_name);

    let mut rt = build_from_env(brand.seed).await?;

    // The tenant shape lights the existing portal-only mode rather than adding
    // a second way to express "no public surface". A mounted brand manifest
    // can still enable it on its own, and a brand never turns it *off*, so a
    // portal-only bundle stays portal-only whatever binary serves it.
    if brand.portal_only {
        rt.state.portal_only = crate::PortalOnly::new(true);
    }

    preflight(&brand, &rt);

    // Start keeping the footer's GitHub star count current. One background
    // task per process, refreshing on an interval, so the count a page render
    // reads is a cache hit and never an outbound call — see
    // `webapp::source_repository`. Deliberately spawned here rather than
    // lazily on first render: a test that builds a router directly never
    // reaches this line, which is what keeps the suite off the network.
    webapp::source_repository::spawn_refresh();

    // Built before `rt.state` moves into `bootstrap`: the public pages are
    // Dioxus routers resolved from state, and a brand that passed none here
    // would 404 its own home page.
    let host_dioxus = (brand.public_dioxus)(&rt.state);
    let router = crate::bootstrap(
        rt.state,
        &rt.public_dir,
        brand.public_routes,
        brand.public_paths,
        host_dioxus,
    )?;

    let addr = SocketAddr::from(([0, 0, 0, 0], rt.config.port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, brand = brand.key, "server listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;
    telemetry_guard.shutdown();
    Ok(())
}

/// Report the resolved brand at boot, so a deploy serving the wrong face says
/// so in its first log lines rather than at the first request.
fn preflight(brand: &Brand, rt: &HostRuntime) {
    tracing::info!(
        brand = brand.key,
        service_name = brand.service_name,
        portal_only = rt.state.portal_only.enabled(),
        environment = rt.config.environment.as_str(),
        "brand configured"
    );
}
