#![allow(clippy::doc_markdown)]
//! Restate-backed regression for #377: a door that commits a Notation and
//! then synchronously starts + drives its Restate workflow in the same
//! request must not 500.
//!
//! The other contract-review coverage (`contract_review_pipeline`) drives
//! the same pipeline through the in-process `DispatchingRuntime`, which
//! cannot exhibit the bug this guards: it needs host `web` and the
//! in-cluster `workflows-service` worker to share one store, so that the
//! worker's `append-event` journal step finds the Notation `web` has just
//! committed. Against separate databases the step raises `RecordNotFound`
//! and the pipeline call returns a runtime error instead of landing at
//! `lawyer_review`.
//!
//! The on-ramp used to be the estate retainer walk (`POST
//! /app/lawyer/retainers/new` with `onboarding__estate`): that door was
//! transcript-driven, so `start_post` started its workflow inside the create
//! request itself. `portal::estate` and the transcript-intake pipeline were
//! removed as dead code (no surviving template has a `transcript_uploaded`
//! edge out of `BEGIN`), and `start_post` no longer starts a workflow for
//! any template — every retainer walk now hands off to the stepwise
//! questionnaire walker instead. The inbound contract-review pipeline
//! ([`portal::contract_review_walk::drive_contract_review`]) is the
//! surviving door with the same shape: it creates the Notation, then in the
//! same call fires `StateMachineRuntime::start` and three signals
//! (`contract_uploaded`, `intake_filed`, `analysis_ready`) against the just-
//! committed row — exactly the "commit a Notation, then journal it through
//! the worker" sequence #377 needs. It is also the one other
//! `StateMachineRuntime::start` call site (besides the two
//! `retainer_walk.rs` sites, `advance_to_lawyer_review` and
//! `drive_closing_workflow`) that opens and starts a workflow for a Notation
//! in one call rather than across two separate requests.
//!
//! This test exercises the **real** `RestateRuntime` against the in-cluster
//! worker, with host `web` pointed at the **same** shared `navigator`
//! database the worker is pinned to (the `dev up` topology). It is the
//! agreement half of the guard: the `worktree-env status` / `ops doctor`
//! check (`cli::devx::restate_db`) warns when they disagree; this proves the
//! flow works when they agree.
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

use std::sync::Arc;

use uuid::Uuid;

use portal::contract_review_walk::{drive_contract_review, ReviewDeps};
use store::playbooks::{NewPlaybook, Position};
use workflows::{IntakeArtifact, RestateRuntime};

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

/// A fixed, deterministic playbook — the deviation analysis needs one on
/// file for the contract's Entity before it will run.
fn sample_positions() -> Vec<Position> {
    vec![Position {
        topic: "Limitation of liability".into(),
        preferred: "Mutual cap at 12 months' fees".into(),
        fallback: "Cap at 2x fees paid".into(),
        walkaway: "Uncapped liability".into(),
        severity: store::playbooks::SEVERITY_HIGH.into(),
    }]
}

/// Seed a fresh Entity + Project + client Person against the shared store,
/// returning `(project_id, person_id, entity_id)`. A fresh suffix each run
/// keeps the pipeline independent of prior state in the reused staging
/// database.
async fn seed_matter(surreal: &store::surreal::SurrealDb) -> (Uuid, Uuid, Uuid) {
    let suffix = Uuid::now_v7();
    let entity_id = store::test_support::seed_entity(surreal).await;
    let project_id = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("contract-review-restate-{suffix}"),
            name: "Contract review (Restate regression)".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .expect("create project")
    .id;
    let person_id = store::persons::create(
        surreal,
        &store::persons::NewPerson::new("Libra", format!("libra-{suffix}@example.com")),
    )
    .await
    .expect("create client person")
    .id;
    (project_id, person_id, entity_id)
}

#[tokio::test]
async fn contract_review_starts_its_workflow_through_the_restate_worker() {
    let Some(broker) = restate_broker_or_skip() else {
        return;
    };
    std::env::set_var("RESTATE_BROKER_URL", &broker);

    // The real Restate runtime — the worker owns the `notation_events`
    // journal, reached through the port-forwarded ingress.
    let runtime = RestateRuntime::from_env();
    // The env-named person store, the same one the running `web` reads —
    // this lane drives the deployed Restate broker, not an in-process
    // fixture, so both stores have to be the deployment's.
    let surreal = store::surreal::connect_from_env()
        .await
        .expect("connect to the deployed person store");
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-matter-open-restate-storage"))
            .await
            .expect("scratch storage"),
    );
    let contract_reviewer: Arc<dyn portal::contract_review::ContractReviewer> =
        Arc::new(portal::contract_review::StubContractReviewer);

    let (project_id, person_id, entity_id) = seed_matter(&surreal).await;
    // `save_version` is a versioned upsert (retries past a concurrent
    // writer), so re-seeding this global template against the shared
    // staging database on every run is safe.
    store::templates::save_version(
        &surreal,
        None,
        "memo__contract_review",
        store::templates::Version {
            title: "Inbound Contract Review".into(),
            respondent_type: "person_and_entity".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .expect("seed the contract-review template");
    let positions = sample_positions();
    store::playbooks::create(
        &surreal,
        &NewPlaybook {
            entity_id,
            name: "Restate regression playbook",
            positions: &positions,
        },
    )
    .await
    .expect("create playbook");

    let deps = ReviewDeps {
        surreal: &surreal,
        workflow_runtime: &runtime,
        storage: &storage,
        contract_reviewer: contract_reviewer.as_ref(),
    };

    // The exact #377 shape: `drive_contract_review` creates the Notation,
    // then in this same call starts its workflow and fires three signals
    // against it. If web and the worker disagree on database, the worker's
    // `append-event` journal step can't find the Notation web just
    // committed and raises `RecordNotFound` — surfaced here as a
    // `ContractReviewError::Runtime`.
    let review_id = drive_contract_review(
        &deps,
        project_id,
        person_id,
        "vendor-msa.txt",
        "MASTER SERVICES AGREEMENT\nLiability is uncapped.",
        IntakeArtifact::Text {
            text: "MASTER SERVICES AGREEMENT\nLiability is uncapped.".into(),
        },
    )
    .await
    .expect(
        "contract review pipeline should complete — a runtime error here means web and the \
         workflows-service worker are not on the same database (the #377 RecordNotFound bug)",
    );

    // The workflow really ran through the worker: the review row exists and
    // the notation reached the attorney gate.
    let review = store::contract_reviews::by_id(&surreal, review_id)
        .await
        .unwrap()
        .expect("review row exists");
    assert_eq!(review.status, store::contract_reviews::STATUS_ANALYZED);

    let notation = store::notations::list_by_project(&surreal, project_id)
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("the contract-review notation was created");
    assert_eq!(notation.state, "lawyer_review");
}
