//! #641 Phase 3 (admin cluster) — the Dioxus generic admin listings: the shared
//! `webapp::admin_listing` scaffold renders database-backed rows server-side.
//!
//! Each migrated generic listing is a thin `#[server]` + component pair over
//! the `admin_listing` gate + view helpers and `AdminListingScaffold`. This
//! exercises that shared path through `LawyerJurisdictions`: the rows are in
//! the SSR HTML (readable before hydration), ordered as the page asks, and
//! the empty state renders when there are none. The jurisdiction rows live
//! in `SurrealDB` (ENG-20), so the render context carries both engine handles
//! exactly as `portal::dioxus_app::admin_listing_router` provides them.

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

/// Insert a jurisdiction with the given display name and code.
async fn insert_jurisdiction(surreal: &SurrealDb, name: &str, code: &str) {
    store::jurisdictions::create(
        surreal,
        &store::jurisdictions::NewJurisdiction::new(name, code, "state"),
    )
    .await
    .expect("insert jurisdiction");
}

/// Render `LawyerJurisdictions` against both engines with `role` injected as
/// the viewer tier (mirroring `portal::dioxus_app::inject_viewer_role`, which
/// the real route runs behind its auth + embedded Rego policy gate),
/// returning the SSR HTML body. Owns the process-global
/// `DIOXUS_PUBLIC_PATH` (safe under nextest's process-per-test isolation).
async fn render_jurisdictions_as(
    surreal: &SurrealDb,
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
            "/app/admin/jurisdictions",
            get(render_handler).layer(axum::Extension(role)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::admin_listings::LawyerJurisdictions,
        ));

    let response = router
        .oneshot(
            Request::builder()
                .uri("/app/admin/jurisdictions")
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
async fn admin_listing_scaffold_ssrs_rows_from_the_database_in_order() {
    let surreal = mem_surreal().await;
    insert_jurisdiction(&surreal, "Nevada", "NV").await;
    insert_jurisdiction(&surreal, "California", "CA").await;

    let (status, html) =
        render_jurisdictions_as(&surreal, webapp::people::ViewerRole::Lawyer).await;

    assert_eq!(status, StatusCode::OK);
    // Both rows are in the SSR HTML before any hydration.
    assert!(
        html.contains("California") && html.contains("Nevada"),
        "SSR HTML must carry the database-backed rows before hydration; got: {html}",
    );
    // Ordered ascending by code (CA before NV), the page's order.
    let ca = html.find("California").expect("California row");
    let nv = html.find("Nevada").expect("Nevada row");
    assert!(
        ca < nv,
        "expected California (CA) before Nevada (NV); got: {html}"
    );
    // The heading and column labels render.
    assert!(html.contains("Jurisdictions"), "heading; got: {html}");
    assert!(
        html.contains("Name") && html.contains("Code"),
        "headers; got: {html}"
    );
}

#[tokio::test]
async fn admin_listing_scaffold_ssrs_the_empty_state() {
    let surreal = mem_surreal().await;

    let (status, html) =
        render_jurisdictions_as(&surreal, webapp::people::ViewerRole::Lawyer).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("No rows yet."),
        "an empty listing must render the empty state; got: {html}",
    );
}

#[tokio::test]
async fn admin_listing_load_refuses_a_non_lawyer_viewer() {
    let surreal = mem_surreal().await;
    insert_jurisdiction(&surreal, "Nevada", "NV").await;

    // A direct hit on the generated `#[server]` endpoint need not carry the
    // route's auth + embedded Rego policy gate, so the injected tier defaults to the
    // least-privileged `Client`. The shared gate must refuse it on its
    // own authority, so the lawyer-only rows never reach a non-lawyer caller.
    let (status, html) =
        render_jurisdictions_as(&surreal, webapp::people::ViewerRole::Client).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        !html.contains("Nevada"),
        "a non-lawyer viewer must not see the lawyer-only rows; got: {html}",
    );
    assert!(
        html.contains("Failed to load."),
        "the refused load must render the error state, not the listing; got: {html}",
    );
}
