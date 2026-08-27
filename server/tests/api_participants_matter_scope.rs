#![allow(clippy::doc_markdown)]
//! Integration tests for `POST`/`PATCH`/`DELETE /app/api/projects/{id}/participants[/{role_id}]`
//! — the matter-scope check added alongside ENG-35's Clerk-visibility toggle.
//!
//! The write engine (`store::participation::{add,update,remove}_participant`) is
//! shared with the lawyer workbench's admin-only form; these tests focus on what
//! this REST door adds on top of the tier gate: a lawyer with no participation
//! row on the target matter is refused the same non-disclosing `404` an
//! unrelated caller would get (mirroring `POST .../close`), while a lawyer
//! already on the matter — or an Owner/Admin with the documented bypass — can
//! use the door to grant or revoke a Clerk's portal visibility by adding or
//! removing that Clerk's participation row.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::SessionData;
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "api-participants-scope-test-key";

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    project_id: Uuid,
    /// A lawyer already participating in the matter — the one this door should
    /// admit.
    lawyer_on_matter: Uuid,
    /// A lawyer with no participation row on this matter at all.
    lawyer_off_matter: Uuid,
    /// An Admin with no participation row on this matter — proves the
    /// documented Owner/Admin bypass still applies through this door.
    admin_off_matter: Uuid,
    /// A Clerk not yet on the matter — the person these tests add and remove
    /// to prove the toggle.
    clerk_id: Uuid,
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let project = store::test_support::seed_project(&surreal, "Matter").await;

    // Designated the matter's lawyer DRI, not just a plain participant: a
    // Clerk's supervised visibility (`supervised_projects`) requires the
    // matter to name a currently licensed lawyer DRI, so the fixture needs a
    // real one for the visibility assertions below to mean anything.
    let lawyer_on_matter = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "On-Matter Lawyer",
            "on-matter@example.com",
            Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        lawyer_on_matter.id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();

    let lawyer_off_matter = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Off-Matter Lawyer",
            "off-matter@example.com",
            Role::Lawyer,
        ),
    )
    .await
    .unwrap();

    let admin_off_matter = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Firm Admin", "admin@example.com", Role::Admin),
    )
    .await
    .unwrap();

    let clerk = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Matter Clerk", "clerk@example.com", Role::Clerk),
    )
    .await
    .unwrap();

    let state = AppState {
        sessions: SessionStore::new(KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        project_id: project.id,
        lawyer_on_matter: lawyer_on_matter.id,
        lawyer_off_matter: lawyer_off_matter.id,
        admin_off_matter: admin_off_matter.id,
        clerk_id: clerk.id,
    }
}

/// A `Bearer` header for a session of `role` acting as `person_id`.
fn bearer(person_id: Uuid, role: Role) -> String {
    let mut session = SessionData::fresh("api-participants-sub", role);
    session.person_id = Some(person_id);
    format!("Bearer {}", SessionStore::new(KEY).encode(&session))
}

async fn add_call(
    app: &axum::Router,
    project_id: Uuid,
    person_id: Uuid,
    auth: &str,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/api/projects/{project_id}/participants"))
                .header("authorization", auth)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "person_id": person_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn remove_call(
    app: &axum::Router,
    project_id: Uuid,
    role_id: Uuid,
    auth: &str,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!(
                    "/app/api/projects/{project_id}/participants/{role_id}"
                ))
                .header("authorization", auth)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn add_participant_is_refused_for_a_lawyer_not_on_the_matter() {
    let fx = build_fixture().await;
    let off_matter = bearer(fx.lawyer_off_matter, Role::Lawyer);

    let resp = add_call(&fx.app, fx.project_id, fx.clerk_id, &off_matter).await;
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "a lawyer with no row on this matter must not learn whether it exists"
    );
    assert!(
        store::projects::participation_for_person(&fx.surreal, fx.clerk_id, fx.project_id)
            .await
            .unwrap()
            .is_none(),
        "the refused add wrote no row"
    );
}

#[tokio::test]
async fn a_participating_lawyer_grants_a_clerk_portal_visibility() {
    let fx = build_fixture().await;
    let on_matter = bearer(fx.lawyer_on_matter, Role::Lawyer);

    assert!(
        !store::access::can_see_project(&fx.surreal, Some(fx.clerk_id), Role::Clerk, fx.project_id)
            .await
            .unwrap(),
        "off before the lawyer grants it"
    );

    let resp = add_call(&fx.app, fx.project_id, fx.clerk_id, &on_matter).await;
    assert_eq!(resp.status(), StatusCode::CREATED);

    assert!(
        store::access::can_see_project(&fx.surreal, Some(fx.clerk_id), Role::Clerk, fx.project_id)
            .await
            .unwrap(),
        "on once a matter lawyer has added the Clerk's participation row"
    );
}

#[tokio::test]
async fn an_admin_with_no_participation_row_still_grants_it() {
    let fx = build_fixture().await;
    let admin = bearer(fx.admin_off_matter, Role::Admin);

    let resp = add_call(&fx.app, fx.project_id, fx.clerk_id, &admin).await;
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "Owner/Admin keep the documented project-scoping bypass through this door"
    );
}

#[tokio::test]
async fn remove_participant_is_refused_for_a_lawyer_not_on_the_matter() {
    let fx = build_fixture().await;
    let on_matter = bearer(fx.lawyer_on_matter, Role::Lawyer);
    let off_matter = bearer(fx.lawyer_off_matter, Role::Lawyer);

    let added = add_call(&fx.app, fx.project_id, fx.clerk_id, &on_matter).await;
    assert_eq!(added.status(), StatusCode::CREATED);
    let row = store::projects::participation_for_person(&fx.surreal, fx.clerk_id, fx.project_id)
        .await
        .unwrap()
        .expect("the grant above wrote a row");

    let resp = remove_call(&fx.app, fx.project_id, row.id, &off_matter).await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert!(
        store::access::can_see_project(&fx.surreal, Some(fx.clerk_id), Role::Clerk, fx.project_id)
            .await
            .unwrap(),
        "the refused remove revoked nothing"
    );
}

#[tokio::test]
async fn a_participating_lawyer_revokes_a_clerks_portal_visibility() {
    let fx = build_fixture().await;
    let on_matter = bearer(fx.lawyer_on_matter, Role::Lawyer);

    let added = add_call(&fx.app, fx.project_id, fx.clerk_id, &on_matter).await;
    assert_eq!(added.status(), StatusCode::CREATED);
    let row = store::projects::participation_for_person(&fx.surreal, fx.clerk_id, fx.project_id)
        .await
        .unwrap()
        .expect("the grant above wrote a row");

    let resp = remove_call(&fx.app, fx.project_id, row.id, &on_matter).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        !store::access::can_see_project(&fx.surreal, Some(fx.clerk_id), Role::Clerk, fx.project_id)
            .await
            .unwrap(),
        "off again once the matter lawyer removes the row"
    );
}
