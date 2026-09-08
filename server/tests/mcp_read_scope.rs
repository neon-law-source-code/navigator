#![allow(clippy::doc_markdown)]
//! `/app/mcp` reads answer through the caller's own lens.
//!
//! Two things had to be true for that, and this drives the composed
//! router rather than either one alone, because either one alone is
//! silently useless:
//!
//! 1. `/app/mcp` has to resolve the `navigator` CLI's own credential — the
//!    HMAC-signed `SessionData` blob `cli_auth` mints — so there is an
//!    identity to scope by. The A2A rpc route already carried
//!    `inject_bearer_session`; `/app/mcp` did not, so `inject_principal`
//!    found no session to read an email from and every read answered as
//!    the deployment.
//! 2. `aida_list_projects` has to scope on that identity: participation
//!    for a firm or client participant, the oversight directory for an
//!    owner or admin.
//!
//! A regression in the layer looks exactly like a regression in the
//! scope — an unscoped list — so the assertions are on what came back,
//! not on the hand-offs.
//!
//! What this fixture cannot show is the endpoint's Rego gate:
//! `test_support::app_state` disables auth and policy enforcement so
//! handler tests can address handlers. That gate is the outer refusal and
//! its matrix lives in `navigator_test.rego`. Everything asserted below
//! therefore holds *with the gate switched off* — which is the stronger
//! claim for a read scope, since a misconfigured policy must not turn a
//! participant into someone who sees every matter.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::{AppState, SessionData, SessionStore};
use serde_json::{json, Value};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &[u8] = b"mcp-read-scope-test-key-0123456789";

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    )
}

/// Mint the credential `site login` writes to `~/.navigator.json`, and
/// the `persons` row the read resolves it against.
async fn cli_bearer(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    role: Role,
) -> (String, Uuid) {
    let person = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap();
    let mut session = SessionData::fresh(email, role);
    session.email = Some(email.to_string());
    session.source = portal::SessionSource::Cli;
    (
        format!("Bearer {}", SessionStore::new(KEY).encode(&session)),
        person.id,
    )
}

async fn seed_matter(surreal: &store::surreal::SurrealDb, name: &str) -> Uuid {
    let entity_id = store::test_support::seed_entity(surreal).await;
    store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("m-{}", Uuid::now_v7().simple()),
            name: name.into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
}

async fn put_on_matter(
    surreal: &store::surreal::SurrealDb,
    person_id: Uuid,
    project_id: Uuid,
    dri: store::participation::DriRequest,
) {
    store::participation::add_participant(
        surreal,
        &store::participation::AddParticipantCommand {
            project_id,
            person_id,
            dri,
            actor: store::participation::DriActor::System,
        },
    )
    .await
    .unwrap();
}

/// Call `aida_list_projects` over `/app/mcp` and return `structuredContent`.
async fn list_projects(app: &axum::Router, bearer: Option<&str>) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "aida_list_projects", "arguments": {} }
    });
    let mut builder = Request::builder()
        .method("POST")
        .uri("/app/mcp")
        .header("content-type", "application/json");
    if let Some(b) = bearer {
        builder = builder.header("authorization", b);
    }
    let resp = app
        .clone()
        .oneshot(
            builder
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a resolved CLI bearer must not be redirected or refused at the transport"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let envelope: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        envelope.get("error").is_none(),
        "unexpected JSON-RPC error: {envelope}"
    );
    envelope["result"]["structuredContent"].clone()
}

fn names(content: &Value) -> Vec<String> {
    content["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_lawyer_lists_the_matter_they_are_on_and_not_the_one_they_are_not() {
    let (app, surreal) = build_app().await;
    let theirs = seed_matter(&surreal, "Matter A").await;
    seed_matter(&surreal, "Matter B").await;
    let (bearer, person_id) = cli_bearer(&surreal, "lawyer@neonlaw.com", Role::Lawyer).await;
    put_on_matter(
        &surreal,
        person_id,
        theirs,
        store::participation::DriRequest::Unchanged,
    )
    .await;

    let content = list_projects(&app, Some(&bearer)).await;
    assert_eq!(content["lens"], "membership");
    assert_eq!(names(&content), vec!["Matter A"], "got: {content}");
}

#[tokio::test]
async fn owner_and_admin_list_every_matter_through_the_directory_lens() {
    for role in [Role::Owner, Role::Admin] {
        let (app, surreal) = build_app().await;
        let assigned = seed_matter(&surreal, "Matter A").await;
        seed_matter(&surreal, "Matter B").await;
        // A lawyer accountable for one matter, so the directory has a DRI
        // to report and an unassigned matter to report as such.
        let (_, dri_id) = cli_bearer(&surreal, "dri@neonlaw.com", Role::Lawyer).await;
        put_on_matter(
            &surreal,
            dri_id,
            assigned,
            store::participation::DriRequest::Designate(store::projects::DriSide::Lawyer),
        )
        .await;
        // The oversight caller holds no participation row at all.
        let (bearer, _) = cli_bearer(&surreal, "overseer@neonlaw.com", role).await;

        let content = list_projects(&app, Some(&bearer)).await;
        assert_eq!(content["lens"], "directory", "{role:?}: {content}");
        let mut listed = names(&content);
        listed.sort();
        assert_eq!(listed, vec!["Matter A", "Matter B"], "{role:?}");

        let rows = content["projects"].as_array().unwrap();
        let a = rows.iter().find(|p| p["name"] == "Matter A").unwrap();
        assert_eq!(a["lawyer_dris"][0], "dri@neonlaw.com", "{role:?}");
        assert!(a["code"].is_string(), "{role:?}: the handle is the code");
        // Oversight is not membership: the lens carries no handle a
        // contents read could take.
        assert!(a["id"].is_null(), "{role:?}: no id on the directory lens");
        let b = rows.iter().find(|p| p["name"] == "Matter B").unwrap();
        assert_eq!(b["lawyer_dris"].as_array().unwrap().len(), 0, "{role:?}");
    }
}

#[tokio::test]
async fn a_client_lists_only_their_own_matter() {
    let (app, surreal) = build_app().await;
    let theirs = seed_matter(&surreal, "Their Matter").await;
    let other = seed_matter(&surreal, "Someone Else").await;
    let (bearer, client_id) = cli_bearer(&surreal, "client@example.com", Role::Client).await;
    put_on_matter(
        &surreal,
        client_id,
        theirs,
        store::participation::DriRequest::Unchanged,
    )
    .await;
    let (_, lawyer_id) = cli_bearer(&surreal, "lawyer@neonlaw.com", Role::Lawyer).await;
    put_on_matter(
        &surreal,
        lawyer_id,
        other,
        store::participation::DriRequest::Unchanged,
    )
    .await;

    let content = list_projects(&app, Some(&bearer)).await;
    assert_eq!(names(&content), vec!["Their Matter"], "got: {content}");
}

#[tokio::test]
async fn an_anonymous_caller_keeps_the_unscoped_local_behavior() {
    // The KIND path and the existing browser harness: no credential, so
    // no principal, so the deployment-wide list this tool has always
    // returned. In production a principal is always injected, so this is
    // not a hole an authenticated caller can fall into.
    let (app, surreal) = build_app().await;
    seed_matter(&surreal, "Matter A").await;
    seed_matter(&surreal, "Matter B").await;

    let content = list_projects(&app, None).await;
    assert_eq!(content["lens"], "membership");
    let mut listed = names(&content);
    listed.sort();
    assert_eq!(listed, vec!["Matter A", "Matter B"], "got: {content}");
}

#[tokio::test]
async fn an_authenticated_stranger_with_no_persons_row_lists_nothing() {
    // Sign-in does not create a Person. A signed session whose email has
    // no row is the caller who must reach no matter — fail-closed, with
    // no privileged exception.
    let (app, surreal) = build_app().await;
    seed_matter(&surreal, "Matter A").await;
    let mut session = SessionData::fresh("stranger@example.com", Role::Lawyer);
    session.email = Some("stranger@example.com".to_string());
    session.source = portal::SessionSource::Cli;
    let bearer = format!("Bearer {}", SessionStore::new(KEY).encode(&session));

    let content = list_projects(&app, Some(&bearer)).await;
    assert_eq!(content["count"], 0, "got: {content}");
}
