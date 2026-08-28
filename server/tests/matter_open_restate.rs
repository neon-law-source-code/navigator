#![allow(clippy::doc_markdown)]
//! Restate-backed regression for #377: opening a matter whose first
//! notation starts a workflow must land on the matter, not a 500.
//!
//! The other matter-open coverage (`matter_open_retainer`) drives the flow
//! through the in-process `InMemoryRuntime`, which cannot exhibit the bug this
//! guards: it needs host `web` and the in-cluster `workflows-service` worker
//! to share one store, so that the worker's `append-event` journal step finds
//! the Notation `web` has just committed. Against separate databases the step
//! raises `RecordNotFound` and the matter-open path returns a 500.
//!
//! The on-ramp is the engagement letter walk (`POST /lawyer/retainers/new`
//! with `onboarding__letter`), which starts its workflow in the create request
//! itself — the same "commit a Notation, then
//! journal it through the worker" shape that exhibits #377. `POST
//! /app/projects` no longer fires a workflow at all (opening a matter and
//! opening its retainer are two steps; see the glossary's Engagement /
//! Retainer entry), so it can no longer carry this guard.
//!
//! This test exercises the **real** `RestateRuntime` against the in-cluster
//! worker, with host `web` pointed at the **same** shared `navigator` database
//! the worker is pinned to (the `dev up` topology). It is the agreement half
//! of the guard: the `worktree-env status` / `ops doctor` check
//! (`cli::devx::restate_db`) warns when they disagree; this proves the flow
//! works when they agree.
//!
//! ## Harness (skips cleanly without it — CI stays green)
//!
//! Needs the KIND fixture: the Restate broker and a **registered**
//! `workflows-service` worker, both from `dev up`. Enable by exporting:
//!
//! ```text
//! RESTATE_BROKER_URL=http://localhost:9080   # port-forwarded Restate ingress
//! ```
//!
//! With `NAV_REQUIRE_HARNESS=1` a missing broker is a hard failure (CI
//! can't self-skip green); unset, it skips.

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use workflows::StateMachineRuntime;

/// The Restate ingress URL when the KIND harness is up, or `None` (with a skip
/// note) when it isn't — unless `NAV_REQUIRE_HARNESS=1`, which turns a missing
/// broker into a hard failure so a self-skip can't pass green in CI.
fn restate_broker_or_skip() -> Option<String> {
    let broker = std::env::var("RESTATE_BROKER_URL")
        .ok()
        .filter(|s| !s.is_empty());
    match (broker, require_harness()) {
        (Some(url), _) => Some(url),
        (None, true) => panic!(
            "NAV_REQUIRE_HARNESS=1 but RESTATE_BROKER_URL is unset: bring up the KIND fixture \
             (`navigator dev up`) and export the port-forwarded Restate ingress."
        ),
        (None, false) => {
            eprintln!(
                "skipping matter_open_restate: RESTATE_BROKER_URL unset (no Restate harness). \
                 Run `navigator dev up` and export RESTATE_BROKER_URL to exercise it."
            );
            None
        }
    }
}

fn require_harness() -> bool {
    std::env::var("NAV_REQUIRE_HARNESS").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

fn admin_bearer() -> String {
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let mut session = portal::SessionData::fresh("admin@neonlaw.com", store::persons::Role::Admin);
    session.source = portal::session::SessionSource::Cli;
    format!("Bearer {}", sessions.encode(&session))
}

/// URL-encode the couple of characters these form values carry.
fn enc(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

async fn post_retainer_walk(app: &axum::Router, body: String) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/lawyer/retainers/new")
                .header("authorization", admin_bearer())
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn matter_open_starts_its_workflow_through_the_restate_worker() {
    let Some(broker) = restate_broker_or_skip() else {
        return;
    };
    std::env::set_var("RESTATE_BROKER_URL", &broker);

    // Matter open hard-depends on repo provisioning; give it a scratch root.
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-matter-open-restate-repos-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    // The real Restate runtime for both timelines — the worker owns the
    // `notation_events` journal, reached through the port-forwarded ingress.
    let runtime: Arc<dyn StateMachineRuntime> = Arc::new(workflows::RestateRuntime::from_env());
    // The env-named person store, the same one the running `web` reads —
    // this lane drives the deployed Restate broker, not an in-process
    // fixture, so both stores have to be the deployment's.
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the deployed person store");
    let mut state = portal::test_support::app_state(surreal.clone()).await;
    store::seed::seed_canonical(&surreal, &state.storage)
        .await
        .expect("seed the reset staging database");
    state.workflow_runtime = runtime.clone();
    state.questionnaire_runtime = runtime;
    let app = server::neon_router(state, Path::new(portal::DEFAULT_PUBLIC_DIR));

    // A fresh client each run keeps the walk independent of prior state.
    let suffix = uuid::Uuid::now_v7();
    let client_email = format!("libra-{suffix}@example.com");

    // The estate retainer is transcript-driven, so `start_post` starts its
    // workflow inside this request: web commits the Notation, then the
    // worker journals `append-event` against its own database. If the two
    // disagree, that journal step raises `RecordNotFound` → 500. This is
    // the #377 bug, reproduced through the door that still fires a workflow
    // at matter open.
    let body = format!(
        "client_email={}&retainer_template_code=onboarding__letter",
        enc(&client_email),
    );
    let resp = post_retainer_walk(&app, body).await;
    let status = resp.status();
    let loc = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();

    // The exact #377 failure: a 500 here means the worker journaled against a
    // database that does not hold the Notation web just committed.
    assert_ne!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "matter-open 500'd — web and the workflows-service worker are not on the same database \
         (the #377 RecordNotFound bug)"
    );
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "matter-open should redirect to the matter"
    );
    assert!(
        loc.starts_with("/app/projects/"),
        "expected a redirect to /app/projects/:code, got {loc:?}"
    );

    // The workflow really started: the Notation exists on the new matter,
    // journaled through the worker rather than merely inserted by web.
    let project_code = loc.trim_start_matches("/app/projects/").to_string();
    let project_id = store::projects::find_by_code(&surreal, &project_code)
        .await
        .unwrap()
        .expect("redirect carries a project code")
        .id;
    let notation = store::notations::list_by_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the estate retainer notation was created");
    assert_eq!(notation.state, "BEGIN");
}
