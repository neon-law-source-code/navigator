#![allow(clippy::doc_markdown)]
//! Integration tests for `POST /app/api/projects/{id}/close` — the REST door
//! that opens a matter's firm-signed closing-letter notation.
//!
//! The write engine (`retainer_walk::open_closing_notation`) is shared with the
//! lawyer close control, so these tests focus on what the REST adapter adds: the
//! tier gate (LawyerSession → 401/403), the matter-scope gate (a lawyer who does
//! not participate is a bare 404, admin bypasses), the no-client refusal (409),
//! and a live 201 proving the closing notation is opened on the matter.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{InMemoryRuntime, StateMachineRuntime};

const KEY: &str = "api-matter-close-test-key";

struct Harness {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
}

async fn build_app() -> Harness {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-api-matter-close-storage"))
            .await
            .unwrap(),
    );
    // The canonical seed brings the `offboarding__letter` template the command
    // resolves.
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime: Arc<dyn StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Harness {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    }
}

/// A matter with `Matter` as its name, its seeded entity, and — unless
/// `with_client` is false — a client participant. Returns the project id.
async fn seed_matter(surreal: &store::surreal::SurrealDb, with_client: bool) -> Uuid {
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    if with_client {
        let client = store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Matter Client",
                format!("client-{}@example.com", Uuid::now_v7()),
                Role::Client,
            ),
        )
        .await
        .unwrap();
        store::projects::add_participation(surreal, project.id, client.id, "client")
            .await
            .unwrap();
    }
    project.id
}

/// A `Bearer` header for a session of `role`, optionally added to `project` as a
/// firm-side participant so the matter-scope check admits them.
async fn bearer(
    surreal: &store::surreal::SurrealDb,
    email: &str,
    role: Role,
    project: Option<Uuid>,
) -> String {
    let actor = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(email, email, role),
    )
    .await
    .unwrap();
    if let Some(project_id) = project {
        store::projects::add_participation(surreal, project_id, actor.id, "lawyer")
            .await
            .unwrap();
    }
    let mut session = SessionData::fresh("api-close-sub", role);
    session.person_id = Some(actor.id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn close(
    app: &axum::Router,
    project_id: Uuid,
    auth: Option<&str>,
) -> axum::http::Response<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/app/api/projects/{project_id}/close"));
    if let Some(auth) = auth {
        req = req.header("authorization", auth);
    }
    app.clone()
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn a_participant_lawyer_opens_the_closing_notation() {
    let h = build_app().await;
    let project_id = seed_matter(&h.surreal, true).await;
    let lawyer = bearer(
        &h.surreal,
        "lawyer@example.com",
        Role::Lawyer,
        Some(project_id),
    )
    .await;

    let resp = close(&h.app, project_id, Some(&lawyer)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let notation_id: Uuid = json["notation_id"]
        .as_str()
        .expect("the body carries a notation id")
        .parse()
        .expect("a uuid");

    let notation = store::notations::find_by_id(&h.surreal, notation_id)
        .await
        .unwrap()
        .expect("the closing notation was created");
    assert_eq!(
        notation.project_id, project_id,
        "the closing notation is bound to the matter"
    );
}

#[tokio::test]
async fn an_admin_bypasses_scope_and_closes() {
    let h = build_app().await;
    let project_id = seed_matter(&h.surreal, true).await;
    // Admin does NOT participate; the scope check bypasses for admin.
    let admin = bearer(&h.surreal, "admin@example.com", Role::Admin, None).await;

    let resp = close(&h.app, project_id, Some(&admin)).await;
    assert_eq!(resp.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn a_lawyer_outside_the_matter_is_not_found() {
    let h = build_app().await;
    let project_id = seed_matter(&h.surreal, true).await;
    // Lawyer tier, but not a participant of this matter.
    let outsider = bearer(&h.surreal, "outsider@example.com", Role::Lawyer, None).await;

    let resp = close(&h.app, project_id, Some(&outsider)).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_client_caller_is_forbidden() {
    let h = build_app().await;
    let project_id = seed_matter(&h.surreal, true).await;
    let client = bearer(&h.surreal, "client-caller@example.com", Role::Client, None).await;

    let resp = close(&h.app, project_id, Some(&client)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_anonymous_caller_is_unauthenticated() {
    let h = build_app().await;
    let project_id = seed_matter(&h.surreal, true).await;

    let resp = close(&h.app, project_id, None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_matter_with_no_client_is_a_conflict() {
    let h = build_app().await;
    // No client participant, but the acting lawyer participates so scope passes
    // and the command reaches its own no-client refusal.
    let project_id = seed_matter(&h.surreal, false).await;
    let lawyer = bearer(
        &h.surreal,
        "lawyer2@example.com",
        Role::Lawyer,
        Some(project_id),
    )
    .await;

    let resp = close(&h.app, project_id, Some(&lawyer)).await;
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}
