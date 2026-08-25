//! #956 Phase 4 — the Dioxus Clerk surface: database-backed SSR of the
//! supervised, read-only Project lens.
//!
//! `webapp::clerk::ClerkProjects` / `ClerkProjectDetail` resolve their server
//! functions during SSR, so the supervised matters and the supervising lawyer's
//! name are in the server-rendered HTML before hydration. These tests drive the
//! same injections the real route runs behind its auth + embedded Rego policy gate (the viewer
//! tier from `portal::dioxus_app::inject_viewer_role`, the linked `persons.id`
//! from `inject_person_id`) and assert the three properties the page
//! guaranteed: the supervisor is always named, no lawyer workbench surface
//! leaks, and anything the Clerk may not see is a `404`.

use std::any::Any;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use dioxus_server::{render_handler, FullstackState, ServeConfig};
use http_body_util::BodyExt;
use tower::ServiceExt;
use uuid::Uuid;

/// A minimal, CDN-free bundle `index.html` with the `main` mount point.
const INDEX_HTML: &str = "<!DOCTYPE html>\n\
<html lang=\"en\"><head><meta charset=\"UTF-8\" />\
<title>Neon Law Navigator</title></head>\
<body><div id=\"main\"></div></body></html>\n";

/// Insert a person with an explicit role.
async fn insert_person(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
    role: store::persons::Role,
) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name.to_string(), email.to_string(), role),
    )
    .await
    .expect("insert person")
    .id
}

/// Seed a matter the Clerk supervises: a firm-side participation row for the
/// Clerk plus a licensed lawyer as the flagged lawyer DRI. Both are required —
/// `store::access::supervised_projects` hides a matter whose supervisor is not a
/// currently licensed `lawyer`/`admin` person.
async fn seed_supervised_project(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    clerk_id: Uuid,
    lawyer_id: Uuid,
) -> store::projects::Project {
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("atlas-{}", Uuid::now_v7()),
            name: name.to_string(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert project");
    store::projects::designate_dri_in_surreal(
        surreal,
        project.id,
        lawyer_id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .expect("designate supervising lawyer");
    store::projects::add_participation(surreal, project.id, clerk_id, "clerk")
        .await
        .expect("assign Clerk to project");
    project
}

/// Stage a CDN-free bundle `index.html` and point the process-global
/// `DIOXUS_PUBLIC_PATH` at it (safe under nextest's process-per-test isolation).
/// The returned handle must outlive the render.
fn staged_bundle() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("index.html"), INDEX_HTML).expect("write index.html");
    std::env::set_var("DIOXUS_PUBLIC_PATH", dir.path());
    dir
}

/// The render context the Clerk routes provide: both store handles the
/// `#[server]` loaders query through. The supervising lawyer the Clerk
/// lens names is a `persons` row — a config that omits the store renders
/// a 500.
fn clerk_config(surreal: &store::surreal::SurrealDb) -> ServeConfig {
    let provider_surreal = surreal.clone();
    ServeConfig::new().context_providers(Arc::new(vec![Box::new(move || {
        Box::new(provider_surreal.clone()) as Box<dyn Any>
    })
        as Box<dyn Fn() -> Box<dyn Any> + Send + Sync>]))
}

/// The two request extensions `portal::dioxus_app::clerk_router` injects behind
/// the auth + embedded Rego policy gate: the viewer's tier and their linked `persons.id`.
fn injections(
    role: webapp::people::ViewerRole,
    person_id: Option<Uuid>,
) -> (
    axum::Extension<webapp::people::ViewerRole>,
    axum::Extension<webapp::portal_project_list::PersonId>,
) {
    (
        axum::Extension(role),
        axum::Extension(webapp::portal_project_list::PersonId(
            person_id.map(|id| id.to_string()),
        )),
    )
}

/// Drive one `GET` through the router and read the SSR body.
async fn fetch(router: Router, uri: &str) -> (StatusCode, String) {
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

async fn render_clerk_list(
    surreal: &store::surreal::SurrealDb,
    role: webapp::people::ViewerRole,
    person_id: Option<Uuid>,
) -> (StatusCode, String) {
    let _bundle = staged_bundle();
    let (viewer, person) = injections(role, person_id);
    let router: Router = Router::<FullstackState>::new()
        .route("/app/projects", get(render_handler))
        .with_state(FullstackState::new(
            clerk_config(surreal),
            webapp::clerk::ClerkProjects,
        ))
        .layer(person)
        .layer(viewer);
    fetch(router, "/app/projects").await
}

async fn render_clerk_detail(
    surreal: &store::surreal::SurrealDb,
    project_code: &str,
    role: webapp::people::ViewerRole,
    person_id: Option<Uuid>,
) -> (StatusCode, String) {
    let _bundle = staged_bundle();
    let (viewer, person) = injections(role, person_id);
    let router: Router = Router::<FullstackState>::new()
        .route("/app/projects/{code}", get(render_handler))
        .with_state(FullstackState::new(
            clerk_config(surreal),
            webapp::clerk::ClerkProjectDetail,
        ))
        .layer(person)
        .layer(viewer);
    fetch(router, &format!("/app/projects/{project_code}")).await
}

#[tokio::test]
async fn clerk_list_ssrs_supervised_matters_and_names_the_supervisor() {
    let surreal = store::test_support::mem_surreal().await;
    let lawyer = insert_person(
        &surreal,
        "Avery Attorney",
        "avery@test.invalid",
        store::persons::Role::Lawyer,
    )
    .await;
    let clerk = insert_person(
        &surreal,
        "Cleo Clerk",
        "cleo@test.invalid",
        store::persons::Role::Clerk,
    )
    .await;
    let project = seed_supervised_project(&surreal, "Atlas LLC", clerk, lawyer).await;

    let (status, html) =
        render_clerk_list(&surreal, webapp::people::ViewerRole::Clerk, Some(clerk)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains(">Atlas LLC<"),
        "matter name in the SSR: {html}"
    );
    assert!(
        html.contains("Supervising lawyer") && html.contains(">Avery Attorney<"),
        "the supervising lawyer must be named on every card: {html}",
    );
    assert!(
        html.contains(&format!("href=\"/app/projects/{}\"", project.code)),
        "each card links to the matter detail: {html}",
    );
    // The lawyer workbench never leaks onto the Clerk lens. The *path* can no
    // longer carry that guarantee — the card above links to the same
    // `/app/projects/{code}` a lawyer uses — so assert on what the workbench
    // renders instead.
    for workbench in [
        "Participation ledger",
        "Matter people",
        "Documents",
        "To close this matter",
        "Repository",
    ] {
        assert!(
            !html.contains(workbench),
            "the Clerk lens must not carry `{workbench}`: {html}"
        );
    }
    // The brand-font download is a firm-wide asset, not a matter surface, so it
    // is a card on the shared `/app/team` home rather than a list item here.
    // The route still admits a Clerk; only the link moved.
    assert!(
        !html.contains("/lawyer/fonts/gorp-serif.zip"),
        "the brand-font download lives on `/app/team`, not the matter list: {html}",
    );
}

#[tokio::test]
async fn clerk_list_empty_state_explains_how_a_matter_appears() {
    let surreal = store::test_support::mem_surreal().await;
    let clerk = insert_person(
        &surreal,
        "Cleo Clerk",
        "cleo@test.invalid",
        store::persons::Role::Clerk,
    )
    .await;

    let (status, html) =
        render_clerk_list(&surreal, webapp::people::ViewerRole::Clerk, Some(clerk)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        html.contains("No supervised projects are assigned to you."),
        "the empty state must explain how a matter appears: {html}",
    );
    assert!(
        !html.contains("/lawyer/fonts/gorp-serif.zip"),
        "the empty list carries no brand-asset section either: {html}",
    );
}

#[tokio::test]
async fn clerk_list_is_not_found_for_a_lawyer() {
    let surreal = store::test_support::mem_surreal().await;
    let lawyer = insert_person(
        &surreal,
        "Avery Attorney",
        "avery@test.invalid",
        store::persons::Role::Lawyer,
    )
    .await;
    let clerk = insert_person(
        &surreal,
        "Cleo Clerk",
        "cleo@test.invalid",
        store::persons::Role::Clerk,
    )
    .await;
    seed_supervised_project(&surreal, "Atlas LLC", clerk, lawyer).await;

    // A lawyer does not use the Clerk surface: it is hidden from them, not
    // merely refused. Admin's general embedded Rego policy bypass must not reach it either, so
    // the loader repeats the exact-role check rather than trusting the gate.
    for (label, role) in [
        ("lawyer", webapp::people::ViewerRole::Lawyer),
        ("admin", webapp::people::ViewerRole::Admin),
        ("client", webapp::people::ViewerRole::Client),
    ] {
        let (status, html) = render_clerk_list(&surreal, role, Some(lawyer)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{label}: {html}");
        assert!(!html.contains("Atlas LLC"), "{label}: {html}");
    }
}

#[tokio::test]
async fn clerk_detail_shows_the_matter_facts_and_the_limited_access_notice() {
    let surreal = store::test_support::mem_surreal().await;
    let lawyer = insert_person(
        &surreal,
        "Avery Attorney",
        "avery@test.invalid",
        store::persons::Role::Lawyer,
    )
    .await;
    let clerk = insert_person(
        &surreal,
        "Cleo Clerk",
        "cleo@test.invalid",
        store::persons::Role::Clerk,
    )
    .await;
    let project = seed_supervised_project(&surreal, "Atlas LLC", clerk, lawyer).await;

    let (status, html) = render_clerk_detail(
        &surreal,
        &project.code,
        webapp::people::ViewerRole::Clerk,
        Some(clerk),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(">Atlas LLC<"), "{html}");
    assert!(html.contains(">open<"), "the matter status: {html}");
    assert!(html.contains(">Avery Attorney<"), "the supervisor: {html}");
    assert!(
        html.contains("Read-only coordination view") && html.contains("Limited access."),
        "the limited-access disclosure must survive the migration: {html}",
    );
    // The Project's client portal is offered on the clerk lens too — the portal
    // serve gate (`can_see_project`) admits exactly the viewers the matter page
    // does, so the same link is live for a supervised Clerk (docs/access-model.md).
    assert!(
        html.contains("Client portal"),
        "the portal link label: {html}"
    );
    // The whole mount, not a `/portal/"` suffix: the short form would also be
    // satisfied by a link to the retired top-level `/portal`, which is served
    // by nothing (`cli/tests/portal_namespace_retired.rs`).
    assert!(
        html.contains(&format!("href=\"/app/projects/{}/portal/\"", project.code)),
        "the portal link targets this Project's client-portal mount: {html}"
    );
    // None of the lawyer work controls exist on this surface.
    assert!(!html.contains("git-token"), "{html}");
    assert!(!html.contains("documents/upload"), "{html}");
    assert!(html.contains("href=\"/app/projects\""), "back link: {html}");
}

#[tokio::test]
async fn clerk_detail_is_not_found_for_an_unsupervised_matter() {
    let surreal = store::test_support::mem_surreal().await;
    let lawyer = insert_person(
        &surreal,
        "Avery Attorney",
        "avery@test.invalid",
        store::persons::Role::Lawyer,
    )
    .await;
    let clerk = insert_person(
        &surreal,
        "Cleo Clerk",
        "cleo@test.invalid",
        store::persons::Role::Clerk,
    )
    .await;
    let other_clerk = insert_person(
        &surreal,
        "Otto Clerk",
        "otto@test.invalid",
        store::persons::Role::Clerk,
    )
    .await;
    let project = seed_supervised_project(&surreal, "Atlas LLC", other_clerk, lawyer).await;

    // Another Clerk's matter is not merely refused — it does not exist here.
    let (status, html) = render_clerk_detail(
        &surreal,
        &project.code,
        webapp::people::ViewerRole::Clerk,
        Some(clerk),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{html}");
    assert!(!html.contains("Atlas LLC"), "{html}");

    // So is an id that resolves to no matter at all.
    let (status, _) = render_clerk_detail(
        &surreal,
        "missing-project",
        webapp::people::ViewerRole::Clerk,
        Some(clerk),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
