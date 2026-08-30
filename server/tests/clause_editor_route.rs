#![allow(clippy::doc_markdown)]
//! Integration tests for `GET /lawyer/notations/{id}/clauses` — the lawyer clause
//! editor, migrated to Dioxus in #956 Phase 4.
//!
//! One path serves two surfaces, and axum cannot register two `GET` handlers on
//! it, so the Dioxus route carries a pre-layer that answers `?format=json` and
//! otherwise falls through to the render. Both halves are asserted here:
//!
//!   1. The HTML editor renders each clause into its own `<textarea>` with the
//!      edit / reorder / delete `POST`s and the session CSRF token.
//!   2. `?format=json` still returns the list the
//!      `navigator retainer clause list` CLI parses — same shape, same keys.
//!   3. An unknown notation is a `404` on both.

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

const TEMPLATE_CODE: &str = "onboarding__letter";
const KEY: &str = "clause-editor-route-test-key";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    notation_id: uuid::Uuid,
    auth: String,
}

async fn build() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-clause-editor-route-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", uuid::Uuid::now_v7()),
            name: "Matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "libra@example.com",
            "libra@example.com",
            Role::Client,
        ),
    )
    .await
    .unwrap();
    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .unwrap();
    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, client.id, project.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;

    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "lawyer@neonlaw.com",
            "lawyer@neonlaw.com",
            Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    store::projects::add_participation(&surreal, project.id, lawyer.id, "lawyer")
        .await
        .unwrap();
    let mut session = SessionData::fresh("clause-editor-sub", Role::Lawyer);
    session.person_id = Some(lawyer.id);
    let auth = format!("Bearer {}", SessionStore::new(KEY).encode(&session));

    Fixture {
        app,
        surreal,
        notation_id,
        auth,
    }
}

impl Fixture {
    async fn get(&self, uri: &str) -> (StatusCode, String) {
        let resp = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", &self.auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn path(&self) -> String {
        format!("/lawyer/notations/{}/clauses", self.notation_id)
    }
}

#[tokio::test]
async fn the_editor_renders_each_clause_with_its_write_forms() {
    let f = build().await;
    let first =
        store::notation_clauses::append(&f.surreal, f.notation_id, "The firm may withdraw.", None)
            .await
            .unwrap();
    let second =
        store::notation_clauses::append(&f.surreal, f.notation_id, "Fees are due monthly.", None)
            .await
            .unwrap();

    let (status, html) = f.get(&f.path()).await;
    assert_eq!(status, StatusCode::OK);

    // Each clause body is the textarea's *content*, not a `value` attribute —
    // getting that wrong renders an empty box that blanks the clause on save.
    assert!(
        html.contains(">The firm may withdraw.</textarea>"),
        "{html}"
    );
    assert!(html.contains(">Fees are due monthly.</textarea>"), "{html}");

    // Every write the editor offered is still reachable, per clause.
    for id in [first, second] {
        for suffix in ["/edit", "/move", "/delete"] {
            assert!(
                html.contains(&format!("action=\"{}/{id}{suffix}\"", f.path())),
                "missing {suffix} for {id}: {html}"
            );
        }
    }
    // ...and the add form posts to the collection path.
    assert!(html.contains(&format!("action=\"{}\"", f.path())), "{html}");
    // Every one of them carries the session CSRF token, or the mutation 403s.
    assert!(html.contains("name=\"_csrf\""), "{html}");
}

#[tokio::test]
async fn an_empty_notation_renders_the_editor_with_nothing_to_edit() {
    let f = build().await;
    let (status, html) = f.get(&f.path()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("No custom clauses yet."), "{html}");
    // The add form is still there — an empty editor is where the first clause
    // gets written.
    assert!(html.contains("Add clause"), "{html}");
}

#[tokio::test]
async fn the_json_surface_the_cli_reads_still_answers_on_the_same_path() {
    // `navigator retainer clause list` GETs this exact URL and parses
    // `{id, position, body, system_authored}`. The Dioxus render must not have
    // taken the path away from it.
    let f = build().await;
    let id = store::notation_clauses::append(&f.surreal, f.notation_id, "Only clause.", None)
        .await
        .unwrap();

    let (status, body) = f.get(&format!("{}?format=json", f.path())).await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).expect("a JSON array: {body}");
    assert_eq!(rows.len(), 1, "{body}");
    assert_eq!(rows[0]["id"], id.to_string());
    assert_eq!(rows[0]["body"], "Only clause.");
    assert_eq!(rows[0]["position"], 0, "positions are 0-based: {body}");
    // No authoring person on a fixture clause, so the CLI reads it as
    // system-authored.
    assert_eq!(rows[0]["system_authored"], true);
}

#[tokio::test]
async fn an_unknown_notation_is_404_on_both_surfaces() {
    let f = build().await;
    let missing = uuid::Uuid::now_v7();
    let path = format!("/lawyer/notations/{missing}/clauses");

    let (status, _) = f.get(&path).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the HTML editor");

    let (status, _) = f.get(&format!("{path}?format=json")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "the JSON surface");
}
