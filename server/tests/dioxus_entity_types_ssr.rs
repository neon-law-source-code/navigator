//! #641 Phase 3 (admin cluster) — the Dioxus entity-types directory:
//! database-backed SSR and the JSON:API `?sort=` URL contract.
//!
//! `webapp::entity_types::LawyerEntityTypes`'s server function reads the request
//! query and the `store::Db` handle injected through the render context, queries
//! the shared `store::entity_types` command, and `use_server_future` server-side
//! renders the sorted rows into the HTML — readable before hydration, the sort
//! header a real anchor.

use std::any::Any;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use dioxus_server::{render_handler, FullstackState, ServeConfig};
use http_body_util::BodyExt;
use store::surreal::SurrealDb;
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// A minimal, CDN-free bundle `index.html` with the `main` mount point.
const INDEX_HTML: &str = "<!DOCTYPE html>\n\
<html lang=\"en\"><head><meta charset=\"UTF-8\" />\
<title>Neon Law Navigator</title></head>\
<body><div id=\"main\"></div></body></html>\n";

/// Insert an entity type with the given display name — into `SurrealDB`,
/// where the table lives (ENG-20).
async fn insert_entity_type(surreal: &SurrealDb, name: &str) {
    store::entity_types::create(surreal, name)
        .await
        .expect("insert entity_type");
}

/// Render `LawyerEntityTypes` at `uri` against both engines with `role`
/// injected as the viewer tier (mirroring
/// `portal::dioxus_app::inject_viewer_role`, which the real route runs behind
/// its auth + embedded Rego policy gate), returning the SSR HTML body. The
/// entity-type rows live in `SurrealDB` (ENG-20), so the render context
/// carries both engine handles exactly as
/// `portal::dioxus_app::entity_types_router` provides them. Owns the
/// process-global `DIOXUS_PUBLIC_PATH` (safe under nextest's process-per-test
/// isolation).
async fn render_entity_types_as(
    surreal: &SurrealDb,
    uri: &str,
    role: webapp::people::ViewerRole,
) -> (StatusCode, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("index.html"), INDEX_HTML).expect("write index.html");
    std::env::set_var("DIOXUS_PUBLIC_PATH", dir.path());
    let provider_surreal = surreal.clone();
    let cfg = ServeConfig::new().context_providers(Arc::new(vec![Box::new(move || {
        Box::new(provider_surreal.clone()) as Box<dyn Any>
    })
        as Box<dyn Fn() -> Box<dyn Any> + Send + Sync>]));

    let router: Router = Router::<FullstackState>::new()
        .route(
            "/app/admin/entity-types",
            get(render_handler).layer(axum::Extension(role)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::entity_types::LawyerEntityTypes,
        ));

    let response = router
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    std::env::remove_var("DIOXUS_PUBLIC_PATH");
    (
        status,
        String::from_utf8(body.to_vec()).expect("utf-8 body"),
    )
}

#[tokio::test]
async fn lawyer_entity_types_component_ssrs_directory_from_the_database() {
    let surreal = mem_surreal().await;
    insert_entity_type(&surreal, "LLC").await;
    insert_entity_type(&surreal, "Trust").await;

    let (status, html) = render_entity_types_as(
        &surreal,
        "/app/admin/entity-types",
        webapp::people::ViewerRole::Lawyer,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("LLC") && html.contains("Trust"),
        "SSR HTML must carry the database-backed entity-type directory before \
         hydration; got: {html}",
    );
}

#[tokio::test]
async fn lawyer_entity_types_honors_jsonapi_sort_descending_by_name() {
    let surreal = mem_surreal().await;
    insert_entity_type(&surreal, "Aaa Type").await;
    insert_entity_type(&surreal, "Zzz Type").await;

    let (status, html) = render_entity_types_as(
        &surreal,
        "/app/admin/entity-types?sort=-name",
        webapp::people::ViewerRole::Lawyer,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let zzz = html.find("Zzz Type").expect("Zzz row rendered");
    let aaa = html.find("Aaa Type").expect("Aaa row rendered");
    assert!(
        zzz < aaa,
        "?sort=-name must render descending (Zzz before Aaa); got: {html}",
    );
    // The sort header is a real anchor carrying the toggled ?sort= value.
    assert!(
        html.contains("/app/admin/entity-types?sort=name")
            || html.contains("/app/admin/entity-types?sort=-name"),
        "sort header must be an anchor with a ?sort= toggle; got: {html}",
    );
}

#[tokio::test]
async fn list_entity_types_refuses_a_non_lawyer_viewer() {
    let surreal = mem_surreal().await;
    insert_entity_type(&surreal, "Secret LLC").await;

    // A direct hit on the generated `#[server]` endpoint need not carry the
    // route's auth + embedded Rego policy gate, so the injected tier defaults to the
    // least-privileged `Client`. The server function must refuse it on its own
    // authority, so the lawyer-only directory never reaches a non-lawyer caller.
    let (status, html) = render_entity_types_as(
        &surreal,
        "/app/admin/entity-types",
        webapp::people::ViewerRole::Client,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains("Secret LLC"),
        "a non-lawyer viewer must not see the lawyer-only directory; got: {html}",
    );
    assert!(
        html.contains("Failed to load entity types."),
        "the refused load must render the error state, not the directory; got: {html}",
    );
}
