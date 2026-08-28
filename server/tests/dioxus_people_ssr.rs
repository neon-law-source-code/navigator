//! #355 Tranche 1 / #641 — the Dioxus people directory: database-backed SSR and
//! the JSON:API `?sort=` URL contract.
//!
//! `webapp::people::AdminPeople`'s server function reads the request query and
//! the `store::Db` handle injected through the render context, queries the
//! shared directory, and `use_server_future` server-side renders the sorted rows
//! into the HTML — readable before hydration, sort headers as real anchors.
//!
//! ENG-304 deleted the `/lawyer/people` mirror, so `/app/admin/people` is the one
//! browser surface this covers, and its gate is `require_admin`.

use std::any::Any;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use dioxus_server::{render_handler, FullstackState, ServeConfig};
use http_body_util::BodyExt;
use store::persons::Role;
use tower::ServiceExt;

/// A minimal, CDN-free bundle `index.html` with the `main` mount point.
const INDEX_HTML: &str = "<!DOCTYPE html>\n\
<html lang=\"en\"><head><meta charset=\"UTF-8\" />\
<title>Neon Law Navigator</title></head>\
<body><div id=\"main\"></div></body></html>\n";

/// Insert a person with a given display name and email.
async fn insert_person(surreal: &store::surreal::SurrealDb, name: &str, email: &str) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::new(name.to_string(), email.to_string()),
    )
    .await
    .expect("insert person");
}

/// Insert a person with an explicit role, for asserting the directory renders
/// the user-facing role label rather than the internal `Role::as_str()` token.
async fn insert_person_with_role(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
    role: Role,
) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name.to_string(), email.to_string(), role),
    )
    .await
    .expect("insert person");
}

/// Render `AdminPeople` at `uri` against `db` with `role` injected as the viewer
/// tier (mirroring `portal::dioxus_app::inject_viewer_role`, which the real route
/// runs behind its auth + embedded policy gate), returning the SSR HTML body. Owns the
/// process-global `DIOXUS_PUBLIC_PATH` (safe under nextest's process-per-test
/// isolation).
async fn render_people_as(
    surreal: &store::surreal::SurrealDb,
    uri: &str,
    role: webapp::people::ViewerRole,
) -> (StatusCode, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("index.html"), INDEX_HTML).expect("write index.html");
    std::env::set_var("DIOXUS_PUBLIC_PATH", dir.path());
    let provider_surreal = surreal.clone();
    let cfg = ServeConfig::new().context_providers(Arc::new(vec![
        // The people surfaces read `persons` from the other engine
        // (#1093; ENG-19); a server fn only reaches what this list
        // provides, so omitting it is a 500 at render time.
        Box::new(move || Box::new(provider_surreal.clone()) as Box<dyn Any>)
            as Box<dyn Fn() -> Box<dyn Any> + Send + Sync>,
    ]));

    let router: Router = Router::<FullstackState>::new()
        .route(
            "/app/admin/people",
            get(render_handler).layer(axum::Extension(role)),
        )
        .with_state(FullstackState::new(cfg, webapp::people::AdminPeople));

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
async fn admin_people_component_ssrs_directory_from_the_database() {
    let surreal = store::test_support::mem_surreal().await;
    store::test_support::dri_person(&surreal).await;

    let (status, html) = render_people_as(
        &surreal,
        "/app/admin/people",
        webapp::people::ViewerRole::Admin,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("DRI Fixture"),
        "SSR HTML must carry the database-backed person directory before hydration; got: {html}",
    );
}

#[tokio::test]
async fn admin_people_renders_role_display_labels_not_raw_tokens() {
    let surreal = store::test_support::mem_surreal().await;
    insert_person_with_role(&surreal, "Cleo Clerk", "cleo@test.invalid", Role::Clerk).await;

    let (status, html) = render_people_as(
        &surreal,
        "/app/admin/people",
        webapp::people::ViewerRole::Admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The role column must show the user-facing label from the contract,
    // not the internal `Role::as_str()` token (`clerk`).
    assert!(
        html.contains("Clerk (non-lawyer)"),
        "role column must render the display label, not the raw token; got: {html}",
    );
    assert!(
        !html.contains(">clerk<"),
        "the internal lowercase role token must not reach the UI; got: {html}",
    );
}

#[tokio::test]
async fn admin_people_honors_jsonapi_sort_descending_by_name() {
    let surreal = store::test_support::mem_surreal().await;
    insert_person(&surreal, "Aaa Person", "aaa@test.invalid").await;
    insert_person(&surreal, "Zzz Person", "zzz@test.invalid").await;

    let (status, html) = render_people_as(
        &surreal,
        "/app/admin/people?sort=-name",
        webapp::people::ViewerRole::Admin,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let zzz = html.find("Zzz Person").expect("Zzz row rendered");
    let aaa = html.find("Aaa Person").expect("Aaa row rendered");
    assert!(
        zzz < aaa,
        "?sort=-name must render descending (Zzz before Aaa); got: {html}",
    );
    // The sort headers are real anchors carrying the toggled ?sort= value.
    assert!(
        html.contains("/app/admin/people?sort=name")
            || html.contains("/app/admin/people?sort=-name"),
        "sort header must be an anchor with a ?sort= toggle; got: {html}",
    );
}

#[tokio::test]
async fn list_admin_people_refuses_a_non_admin_viewer() {
    let surreal = store::test_support::mem_surreal().await;
    insert_person(&surreal, "Secret Person", "secret@test.invalid").await;

    // A direct hit on the generated `#[server]` endpoint need not carry the
    // route's auth + embedded Rego policy gate, so the injected tier defaults to
    // the least-privileged `Client`. The server function must refuse it on its
    // own authority, so the directory never reaches a non-admin caller.
    //
    // `Lawyer` is the interesting tier here rather than `Client`: since ENG-304
    // deleted the `/lawyer/people` mirror, a lawyer has no browser surface onto
    // the directory at all, and `require_admin` is what makes that true.
    //
    // `require_admin` commits a real `403` rather than answering `200` with an
    // error body, so the refusal reads as a refusal to clients, caches, and
    // monitoring — not as a successful render.
    for (label, role) in [
        ("client", webapp::people::ViewerRole::Client),
        ("clerk", webapp::people::ViewerRole::Clerk),
        ("lawyer", webapp::people::ViewerRole::Lawyer),
    ] {
        let (status, html) = render_people_as(&surreal, "/app/admin/people", role).await;

        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "a {label} viewer must be refused with a real 403; got: {html}",
        );
        assert!(
            !html.contains("Secret Person"),
            "a {label} viewer must not see the admin-only directory; got: {html}",
        );
        assert!(
            html.contains("Failed to load people."),
            "the refused load must render the error state, not the directory; got: {html}",
        );
    }
}
