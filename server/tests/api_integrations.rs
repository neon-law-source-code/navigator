#![allow(clippy::doc_markdown)]
//! Integration tests for the `/app/api/integrations/*` doors.
//!
//! Three things are proved here that nothing else proves:
//!
//! - **The tier is enforced in the handler**, not only in policy.
//!   `portal::test_support::app_state` builds a router with the policy layer
//!   disabled, so a 403 observed here is the handler's own `is_admin_tier`
//!   check. The Rego half of the same gate lives in
//!   `portal/policy/navigator_test.rego`; neither test catches the other's
//!   regression.
//! - **`--all` is the caller's lens, not the whole deployment.** The sweep
//!   runs `store::access::visible_projects`, so an admin who participates in
//!   nothing sweeps nothing. A door that read every row instead would let an
//!   `--all` run touch a matter the read surface hides.
//! - **A deployment with no runtime KMS key refuses rather than falling
//!   back.** That is the whole point of the Firm-secret boundary: a Project
//!   must never reach a provider credential its Firm did not write, so
//!   "unconfigured" is an outcome and not a stub that pretends to succeed.

use std::io::Write;
use std::sync::{Arc, Mutex, Once};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::integrations::FakeIntegrations;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use tracing_subscriber::prelude::*;
use uuid::Uuid;

const KEY: &str = "api-integrations-test-key";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    providers: FakeIntegrations,
    code: String,
    admin: String,
    /// An admin holding no participation row on any matter, for the `all` lens.
    unassigned_admin: String,
    lawyer: String,
    client: String,
}

struct TwoFirmFixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    providers: FakeIntegrations,
    code: String,
    entity_a: Uuid,
    entity_b: Uuid,
    firm_a: Uuid,
    firm_b: Uuid,
    admin_a_id: Uuid,
    admin_a: String,
    admin_b: String,
    admin_b_id: Uuid,
}

trait AppFixture {
    fn app(&self) -> &axum::Router;
}

impl AppFixture for Fixture {
    fn app(&self) -> &axum::Router {
        &self.app
    }
}

impl AppFixture for TwoFirmFixture {
    fn app(&self) -> &axum::Router {
        &self.app
    }
}

fn bearer(person_id: Uuid, role: Role) -> String {
    let mut session = SessionData::fresh("api-integrations-sub", role);
    session.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn person(surreal: &store::surreal::SurrealDb, name: &str, role: Role) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            name,
            format!("{}-{}@example.com", name.to_lowercase(), Uuid::now_v7()),
            role,
        ),
    )
    .await
    .unwrap()
    .id
}

/// Build the router. `configured` selects the provider resolver: the fakes,
/// or the refusing resolver a checkout with no runtime KMS key gets.
async fn build_fixture(configured: bool) -> Fixture {
    let surreal = mem_surreal().await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let admin_id = person(&surreal, "Admin", Role::Admin).await;
    let unassigned_id = person(&surreal, "Unassigned", Role::Admin).await;
    let lawyer_id = person(&surreal, "Lawyer", Role::Lawyer).await;
    let client_id = person(&surreal, "Client", Role::Client).await;
    let firm = store::firms::create(
        &surreal,
        &store::firms::NewFirm {
            name: "Integration Test Firm".into(),
            status: "active".into(),
            entity_id,
            admin_dri_person_id: admin_id,
        },
    )
    .await
    .unwrap();
    let code = format!("matter-{}", &Uuid::now_v7().simple().to_string()[..12]);
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: "Matter".into(),
            status: "open".into(),
            entity_id,
            firm_id: Some(firm.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for (person_id, role) in [
        (admin_id, "admin"),
        (lawyer_id, "lawyer"),
        (client_id, "client"),
    ] {
        store::projects::add_participation(&surreal, project.id, person_id, role)
            .await
            .unwrap();
    }

    let providers = FakeIntegrations::new();
    let mut state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    if configured {
        state.integration_providers = Arc::new(providers.clone());
    }
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        providers,
        code,
        admin: bearer(admin_id, Role::Admin),
        unassigned_admin: bearer(unassigned_id, Role::Admin),
        lawyer: bearer(lawyer_id, Role::Lawyer),
        client: bearer(client_id, Role::Client),
    }
}

async fn build_two_firm_fixture() -> TwoFirmFixture {
    let surreal = mem_surreal().await;
    let entity_a = store::test_support::seed_entity(&surreal).await;
    let entity_b = store::test_support::seed_entity(&surreal).await;
    let requester_id = person(&surreal, "Admin A", Role::Admin).await;
    let member_id = person(&surreal, "Admin B", Role::Admin).await;
    let firm_a = store::firms::create(
        &surreal,
        &store::firms::NewFirm {
            name: "Firm A".into(),
            status: "active".into(),
            entity_id: entity_a,
            admin_dri_person_id: requester_id,
        },
    )
    .await
    .unwrap();
    let firm_b = store::firms::create(
        &surreal,
        &store::firms::NewFirm {
            name: "Firm B".into(),
            status: "active".into(),
            entity_id: entity_b,
            admin_dri_person_id: member_id,
        },
    )
    .await
    .unwrap();
    let code = format!(
        "firm-b-matter-{}",
        &Uuid::now_v7().simple().to_string()[..12]
    );
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: "Firm B Matter".into(),
            status: "open".into(),
            entity_id: entity_b,
            firm_id: Some(firm_b.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for person_id in [requester_id, member_id] {
        store::projects::add_participation(&surreal, project.id, person_id, "admin")
            .await
            .unwrap();
    }

    let kms = cloud::FakeKms::new("api-integrations-test-kms");
    for (provider, kind, value) in [
        (
            store::firm_secrets::IntegrationProvider::Notion,
            store::firm_secrets::IntegrationSecretKind::NotionToken,
            "firm-b-notion-token",
        ),
        (
            store::firm_secrets::IntegrationProvider::Slack,
            store::firm_secrets::IntegrationSecretKind::SlackBotToken,
            "firm-b-slack-token",
        ),
    ] {
        store::firm_secrets::put(
            &surreal,
            store::firm_secrets::SecretPutRequest {
                actor_role: Role::Admin,
                actor_person_id: Some(member_id),
                firm_id: firm_b.id,
                provider,
                kind,
                value,
            },
            &kms,
        )
        .await
        .unwrap();
    }

    let providers = FakeIntegrations::new();
    let mut state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    state.integration_providers = Arc::new(providers.clone());
    TwoFirmFixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        providers,
        code,
        entity_a,
        entity_b,
        firm_a: firm_a.id,
        firm_b: firm_b.id,
        admin_a_id: requester_id,
        admin_a: bearer(requester_id, Role::Admin),
        admin_b: bearer(member_id, Role::Admin),
        admin_b_id: member_id,
    }
}

fn ensure_callsite_interest() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::INFO),
        );
    });
}

#[derive(Clone)]
struct TelemetryWriter(Arc<Mutex<Vec<u8>>>);

impl Write for TelemetryWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("telemetry writer lock poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TelemetryWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

async fn post<F: AppFixture + Sync>(
    fx: &F,
    path: &str,
    auth: Option<&str>,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    fx.app()
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

async fn json(resp: axum::http::Response<Body>) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("the response is JSON")
}

/// The outcomes in a report, as `(code, outcome)` pairs.
fn outcomes(report: &serde_json::Value) -> Vec<(String, String)> {
    report["results"]
        .as_array()
        .expect("results is an array")
        .iter()
        .map(|row| {
            (
                row["project_code"].as_str().unwrap_or_default().to_string(),
                row["outcome"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

const ALL_DOORS: [&str; 4] = [
    "/app/api/integrations/notion/ensure",
    "/app/api/integrations/notion/reconcile",
    "/app/api/integrations/slack/ensure",
    "/app/api/integrations/slack/notify",
];

/// Provisioning a Firm-private resource is the same kind of act as
/// `POST /app/api/project-surfaces/{id}`, which is admin-only. A lawyer on
/// the matter and a client on the matter are both refused; the check is the
/// handler's, because this router runs with the policy layer disabled.
///
/// The body deliberately varies, and one case is nonsense. The tier gate is
/// an extractor, so it runs before the body is deserialized — a caller
/// outside the tier must be told 403 rather than 422 about a body they were
/// never allowed to send.
#[tokio::test]
async fn every_door_is_admin_tier_and_needs_a_session() {
    let fx = build_fixture(true).await;
    for door in ALL_DOORS {
        for body in [
            serde_json::json!({ "project_code": fx.code }),
            serde_json::json!({ "not_a_field": true }),
        ] {
            assert_eq!(
                post(&fx, door, Some(&fx.lawyer), body.clone())
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "{door} admits a lawyer with body {body}"
            );
            assert_eq!(
                post(&fx, door, Some(&fx.client), body.clone())
                    .await
                    .status(),
                StatusCode::FORBIDDEN,
                "{door} admits a client with body {body}"
            );
            assert_eq!(
                post(&fx, door, None, body.clone()).await.status(),
                StatusCode::UNAUTHORIZED,
                "{door} admits an anonymous caller with body {body}"
            );
        }
    }
}

/// Ensure is find-then-create, so the second run adopts. The distinction is
/// the operator's only signal that a re-run did not make a second
/// Firm-private page, and the page address is recorded on the row both times.
#[tokio::test]
async fn notion_ensure_creates_then_adopts_and_records_the_address() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });

    let first = post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        outcomes(&json(first).await),
        vec![(fx.code.clone(), "created".to_string())]
    );

    let second = post(&fx, ALL_DOORS[0], Some(&fx.admin), body).await;
    assert_eq!(
        outcomes(&json(second).await),
        vec![(fx.code.clone(), "adopted".to_string())],
        "a re-run adopts rather than reporting a second creation"
    );
    assert_eq!(
        fx.providers.notion.create_calls(),
        1,
        "exactly one page was created across both runs"
    );

    // The response deliberately carries no coordinate, so the row is the
    // only place the address is observable.
    let page = fx
        .providers
        .notion
        .page(&fx.code)
        .expect("the fake holds the provisioned page");
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .expect("the matter row is readable");
    assert_eq!(stored.private_notion_page_url.as_deref(), Some(&*page.url));
}

/// The reconciler's whole value is telling a duplicate apart from a missing
/// page, which needs a lookup that returns every match rather than the first.
#[tokio::test]
async fn notion_reconcile_reports_a_duplicate_rather_than_repairing_one() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;

    // A second page carrying the same code, as a Firm's workspace holds when
    // somebody made one by hand beside the provisioned page.
    fx.providers.notion.add_duplicate(&fx.code, "page-by-hand");

    let report = json(post(&fx, ALL_DOORS[1], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "duplicate".to_string())]
    );
    assert_eq!(
        report["results"][0]["detail"].as_str(),
        Some("2 pages carry this code"),
        "the operator is told how many, so the repair is theirs to make"
    );
}

/// An unchanged page is reported as unchanged rather than rewritten: the
/// reconciler preserves manual Notion fields, and a needless write is the
/// thing that would lose them.
#[tokio::test]
async fn notion_reconcile_leaves_a_matching_page_alone() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;

    let report = json(post(&fx, ALL_DOORS[1], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "unchanged".to_string())]
    );
    assert_eq!(
        fx.providers.notion.update_calls(),
        0,
        "an unchanged page is never sent to the provider for update"
    );
}

fn body_for_door(door: &str, project_code: &str) -> serde_json::Value {
    if door.ends_with("notify") {
        serde_json::json!({ "project_code": project_code, "event": "project_opened" })
    } else {
        serde_json::json!({ "project_code": project_code })
    }
}

/// Firm authorization is checked after Project selection but before the
/// integration resolver or provider service is reached. A visible Project in
/// another Firm is therefore indistinguishable from a missing Project, and the
/// same check covers all four doors.
#[tokio::test]
async fn integration_doors_authorize_the_target_firm_before_provider_lookup() {
    for door in ALL_DOORS {
        let fx = build_two_firm_fixture().await;
        let denied = post(&fx, door, Some(&fx.admin_a), body_for_door(door, &fx.code)).await;
        let denied_status = denied.status();
        let denied_body = json(denied).await;
        let missing = post(
            &fx,
            door,
            Some(&fx.admin_a),
            body_for_door(door, "missing-matter"),
        )
        .await;
        assert_eq!(denied_status, StatusCode::NOT_FOUND, "{door}");
        assert_eq!(denied_status, missing.status(), "{door}");
        assert_eq!(
            denied_body,
            json(missing).await,
            "cross-Firm refusal must match a missing Project: {door}"
        );
        if door.contains("notion") {
            assert_eq!(fx.providers.notion_calls(), 0, "{door}");
        } else {
            assert_eq!(fx.providers.slack_calls(), 0, "{door}");
        }

        let admitted = post(&fx, door, Some(&fx.admin_b), body_for_door(door, &fx.code)).await;
        assert_eq!(admitted.status(), StatusCode::OK, "{door}");
        if door.contains("notion") {
            assert_eq!(fx.providers.notion_calls(), 1, "{door}");
        } else {
            assert_eq!(fx.providers.slack_calls(), 1, "{door}");
        }
    }
}

/// The `all` sweep filters by the same capability the single-code door does.
/// This is the path worth pinning: `admin_a` holds a participation row on the
/// Firm B matter, so the visibility lens alone would hand them a sweep that
/// spends Firm B's credentials on Firm B's Project. Only the two selector
/// doors take `all`; the Slack doors require a code.
#[tokio::test]
async fn the_all_sweep_drops_a_visible_project_in_another_firm() {
    for door in [ALL_DOORS[0], ALL_DOORS[1]] {
        let fx = build_two_firm_fixture().await;
        let swept = post(
            &fx,
            door,
            Some(&fx.admin_a),
            serde_json::json!({ "all": true }),
        )
        .await;
        assert_eq!(swept.status(), StatusCode::OK, "{door}");
        assert!(
            outcomes(&json(swept).await).is_empty(),
            "the sweep reported a Project the caller may not act on: {door}"
        );
        assert_eq!(
            fx.providers.notion_calls(),
            0,
            "a dropped Project must not reach the resolver: {door}"
        );

        let admitted = post(
            &fx,
            door,
            Some(&fx.admin_b),
            serde_json::json!({ "all": true }),
        )
        .await;
        assert_eq!(admitted.status(), StatusCode::OK, "{door}");
        assert_eq!(
            outcomes(&json(admitted).await)
                .into_iter()
                .map(|(code, _)| code)
                .collect::<Vec<_>>(),
            vec![fx.code.clone()],
            "the Firm's own Admin still sweeps its matter: {door}"
        );
    }
}

/// A mixed visibility set proves the batch filter does not turn a caller's
/// admitted Firm into permission to spend another Firm's credential.
#[tokio::test]
async fn the_all_sweep_keeps_admitted_firms_and_drops_denied_firms() {
    for door in [ALL_DOORS[0], ALL_DOORS[1]] {
        let fx = build_two_firm_fixture().await;
        let admitted_code = format!("firm-a-matter-{}", Uuid::now_v7().simple());
        let admitted = store::projects::create(
            &fx.surreal,
            &store::projects::NewProject {
                code: admitted_code.clone(),
                name: "Firm A Matter".into(),
                status: "open".into(),
                entity_id: fx.entity_a,
                firm_id: Some(fx.firm_a),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::projects::add_participation(&fx.surreal, admitted.id, fx.admin_a_id, "admin")
            .await
            .unwrap();

        let swept = post(
            &fx,
            door,
            Some(&fx.admin_a),
            serde_json::json!({ "all": true }),
        )
        .await;
        assert_eq!(swept.status(), StatusCode::OK, "{door}");
        assert_eq!(
            outcomes(&json(swept).await)
                .into_iter()
                .map(|(code, _)| code)
                .collect::<Vec<_>>(),
            vec![admitted_code],
            "only the caller's admitted Firm remains in the sweep: {door}"
        );
        assert_eq!(
            fx.providers.notion_calls(),
            1,
            "the denied Firm's provider is never touched: {door}"
        );
    }
}

/// The batch capability resolver is called once for the whole sweep, even
/// when the caller can see more than one Project in the same Firm.
#[tokio::test]
async fn the_all_sweep_reads_capability_once_for_all_visible_projects() {
    let fx = build_two_firm_fixture().await;
    let second_code = format!("second-firm-b-matter-{}", Uuid::now_v7().simple());
    let second = store::projects::create(
        &fx.surreal,
        &store::projects::NewProject {
            code: second_code.clone(),
            name: "Second Firm B Matter".into(),
            status: "open".into(),
            entity_id: fx.entity_b,
            firm_id: Some(fx.firm_b),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(&fx.surreal, second.id, fx.admin_b_id, "admin")
        .await
        .unwrap();

    ensure_callsite_interest();
    let output = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_max_level(tracing::Level::INFO)
        .with_writer(TelemetryWriter(output.clone()))
        .finish();
    let swept = {
        let _guard = tracing::subscriber::set_default(subscriber);
        post(
            &fx,
            ALL_DOORS[0],
            Some(&fx.admin_b),
            serde_json::json!({ "all": true }),
        )
        .await
    };
    assert_eq!(swept.status(), StatusCode::OK);
    assert_eq!(fx.providers.notion_calls(), 2);
    assert_eq!(
        outcomes(&json(swept).await)
            .into_iter()
            .map(|(code, _)| code)
            .collect::<Vec<_>>(),
        vec![fx.code.clone(), second_code],
    );

    let logged = String::from_utf8(
        output
            .lock()
            .expect("telemetry output lock poisoned")
            .clone(),
    )
    .unwrap();
    let events: Vec<&str> = logged
        .lines()
        .filter(|line| line.contains("firm capability sweep"))
        .collect();
    assert_eq!(events.len(), 1, "one capability event per sweep: {logged}");
    assert!(
        events[0].contains("\"visible_project_count\":2"),
        "{logged}"
    );
    assert!(
        events[0].contains("\"selected_project_count\":2"),
        "{logged}"
    );
}

/// Slack ensure records the channel id and invites nobody: the adapter takes
/// only provider-issued member ids, Navigator stores none, and it will not
/// turn a participation row into an invite.
#[tokio::test]
async fn slack_ensure_records_the_channel_and_invites_no_one() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });

    let report = json(post(&fx, ALL_DOORS[2], Some(&fx.admin), body.clone()).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "created".to_string())]
    );
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    let channel_id = stored
        .internal_slack_channel_id
        .expect("the channel id is recorded on the row");
    assert!(
        fx.providers.slack.invited_members(&channel_id).is_empty(),
        "no participation row became a Slack invite"
    );

    let again = json(post(&fx, ALL_DOORS[2], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&again),
        vec![(fx.code.clone(), "adopted".to_string())]
    );
}

/// The event vocabulary is closed so this door cannot carry arbitrary text —
/// a client name, a document title — into a Firm channel. An unrecognized
/// kind is refused before any provider call.
#[tokio::test]
async fn slack_notify_refuses_free_text_and_posts_only_the_event() {
    let fx = build_fixture(true).await;
    post(
        &fx,
        ALL_DOORS[2],
        Some(&fx.admin),
        serde_json::json!({ "project_code": fx.code }),
    )
    .await;

    let refused = post(
        &fx,
        ALL_DOORS[3],
        Some(&fx.admin),
        serde_json::json!({ "project_code": fx.code, "event": "the client called" }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(
        fx.providers.slack.messages().is_empty(),
        "a refused event reached no channel"
    );

    let posted = post(
        &fx,
        ALL_DOORS[3],
        Some(&fx.admin),
        serde_json::json!({ "project_code": fx.code, "event": "project_reconciled" }),
    )
    .await;
    assert_eq!(
        outcomes(&json(posted).await),
        vec![(fx.code.clone(), "notified".to_string())]
    );
    let messages = fx.providers.slack.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].1, "Navigator integration event: project_reconciled.",
        "the text is derived from the kind, never from the request"
    );
}

/// Notify never provisions. A notice that quietly created the channel it
/// wanted to post into would make the Firm-private channel a side effect of
/// an event rather than a deliberate act.
#[tokio::test]
async fn slack_notify_does_not_create_the_channel_it_wants() {
    let fx = build_fixture(true).await;
    let report = json(
        post(
            &fx,
            ALL_DOORS[3],
            Some(&fx.admin),
            serde_json::json!({ "project_code": fx.code, "event": "project_opened" }),
        )
        .await,
    )
    .await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "no_channel".to_string())]
    );
    assert!(fx.providers.slack.messages().is_empty());
}

/// `all` is the caller's participation lens, the same one `GET /app/api/projects`
/// uses. An admin who participates in nothing sweeps nothing — a door reading
/// every row instead would let one `--all` run reach a matter the read
/// surface hides.
#[tokio::test]
async fn the_all_sweep_is_the_callers_lens() {
    let fx = build_fixture(true).await;

    let assigned = json(
        post(
            &fx,
            ALL_DOORS[0],
            Some(&fx.admin),
            serde_json::json!({ "all": true }),
        )
        .await,
    )
    .await;
    assert_eq!(
        outcomes(&assigned),
        vec![(fx.code.clone(), "created".to_string())]
    );

    let unassigned = json(
        post(
            &fx,
            ALL_DOORS[0],
            Some(&fx.unassigned_admin),
            serde_json::json!({ "all": true }),
        )
        .await,
    )
    .await;
    assert!(
        outcomes(&unassigned).is_empty(),
        "an admin with no participation row sweeps no matter"
    );
}

/// A code with `all`, or neither, is a 400: `all` beside a code leaves it
/// ambiguous whether the code narrowed the sweep or was ignored.
#[tokio::test]
async fn the_selector_takes_exactly_one_of_a_code_and_all() {
    let fx = build_fixture(true).await;
    for body in [
        serde_json::json!({ "project_code": fx.code, "all": true }),
        serde_json::json!({}),
    ] {
        assert_eq!(
            post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone())
                .await
                .status(),
            StatusCode::BAD_REQUEST,
            "{body} was accepted"
        );
    }
    // A code nobody can see is a 404, not an empty success — the caller asked
    // about one matter and has to learn it was not found.
    assert_eq!(
        post(
            &fx,
            ALL_DOORS[0],
            Some(&fx.admin),
            serde_json::json!({ "project_code": "no-such-matter" })
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

/// The default resolver on a checkout with no runtime KMS key refuses. It
/// does not fall back to a deployment-wide provider token, because that is
/// exactly the arrangement the Firm-secret boundary replaces — and it does
/// not answer with a stub that pretends a page exists.
#[tokio::test]
async fn an_unconfigured_deployment_reports_the_refusal() {
    let fx = build_fixture(false).await;
    let report = json(
        post(
            &fx,
            ALL_DOORS[0],
            Some(&fx.admin),
            serde_json::json!({ "project_code": fx.code }),
        )
        .await,
    )
    .await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "runtime_not_configured".to_string())]
    );
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    assert!(
        stored.private_notion_page_url.is_none(),
        "a refusal records no address"
    );
}

// ---- ENG-807: recorded-identity validation, recovery, and concurrency ----

/// A title search can never see an archived page (Notion excludes archived
/// pages from `/search`), so before this fix `ensure` would report a fresh
/// `created` and leave a duplicate beside the archived original. Validating
/// the recorded id directly catches this instead.
#[tokio::test]
async fn notion_ensure_reports_an_archived_recorded_page_instead_of_duplicating_it() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;
    let page = fx
        .providers
        .notion
        .page(&fx.code)
        .expect("first ensure recorded a page");

    fx.providers.notion.archive(&page.id);

    let report = json(post(&fx, ALL_DOORS[0], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "archived".to_string())]
    );
    assert_eq!(
        fx.providers.notion.create_calls(),
        1,
        "an archived recorded page is reported, never silently replaced"
    );
}

/// Same as the archived case, but for a page someone renamed by hand — a
/// title search cannot see it either, since the title no longer matches the
/// query, so this is only catchable by looking up the recorded id directly.
#[tokio::test]
async fn notion_ensure_reports_a_renamed_recorded_page_instead_of_duplicating_it() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;
    let page = fx
        .providers
        .notion
        .page(&fx.code)
        .expect("first ensure recorded a page");

    fx.providers.notion.rename(&page.id, "someone-renamed-this");

    let report = json(post(&fx, ALL_DOORS[0], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "renamed".to_string())]
    );
    assert_eq!(
        report["results"][0]["detail"].as_str(),
        Some("someone-renamed-this")
    );
    assert_eq!(fx.providers.notion.create_calls(), 1);
}

/// The same two identity checks apply through `reconcile`, which already had
/// its own "leaves a matching page alone" / "duplicate" coverage — this adds
/// the archived/renamed branch a title search alone cannot reach.
#[tokio::test]
async fn notion_reconcile_reports_archived_and_renamed_recorded_pages() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;
    let page = fx
        .providers
        .notion
        .page(&fx.code)
        .expect("ensure recorded a page");

    fx.providers.notion.archive(&page.id);
    let archived = json(post(&fx, ALL_DOORS[1], Some(&fx.admin), body.clone()).await).await;
    assert_eq!(
        outcomes(&archived),
        vec![(fx.code.clone(), "archived".to_string())]
    );

    // Un-archive and rename instead, to exercise the other branch.
    fx.providers.notion.rename(&page.id, "someone-renamed-this");
    let renamed = json(post(&fx, ALL_DOORS[1], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&renamed),
        vec![(fx.code.clone(), "renamed".to_string())]
    );
}

/// Recovery after the provider created the resource but before Navigator
/// persisted its address: the next `ensure` must adopt the already-created
/// page rather than creating a second one, and it must repair the row.
#[tokio::test]
async fn notion_ensure_recovers_when_the_page_exists_but_the_address_was_never_persisted() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()).await;

    // Simulate the crash: the provider call succeeded (the fake still holds
    // the page) but the row never recorded it.
    let project = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    store::projects::update_project(
        &fx.surreal,
        project.id,
        &store::projects::UpdateProjectCommand {
            private_notion_page_url: Some(String::new()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let report = json(post(&fx, ALL_DOORS[0], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "adopted".to_string())],
        "the already-created page is adopted, not duplicated"
    );
    assert_eq!(
        fx.providers.notion.create_calls(),
        1,
        "exactly one page was ever created"
    );
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    assert!(
        stored.private_notion_page_url.is_some(),
        "the address is repaired on the recovering run"
    );
}

/// Concurrent `ensure` calls for the same, never-yet-provisioned Project must
/// not race into two pages. The per-`(project, provider)` lock in
/// `portal::integrations_api` is what this exercises.
#[tokio::test]
async fn concurrent_notion_ensure_creates_exactly_one_page() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });

    let (first, second) = tokio::join!(
        post(&fx, ALL_DOORS[0], Some(&fx.admin), body.clone()),
        post(&fx, ALL_DOORS[0], Some(&fx.admin), body),
    );
    let first_outcome = outcomes(&json(first).await);
    let second_outcome = outcomes(&json(second).await);
    let created_count = [&first_outcome, &second_outcome]
        .iter()
        .filter(|outcomes| outcomes[0].1 == "created")
        .count();
    assert_eq!(
        created_count, 1,
        "exactly one of the two concurrent calls created the page: {first_outcome:?} {second_outcome:?}"
    );
    assert_eq!(
        fx.providers.notion.create_calls(),
        1,
        "the provider saw exactly one create despite the race"
    );
}

/// Slack ensure now records the canonical URL alongside the channel id, and
/// the two must agree — the concrete fix for the id/URL inconsistency ENG-807
/// called out.
#[tokio::test]
async fn slack_ensure_records_a_url_consistent_with_the_channel_id() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[2], Some(&fx.admin), body).await;

    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    let channel_id = stored
        .internal_slack_channel_id
        .expect("channel id recorded");
    let channel_url = stored
        .internal_slack_channel_url
        .expect("channel URL recorded");
    assert_eq!(channel_url, cloud::slack_channel_url(&channel_id));
}

/// A search-based `find_private_channel` cannot see an archived channel —
/// `conversations.list` excludes archived channels — mirroring the Notion
/// identity-validation fix.
#[tokio::test]
async fn slack_ensure_reports_an_archived_recorded_channel() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[2], Some(&fx.admin), body.clone()).await;
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    let channel_id = stored
        .internal_slack_channel_id
        .expect("channel id recorded");

    fx.providers.slack.archive(&channel_id);
    let archived = json(post(&fx, ALL_DOORS[2], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&archived),
        vec![(fx.code.clone(), "archived".to_string())]
    );
}

/// Same as the archived case, but for a channel someone renamed by hand — a
/// name search cannot see it either, since the name no longer matches the
/// query.
#[tokio::test]
async fn slack_ensure_reports_a_renamed_recorded_channel() {
    let fx = build_fixture(true).await;
    let body = serde_json::json!({ "project_code": fx.code });
    post(&fx, ALL_DOORS[2], Some(&fx.admin), body.clone()).await;
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    let channel_id = stored
        .internal_slack_channel_id
        .expect("channel id recorded");

    fx.providers
        .slack
        .rename(&channel_id, "someone-renamed-this");
    let renamed = json(post(&fx, ALL_DOORS[2], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&renamed),
        vec![(fx.code.clone(), "renamed".to_string())]
    );
    assert_eq!(
        renamed["results"][0]["detail"].as_str(),
        Some("someone-renamed-this")
    );
}

/// A missing or revoked Firm credential reports its own distinct outcome
/// through the real resolver (`portal::integrations::FirmIntegrations`), not
/// only through the fully-stubbed `FakeIntegrations` every other test in this
/// file uses. This is the acceptance criterion the fakes alone cannot prove:
/// the store's `SecretStoreError` really does map to `credential_missing`
/// end to end through the HTTP door.
#[tokio::test]
async fn missing_and_revoked_credentials_report_their_own_outcome_through_the_real_resolver() {
    let surreal = mem_surreal().await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let dri_id = person(&surreal, "Dri", Role::Admin).await;
    let firm = store::firms::create(
        &surreal,
        &store::firms::NewFirm {
            name: "Real Resolver Firm".into(),
            status: "active".into(),
            entity_id,
            admin_dri_person_id: dri_id,
        },
    )
    .await
    .unwrap();
    let code = format!(
        "real-resolver-{}",
        &Uuid::now_v7().simple().to_string()[..12]
    );
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: "Real Resolver Matter".into(),
            status: "open".into(),
            entity_id,
            firm_id: Some(firm.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, dri_id, "admin")
        .await
        .unwrap();

    let kms = cloud::FakeKms::new("real-resolver-test-kms");
    let mut state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    state.integration_providers = std::sync::Arc::new(portal::integrations::FirmIntegrations::new(
        std::sync::Arc::new(kms.clone()),
        "database-1",
    ));
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let admin = bearer(dri_id, Role::Admin);
    let body = serde_json::json!({ "project_code": code });

    // Nothing configured yet.
    let missing = json(post_app(&app, ALL_DOORS[2], Some(&admin), body.clone()).await).await;
    assert_eq!(
        outcomes(&missing),
        vec![(code.clone(), "credential_missing".to_string())]
    );

    // Configure, then revoke: the resolver must stop finding it.
    store::firm_secrets::put(
        &surreal,
        store::firm_secrets::SecretPutRequest {
            actor_role: Role::Admin,
            actor_person_id: Some(dri_id),
            firm_id: firm.id,
            provider: store::firm_secrets::IntegrationProvider::Slack,
            kind: store::firm_secrets::IntegrationSecretKind::SlackBotToken,
            value: "real-resolver-slack-token",
        },
        &kms,
    )
    .await
    .unwrap();
    store::firm_secrets::revoke(
        &surreal,
        Role::Admin,
        Some(dri_id),
        firm.id,
        store::firm_secrets::IntegrationProvider::Slack,
        store::firm_secrets::IntegrationSecretKind::SlackBotToken,
    )
    .await
    .unwrap();
    let revoked = json(post_app(&app, ALL_DOORS[2], Some(&admin), body).await).await;
    assert_eq!(
        outcomes(&revoked),
        vec![(code, "credential_missing".to_string())],
        "a revoked credential resolves the same as one never configured"
    );
}

/// [`post`] takes an [`AppFixture`]; this test builds its own router directly
/// rather than adding a third fixture shape, so it posts to the raw
/// `axum::Router` instead.
async fn post_app(
    app: &axum::Router,
    path: &str,
    auth: Option<&str>,
    body: serde_json::Value,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap()
}

/// A Slack name collision the fake never created (a channel the bot cannot
/// see) is reported as its own `conflict` outcome, not folded into the
/// generic `provider_unavailable` slug.
#[tokio::test]
async fn slack_ensure_reports_a_name_collision_as_conflict() {
    let fx = build_fixture(true).await;
    fx.providers.slack.simulate_name_taken(&fx.code);
    let body = serde_json::json!({ "project_code": fx.code });

    let report = json(post(&fx, ALL_DOORS[2], Some(&fx.admin), body).await).await;
    assert_eq!(
        outcomes(&report),
        vec![(fx.code.clone(), "conflict".to_string())]
    );
    let stored = store::projects::find_by_code(&fx.surreal, &fx.code)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.internal_slack_channel_id.is_none());
}
