//! Integration test: a Project that records no repository URL shows no
//! repository pointer on `GET /app/projects/:code`.
//!
//! A Project's repository is a whole URL **stored on the row**, and a matter is
//! free not to have one — a matter opens before anyone creates its repository.
//! That is a legitimate outcome rather than a degraded one, so the lawyer matter
//! page simply omits the pointer.
//!
//! Nothing derives a URL to fill the gap, and that is the point of this test.
//! A composed `{host}/{org}/{code}` coordinate always exists, so it produced a
//! confident link to a repository that might not — and, when the host fell back
//! to a public forge, into a namespace the Firm does not control.
//!
//! The populated case lives in `project_repo_clone_url.rs`. Each target asserts
//! one state of the column, which keeps either failure legible on its own.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;

const KEY: &str = "test-session-key-not-for-production";

#[tokio::test]
async fn a_project_with_no_recorded_repository_has_no_repository_section() {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-repo-pointer-absent-test"))
            .await
            .unwrap(),
    );

    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: "libra-formation".into(),
            name: "Libra formation".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Lawyer Member", "lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    for (person_id, participation) in [(lawyer.id, "lawyer"), (client.id, "client")] {
        store::projects::add_participation(&surreal, project.id, person_id, participation)
            .await
            .unwrap();
    }

    let sessions = SessionStore::new(KEY);
    let mut lawyer_session = SessionData::fresh("lawyer-sub", Role::Lawyer);
    lawyer_session.person_id = Some(lawyer.id);
    let lawyer_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&lawyer_session));

    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let runtime = Arc::new(workflows::InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", project.code))
                .header("cookie", &lawyer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();

    // The page renders — so the assertions below are about an absent pointer on
    // a real matter page, not a vacuous pass on an empty body.
    assert!(
        html.contains("Libra formation"),
        "the lawyer matter page must render the matter it names"
    );
    assert!(
        !html.contains("Source repository"),
        "a matter recording no repository URL must show no repository pointer"
    );
    // Nothing invents one. A derivation would have produced a link here.
    // Measured over the matter page above the `/app` footer: the footer's
    // own off-site links — the firm's other brands and its association
    // membership — are the firm's, not a pointer composed for this matter.
    let page = html
        .split_once(r#"<footer class="app-footer""#)
        .map_or(html.as_str(), |(page, _)| page);
    assert!(
        !page.contains("https://"),
        "no URL may be composed to stand in for the absent one"
    );
}
