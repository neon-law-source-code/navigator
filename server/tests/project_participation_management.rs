#![allow(clippy::doc_markdown)]
//! Integration coverage for editing the `person_project_roles` ledger from
//! the lawyer project workbench (issue #443).

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

struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    project_id: Uuid,
    project_code: String,
    assigned_role_id: Uuid,
    /// The matter's seeded lawyer DRI, and a session for them: the workbench
    /// controls are theirs to fire, so the tests need to act as that person.
    dri_id: Uuid,
    dri_cookie: String,
    dri_csrf_token: String,
    candidate_id: Uuid,
    replacement_id: Uuid,
    client_id: Uuid,
    lawyer_cookie: String,
    admin_cookie: String,
    outsider_cookie: String,
    lawyer_csrf_token: String,
    admin_csrf_token: String,
    outsider_csrf_token: String,
}

#[allow(clippy::too_many_lines)]
async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-project-participation-test"))
            .await
            .unwrap(),
    );
    let dri = store::test_support::dri_person(&surreal).await;
    store::persons::set_role(&surreal, dri, Role::Lawyer)
        .await
        .unwrap();
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: "libra-formation".into(),
            name: "Libra formation".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Designate the licensed lawyer person as this matter's lawyer DRI, the
    // participation-row form of the retired `lawyer_dri_person_id` column. The
    // lockout tests below reach for this row and expect its removal refused.
    store::projects::designate_dri_in_surreal(
        &surreal,
        project.id,
        dri,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
    let admin = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Firm Administrator",
            "admin@example.com",
            Role::Admin,
        ),
    )
    .await
    .unwrap();
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Lawyer Member", "lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let candidate = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Paralegal Candidate",
            "paralegal@example.com",
            Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let replacement = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role(
            "Attorney Candidate",
            "attorney@example.com",
            Role::Lawyer,
        ),
    )
    .await
    .unwrap();
    let outsider = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Other Lawyer", "other@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    // The form derives participation from the tier, so the fixture needs a
    // client-tier person to prove the client side of that derivation.
    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra Client", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let assigned_role =
        store::projects::add_participation(&surreal, project.id, lawyer.id, "paralegal")
            .await
            .unwrap();

    let sessions = SessionStore::new(KEY);
    let mut lawyer_session = SessionData::fresh("lawyer-sub", Role::Lawyer);
    lawyer_session.person_id = Some(lawyer.id);
    let lawyer_csrf_token = lawyer_session.csrf_token.clone();
    let lawyer_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&lawyer_session));
    let mut admin_session = SessionData::fresh("admin-sub", Role::Admin);
    admin_session.person_id = Some(admin.id);
    let admin_csrf_token = admin_session.csrf_token.clone();
    let admin_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&admin_session));
    let mut dri_session = SessionData::fresh("dri-sub", Role::Lawyer);
    dri_session.person_id = Some(dri);
    let dri_csrf_token = dri_session.csrf_token.clone();
    let dri_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&dri_session));
    let mut outsider_session = SessionData::fresh("outsider-sub", Role::Lawyer);
    outsider_session.person_id = Some(outsider.id);
    let outsider_csrf_token = outsider_session.csrf_token.clone();
    let outsider_cookie = format!(
        "{SESSION_COOKIE_NAME}={}",
        sessions.encode(&outsider_session)
    );

    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let runtime = Arc::new(workflows::InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    Fixture {
        app,
        surreal,
        project_id: project.id,
        project_code: project.code,
        assigned_role_id: assigned_role.id,
        dri_id: dri,
        dri_cookie,
        dri_csrf_token,
        candidate_id: candidate.id,
        replacement_id: replacement.id,
        client_id: client.id,
        lawyer_cookie,
        admin_cookie,
        outsider_cookie,
        lawyer_csrf_token,
        admin_csrf_token,
        outsider_csrf_token,
    }
}

fn form_request(uri: String, cookie: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("cookie", cookie)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap()
}

/// Assert `response` is the participation refusal redirect and its `?error=`
/// carries `message`. Every refusal on this cluster is post/redirect/get back to
/// the form, with the message and the submitted values in the query.
fn assert_refused(response: &axum::http::Response<Body>, message: &str) {
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let loc = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        loc.contains("?error="),
        "expected an error flash, got: {loc}"
    );
    let decoded = loc.replace("%20", " ").replace("%27", "'");
    assert!(
        decoded.contains(message),
        "expected the refusal to say {message:?}; got: {decoded}",
    );
}

async fn body_string(response: axum::http::Response<Body>) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn admin_manages_project_participation_from_the_matter_workbench() {
    let fixture = build_fixture().await;

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", fixture.project_code))
                .header("cookie", &fixture.lawyer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Matter people"), "html: {html}");
    assert!(html.contains("Lawyer Member"), "html: {html}");
    assert!(html.contains("System tier"), "html: {html}");
    assert!(html.contains("paralegal"), "html: {html}");
    assert!(!html.contains("Add person"), "html: {html}");
    assert!(!html.contains(&format!(
        "/app/projects/{}/people/{}/edit",
        fixture.project_code, fixture.assigned_role_id
    )));

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/people/new", fixture.project_code))
                .header("cookie", &fixture.lawyer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/people/new", fixture.project_code))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Add matter person"), "html: {html}");
    assert!(html.contains("Paralegal Candidate"), "html: {html}");

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/people/new", Uuid::now_v7()))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{}/people/{}/edit",
                    fixture.project_code, fixture.assigned_role_id
                ))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Edit matter person"), "html: {html}");
    assert!(html.contains("paralegal"), "html: {html}");

    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/app/projects/{}/people/{}/edit",
                    fixture.project_code,
                    Uuid::now_v7()
                ))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    for (body, message) in [
        (
            format!("_csrf={}", fixture.admin_csrf_token),
            "Choose a person to assign to this matter.",
        ),
        (
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token,
                Uuid::now_v7()
            ),
            "That person was not found.",
        ),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(form_request(
                format!("/app/projects/{}/people", fixture.project_code),
                &fixture.admin_cookie,
                body,
            ))
            .await
            .unwrap();
        assert_refused(&response, message);
    }

    let missing_role_id = Uuid::now_v7();
    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/edit",
                fixture.project_code, missing_role_id
            ),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token, fixture.candidate_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token, fixture.candidate_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let role = store::projects::participation_for_person(
        &fixture.surreal,
        fixture.candidate_id,
        fixture.project_id,
    )
    .await
    .unwrap()
    .expect("candidate participation inserted");
    // Derived from the candidate's `persons.role`, not from a typed field.
    assert_eq!(role.participation, "lawyer");

    for (body, message) in [
        (
            format!("_csrf={}", fixture.admin_csrf_token),
            "Choose a person to assign to this matter.",
        ),
        (
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token,
                Uuid::now_v7()
            ),
            "That person was not found.",
        ),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(form_request(
                format!(
                    "/app/projects/{}/people/{}/edit",
                    fixture.project_code, role.id
                ),
                &fixture.admin_cookie,
                body,
            ))
            .await
            .unwrap();
        assert_refused(&response, message);
    }

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token, fixture.candidate_id
            ),
        ))
        .await
        .unwrap();
    assert_refused(&response, "That person is already assigned to this matter.");

    let replacement_role = store::projects::add_participation(
        &fixture.surreal,
        fixture.project_id,
        fixture.replacement_id,
        "attorney",
    )
    .await
    .unwrap();

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/edit",
                fixture.project_code, role.id
            ),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token, fixture.replacement_id
            ),
        ))
        .await
        .unwrap();
    assert_refused(&response, "That person is already assigned to this matter.");

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/edit",
                fixture.project_code, role.id
            ),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token, fixture.client_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let role = store::projects::participation_by_id(&fixture.surreal, role.id)
        .await
        .unwrap()
        .expect("participation still exists after update");
    assert_eq!(role.person_id, fixture.client_id);
    // Re-pointing the row re-derives its side of the matter from the incoming
    // person's tier: a client-tier person lands client-side, not `lawyer`.
    assert_eq!(role.participation, "client");

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/delete",
                fixture.project_code, role.id
            ),
            &fixture.admin_cookie,
            format!("_csrf={}", fixture.admin_csrf_token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(
        store::projects::participation_by_id(&fixture.surreal, role.id)
            .await
            .unwrap()
            .is_none(),
        "deleted participation must not remain in the ledger"
    );

    // The remove control is a plain form post, so a successful removal
    // redirects back to the matter rather than answering a swap fragment.
    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/delete",
                fixture.project_code, replacement_role.id
            ),
            &fixture.admin_cookie,
            format!("_csrf={}", fixture.admin_csrf_token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some(format!("/app/projects/{}", fixture.project_code).as_str()),
        "a removal must land back on the matter it changed",
    );

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/delete",
                fixture.project_code,
                Uuid::now_v7()
            ),
            &fixture.admin_cookie,
            format!("_csrf={}", fixture.admin_csrf_token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn undisclosed_lawyer_cannot_change_another_matters_people() {
    let fixture = build_fixture().await;
    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.outsider_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.outsider_csrf_token, fixture.candidate_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/delete",
                fixture.project_code, fixture.assigned_role_id
            ),
            &fixture.lawyer_cookie,
            format!("_csrf={}", fixture.lawyer_csrf_token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Lawyer reach a matter through their participation row, so removing the
/// `lawyer_dri` row while the column still names that lawyer would leave a
/// matter its own accountable lawyer cannot open. The removal is refused
/// until a different DRI is assigned.
#[tokio::test]
async fn removing_the_lawyer_dri_participation_is_refused() {
    let fixture = build_fixture().await;
    let dri_role =
        store::projects::participations_for_project(&fixture.surreal, fixture.project_id)
            .await
            .unwrap()
            .into_iter()
            .find(|role| role.is_lawyer_dri)
            .expect("the DRI designation wrote a membership row");
    let dri = dri_role.person_id;

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/delete",
                fixture.project_code, dri_role.id
            ),
            &fixture.admin_cookie,
            format!("_csrf={}", fixture.admin_csrf_token),
        ))
        .await
        .unwrap();
    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "an admin may reach this matter's people"
    );

    // The refusal has to reach the person who asked for it (navigator#995).
    // The surviving row is invisible on its own — without the flash the reload
    // reads as a no-op rather than as a refusal — so the reason rides the
    // redirect as the `?error=` flash, and the listing renders it.
    assert_refused(
        &response,
        "This person is the matter's lawyer DRI. Assign a different lawyer DRI before removing them.",
    );
    let listing = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(
                    response
                        .headers()
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .unwrap(),
                )
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listing.status(), StatusCode::OK);
    let body = body_string(listing).await;
    assert!(
        body.contains("Assign a different lawyer DRI before removing them."),
        "the listing must say why the row survived: {body}"
    );

    let surviving = store::projects::participation_by_id(&fixture.surreal, dri_role.id)
        .await
        .unwrap();
    assert!(
        surviving.is_some(),
        "the lawyer DRI's participation must survive so the matter keeps an accountable lawyer"
    );
    assert!(
        store::projects::can_access_as_lawyer_in_surreal(
            &fixture.surreal,
            Some(dri),
            Role::Lawyer,
            fixture.project_id
        )
        .await
        .unwrap(),
        "the DRI must still reach the matter after the refused removal"
    );
}

/// The update handler reaches the same lockout as deletion by two doors:
/// reassigning the DRI's row to someone else, or flipping it to a client-side
/// participation. Either leaves the named lawyer with no firm-side row.
#[tokio::test]
async fn updating_the_lawyer_dri_participation_into_a_lockout_is_refused() {
    let fixture = build_fixture().await;
    let dri_role =
        store::projects::participations_for_project(&fixture.surreal, fixture.project_id)
            .await
            .unwrap()
            .into_iter()
            .find(|role| role.is_lawyer_dri)
            .expect("the DRI designation wrote a membership row");
    let dri = dri_role.person_id;

    for body in [
        // Hand the DRI's row to another person.
        format!(
            "_csrf={}&person_id={}",
            fixture.admin_csrf_token, fixture.replacement_id
        ),
        // Keep the person, but demote the row to the client side.
        format!("_csrf={}&person_id={dri}", fixture.admin_csrf_token),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(form_request(
                format!(
                    "/app/projects/{}/people/{}/edit",
                    fixture.project_code, dri_role.id
                ),
                &fixture.admin_cookie,
                body.clone(),
            ))
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "body: {body}");

        let row = store::projects::participation_by_id(&fixture.surreal, dri_role.id)
            .await
            .unwrap()
            .expect("the DRI's participation row must survive");
        assert_eq!(row.person_id, dri, "body: {body}");
        assert!(row.is_lawyer_dri, "body: {body}");
        assert!(
            store::projects::can_access_as_lawyer_in_surreal(
                &fixture.surreal,
                Some(dri),
                Role::Lawyer,
                fixture.project_id
            )
            .await
            .unwrap(),
            "the DRI must still reach the matter after the refused update: {body}"
        );
    }
}

/// The guard is narrow: participation rows that are not the DRI's still edit
/// freely, so ordinary staffing changes are unaffected.
#[tokio::test]
async fn updating_a_non_dri_participation_still_succeeds() {
    let fixture = build_fixture().await;
    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/edit",
                fixture.project_code, fixture.assigned_role_id
            ),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}",
                fixture.admin_csrf_token, fixture.candidate_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let row = store::projects::participation_by_id(&fixture.surreal, fixture.assigned_role_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.person_id, fixture.candidate_id);
    assert_eq!(row.participation, "lawyer");
}

#[tokio::test]
async fn database_rejects_duplicate_person_project_participation() {
    let fixture = build_fixture().await;
    let participation = store::participation::add_participant(
        &fixture.surreal,
        &store::participation::AddParticipantCommand {
            project_id: fixture.project_id,
            person_id: fixture.candidate_id,
            dri: store::participation::DriRequest::Unchanged,
            actor: store::participation::DriActor::System,
        },
    )
    .await
    .unwrap();

    let duplicate = store::participation::add_participant(
        &fixture.surreal,
        &store::participation::AddParticipantCommand {
            project_id: fixture.project_id,
            person_id: fixture.candidate_id,
            dri: store::participation::DriRequest::Unchanged,
            actor: store::participation::DriActor::System,
        },
    )
    .await;
    assert!(
        matches!(
            duplicate,
            Err(store::participation::AddParticipantError::Duplicate)
        ),
        "the command must prevent a second role for the same person and matter"
    );
    assert_eq!(participation.participation, "lawyer");
}

/// The form no longer asks for a participation. Which side of a matter someone
/// is on already follows from `persons.role`, so the control — and the datalist
/// apparatus that made an open vocabulary typeable — is gone from the page.
#[tokio::test]
async fn the_form_does_not_offer_a_participation_control() {
    let fixture = build_fixture().await;
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/people/new", fixture.project_code))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;

    assert!(
        !html.contains(r#"name="participation""#),
        "the participation control must be gone; html: {html}"
    );
    assert!(
        !html.contains("participation-suggestions"),
        "the datalist went with the control; html: {html}"
    );
    // The person picker is what survives, and it prints the tier that becomes
    // the row's participation so the derivation is visible before the submit.
    assert!(html.contains(r#"name="person_id""#), "html: {html}");
    assert!(
        html.contains("Paralegal Candidate &#60;paralegal@example.com&#62; — lawyer"),
        "html: {html}"
    );
}

/// The derivation is the whole point: a firm tier lands firm-side under its own
/// name and a `client` lands client-side, without an admin naming either. These
/// are the values `store::access` reads to answer which lens sees the matter.
#[tokio::test]
async fn participation_is_derived_from_the_person_tier() {
    let fixture = build_fixture().await;
    for (person_id, expected) in [
        (fixture.candidate_id, "lawyer"),
        (fixture.client_id, "client"),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(form_request(
                format!("/app/projects/{}/people", fixture.project_code),
                &fixture.admin_cookie,
                format!("_csrf={}&person_id={person_id}", fixture.admin_csrf_token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let row = store::projects::participation_for_person(
            &fixture.surreal,
            person_id,
            fixture.project_id,
        )
        .await
        .unwrap()
        .expect("participation inserted");
        assert_eq!(row.participation, expected, "person {person_id}");
    }

    // The client-tier row is client-side, so it must not hand out the firm lens.
    assert!(
        !store::projects::can_access_as_lawyer_in_surreal(
            &fixture.surreal,
            Some(fixture.client_id),
            Role::Client,
            fixture.project_id
        )
        .await
        .unwrap(),
        "a derived `client` participation is client-side"
    );
}

/// A submitted `participation` is not a hidden back door. The handler derives
/// the value and never reads the form field, so a hand-crafted POST that names
/// `attorney` for a client-tier person still writes `client`.
#[tokio::test]
async fn a_posted_participation_field_is_ignored() {
    let fixture = build_fixture().await;
    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}&participation=attorney",
                fixture.admin_csrf_token, fixture.client_id
            ),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);

    let row = store::projects::participation_for_person(
        &fixture.surreal,
        fixture.client_id,
        fixture.project_id,
    )
    .await
    .unwrap()
    .expect("participation inserted");
    assert_eq!(row.participation, "client");
}

/// Everyone carrying the matter's lawyer marker right now, sorted.
async fn lawyer_dri_holders(fixture: &Fixture) -> Vec<Uuid> {
    let mut ids = store::participation::holders(
        &fixture.surreal,
        fixture.project_id,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
    ids.sort();
    ids
}

/// The single lawyer DRI, for the fixtures that seed exactly one.
async fn lawyer_dri_holder(fixture: &Fixture) -> Option<Uuid> {
    lawyer_dri_holders(fixture).await.first().copied()
}

/// Designating a second lawyer DRI **adds** them: nothing is taken from the
/// first, and the submit needs no confirmation step because it displaces nobody.
/// A matter that is genuinely two lawyers' responsibility now says so.
#[tokio::test]
async fn designating_a_second_lawyer_dri_adds_to_the_set() {
    let fixture = build_fixture().await;
    let before = lawyer_dri_holder(&fixture)
        .await
        .expect("fixture designates");
    assert_ne!(before, fixture.replacement_id);

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}&dri=lawyer",
                fixture.admin_csrf_token, fixture.replacement_id
            ),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        !location.contains("confirm="),
        "an additive designation needs no confirmation step: {location}"
    );

    let mut expected = vec![before, fixture.replacement_id];
    expected.sort();
    assert_eq!(
        lawyer_dri_holders(&fixture).await,
        expected,
        "both lawyers are accountable for the matter"
    );
}

/// The first lawyer DRI keeps their marker until someone removes it, so a second
/// designation is not a quiet handover.
#[tokio::test]
async fn the_first_lawyer_dri_keeps_the_marker_when_a_second_is_added() {
    let fixture = build_fixture().await;
    let before = lawyer_dri_holder(&fixture)
        .await
        .expect("fixture designates");

    fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}&dri=lawyer",
                fixture.admin_csrf_token, fixture.replacement_id
            ),
        ))
        .await
        .unwrap();

    let rows = store::projects::participations_for_project(&fixture.surreal, fixture.project_id)
        .await
        .unwrap();
    assert!(
        rows.iter()
            .any(|row| row.person_id == before && row.is_lawyer_dri),
        "the original lawyer DRI still carries the marker: {rows:?}"
    );
}

/// The lawyer DRI is the accountable lawyer, so a client-tier person cannot carry
/// the marker however the field arrives.
#[tokio::test]
async fn a_client_cannot_be_made_the_lawyer_dri() {
    let fixture = build_fixture().await;
    let before = lawyer_dri_holder(&fixture).await;

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!("/app/projects/{}/people", fixture.project_code),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={}&dri=lawyer",
                fixture.admin_csrf_token, fixture.client_id
            ),
        ))
        .await
        .unwrap();

    assert_refused(&response, "Only a firm-side lawyer can be the lawyer DRI");
    assert_eq!(lawyer_dri_holder(&fixture).await, before);
}

/// A matter always has one lawyer DRI, so the marker cannot be handed back — the
/// edit form's "Not a DRI" choice is refused on that row and the marker stays.
#[tokio::test]
async fn the_lawyer_dri_marker_cannot_be_cleared() {
    let fixture = build_fixture().await;
    let holder = lawyer_dri_holder(&fixture)
        .await
        .expect("fixture designates");
    let row =
        store::projects::participation_for_person(&fixture.surreal, holder, fixture.project_id)
            .await
            .unwrap()
            .expect("the designation wrote a row");

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/edit",
                fixture.project_code, row.id
            ),
            &fixture.admin_cookie,
            format!(
                "_csrf={}&person_id={holder}&dri=none",
                fixture.admin_csrf_token
            ),
        ))
        .await
        .unwrap();

    assert_refused(&response, "This matter always has a lawyer DRI");
    assert_eq!(lawyer_dri_holder(&fixture).await, Some(holder));
}

/// The add form leaves a held side selectable and names who already holds it.
/// Nothing is greyed out: designation adds to a set, so an admin is adding a
/// second accountable lawyer rather than taking the marker from the first.
#[tokio::test]
async fn the_add_form_names_the_current_holders_without_locking_the_side() {
    let fixture = build_fixture().await;
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/people/new", fixture.project_code))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains(r#"name="dri""#), "the radio renders: {html}");
    assert!(
        !html.contains("nav-radio--locked"),
        "a held side stays selectable: {html}"
    );
    assert!(
        html.contains("Lawyer DRI (accountable lawyer) — currently"),
        "the current holders are named on the choice: {html}"
    );
    assert!(
        html.contains("more than one DRI on each side"),
        "the help says the control adds rather than replaces: {html}"
    );
}

/// The ledger on the workbench is where the marker is visible: the accountable
/// lawyer is labelled, not left to be inferred from a name in the header.
#[tokio::test]
async fn the_matter_people_ledger_labels_the_accountability_marker() {
    let fixture = build_fixture().await;
    // The lawyer cookie, not the admin one: Owner and Admin are scoped by the
    // participation ledger like every other tier, and the fixture's admin is not
    // on this matter. The label is not admin-only anyway — every firm
    // participant reads who is accountable.
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", fixture.project_code))
                .header("cookie", &fixture.lawyer_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let html = body_string(response).await;
    assert!(html.contains("Accountability"), "html: {html}");
    assert!(html.contains(">Lawyer DRI<"), "html: {html}");
}

/// The workbench accountability control is the lawyer side governing itself: a
/// lawyer DRI makes a peer on the matter accountable too, with no admin in the
/// loop and no firm-wide people directory read.
#[tokio::test]
async fn a_lawyer_dri_designates_a_peer_from_the_workbench() {
    let fixture = build_fixture().await;
    let before = lawyer_dri_holders(&fixture).await;
    assert_eq!(before, vec![fixture.dri_id]);

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/dri",
                fixture.project_code, fixture.assigned_role_id
            ),
            &fixture.dri_cookie,
            format!("_csrf={}", fixture.dri_csrf_token),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        lawyer_dri_holders(&fixture).await.len(),
        2,
        "the peer joins the matter's accountable lawyers"
    );
}

/// Self-governing means the people already accountable, not the tier. A lawyer
/// assigned to the matter who holds no marker is refused, and nothing moves.
#[tokio::test]
async fn a_lawyer_without_the_marker_cannot_designate_from_the_workbench() {
    let fixture = build_fixture().await;
    let before = lawyer_dri_holders(&fixture).await;

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/dri",
                fixture.project_code, fixture.assigned_role_id
            ),
            &fixture.lawyer_cookie,
            format!("_csrf={}", fixture.lawyer_csrf_token),
        ))
        .await
        .unwrap();

    assert_refused(&response, "you may not change");
    assert_eq!(
        lawyer_dri_holders(&fixture).await,
        before,
        "a refused designation moves nothing"
    );
}

/// A lawyer DRI may take a peer's marker back, and the emptiness rule stops the
/// last one — the same invariant the removal lockout defends, reached through
/// the workbench rather than the admin form.
#[tokio::test]
async fn the_workbench_removes_a_peer_but_never_the_last_lawyer_dri() {
    let fixture = build_fixture().await;
    let designate = format!(
        "/app/projects/{}/people/{}/dri",
        fixture.project_code, fixture.assigned_role_id
    );
    fixture
        .app
        .clone()
        .oneshot(form_request(
            designate,
            &fixture.dri_cookie,
            format!("_csrf={}", fixture.dri_csrf_token),
        ))
        .await
        .unwrap();
    assert_eq!(lawyer_dri_holders(&fixture).await.len(), 2);

    let remove_peer = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/dri/remove",
                fixture.project_code, fixture.assigned_role_id
            ),
            &fixture.dri_cookie,
            format!("_csrf={}", fixture.dri_csrf_token),
        ))
        .await
        .unwrap();
    assert_eq!(remove_peer.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        lawyer_dri_holders(&fixture).await,
        vec![fixture.dri_id],
        "the peer steps off and the original stays accountable"
    );

    // Now they are the last one, so the same control refuses them.
    let dri_row = store::projects::participation_for_person(
        &fixture.surreal,
        fixture.dri_id,
        fixture.project_id,
    )
    .await
    .unwrap()
    .expect("the fixture designated a lawyer DRI");
    let remove_last = fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/dri/remove",
                fixture.project_code, dri_row.id
            ),
            &fixture.dri_cookie,
            format!("_csrf={}", fixture.dri_csrf_token),
        ))
        .await
        .unwrap();
    assert_refused(&remove_last, "always has a lawyer DRI");
    assert_eq!(lawyer_dri_holders(&fixture).await, vec![fixture.dri_id]);
}

/// Every workbench change lands on the audit trail, naming who fired it.
#[tokio::test]
async fn the_workbench_dri_change_is_audited() {
    let fixture = build_fixture().await;
    fixture
        .app
        .clone()
        .oneshot(form_request(
            format!(
                "/app/projects/{}/people/{}/dri",
                fixture.project_code, fixture.assigned_role_id
            ),
            &fixture.dri_cookie,
            format!("_csrf={}", fixture.dri_csrf_token),
        ))
        .await
        .unwrap();

    let entry = store::relationship_logs::all(&fixture.surreal)
        .await
        .unwrap()
        .into_iter()
        .find(|log| log.subject_id == fixture.project_id && log.action == "lawyer_dri_designated")
        .expect("the designation is on the trail");
    assert_eq!(entry.actor_person_id, Some(fixture.dri_id));
}

/// The token in the first `_csrf` hidden input on the page.
fn csrf_from_html(html: &str) -> String {
    html.split(r#"name="_csrf" value=""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .filter(|token| !token.is_empty())
        .unwrap_or_else(|| panic!("the page rendered no session CSRF token: {html}"))
        .to_string()
}

/// The workbench Make DRI control posts through the HTML the reader sees: the
/// action and the CSRF token both come off the page, then the store and the
/// matter header both name the new holder.
#[tokio::test]
async fn the_workbench_html_form_persists_a_lawyer_dri() {
    let fixture = build_fixture().await;
    let page = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", fixture.project_code))
                .header("cookie", &fixture.dri_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_string(page).await;
    let action = format!(
        "/app/projects/{}/people/{}/dri",
        fixture.project_code, fixture.assigned_role_id
    );
    assert!(
        html.contains(&format!(r#"action="{action}""#)),
        "the Make DRI control is on the page: {html}"
    );
    let csrf = csrf_from_html(&html);
    assert_eq!(csrf, fixture.dri_csrf_token);

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            action,
            &fixture.dri_cookie,
            format!("_csrf={csrf}"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(lawyer_dri_holders(&fixture).await.len(), 2);

    let reloaded = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", fixture.project_code))
                .header("cookie", &fixture.dri_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(reloaded).await;
    assert!(body.contains("Lawyer DRI:"), "{body}");
    assert!(body.contains("Lawyer Member"), "{body}");

    let directory = store::projects::matter_directory(&fixture.surreal, Role::Owner)
        .await
        .unwrap();
    let entry = directory
        .iter()
        .find(|row| row.code == fixture.project_code)
        .expect("the matter stays in the directory");
    assert!(
        entry.lawyer_dris.iter().any(|name| name == "Lawyer Member"),
        "directory (and MCP directory reads) name the new holder: {entry:?}"
    );
}

/// An Owner/Admin who holds no row still reaches the participation-only page
/// and can designate from the same workbench control — that is the staffing
/// door for a matter nobody has put them on yet.
#[tokio::test]
async fn an_unassigned_admin_designates_from_the_participation_page() {
    let fixture = build_fixture().await;
    let page = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", fixture.project_code))
                .header("cookie", &fixture.admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_string(page).await;
    assert!(
        html.contains("You are not assigned to this matter"),
        "{html}"
    );
    let action = format!(
        "/app/projects/{}/people/{}/dri",
        fixture.project_code, fixture.assigned_role_id
    );
    assert!(html.contains(&format!(r#"action="{action}""#)), "{html}");
    let csrf = csrf_from_html(&html);
    assert_eq!(csrf, fixture.admin_csrf_token);

    let response = fixture
        .app
        .clone()
        .oneshot(form_request(
            action,
            &fixture.admin_cookie,
            format!("_csrf={csrf}"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(lawyer_dri_holders(&fixture).await.len(), 2);
}

/// A lawyer already on a matter whose lawyer set is empty can name themselves
/// from the workbench. That is the write the empty-set gap used to refuse.
#[tokio::test]
async fn a_lawyer_on_an_unassigned_matter_names_themselves_dri() {
    let surreal = mem_surreal().await;
    let storage: std::sync::Arc<dyn cloud::StorageService> = std::sync::Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-empty-dri-workbench"))
            .await
            .unwrap(),
    );
    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("On Matter", "on-matter@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: "empty-dri-matter".into(),
            name: "Empty DRI matter".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let row = store::projects::add_participation(&surreal, project.id, lawyer.id, "lawyer")
        .await
        .unwrap();
    assert!(
        store::participation::holders(&surreal, project.id, store::projects::DriSide::Lawyer)
            .await
            .unwrap()
            .is_empty()
    );

    let sessions = SessionStore::new(KEY);
    let mut session = SessionData::fresh("empty-dri-sub", Role::Lawyer);
    session.person_id = Some(lawyer.id);
    let csrf = session.csrf_token.clone();
    let cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&session));
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let page = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", project.code))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let html = body_string(page).await;
    assert!(html.contains("This matter has no lawyer DRI"), "{html}");
    assert!(html.contains("Make DRI"), "{html}");
    let action = format!("/app/projects/{}/people/{}/dri", project.code, row.id);
    assert!(html.contains(&format!(r#"action="{action}""#)), "{html}");
    assert_eq!(csrf_from_html(&html), csrf);

    let response = app
        .clone()
        .oneshot(form_request(action, &cookie, format!("_csrf={csrf}")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let holders =
        store::participation::holders(&surreal, project.id, store::projects::DriSide::Lawyer)
            .await
            .unwrap();
    assert_eq!(holders, vec![lawyer.id]);

    let reloaded = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}", project.code))
                .header("cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_string(reloaded).await;
    assert!(body.contains("Lawyer DRI: On Matter"), "{body}");
    assert!(body.contains(">Lawyer DRI<"), "{body}");
}
