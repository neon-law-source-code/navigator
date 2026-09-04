//! Manual queueing for the Cron schedules lawyer page.
//!
//! Every scheduled workflow is started through this module so the operations
//! page, rather than the lawyer dashboard, is the one place to run it on
//! demand. A manual invocation always uses a unique key: an operator's
//! explicit run must not be deduplicated against the schedule's date key.

use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use uuid::Uuid;

struct ManualJob {
    slug: &'static str,
    service: &'static str,
}

const MANUAL_JOBS: &[ManualJob] = &[
    ManualJob {
        slug: "archives",
        service: "Archives",
    },
    ManualJob {
        slug: "billing-canary",
        service: "BillingCanary",
    },
    ManualJob {
        slug: "billing-digest",
        service: "BillingDigest",
    },
    ManualJob {
        slug: "heartbeat",
        service: "Heartbeat",
    },
    ManualJob {
        slug: "reconcile-invoices",
        service: "ReconcileInvoices",
    },
];

/// `POST /app/admin/schedules/:job/run` — queue one known CronJob workflow.
pub async fn run(Path(slug): Path<String>) -> Response {
    let broker = restate_broker_url();
    let token = std::env::var("RESTATE_AUTH_TOKEN").ok();
    run_configured(&slug, broker.as_deref(), token.as_deref()).await
}

async fn run_configured(slug: &str, broker: Option<&str>, token: Option<&str>) -> Response {
    let Some(job) = MANUAL_JOBS.iter().find(|job| job.slug == slug) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(broker) = broker else {
        return redirect_with_notice(job.slug, "not_queued");
    };
    let run_key = format!("manual-{}", Uuid::new_v4());
    match trigger(broker, token, job, &run_key).await {
        Ok(response) => {
            tracing::info!(
                job = job.slug,
                service = job.service,
                run_key,
                response,
                "manual cron job queued"
            );
            redirect_with_notice(job.slug, "queued")
        }
        Err(error) => {
            tracing::error!(job = job.slug, service = job.service, run_key, %error, "manual cron job was not queued");
            redirect_with_notice(job.slug, "not_queued")
        }
    }
}

fn redirect_with_notice(slug: &str, outcome: &str) -> Response {
    Redirect::to(&format!("/app/admin/schedules?notice={outcome}:{slug}")).into_response()
}

async fn trigger(
    broker: &str,
    token: Option<&str>,
    job: &ManualJob,
    run_key: &str,
) -> Result<String, workflows::TriggerError> {
    workflows::start_workflow(
        broker,
        token,
        job.service,
        run_key,
        "run",
        &serde_json::json!({}),
        true,
    )
    .await
}

/// The Restate ingress base URL from `RESTATE_BROKER_URL`, trimmed, or `None`
/// when unset/empty (dev, local, and tests).
pub(crate) fn restate_broker_url() -> Option<String> {
    normalize_broker_url(std::env::var("RESTATE_BROKER_URL").ok())
}

fn normalize_broker_url(url: Option<String>) -> Option<String> {
    url.map(|url| url.trim_end_matches('/').to_string())
        .filter(|url| !url.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use axum::{
        extract::Path,
        http::{header::LOCATION, StatusCode},
    };

    use super::{normalize_broker_url, run, run_configured, trigger, MANUAL_JOBS};
    use wiremock::{
        matchers::{body_partial_json, header, method, path_regex},
        Mock, MockServer, ResponseTemplate,
    };

    fn location(response: &axum::response::Response) -> &str {
        response
            .headers()
            .get(LOCATION)
            .expect("redirect location")
            .to_str()
            .expect("ASCII redirect location")
    }

    #[test]
    fn every_manual_control_maps_to_a_known_workflow() {
        assert_eq!(MANUAL_JOBS.len(), 5);
        assert!(MANUAL_JOBS
            .iter()
            .all(|job| !job.slug.is_empty() && !job.service.is_empty()));
    }

    #[tokio::test]
    async fn trigger_queues_the_selected_workflow_with_a_manual_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/BillingCanary/manual-[0-9a-f-]+/run/send$"))
            .and(body_partial_json(serde_json::json!({})))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("{\"invocationId\":\"inv_x\"}"),
            )
            .mount(&server)
            .await;
        let job = MANUAL_JOBS
            .iter()
            .find(|job| job.slug == "billing-canary")
            .unwrap();
        let body = trigger(
            &server.uri(),
            None,
            job,
            "manual-4dc99f6d-55a4-4408-b8a0-0df5a8437f6e",
        )
        .await
        .expect("ingress accepts invocation");
        assert!(body.contains("inv_x"));
    }

    #[tokio::test]
    async fn run_covers_unknown_unavailable_accepted_and_rejected_jobs() {
        let response = run(Path("unknown".to_string())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = run_configured("archives", None, None).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            location(&response),
            "/app/admin/schedules?notice=not_queued:archives"
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex(r"^/BillingCanary/manual-[0-9a-f-]+/run/send$"))
            .and(header("authorization", "Bearer secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("accepted"))
            .expect(1)
            .mount(&server)
            .await;
        let broker =
            normalize_broker_url(Some(format!("{}/", server.uri()))).expect("non-empty broker URL");

        let response = run_configured("billing-canary", Some(&broker), Some("secret")).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            location(&response),
            "/app/admin/schedules?notice=queued:billing-canary"
        );

        let rejecting_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("unavailable"))
            .expect(1)
            .mount(&rejecting_server)
            .await;

        let response = run_configured("billing-digest", Some(&rejecting_server.uri()), None).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            location(&response),
            "/app/admin/schedules?notice=not_queued:billing-digest"
        );

        assert_eq!(
            normalize_broker_url(Some("https://restate.example.com////".to_string())).as_deref(),
            Some("https://restate.example.com")
        );
        assert!(normalize_broker_url(Some("///".to_string())).is_none());
        assert!(normalize_broker_url(None).is_none());
    }

    #[test]
    fn every_run_now_button_the_console_renders_has_a_handler_here() {
        // `webapp::schedules` derives its rows from the deployment render, so a
        // manifest landing in the exports kustomization puts a job on the page
        // without touching this file. A row carrying a slug this dispatch table
        // does not know would render a button that answers 404, so bind the two
        // rather than trusting them to be edited together.
        for job in webapp::schedules::cron_jobs() {
            let Some(slug) = job.slug else { continue };
            assert!(
                MANUAL_JOBS.iter().any(|manual| manual.slug == slug),
                "the console offers `Run now` for {} (slug {slug}), which no MANUAL_JOBS entry starts",
                job.name
            );
        }
    }
}
