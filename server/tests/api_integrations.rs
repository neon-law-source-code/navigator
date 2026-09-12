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

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::integrations::FakeIntegrations;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
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
    providers: FakeIntegrations,
    code: String,
    admin_a: String,
    admin_b: String,
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
    let _firm_a = store::firms::create(
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
        ..portal::test_support::app_state(surreal).await
    };
    state.integration_providers = Arc::new(providers.clone());
    TwoFirmFixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        providers,
        code,
        admin_a: bearer(requester_id, Role::Admin),
        admin_b: bearer(member_id, Role::Admin),
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
