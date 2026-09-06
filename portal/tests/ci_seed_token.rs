//! `POST /auth/ci/seed-token` mints a project-scoped seed session.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use portal::github_oidc::{GitHubActionsClaims, GitHubOidc};
use store::persons::{NewPerson, Role};
use store::test_support::mem_surreal;
use tower::ServiceExt;

async fn fixture_app() -> (axum::Router, String) {
    let surreal = mem_surreal().await;
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: "acme".into(),
            name: "Acme".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::set_repository_url(
        &surreal,
        project.id,
        Some("https://github.com/neon-law-staging/acme"),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &NewPerson::with_role("Lawyer Example", "lawyer@neonlaw.com", Role::Lawyer),
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        lawyer.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();

    let mut state = portal::test_support::app_state(surreal).await;
    state.canonical_host = portal::CanonicalHost::new(Some("staging.neonlaw.com".into()));
    state.github_oidc = GitHubOidc::fixed(GitHubActionsClaims {
        sub: "repo:neon-law-staging/acme:ref:refs/heads/main".into(),
        repository: "neon-law-staging/acme".into(),
        repository_owner: "neon-law-staging".into(),
        git_ref: "refs/heads/main".into(),
        event_name: "push".into(),
        ..GitHubActionsClaims::default()
    });
    (portal::router(state), "github-jwt".into())
}

async fn mint(app: &axum::Router, github_token: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/ci/seed-token")
                .header(header::HOST, "staging.neonlaw.com")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({ "token": github_token })).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn minting_a_ci_token_scopes_it_to_seed_and_the_named_project() {
    let (app, github_token) = fixture_app().await;
    let response = mint(&app, &github_token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let minted: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(minted["project_code"], "acme");
    let token = minted["token"].as_str().expect("minted token");

    let seed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/api/seed")
                .header(header::HOST, "staging.neonlaw.com")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "model": "person",
                        "yaml": "lookup_fields:\n  - email\nrecords: []\n",
                        "overwrite": false,
                        "dry_run": true
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(seed.status(), StatusCode::OK);

    let people = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/app/api/people")
                .header(header::HOST, "staging.neonlaw.com")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(people.status(), StatusCode::FORBIDDEN);
    let people_body = axum::body::to_bytes(people.into_body(), usize::MAX)
        .await
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&people_body).unwrap();
    assert_eq!(document["error"], "scope_violation");
}

#[tokio::test]
async fn a_pull_request_oidc_token_is_refused() {
    let surreal = mem_surreal().await;
    let mut state = portal::test_support::app_state(surreal).await;
    state.canonical_host = portal::CanonicalHost::new(Some("staging.neonlaw.com".into()));
    state.github_oidc = GitHubOidc::fixed(GitHubActionsClaims {
        repository: "neon-law-staging/acme".into(),
        repository_owner: "neon-law-staging".into(),
        git_ref: "refs/heads/main".into(),
        event_name: "pull_request".into(),
        ..GitHubActionsClaims::default()
    });
    let app = portal::router(state);
    let response = mint(&app, "github-jwt").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
