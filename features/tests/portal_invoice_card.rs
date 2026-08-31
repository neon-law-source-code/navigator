//! Cucumber runner for `features/portal_invoice_card.feature`.
//!
//! Grounds the read side of the per-project invoice card: a row in the
//! `xero_invoice` mirror (raised at matter close, reconciled by the
//! nightly `ReconcileInvoices` workflow) drives what the client sees at
//! `GET /app/projects/:code`. The runner shape mirrors
//! `portal_projects_detail.rs` — forge a session cookie, send the
//! request, assert on the rendered card — adding mirror-row setup via
//! `store::xero_invoices`.

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
use store::xero_invoices::{self, UpsertXeroInvoice};
use tower::ServiceExt;
use uuid::Uuid;
use workflows::InMemoryRuntime;

#[derive(Default, World)]
#[world(init = Self::default)]
struct CardWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    persons: HashMap<String, Uuid>,
    projects: HashMap<String, Uuid>,
    project_codes: HashMap<String, String>,
    last_status: Option<StatusCode>,
    last_body: String,
}

impl std::fmt::Debug for CardWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl CardWorld {
    fn sessions(&self) -> &SessionStore {
        self.sessions.as_ref().expect("sessions not built")
    }

    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }

    fn project_id(&self, name: &str) -> Uuid {
        *self.projects.get(name).expect("project was seeded earlier")
    }

    fn project_code(&self, name: &str) -> &str {
        self.project_codes
            .get(name)
            .expect("project was seeded earlier")
    }
}

#[given("the Neon Law Navigator app is running")]
async fn build_app(world: &mut CardWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-invoice-card").await;
    let sessions = SessionStore::new("test-session-key-not-for-production");
    let state = app_state(
        runtime,
        storage,
        PolicyClient::passthrough(),
        None,
        sessions.clone(),
    )
    .await;
    world.sessions = Some(sessions);
    world.app = Some(features::neon_router(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
    ));
}

#[given(regex = r#"^a seeded person "([^"]+)" with role "([^"]+)"$"#)]
async fn seed_person(world: &mut CardWorld, email: String, role: String) {
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
    world: &mut CardWorld,
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

#[given(regex = r#"^an AUTHORISED invoice of (\d+) cents is mirrored for "([^"]+)"$"#)]
async fn mirror_invoice(world: &mut CardWorld, amount_cents: i64, project_name: String) {
    let project_id = world.project_id(&project_name);
    xero_invoices::upsert(
        &features::shared_surreal().await,
        &UpsertXeroInvoice {
            project_id,
            xero_invoice_id: "INV-TEST-001".into(),
            reference: "Matter close fee".into(),
            status: "AUTHORISED".into(),
            amount_cents,
            currency: "USD".into(),
        },
    )
    .await
    .expect("upsert mirror invoice");
}

#[given(regex = r#"^the invoice for "([^"]+)" is reconciled as paid in full$"#)]
async fn reconcile_paid(world: &mut CardWorld, project_name: String) {
    let project_id = world.project_id(&project_name);
    // Read the mirrored total back, then fold a PAID/paid-in-full
    // result onto it exactly as the reconcile workflow does.
    let row = xero_invoices::for_projects(&features::shared_surreal().await, &[project_id])
        .await
        .expect("read mirror")
        .into_iter()
        .next()
        .expect("a mirror row was created earlier");
    xero_invoices::record_reconcile(
        &features::shared_surreal().await,
        project_id,
        "PAID",
        row.amount_cents,
    )
    .await
    .expect("record reconcile")
    .expect("mirror row exists to reconcile");
}

#[when(regex = r#"^"([^"]+)" opens the detail page for "([^"]+)"$"#)]
async fn open_detail(world: &mut CardWorld, email: String, project_name: String) {
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
async fn status_is(world: &mut CardWorld, code: u16) {
    let actual = world.last_status.expect("no response captured");
    assert_eq!(
        actual.as_u16(),
        code,
        "expected {code}, got {} (body: {})",
        actual,
        truncated(&world.last_body)
    );
}

#[then(regex = r#"^the response body contains "([^"]+)"$"#)]
async fn body_contains(world: &mut CardWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "expected body to contain {needle:?}; body was: {}",
        truncated(&world.last_body)
    );
}

#[then(regex = r#"^the invoice card shows the "([^"]+)" badge$"#)]
async fn card_shows_badge(world: &mut CardWorld, label: String) {
    // The card renders a success "Paid" status chip when reconciled paid
    // in full, otherwise a warning "Due" chip
    // (webapp::portal_project_detail::ClientProjectDetail).
    let class = match label.as_str() {
        "Paid" => "status-chip--paid",
        "Due" => "status-chip--due",
        other => panic!("unknown badge label {other:?}"),
    };
    assert!(
        world.last_body.contains(class) && world.last_body.contains(&label),
        "expected the {label:?} badge ({class}); body was: {}",
        truncated(&world.last_body)
    );
}

#[then("the page shows no invoice card")]
async fn no_invoice_card(world: &mut CardWorld) {
    // The card's heading is an `<h2>Invoice</h2>`; match the closing tag so a
    // Dioxus hydration comment between `>` and the text cannot make this a
    // vacuous (always-absent) check that a wrongly-rendered card would pass.
    assert!(
        !world.last_body.contains("Invoice</h2>"),
        "expected no invoice card, but one rendered; body was: {}",
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
    CardWorld::cucumber()
        .run_and_exit("tests/features/portal_invoice_card.feature")
        .await;
}
