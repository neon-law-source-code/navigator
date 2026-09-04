//! Worker entry point. Opens the shared store connection,
//! builds the worker's `EmailService` (bare `SendGrid` in prod,
//! `CapturingEmail` otherwise), wires the `Notation` virtual-object
//! endpoint, and listens on the port the Restate broker discovers
//! via `restate-cli register`.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use archives::workflow::ArchivesService;
use billing_workflows::canary::BillingCanaryService;
use billing_workflows::digest::BillingDigestService;
use billing_workflows::reconcile::ReconcileInvoicesService;
use github_webhooks::authority::is_automation_home;
use github_webhooks::guardrails::{GitHubGuardrailsService, GuardrailConfig};
use github_webhooks::worker::{
    DevxIssueTriageService, DevxPrService, RepositoryResolver, UnconfiguredRepositoryResolver,
};
use restate_sdk::prelude::*;
use workflows::{EmailService, SlackOpsDelivery};
use workflows_service::dri_digest::DriDigestService;
use workflows_service::github_automation_heartbeat::GitHubAutomationHeartbeatService;
use workflows_service::heartbeat::HeartbeatService;
use workflows_service::request_identity::apply_identity_key;
use workflows_service::{
    email_from_env, notifier_from_env, project_slack::ProjectSlackService,
    repository_correlation::ProjectRepositoryResolver, slack_bot_from_env, NotationService,
};

macro_rules! bind_common_services {
    ($endpoint:expr, $surreal:expr, $email:expr, $storage:expr, $notifier:expr, $ops_delivery:expr, $slack_bot:expr) => {
        $endpoint
            .bind(NotationService::new(
                $surreal.clone(),
                $email.clone(),
                $storage,
            ))
            .bind(ProjectSlackService::new(
                $surreal.clone(),
                $slack_bot.clone(),
            ))
            .bind(ArchivesService::new($notifier.clone()))
            .bind(HeartbeatService::new($notifier.clone()))
            .bind(BillingCanaryService::new($ops_delivery.clone()))
            .bind(BillingDigestService::new($ops_delivery))
            .bind(ReconcileInvoicesService::new($surreal.clone()))
            .bind(DriDigestService::new($surreal, $notifier))
    };
}

#[tokio::main]
#[allow(clippy::too_many_lines)] // Typed Restate endpoint branches must stay beside startup wiring.
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    // `.devx/env` overlay for local KIND iteration (port-forward URLs,
    // dev OAuth secrets). Loaded second so `.env` always wins.
    let _ = dotenvy::from_path(".devx/env");
    // One observability seam for every binary: stdout logs (JSON when an
    // OTLP endpoint is set) plus OTLP traces + metrics. Held to end of main
    // so the drop flushes any batched export before the process exits.
    let _telemetry = telemetry::init("navigator-workflows-service");

    let environment =
        store::DeploymentEnvironment::from_env().context("parse NAVIGATOR_ENVIRONMENT")?;
    let endpoint_builder = apply_identity_key(Endpoint::builder(), environment, |key| {
        std::env::var(key).ok()
    })
    .map_err(anyhow::Error::msg)
    .context("configure Restate request identity")?;
    // Branding is Neon Law by default; NAVIGATOR_CUSTOM_BRANDING opts into a
    // mounted white-label bundle (fail closed when set but absent).
    let brand_bundle = views::brand_bundle::BrandBundle::from_env()
        .context("resolve brand bundle from NAVIGATOR_CUSTOM_BRANDING")?;
    let branding = brand_bundle
        .as_ref()
        .map_or(&views::brand::DEFAULT_BRANDING, |bundle| {
            views::brand::Branding::from_manifest(&bundle.manifest)
        });
    views::brand::install_process_branding(branding)
        .map_err(anyhow::Error::msg)
        .context("install process branding")?;
    tracing::info!(environment = environment.as_str(), "configured deployment");
    workflows_service::email_config::validate_for_deployment(environment, |key| {
        std::env::var(key).ok()
    })
    .context("validate deployment email configuration")?;
    // The email `@font-face` block renders here, not in `web`, so the asset
    // origin must be validated in this binary too — a malformed value would
    // otherwise reach the outbound-mail `<style>` block un-checked.
    workflows_service::asset_config::validate_for_deployment(|key| std::env::var(key).ok())
        .context("validate deployment asset base URL")?;

    let surreal = store::surreal::connect_from_env()
        .await
        .context("connect to SurrealDB")?;
    let repository_resolver: Arc<dyn RepositoryResolver> = match (
        std::env::var("NAVIGATOR_GITHUB_CANONICAL_REPOSITORY"),
        std::env::var("NAVIGATOR_GITHUB_ORG"),
    ) {
        (Ok(canonical_repository), Ok(project_owner)) => Arc::new(ProjectRepositoryResolver::new(
            surreal.clone(),
            canonical_repository,
            project_owner,
        )),
        _ => Arc::new(UnconfiguredRepositoryResolver),
    };

    let email = email_from_env().context("build email service from env")?;
    tracing::info!(
        backend = if std::env::var("NAVIGATOR_EMAIL_BACKEND").as_deref() == Ok("sendgrid") {
            "SendGrid"
        } else {
            "Capturing"
        },
        "workflows-service email backend"
    );

    // Internal ops notifications deliver to the engineering channel through a
    // Slack incoming webhook. `BillingCanary` and `BillingDigest` route plain
    // text through `SlackOpsDelivery`, which fences the message in a code block. `Archives` and `Heartbeat` post mrkdwn directly to the
    // notifier so links and compact status formatting render correctly.
    // The client-facing Notation service uses the email backend, keeping
    // client content inside the client delivery channel.
    let notifier = notifier_from_env();
    let slack_bot = slack_bot_from_env();
    tracing::info!(
        backend = if workflows_service::notify_config::slack_enabled(|k| std::env::var(k).ok()) {
            "Slack"
        } else {
            "Capturing"
        },
        "workflows-service ops-notification backend"
    );
    let ops_delivery: Arc<dyn EmailService> = Arc::new(SlackOpsDelivery::new(notifier.clone()));

    // Object storage for `generate_pdf__*` step dispatch (the worker
    // renders the PDF and persists it here). Same `cloud::from_env`
    // backend selection as `web`: GCS in prod, FsStorage in dev.
    let storage = cloud::from_env()
        .await
        .context("configure object storage")?;

    let listen: SocketAddr = std::env::var("WORKFLOWS_SERVICE_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:9080".into())
        .parse()
        .context("parse WORKFLOWS_SERVICE_LISTEN")?;

    tracing::info!(%listen, "workflows-service listening");

    // One endpoint hosts every workflow: the `Notation` virtual object and
    // the `Archives` nightly-export, `Heartbeat`
    // durable-execution liveness canary, `BillingCanary`, `BillingDigest`
    // (daily GCP cost email), `ReconcileInvoices`, and `DriDigest` (nightly
    // project-DRI Slack notice) workflows, each with a thin `*-trigger`
    // CronJob. All run against this one worker — there is no per-workflow
    // pod. The exact set of service names bound here is mirrored in
    // `workflows_service::registry`, which the registry tests guard against
    // drift (count + PascalCase naming).
    let github_automation_home =
        is_automation_home(std::env::var("NAVIGATOR_GCP_PROJECT_ID").ok().as_deref());
    tracing::info!(
        github_automation_home,
        "configured GitHub automation authority"
    );

    let server = if github_automation_home {
        let guardrails =
            GuardrailConfig::from_env().context("read GitHub automation spending guardrails")?;
        HttpServer::new(
            bind_common_services!(
                endpoint_builder
                    // GitHub webhook durable notices (folded in from the former
                    // standalone DevX worker): the receiver submits only in the
                    // authoritative automation-home deployment.
                    .bind(DevxIssueTriageService::new(
                        notifier.clone(),
                        repository_resolver.clone(),
                    ))
                    .bind(DevxPrService::new(notifier.clone(), repository_resolver))
                    .bind(GitHubGuardrailsService::new(guardrails))
                    .bind(GitHubAutomationHeartbeatService::new(notifier.clone())),
                surreal,
                email,
                storage,
                notifier,
                ops_delivery,
                slack_bot
            )
            .build(),
        )
        .listen_and_serve(listen)
    } else {
        HttpServer::new(
            bind_common_services!(
                endpoint_builder,
                surreal,
                email,
                storage,
                notifier,
                ops_delivery,
                slack_bot
            )
            .build(),
        )
        .listen_and_serve(listen)
    };

    // The GitHub webhook receiver runs on its own Axum listener beside the
    // Restate endpoint: `www.<domain>` goes behind the tailnet, so the receiver
    // moves to the public `workflows.<domain>` host, where Envoy routes
    // `/webhooks/github/*` here and everything else to the Restate leg. Present
    // only on the automation-home deployment (`receiver_from_env` is `None`
    // otherwise). Bind eagerly so a port conflict fails the boot rather than
    // silently dropping webhooks; then serve it beside the Restate endpoint.
    if let Some(router) = workflows_service::webhook::receiver_from_env() {
        let addr = workflows_service::webhook::webhook_listen_addr(|key| std::env::var(key).ok())?;
        let webhook_listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("bind GitHub webhook receiver on {addr}"))?;
        tracing::info!(%addr, "github webhook receiver listening");
        tokio::spawn(async move {
            if let Err(error) = axum::serve(webhook_listener, router).await {
                tracing::error!(%error, "github webhook receiver stopped");
            }
        });
    }

    server.await;

    Ok(())
}
