#![allow(clippy::doc_markdown)]
//! Dev e2e for "open a matter for an existing client."
//!
//! Drives the real HTTP path (`POST /app/projects`, selecting an existing
//! `Role::Client` as the client DRI) against `StubSignatureProvider`, so
//! nothing reaches DocuSign and the test is CI-safe.
//!
//! Opening a matter opens the **matter, and only the matter**: the Project
//! and its participation ledger. The retainer is a separate,
//! deliberate step (`navigator notation create <retainer_code> --project
//! <code>`, or the lawyer retainer walk at `/app/lawyer/retainers/new`), so these
//! tests assert this door creates *zero* notations and sends nothing. The
//! retainer lifecycle that follows — walk → `lawyer_review` → approve → send
//! exactly one envelope — is covered at its own door by
//! `features/tests/mutable_intake_docusign.rs` and
//! `server/tests/llc_formation_cli_surface.rs`.
//!
//! Covers:
//!
//!   1. Happy path — the project lands with its client DRI
//!      column and both participations (lawyer DRI + client) seeded, and
//!      redirects to the matter. No notation, no envelope.
//!   2. Negative — no client selected → the refusal redirect and **no** matter.
//!   3. Negative — a non-client person chosen as the client DRI → refused.
//!   4. Negative — no entity, or a blank entity/client `<select>` → refused
//!      with the form's own message, not a bare extractor rejection.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::signature::StubSignatureProvider;
use portal::AppState;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

fn admin_bearer() -> String {
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let mut session = portal::SessionData::fresh("admin@neonlaw.com", store::persons::Role::Admin);
    session.source = portal::session::SessionSource::Cli;
    format!("Bearer {}", sessions.encode(&session))
}

async fn build_app(
    tag: &str,
) -> (
    axum::Router,
    store::surreal::SurrealDb,
    Arc<StubSignatureProvider>,
) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-matter-open-repos-{tag}-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-matter-open-{tag}")))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    // The `generate_pdf__retainer_pdf` step is worker-dispatched, so the
    // in-memory runtime is wrapped in `DispatchingRuntime` (the same
    // in-process path the dev binary uses) — otherwise the PDF is never
    // rendered/persisted and the signature read-back 404s.
    let runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(DispatchingRuntime::new(
        runtime.clone(),
        email.clone(),
        storage.clone(),
    ));
    let stub = Arc::new(StubSignatureProvider::new());
    let state = AppState {
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        signature_provider: stub.clone(),
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        stub,
    )
}

/// The `Location` a refused open redirects to. Every refusal is
/// post/redirect/get back to `/app/projects/new`, with the message in
/// `?error=` and the submitted fields echoed beside it, so the assertions read
/// the header rather than a re-rendered body.
fn location(resp: &axum::http::Response<Body>) -> String {
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// Assert `resp` is the matter-open refusal redirect and its `?error=` contains
/// `needle` (matched against the decoded message).
fn assert_refused_with(resp: &axum::http::Response<Body>, needle: &str) {
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(resp);
    assert!(
        loc.starts_with("/app/projects/new?error="),
        "a refused open must redirect back to the form; got: {loc}",
    );
    let decoded = loc.replace("%20", " ").replace("%26", "&");
    assert!(
        decoded.contains(needle),
        "expected the refusal to say {needle:?}; got: {decoded}",
    );
}

/// Tiny URL-encoder for the form bodies — only escapes what these values
/// actually contain.
fn enc(s: &str) -> String {
    s.replace(' ', "%20").replace('@', "%40")
}

/// Seed a pre-existing `Role::Client` person — the matter-open form now
/// opens a matter *for* an existing client (required `client_dri_person_id`
/// picker), so the client must exist before the POST.
async fn seed_client(surreal: &store::surreal::SurrealDb, name: &str, email: &str) -> uuid::Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, store::persons::Role::Client),
    )
    .await
    .unwrap()
    .id
}

/// Whether a project with `name` exists — the negative assertions want to
/// confirm a rejected open created no matter, without tripping over the
/// demo projects `seed_canonical` already inserted.
async fn project_named_exists(surreal: &store::surreal::SurrealDb, name: &str) -> bool {
    store::projects::find_by_name(surreal, name)
        .await
        .unwrap()
        .is_some()
}

async fn post_projects(app: &axum::Router, body: String) -> axum::http::Response<Body> {
    post_projects_as(app, body, &admin_bearer()).await
}

async fn post_projects_as(
    app: &axum::Router,
    body: String,
    bearer: &str,
) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/app/projects")
                .header("authorization", bearer)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// A lawyer/admin bearer whose `person_id` points at a person that does not
/// exist. Opening a matter inserts a lawyer-DRI `person_project_roles` row for
/// that id, so the insert trips its foreign key — a deterministic mid-open DB
/// error that exercises the "surface, don't swallow" refusal path.
fn bearer_with_missing_person(role: store::persons::Role) -> String {
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let mut session = portal::SessionData::fresh("operator@neonlaw.com", role);
    session.source = portal::session::SessionSource::Cli;
    session.person_id = Some(uuid::Uuid::now_v7());
    format!("Bearer {}", sessions.encode(&session))
}

#[tokio::test]
async fn matter_open_asks_for_no_service() {
    // Every engagement is bespoke, so the lawyer form no longer asks which
    // service a matter is for, and the matter row carries no catalog
    // correlation at all.
    let (app, surreal, _stub) = build_app("product-code").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let client_id = seed_client(&surreal, "Formation Client", "formation-client@example.com").await;

    let body = format!(
        "name={}&code=matter-open-1&status=open&entity_id={entity_id}\
         &client_dri_person_id={client_id}\
         &attestation=1",
        enc("Uncontested divorce"),
    );
    let resp = post_projects(&app, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    store::projects::find_by_name(&surreal, "Uncontested divorce")
        .await
        .unwrap()
        .expect("project row inserted");
}

#[tokio::test]
async fn matter_open_without_attestation_is_refused_with_no_matter() {
    // The attorney's conflict attestation is required on every open. A submit
    // that omits the attestation checkbox redirects back to the form with the
    // attest message and opens nothing — the shared command's
    // `AttestationRequired`, surfaced by the form adapter. The submit is
    // otherwise valid (client and entity both resolve), so the refusal
    // is the attestation gate, not a stray field error.
    let (app, surreal, stub) = build_app("no-attestation").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let client_id = seed_client(&surreal, "Scorpio Client", "scorpio@example.com").await;

    let body = format!(
        "name={}&code=matter-open-2&entity_id={entity_id}\
         &client_dri_person_id={client_id}",
        enc("Unattested matter"),
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(
        &resp,
        "Attest that you have checked for conflicts, and that either none prevent opening this Project or this Project is not legal advice",
    );
    assert!(
        !project_named_exists(&surreal, "Unattested matter").await,
        "a refused open writes no matter",
    );
    assert!(stub.calls().is_empty());
}

#[tokio::test]
async fn matter_open_without_a_client_is_rejected_with_no_matter() {
    let (app, surreal, stub) = build_app("no-client").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;

    // Entity + template present, but no client selected. Every matter opens
    // *for* a real client, so this is refused and opens nothing.
    let body = format!(
        "name={}&code=matter-open-5&status=open&entity_id={entity_id}",
        enc("Aries screening shield"),
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(&resp, "Pick the client this matter is for");

    // No half-open matter.
    assert!(
        store::projects::find_by_name(&surreal, "Aries screening shield")
            .await
            .unwrap()
            .is_none(),
        "no project should be created without a client",
    );
    assert!(stub.calls().is_empty());
}

#[tokio::test]
async fn matter_open_with_a_non_client_person_as_client_is_rejected() {
    let (app, surreal, stub) = build_app("non-client").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;

    // `nick@neonlaw.com` is the seeded admin — not a client. Selecting a
    // non-client as the client DRI is refused: the client of record is a
    // client, never a firm attorney.
    let admin = store::persons::find_by_email_ci(&surreal, "nick@neonlaw.com")
        .await
        .unwrap()
        .expect("seeded admin");
    let body = format!(
        "name={}&code=matter-open-6&status=open&entity_id={entity_id}\
         &client_dri_person_id={}",
        enc("Aries screening shield"),
        admin.id,
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(&resp, "The client DRI must be an existing client person");
    assert!(stub.calls().is_empty());
}

#[tokio::test]
async fn matter_open_creates_the_matter_and_no_notation() {
    // Opening a matter opens the *matter*. The retainer is its own step —
    // `navigator notation create <retainer_code> --project <code>`, or the
    // lawyer retainer walk — so this door creates zero notations and lands
    // lawyer on the matter, not on a notation review screen it opened for
    // them. See the glossary's Engagement / Retainer entry.
    let (app, surreal, stub) = build_app("no-auto-retainer").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let client_id = seed_client(&surreal, "Libra Client", "libra@example.com").await;

    let body = format!(
        "name={}&code=matter-open-7&status=open&entity_id={entity_id}\
         &client_dri_person_id={client_id}\
         &attestation=1",
        enc("Unretained matter"),
    );
    let resp = post_projects(&app, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let project = store::projects::find_by_name(&surreal, "Unretained matter")
        .await
        .unwrap()
        .expect("the matter itself is created");

    // Lands on the matter, never on `/app/lawyer/notations/:id/review`.
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(location, format!("/app/projects/{}", project.code));

    // The load-bearing assertion: no notation was conjured.
    let notations = store::notations::list_by_project(&surreal, project.id)
        .await
        .unwrap();
    assert!(
        notations.is_empty(),
        "opening a matter must not open a retainer; got {} notation(s)",
        notations.len(),
    );

    // Nothing was dispatched to the client, either.
    assert!(stub.calls().is_empty());
}

#[tokio::test]
async fn matter_open_seeds_client_and_lawyer_participation() {
    // Opening the matter seeds both participations, exactly as the CLI's
    // `project create` does (`cli/src/project.rs`). Being the client DRI and
    // participating as the client are now one row, so the ledger and the
    // accountability marker cannot disagree. This used to ride along on the
    // retainer the handler auto-opened; the matter must seed it in its own
    // right.
    let (app, surreal, _stub) = build_app("participation").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let client_id = seed_client(&surreal, "Gemini Client", "gemini@example.com").await;

    let body = format!(
        "name={}&code=matter-open-8&status=open&entity_id={entity_id}\
         &client_dri_person_id={client_id}\
         &attestation=1",
        enc("Participation matter"),
    );
    let resp = post_projects(&app, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let project = store::projects::find_by_name(&surreal, "Participation matter")
        .await
        .unwrap()
        .expect("matter created");

    let participations: Vec<String> =
        store::projects::participations_for_project(&surreal, project.id)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.person_id == client_id)
            .map(|r| r.participation)
            .collect();
    assert!(
        participations.iter().any(|p| p == "client"),
        "the client belongs on the matter's participation ledger; got {participations:?}",
    );
    // …and that same row carries the client-DRI marker.
    assert_eq!(
        store::projects::participations_for_project(&surreal, project.id)
            .await
            .unwrap()
            .into_iter()
            .find(|p| p.is_client_dri)
            .map(|p| p.person_id),
        Some(client_id)
    );
}

#[tokio::test]
async fn matter_open_persists_the_description_without_seeding_a_clause() {
    // `description` stays a first-class Project column. It used to double
    // as the seed for a retainer's position-0 custom clause; with no
    // retainer to seed, it is simply the matter's scope narrative.
    let (app, surreal, _stub) = build_app("description").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let client_id = seed_client(&surreal, "Virgo Client", "virgo@example.com").await;

    let body = format!(
        "name={}&code=matter-open-9&status=open&entity_id={entity_id}\
         &client_dri_person_id={client_id}\
         &description={}\
         &attestation=1",
        enc("Described matter"),
        enc("Flat-fee estate planning for the Virgo family."),
    );
    let resp = post_projects(&app, body).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let project = store::projects::find_by_name(&surreal, "Described matter")
        .await
        .unwrap()
        .expect("matter created");
    assert_eq!(
        project.description.as_deref(),
        Some("Flat-fee estate planning for the Virgo family."),
    );
    // No notation means no clause to seed it into.
    let notations = store::notations::list_by_project(&surreal, project.id)
        .await
        .unwrap();
    let mut clauses = Vec::new();
    for notation in &notations {
        clauses.extend(
            store::notation_clauses::for_notation(&surreal, notation.id)
                .await
                .unwrap(),
        );
    }
    assert!(clauses.is_empty());
}

#[tokio::test]
async fn matter_open_with_a_blank_entity_select_shows_the_pick_an_entity_error() {
    // The browser posts an unselected `<select>` as `entity_id=` — an
    // empty string, not an absent field. `Option<Uuid>` alone parses that
    // as a malformed Uuid, so Axum's `Form` extractor rejects the body
    // with a bare 422 and the handler's own "Pick an entity" message never
    // reaches the lawyer. The lawyer must get the form back with the reason, not a wall
    // of nothing.
    let (app, surreal, _stub) = build_app("blank-entity").await;
    let client_id = seed_client(&surreal, "Pisces Client", "pisces@example.com").await;
    let body = format!(
        "name={}&code=matter-open-10&status=open&entity_id=\
         &client_dri_person_id={client_id}",
        enc("Blank entity matter"),
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(&resp, "Pick an entity to open the matter against");
    assert!(!project_named_exists(&surreal, "Blank entity matter").await);
}

#[tokio::test]
async fn matter_open_with_a_blank_client_select_shows_the_pick_a_client_error() {
    // Same trap, the picker next door: a blank client `<select>` posts
    // `client_dri_person_id=` and must reach the handler's own validation.
    let (app, surreal, _stub) = build_app("blank-client").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let body = format!(
        "name={}&code=matter-open-11&status=open&entity_id={entity_id}\
         &client_dri_person_id=",
        enc("Blank client matter"),
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(&resp, "Pick the client this matter is for");
    assert!(!project_named_exists(&surreal, "Blank client matter").await);
}

#[tokio::test]
async fn matter_open_without_an_entity_is_rejected() {
    // Commit 4: a matter always opens against a pre-existing entity. A
    // create with no `entity_id` is refused and opens nothing.
    let (app, surreal, _stub) = build_app("no-entity").await;
    let client_id = seed_client(&surreal, "Pisces Client", "pisces@example.com").await;
    let body = format!(
        "name={}&code=matter-open-12&status=open&client_dri_person_id={client_id}\
         &scope_of_services={}",
        enc("Entityless matter"),
        enc("Flat-fee estate planning"),
    );
    let resp = post_projects(&app, body).await;
    assert_refused_with(&resp, "Pick an entity to open the matter against");
    assert!(
        store::projects::find_by_name(&surreal, "Entityless matter")
            .await
            .unwrap()
            .is_none(),
        "no project should be created without an entity",
    );
}

/// A lawyer session whose `person_id` names a person that no longer exists is
/// refused cleanly. The shared command validates the attester (the accountable
/// lawyer DRI) exists before it writes anything, so this is a clean refusal with
/// no matter — not the mid-open foreign-key error the old inline handler tripped.
/// The up-front reference validation replaces that failure mode.
#[tokio::test]
async fn matter_open_with_an_unbacked_lawyer_session_is_refused() {
    let (app, surreal, _stub) = build_app("unbacked-session").await;
    let entity_id = store::test_support::seed_entity(&surreal).await;
    let client_id = seed_client(&surreal, "Aries Client", "aries@example.com").await;

    let body = format!(
        "name={}&code=matter-open-13&entity_id={entity_id}\
         &client_dri_person_id={client_id}\
         &attestation=1",
        enc("Aries estate"),
    );
    let resp = post_projects_as(
        &app,
        body,
        &bearer_with_missing_person(store::persons::Role::Admin),
    )
    .await;

    assert_refused_with(&resp, "attester");
    assert!(
        !project_named_exists(&surreal, "Aries estate").await,
        "a refused open persists no matter",
    );
}
