//! Router-level proof for the client matter page's "Review and sign" action
//! (ENG-558): it must render only for a live envelope's own signer, never
//! for another participant on the same matter, and never for a dead
//! (declined) envelope even to the signer.
//!
//! `server/tests/notation_documents_acl.rs` already pins the participation
//! gate on the notation-PDF routes and the project page's signed-copy link;
//! this file is the sibling proof for the sign action `webapp::portal_project_detail`
//! now renders alongside it.

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
const TEMPLATE_CODE: &str = "onboarding__letter";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    sessions: SessionStore,
    project_code: String,
    project_id: Uuid,
    /// The notation's bound signer.
    signer: Uuid,
    /// A second client participant on the same matter who is not the signer.
    co_client: Uuid,
}

async fn build() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!(
            "navigator-sign-action-visibility-{}",
            Uuid::now_v7()
        )))
        .await
        .unwrap(),
    );
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .expect("canonical seed");

    let signer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap()
    .id;
    let co_client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Spouse", "spouse@example.com", Role::Client),
    )
    .await
    .unwrap()
    .id;

    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-retainer-{}", Uuid::now_v7()),
            name: "Libra retainer".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for pid in [signer, co_client] {
        store::projects::add_participation(&surreal, project.id, pid, "client")
            .await
            .unwrap();
    }

    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        sessions: SessionStore::new(KEY),
        project_code: project.code,
        project_id: project.id,
        signer,
        co_client,
    }
}

async fn new_notation(f: &Fixture, tmpl_id: Uuid) -> Uuid {
    store::notations::create(
        &f.surreal,
        &store::notations::NewNotation::new(
            tmpl_id,
            f.signer,
            f.project_id,
            "sent_for_signature__pending",
        ),
    )
    .await
    .unwrap()
    .id
}

fn cookie_for(sessions: &SessionStore, role: Role, person_id: Uuid) -> String {
    let mut s = SessionData::fresh("sub", role);
    s.person_id = Some(person_id);
    format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&s))
}

async fn matter_page_html(f: &Fixture, cookie: &str) -> String {
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", f.project_code))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn the_sign_action_renders_only_for_a_live_envelopes_own_signer() {
    let f = build().await;
    let tmpl = store::templates::resolve(&f.surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed inserts the retainer template");
    let notation_id = new_notation(&f, tmpl.id).await;
    store::signatures::record_request(
        &f.surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        "env-eng-558-visibility",
    )
    .await
    .unwrap();

    let sign_href = format!("/app/notations/{notation_id}/sign");

    let signer_html = matter_page_html(&f, &cookie_for(&f.sessions, Role::Client, f.signer)).await;
    assert!(
        signer_html.contains(&sign_href) && signer_html.contains("Review and sign"),
        "the bound signer must see the review-and-sign action for their own live envelope",
    );

    let co_client_html =
        matter_page_html(&f, &cookie_for(&f.sessions, Role::Client, f.co_client)).await;
    assert!(
        !co_client_html.contains(&sign_href),
        "another participant on the same matter must never see the action for someone else's envelope",
    );
}

#[tokio::test]
async fn a_declined_envelope_never_offers_the_sign_action_even_to_its_own_signer() {
    let f = build().await;
    let tmpl = store::templates::resolve(&f.surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed inserts the retainer template");
    let notation_id = new_notation(&f, tmpl.id).await;
    store::signatures::record_request(
        &f.surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        "env-eng-558-declined",
    )
    .await
    .unwrap();
    store::signatures::stamp_declined(
        &f.surreal,
        store::signatures::SignatureProvider::DocuSign,
        "env-eng-558-declined",
    )
    .await
    .unwrap();

    let html = matter_page_html(&f, &cookie_for(&f.sessions, Role::Client, f.signer)).await;
    assert!(
        !html.contains(&format!("/app/notations/{notation_id}/sign")),
        "a declined envelope must never offer a signing action",
    );
    assert!(
        html.contains("Signing was declined. Contact the firm to continue."),
        "the declined label must read correctly, not merely distinct from a live one",
    );
}
