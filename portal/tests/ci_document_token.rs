//! Router-level least-privilege tests for the minted document-verification CI credential.

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::Router;
use portal::github_oidc::{GitHubActionsClaims, GitHubOidc};
use portal::session::{now_unix_secs, SessionSource, SessionStore};
use portal::test_support::TEST_SESSION_KEY;
use store::persons::{NewPerson, Role};
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    app: Router,
    project_id: Uuid,
    other_project_id: Uuid,
    lawyer_id: Uuid,
}

async fn fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let project_id = store::projects::create(
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
    .unwrap()
    .id;
    let other_project_id = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: "widgets".into(),
            name: "Widgets".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id;
    store::projects::set_repository_url(
        &surreal,
        project_id,
        Some("https://github.com/neon-law-staging/acme"),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &NewPerson::with_role("Synthetic Lawyer", "ci-lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    for project in [project_id, other_project_id] {
        store::projects::designate_dri_in_surreal(
            &surreal,
            project,
            lawyer.id,
            store::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();
    }

    let mut state = portal::test_support::app_state(surreal.clone()).await;
    let args = store::documents::IngestArgs {
        project_id,
        source: store::documents::source::UPLOAD,
        filename: "agreement.pdf",
        kind: "unclassified",
        content_type: "application/pdf",
        description: None,
        visibility: store::documents::visibility::INTERNAL,
        secondary_storage_key: None,
    };
    let identity = store::documents::DocumentIdentity {
        slug: Some("agreement"),
        ..Default::default()
    };
    store::assets::file_revision(
        &surreal,
        &state.storage,
        &args,
        &identity,
        b"synthetic bytes",
    )
    .await
    .unwrap();
    state.canonical_host = portal::CanonicalHost::new(Some("staging.neonlaw.com".into()));
    state.github_oidc = GitHubOidc::fixed(GitHubActionsClaims {
        sub: "repo:neon-law-staging/acme:ref:refs/heads/main".into(),
        repository: "neon-law-staging/acme".into(),
        repository_owner: "neon-law-staging".into(),
        git_ref: "refs/heads/main".into(),
        event_name: "push".into(),
        jti: "jti-document-acme".into(),
        exp: 4_000_000_000,
        ..Default::default()
    });

    Fixture {
        app: portal::router(state),
        project_id,
        other_project_id,
        lawyer_id: lawyer.id,
    }
}

async fn mint(app: &Router) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/ci/document-token")
                .header(header::HOST, "staging.neonlaw.com")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"token":"github-jwt"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    json_response(response).await
}

async fn json_response(response: Response<Body>) -> (StatusCode, serde_json::Value) {
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&body).unwrap_or_else(
        |_| serde_json::json!({"body": String::from_utf8_lossy(&body).to_string()}),
    );
    (status, json)
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

async fn request(
    app: &Router,
    method: &str,
    uri: &str,
    authorization: Option<&str>,
    cookie: Option<&str>,
    body: Body,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "staging.neonlaw.com");
    if let Some(authorization) = authorization {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, format!("navigator_session={cookie}"));
    }
    let response = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    json_response(response).await
}

#[tokio::test]
async fn a_minted_document_token_allows_only_its_project_metadata_for_bearer_and_cookie() {
    let fixture = fixture().await;
    let (status, minted) = mint(&fixture.app).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(minted["project_code"], "acme");
    let token = minted["token"].as_str().unwrap();

    for credential in [Some(bearer(token)), None] {
        let cookie = credential.is_none().then_some(token);
        let authorization = credential.as_deref();
        let (status, projects) = request(
            &fixture.app,
            "GET",
            "/app/api/projects",
            authorization,
            cookie,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(projects.as_array().unwrap().len(), 1);
        assert_eq!(projects[0]["code"], "acme");
        assert_eq!(projects[0].as_object().unwrap().len(), 2);

        let (status, revisions) = request(
            &fixture.app,
            "GET",
            &format!(
                "/app/api/projects/{}/documents/revisions?slug=agreement",
                fixture.project_id
            ),
            authorization,
            cookie,
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(revisions["revisions"].as_array().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn a_minted_document_token_refuses_cross_project_downloads_writes_and_routes() {
    let fixture = fixture().await;
    let (_, minted) = mint(&fixture.app).await;
    let token = minted["token"].as_str().unwrap();
    let auth = bearer(token);

    for (authorization, cookie) in [(Some(auth.as_str()), None), (None, Some(token))] {
        for (method, uri, body) in [
            (
                "GET",
                format!(
                    "/app/api/projects/{}/documents/revisions?slug=agreement",
                    fixture.other_project_id
                ),
                Body::empty(),
            ),
            (
                "GET",
                format!("/app/projects/acme/documents/{}/download", Uuid::new_v4()),
                Body::empty(),
            ),
            (
                "GET",
                format!(
                    "/app/lawyer/notations/{}/documents/document",
                    Uuid::new_v4()
                ),
                Body::empty(),
            ),
            (
                "POST",
                format!("/app/api/projects/{}/documents", fixture.project_id),
                Body::from("{}"),
            ),
            (
                "POST",
                "/app/projects/acme/documents/upload".to_string(),
                Body::empty(),
            ),
            ("GET", "/app/api/people".to_string(), Body::empty()),
        ] {
            let (status, body) =
                request(&fixture.app, method, &uri, authorization, cookie, body).await;
            let body_text = body.to_string();
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {body_text}");
            assert_eq!(body["error"], "scope_violation");
            assert!(
                !body_text.contains("synthetic bytes"),
                "{method} {uri}: {body_text}"
            );
            assert!(
                !body_text.contains("https://"),
                "{method} {uri}: {body_text}"
            );
        }
    }
}

#[tokio::test]
async fn old_unscoped_ci_credentials_fail_closed_through_bearer_and_cookie_admission() {
    let fixture = fixture().await;
    let sessions = SessionStore::new(TEST_SESSION_KEY);
    let legacy = sessions.encode(&portal::SessionData {
        sub: "legacy-ci".into(),
        email: Some("ci-lawyer@example.com".into()),
        person_id: Some(fixture.lawyer_id),
        exp: now_unix_secs() + 60,
        role: Role::Lawyer,
        csrf_token: "legacy-csrf".into(),
        source: SessionSource::Ci,
        provider: None,
        viewing_as_dri: None,
        scope: None,
    });

    let (bearer_status, _) = request(
        &fixture.app,
        "GET",
        "/app/api/projects",
        Some(&bearer(&legacy)),
        None,
        Body::empty(),
    )
    .await;
    let (cookie_status, _) = request(
        &fixture.app,
        "GET",
        "/app/api/projects",
        None,
        Some(&legacy),
        Body::empty(),
    )
    .await;
    assert_eq!(bearer_status, StatusCode::UNAUTHORIZED);
    assert_eq!(cookie_status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_document_token_expiry_is_enforced_for_bearer_and_cookie() {
    let fixture = fixture().await;
    let (_, minted) = mint(&fixture.app).await;
    let sessions = SessionStore::new(TEST_SESSION_KEY);
    let mut expired = sessions.decode(minted["token"].as_str().unwrap()).unwrap();
    expired.exp = now_unix_secs() - 1;
    let expired = sessions.encode(&expired);

    for (authorization, cookie) in [(Some(bearer(&expired)), None), (None, Some(expired))] {
        let (status, _) = request(
            &fixture.app,
            "GET",
            "/app/api/projects",
            authorization.as_deref(),
            cookie.as_deref(),
            Body::empty(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn a_github_oidc_document_mint_is_single_use_and_pr_merge_refs_are_read_only() {
    let fixture = fixture().await;
    let (first, _) = mint(&fixture.app).await;
    assert_eq!(first, StatusCode::OK);
    let (replay, body) = mint(&fixture.app).await;
    assert_eq!(replay, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");

    let surreal = mem_surreal().await;
    let project_id = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: "branch".into(),
            name: "Branch".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id;
    store::projects::set_repository_url(
        &surreal,
        project_id,
        Some("https://github.com/neon-law-staging/branch"),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &NewPerson::with_role("Branch Lawyer", "branch-lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        project_id,
        lawyer.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
    let mut state = portal::test_support::app_state(surreal).await;
    state.canonical_host = portal::CanonicalHost::new(Some("staging.neonlaw.com".into()));
    state.github_oidc = GitHubOidc::fixed(GitHubActionsClaims {
        sub: "repo:neon-law-staging/branch:pull_request".into(),
        repository: "neon-law-staging/branch".into(),
        repository_owner: "neon-law-staging".into(),
        git_ref: "refs/pull/17/merge".into(),
        event_name: "pull_request".into(),
        jti: "jti-document-pr".into(),
        exp: 4_000_000_000,
        ..Default::default()
    });
    let app = portal::router(state);
    let (status, body) = mint(&app).await;
    assert_eq!(status, StatusCode::OK);
    let token = body["token"].as_str().unwrap();
    let (status, body) = request(
        &app,
        "POST",
        "/app/api/projects/whatever/documents",
        Some(&bearer(token)),
        None,
        Body::from("{}"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "scope_violation");
}
