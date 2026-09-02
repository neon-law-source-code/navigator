//! Cucumber runner for `features/portal_signed_document.feature`.
//!
//! ENG-421: the client portal's matter-detail page must read the
//! "Signed copy" download off a completed `store::signatures` record,
//! never off `storage.exists()` at the signed-document key alone — the
//! runner shape mirrors `portal_invoice_card.rs` (forge a session
//! cookie, send the request, assert on the rendered card), adding a
//! notation plus direct storage/`store::signatures` setup so a
//! scenario can put bytes at the signed key without ever recording a
//! signature, and vice versa.

// Cucumber's step-attribute macros want `async fn` everywhere.
#![allow(clippy::unused_async)]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{policy::PolicyClient, SessionStore};
use tower::ServiceExt;
use uuid::Uuid;
use workflows::InMemoryRuntime;

#[derive(Default, World)]
#[world(init = Self::default)]
struct SignedDocumentWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    storage: Option<Arc<dyn cloud::StorageService>>,
    persons: HashMap<String, Uuid>,
    projects: HashMap<String, Uuid>,
    project_codes: HashMap<String, String>,
    notations: HashMap<String, Uuid>,
    last_status: Option<StatusCode>,
    last_body: String,
}

impl std::fmt::Debug for SignedDocumentWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedDocumentWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl SignedDocumentWorld {
    fn sessions(&self) -> &SessionStore {
        self.sessions.as_ref().expect("sessions not built")
    }

    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }

    fn storage(&self) -> &Arc<dyn cloud::StorageService> {
        self.storage.as_ref().expect("storage not built")
    }

    fn project_id(&self, name: &str) -> Uuid {
        *self.projects.get(name).expect("project was seeded earlier")
    }

    fn project_code(&self, name: &str) -> &str {
        self.project_codes
            .get(name)
            .expect("project was seeded earlier")
    }

    fn notation_id(&self, project_name: &str) -> Uuid {
        *self
            .notations
            .get(project_name)
            .expect("notation was seeded earlier")
    }
}

#[given("the Neon Law Navigator app is running")]
async fn build_app(world: &mut SignedDocumentWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-signed-document").await;
    let sessions = SessionStore::new("test-session-key-not-for-production");
    let state = app_state(
        runtime,
        storage.clone(),
        PolicyClient::passthrough(),
        None,
        sessions.clone(),
    )
    .await;
    world.sessions = Some(sessions);
    world.storage = Some(storage);
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
}

#[given(regex = r#"^a seeded person "([^"]+)" with role "([^"]+)"$"#)]
async fn seed_person(world: &mut SignedDocumentWorld, email: String, role: String) {
    let role = match role.as_str() {
        "owner" => store::persons::Role::Owner,
        "admin" => store::persons::Role::Admin,
        "lawyer" => store::persons::Role::Lawyer,
        _ => store::persons::Role::Client,
    };
    let inserted = store::test_support::ensure_person(
        &features::shared_surreal().await,
        &store::persons::NewPerson {
            oidc_subject: Some(format!("rauthy-{email}-subject")),
            ..store::persons::NewPerson::with_role(email.clone(), email.clone(), role)
        },
    )
    .await;
    world.persons.insert(email, inserted.id);
}

#[given(regex = r#"^a project "([^"]+)" with "([^"]+)" as a participant$"#)]
async fn seed_project_with_participant(
    world: &mut SignedDocumentWorld,
    project_name: String,
    participant_email: String,
) {
    let person_id = *world
        .persons
        .get(&participant_email)
        .expect("participant person was seeded earlier");
    let surreal = features::shared_surreal().await;
    let code = format!("test-{}", Uuid::now_v7().simple());
    let inserted = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: code.clone(),
            name: project_name.clone(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&features::shared_surreal().await).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert project");
    world.projects.insert(project_name.clone(), inserted.id);
    world.project_codes.insert(project_name, code);
    store::projects::add_participation(
        &features::shared_surreal().await,
        inserted.id,
        person_id,
        "client",
    )
    .await
    .expect("insert SurrealDB person_project_role");
}

#[given(regex = r#"^a notation for "([^"]+)" sent for signature$"#)]
async fn seed_sent_notation(world: &mut SignedDocumentWorld, project_name: String) {
    let project_id = world.project_id(&project_name);
    let person_id = *world
        .persons
        .values()
        .next()
        .expect("a person was seeded earlier");
    let surreal = features::shared_surreal().await;
    let template = store::templates::save_version(
        &surreal,
        Some(project_id),
        &format!("engagement-letter-{}", Uuid::now_v7()),
        store::templates::Version {
            title: "Engagement Letter".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: Some("onboarding".into()),
            source_commit_sha: None,
        },
    )
    .await
    .expect("save template version")
    .into_model();
    let notation = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(
            template.id,
            person_id,
            project_id,
            "sent_for_signature__pending",
        ),
    )
    .await
    .expect("create notation");
    world
        .storage()
        .put(
            &store::notations::document_pdf_storage_key(notation.id),
            b"rendered-engagement-letter",
            "application/pdf",
        )
        .await
        .expect("write the rendered document");
    world.notations.insert(project_name, notation.id);
}

#[given(
    regex = r#"^a document lands at the signed-document storage key for "([^"]+)" outside the signature webhook$"#
)]
async fn upload_signed_key_outside_webhook(world: &mut SignedDocumentWorld, project_name: String) {
    let notation_id = world.notation_id(&project_name);
    world
        .storage()
        .put(
            &store::notations::signed_document_storage_key(notation_id),
            b"looks-signed-but-no-provider-ever-confirmed-it",
            "application/pdf",
        )
        .await
        .expect("write object at the signed key");
}

#[given(regex = r#"^the notation's signature for "([^"]+)" is completed by the provider$"#)]
async fn signature_completed(world: &mut SignedDocumentWorld, project_name: String) {
    let notation_id = world.notation_id(&project_name);
    let surreal = features::shared_surreal().await;
    store::signatures::record_request(
        &surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        &format!("env-{notation_id}"),
    )
    .await
    .expect("record signature request");
    store::signatures::stamp_signed(
        &surreal,
        store::signatures::SignatureProvider::DocuSign,
        &format!("env-{notation_id}"),
        "2026-06-30T00:00:00Z",
    )
    .await
    .expect("stamp signed_at");
}

#[given(regex = r#"^the notation's signature for "([^"]+)" is declined by the provider$"#)]
async fn signature_declined(world: &mut SignedDocumentWorld, project_name: String) {
    let notation_id = world.notation_id(&project_name);
    let surreal = features::shared_surreal().await;
    store::signatures::record_request(
        &surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        &format!("env-{notation_id}"),
    )
    .await
    .expect("record signature request");
    // Mirrors exactly what `esignature_webhook::advance` does on a
    // `signature_declined` signal: the workflow's terminal state is
    // written to the notation, and `signed_at` is never stamped (the
    // webhook only stamps it on `signature_received`).
    store::notations::update_state(&surreal, notation_id, "END")
        .await
        .expect("advance notation to the declined-terminal state");
}

#[when(regex = r#"^"([^"]+)" opens the detail page for "([^"]+)"$"#)]
async fn open_detail(world: &mut SignedDocumentWorld, email: String, project_name: String) {
    let person_id = *world.persons.get(&email).expect("actor was seeded earlier");
    let project_code = world.project_code(&project_name);
    let role = role_for(&features::shared_surreal().await, person_id).await;
    let session = SessionData {
        sub: format!("rauthy-{email}-subject"),
        email: Some(email.clone()),
        person_id: Some(person_id),
        exp: portal::session::now_unix_secs() + 60,
        role,
        csrf_token: "test-csrf".into(),
        source: portal::session::SessionSource::Browser,
        provider: None,
        impersonation: None,
        scope: None,
    };
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={}",
        world.sessions().encode(&session)
    );
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_body = body_string(resp).await;
}

async fn role_for(surreal: &store::surreal::SurrealDb, person_id: Uuid) -> store::persons::Role {
    store::persons::find_by_id(surreal, person_id)
        .await
        .expect("query person")
        .expect("person row exists")
        .role
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status_is(world: &mut SignedDocumentWorld, code: u16) {
    let actual = world.last_status.expect("no response captured");
    assert_eq!(
        actual.as_u16(),
        code,
        "expected {code}, got {} (body: {})",
        actual,
        truncated(&world.last_body)
    );
}

#[then("the page offers a signed copy download")]
async fn offers_signed_copy(world: &mut SignedDocumentWorld) {
    let notation_id = world
        .notations
        .values()
        .next()
        .expect("a notation was seeded earlier");
    let needle = format!(r#"href="/app/notations/{notation_id}/documents/signed""#);
    assert!(
        world.last_body.contains(&needle),
        "expected the signed-copy link ({needle}); body was: {}",
        truncated(&world.last_body)
    );
}

#[then("the page offers no signed copy download")]
async fn offers_no_signed_copy(world: &mut SignedDocumentWorld) {
    let notation_id = world
        .notations
        .values()
        .next()
        .expect("a notation was seeded earlier");
    let needle = format!(r#"href="/app/notations/{notation_id}/documents/signed""#);
    assert!(
        !world.last_body.contains(&needle),
        "expected no signed-copy link ({needle}), but one rendered; body was: {}",
        truncated(&world.last_body)
    );
}

fn truncated(s: &str) -> String {
    const LIMIT: usize = 400;
    if s.len() <= LIMIT {
        s.to_string()
    } else {
        format!("{}…", &s[..LIMIT])
    }
}

#[tokio::main]
async fn main() {
    SignedDocumentWorld::cucumber()
        .run_and_exit("tests/features/portal_signed_document.feature")
        .await;
}
