#![allow(clippy::doc_markdown)]
//! Integration tests for `GET /app/api/project-repositories` — the admin-only
//! reconciliation read.
//!
//! Two things are proved here that nothing else can prove:
//!
//! - **The report is an inventory, not a lens.** Every other matter read on
//!   this surface goes through `store::access::visible_projects`, which scopes
//!   to the caller's participation rows for every firm tier, Owner and Admin
//!   included. A reconciliation report built on that read would report a matter
//!   the caller does not participate in as a matter that does not exist. So the
//!   central test here holds one admin who participates in *nothing* and asserts
//!   that `GET /app/api/projects` returns zero for them while this door still
//!   reconciles every row.
//! - **The tier is enforced in the handler**, not only in policy.
//!   `portal::test_support::app_state` builds a router with the policy layer
//!   disabled, so a 403 observed here is the extractor's own `is_admin_tier`
//!   check. The Rego half of the same gate is proved separately, by the
//!   five-case matrix in `portal/policy/navigator_test.rego`. Both layers are
//!   deliberate: neither test would catch the other's regression.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "api-project-repositories-test-key";
const PATH: &str = "/app/api/project-repositories";

/// A synthetic forge. No real organization is a fixture value in this
/// workspace — which organization holds a deployment's Project repositories is
/// configuration.
const A_FORGE: &str = "https://forge.example/an-organization";

struct Fixture {
    app: axum::Router,
    /// An admin holding no participation row on any matter.
    unassigned_admin: String,
    /// A lawyer holding a participation row on the reconciled matter.
    lawyer: String,
    client: String,
    clerk: String,
    /// The codes seeded, in creation order: matched, drifted, no repository.
    matched: String,
    drifted: String,
    unrecorded: String,
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut session = SessionData::fresh("api-project-repositories-sub", role);
    session.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn person(surreal: &store::surreal::SurrealDb, name: &str, role: Role) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            name,
            format!("{}@example.com", name.to_lowercase()),
            role,
        ),
    )
    .await
    .unwrap()
    .id
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let entity_id = store::test_support::seed_entity(&surreal).await;

    // Unique per run so a shared engine cannot collide on `project_code`.
    let suffix = Uuid::now_v7().simple().to_string();
    let matched = format!("matched-{}", &suffix[..8]);
    let drifted = format!("drifted-{}", &suffix[..8]);
    let unrecorded = format!("unrecorded-{}", &suffix[..8]);

    let mut created = Vec::new();
    for code in [&matched, &drifted, &unrecorded] {
        let project = store::projects::create(
            &surreal,
            &store::projects::NewProject {
                code: code.clone(),
                name: "Matter".into(),
                status: "open".into(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        created.push(project.id);
    }

    // The matched row records the repository its own code names.
    store::projects::set_repository_url(
        &surreal,
        created[0],
        Some(&format!("{A_FORGE}/{matched}")),
    )
    .await
    .unwrap();
    // The drifted row records a repository named for something else — the
    // shape a rename leaves behind, and the one this door exists to catch.
    store::projects::set_repository_url(
        &surreal,
        created[1],
        Some(&format!("{A_FORGE}/somewhere-else")),
    )
    .await
    .unwrap();
    // The third records nothing at all.

    let admin_id = person(&surreal, "Admin", Role::Admin).await;
    let lawyer_id = person(&surreal, "Lawyer", Role::Lawyer).await;
    let client_id = person(&surreal, "Client", Role::Client).await;
    let clerk_id = person(&surreal, "Clerk", Role::Clerk).await;

    // The lawyer participates in one matter; the admin participates in none.
    // That asymmetry is the point of `an_unassigned_admin_still_reconciles_every_row`.
    store::projects::add_participation(&surreal, created[0], lawyer_id, "attorney")
        .await
        .unwrap();

    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        unassigned_admin: bearer(admin_id, Role::Admin),
        lawyer: bearer(lawyer_id, Role::Lawyer),
        client: bearer(client_id, Role::Client),
        clerk: bearer(clerk_id, Role::Clerk),
        matched,
        drifted,
        unrecorded,
    }
}

async fn get(fx: &Fixture, path: &str, auth: Option<&str>) -> axum::http::Response<Body> {
    let mut req = Request::builder().method("GET").uri(path);
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    fx.app
        .clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("the response is JSON")
}

/// Every finding about one code, by kind.
fn findings_for<'a>(report: &'a serde_json::Value, code: &str) -> Vec<&'a serde_json::Value> {
    report["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .filter(|finding| finding["code"] == code)
        .collect()
}

/// **The regression test for the participation lens.**
///
/// An admin with no participation row sees nothing through
/// `GET /app/api/projects` — that is the documented, correct behaviour of
/// `visible_projects_as_lawyer`, which grants no silent directory bypass. A
/// reconciliation built on that read would therefore have concluded that every
/// matter in the deployment was missing.
///
/// This door reads `store::projects::all` instead, so the same caller
/// reconciles every row. If someone later reroutes it through the scoped read,
/// this fails.
#[tokio::test]
async fn an_unassigned_admin_still_reconciles_every_row() {
    let fx = build_fixture().await;

    let scoped = get(&fx, "/app/api/projects", Some(&fx.unassigned_admin)).await;
    assert_eq!(scoped.status(), StatusCode::OK);
    assert_eq!(
        json(scoped).await.as_array().expect("an array").len(),
        0,
        "the participation lens shows an unassigned admin no matters at all"
    );

    let resp = get(&fx, PATH, Some(&fx.unassigned_admin)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let report = json(resp).await;

    assert!(
        report["rows"].as_u64().expect("rows is a count") >= 3,
        "the same caller reconciles every row, not their own: {report}"
    );
    for code in [&fx.matched, &fx.drifted, &fx.unrecorded] {
        assert!(
            report["rows"].as_u64().unwrap() >= 3,
            "expected {code} to be among the reconciled rows: {report}"
        );
    }
}

/// The failing finding, with the structured fields a gate reads by name rather
/// than parsing out of a sentence.
#[tokio::test]
async fn a_row_naming_another_repository_is_reported_as_a_failure() {
    let fx = build_fixture().await;

    let report = json(get(&fx, PATH, Some(&fx.unassigned_admin)).await).await;

    let drifted = findings_for(&report, &fx.drifted);
    assert_eq!(drifted.len(), 1, "expected one finding: {report}");
    assert_eq!(drifted[0]["kind"], "repository-name-is-not-code");
    assert_eq!(drifted[0]["severity"], "fail");
    assert_eq!(drifted[0]["named"], "somewhere-else");
    assert_eq!(
        drifted[0]["recorded"],
        format!("{A_FORGE}/somewhere-else"),
        "the finding carries the URL it read, not a rendering of it"
    );
    assert_eq!(
        report["reconciled"], false,
        "one failing finding makes the deployment unreconciled"
    );
}

/// A row that records nothing is a warning, and a warning does not make the
/// deployment unreconciled. Proved on its own fixture so the drifted row's
/// failure cannot mask it.
#[tokio::test]
async fn a_row_recording_no_repository_is_only_a_warning() {
    let fx = build_fixture().await;

    let report = json(get(&fx, PATH, Some(&fx.unassigned_admin)).await).await;

    let unrecorded = findings_for(&report, &fx.unrecorded);
    assert_eq!(unrecorded.len(), 1, "expected one finding: {report}");
    assert_eq!(unrecorded[0]["kind"], "no-repository-url");
    assert_eq!(unrecorded[0]["severity"], "warn");

    assert!(
        findings_for(&report, &fx.matched).is_empty(),
        "a row naming its own repository says nothing: {report}"
    );
}

/// The report says whether it had a deployment forge pair to compare against,
/// so an absent warning is never read as agreement. The test suite configures
/// no deployment, which is exactly the shape the local loop has.
#[tokio::test]
async fn the_report_states_whether_it_compared_against_a_deployment_forge() {
    let fx = build_fixture().await;

    let report = json(get(&fx, PATH, Some(&fx.unassigned_admin)).await).await;

    assert!(
        report["compared_against_deployment_forge"].is_boolean(),
        "the flag is always present: {report}"
    );
    // Every failing finding is computable without configuration, so the
    // drifted row is still caught with no forge pair resolved.
    assert_eq!(report["reconciled"], false);
}

/// The tier, enforced by the handler. Policy is proved separately — this
/// router has the policy layer disabled, so what refuses here is
/// `is_admin_tier` on the extracted session.
#[tokio::test]
async fn only_the_admin_tier_reaches_the_report() {
    let fx = build_fixture().await;

    assert_eq!(
        get(&fx, PATH, Some(&fx.unassigned_admin)).await.status(),
        StatusCode::OK
    );

    for (tier, auth) in [
        ("lawyer", &fx.lawyer),
        ("clerk", &fx.clerk),
        ("client", &fx.client),
    ] {
        assert_eq!(
            get(&fx, PATH, Some(auth)).await.status(),
            StatusCode::FORBIDDEN,
            "{tier} must not reach an all-rows report"
        );
    }

    assert_eq!(
        get(&fx, PATH, None).await.status(),
        StatusCode::UNAUTHORIZED,
        "an anonymous caller is refused before the tier is considered"
    );
}
