//! Integration test for the firm brand-font download at
//! `GET /app/team/fonts/gorp-serif.zip`.
//!
//! Proves the route is wired under the team home's own prefix and that an
//! authenticated firm session streams the ZIP staged in the private documents
//! bucket with the right headers. The `.otf` bytes are never committed — the
//! bucket is the only place they live, so this route is the only path to them.
//!
//! This harness deliberately disables auth (`AuthConfig::new(true, …)`)
//! and runs the policy in passthrough (`PolicyClient::passthrough()`), so the
//! role gate is NOT exercised here — it is embedded Rego's `/app/team` rules,
//! proven per tier in `portal/policy/navigator_test.rego`
//! (`test_lawyer_reaches_font_download`, `test_admin_reaches_font_download`,
//! `test_owner_reaches_font_download`, `test_clerk_reaches_font_download`,
//! `test_client_denied_on_font_download`,
//! `test_anonymous_denied_on_font_download`) and end-to-end by the browser
//! suite.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "test-session-key-not-for-production";
const GORP_OTF_ZIP_KEY: &str = "fonts/gorp-serif/gorp-serif-otf.zip";
/// Minimal well-formed ZIP header — enough to prove the handler streams
/// exactly the staged bytes back; the handler does not parse the archive.
const STAGED_ZIP: &[u8] = b"PK\x03\x04 staged gorp-serif faces";

struct Fixture {
    app: axum::Router,
    sessions: SessionStore,
}

async fn build() -> Fixture {
    let surreal = mem_surreal().await;
    // The ZIP lives in the private documents lane (`storage`), never the
    // public assets bucket — the route is the only path to the bytes.
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("navigator-brand-fonts-e2e-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );
    storage
        .put(GORP_OTF_ZIP_KEY, STAGED_ZIP, "application/zip")
        .await
        .unwrap();

    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        sessions: SessionStore::new(KEY),
    }
}

fn cookie_for(sessions: &SessionStore, role: Role) -> String {
    let mut s = SessionData::fresh("firm-sub", role);
    s.person_id = Some(Uuid::now_v7());
    format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&s))
}

fn lawyer_cookie(sessions: &SessionStore) -> String {
    cookie_for(sessions, Role::Lawyer)
}

#[tokio::test]
async fn lawyer_downloads_the_gorp_serif_zip() {
    let f = build().await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/app/team/fonts/gorp-serif.zip")
                .header("cookie", lawyer_cookie(&f.sessions))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "application/zip");
    assert!(resp.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .contains("gorp-serif.zip"));
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), STAGED_ZIP);
}

#[tokio::test]
async fn a_missing_bucket_object_is_a_loud_502() {
    // Nothing staged in the documents bucket: the handler surfaces the gap as
    // a 502 rather than a 200 with an empty or wrong body.
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-brand-fonts-e2e-empty-{}",
            Uuid::now_v7()
        )))
        .await
        .unwrap(),
    );
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let sessions = SessionStore::new(KEY);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/app/team/fonts/gorp-serif.zip")
                .header("cookie", lawyer_cookie(&sessions))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

/// The route moved off `/app/lawyer` so that a Clerk — the narrowest tier the team
/// home renders for — reaches it through the page's own prefix rather than an
/// exact-path exception. This proves the wiring answers a Clerk session; the
/// tier gate itself is the Rego suite's, since this harness runs the policy in
/// passthrough.
#[tokio::test]
async fn a_clerk_session_reaches_the_download_at_the_team_path() {
    let f = build().await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/app/team/fonts/gorp-serif.zip")
                .header("cookie", cookie_for(&f.sessions, Role::Clerk))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(bytes.as_ref(), STAGED_ZIP);
}

/// The old `/app/lawyer` path is gone, not aliased. A stale bookmark must 404
/// rather than quietly keep working — otherwise the exact-path Clerk exception
/// this move retired would still be reachable.
#[tokio::test]
async fn the_retired_lawyer_path_is_not_served() {
    let f = build().await;
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/app/lawyer/fonts/gorp-serif.zip")
                .header("cookie", lawyer_cookie(&f.sessions))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
