//! Cucumber runner for `features/portal_trust_position.feature`.
//!
//! Grounds what a client reads about the funds the firm holds for them. A
//! deposit mirrored from Xero onto `store::trust` drives the trust card at
//! `GET /app/projects/:code`; the pooled IOLTA balance and every other
//! matter's postings stay off that page. The runner shape mirrors
//! `portal_invoice_card.rs` — forge a session cookie, send the request,
//! assert on the rendered card — with trust-ledger setup in place of mirror
//! rows.

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
struct TrustWorld {
    app: Option<axum::Router>,
    sessions: Option<SessionStore>,
    persons: HashMap<String, Uuid>,
    projects: HashMap<String, Uuid>,
    project_codes: HashMap<String, String>,
    last_status: Option<StatusCode>,
    last_body: String,
}

impl std::fmt::Debug for TrustWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustWorld")
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl TrustWorld {
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
async fn build_app(world: &mut TrustWorld) {
    let runtime = Arc::new(InMemoryRuntime::new());
    let storage = fs_storage("portal-trust-position").await;
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
async fn seed_person(world: &mut TrustWorld, email: String, role: String) {
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

/// A matter with a notation, because a trust posting anchors to one
/// (`store::trust::engagement_anchor`). `seed_notation` mints its own
/// Project, so the participation is added to that Project rather than to a
/// separately created one.
#[given(regex = r#"^a project "([^"]+)" with "([^"]+)" as a participant$"#)]
async fn seed_project_with_participant(
    world: &mut TrustWorld,
    project_name: String,
    participant_email: String,
) {
    let person_id = *world
        .persons
        .get(&participant_email)
        .expect("participant person was seeded earlier");
    let surreal = features::shared_surreal().await;
    let notation_id = store::test_support::seed_notation(&surreal).await;
    let project_id = store::notations::find_by_id(&surreal, notation_id)
        .await
        .expect("read notation")
        .expect("seeded notation exists")
        .project_id;
    let project = store::projects::find_by_id(&surreal, project_id)
        .await
        .expect("read project")
        .expect("seeded project exists");

    world.projects.insert(project_name.clone(), project.id);
    world.project_codes.insert(project_name, project.code);
    store::projects::add_participation(&surreal, project.id, person_id, "client")
        .await
        .expect("insert SurrealDB person_project_role");
}

#[given(regex = r#"^a trust deposit of (\d+) cents is mirrored for "([^"]+)"$"#)]
async fn mirror_deposit(world: &mut TrustWorld, amount_cents: i64, project_name: String) {
    let project_id = world.project_id(&project_name);
    let movement = store::trust::Movement::deposit(
        project_id,
        "USD",
        cents_to_decimal(amount_cents),
        amount_cents,
        "2026-09-01T00:00:00Z",
    )
    // The Xero bank-transaction id in production. Scoped to the matter here:
    // every scenario runs against one shared engine, so a literal id reused
    // across scenarios would read as "already mirrored" and post nothing.
    .with_external_ref(format!("bt-deposit-{project_id}"));
    assert_eq!(
        store::trust::record_project_movement(
            &features::shared_surreal().await,
            project_id,
            &movement
        )
        .await
        .expect("record deposit"),
        store::trust::Recorded::Posted
    );
}

#[given(regex = r#"^a trust refund of (\d+) cents is mirrored for "([^"]+)"$"#)]
async fn mirror_refund(world: &mut TrustWorld, amount_cents: i64, project_name: String) {
    let project_id = world.project_id(&project_name);
    let movement = store::trust::Movement::refund(project_id, amount_cents, "2026-09-10T00:00:00Z")
        .with_external_ref(format!("bt-refund-{project_id}"));
    assert_eq!(
        store::trust::record_project_movement(
            &features::shared_surreal().await,
            project_id,
            &movement
        )
        .await
        .expect("record refund"),
        store::trust::Recorded::Posted
    );
}

/// One bank transfer out of the pooled Nevada account, split across two
/// matters' invoices — the shape the whole feature exists to keep separate.
/// Both matters are put on the Nevada pool first, since a withdrawal may only
/// settle matters that sit on the pool the money left.
#[given(
    regex = r#"^one pooled withdrawal settles (\d+) cents for "([^"]+)" and (\d+) cents for "([^"]+)"$"#
)]
async fn pooled_withdrawal(
    world: &mut TrustWorld,
    first_cents: i64,
    first_project: String,
    second_cents: i64,
    second_project: String,
) {
    let surreal = features::shared_surreal().await;
    let first_id = world.project_id(&first_project);
    let second_id = world.project_id(&second_project);
    let jurisdiction = store::jurisdictions::find_or_create(
        &surreal,
        &store::jurisdictions::NewJurisdiction::new("Nevada", "NV", "state"),
    )
    .await
    .expect("seed Nevada");
    // Scoped to this scenario's matters: the suite shares one engine, and a
    // pooled account is UNIQUE per state, so the account is found-or-kept
    // rather than re-created per scenario.
    let account_id = "xero-nv-features".to_string();
    let _ = store::iolta_accounts::upsert(
        &surreal,
        &store::iolta_accounts::UpsertIoltaAccount {
            jurisdiction_id: jurisdiction.id,
            xero_account_id: account_id.clone(),
            xero_account_code: Some("090".into()),
            name: "IOLTA NV — Trust".into(),
            currency: "USD".into(),
            balance_cents: 0,
            mirrored_at: chrono::Utc::now(),
        },
    )
    .await
    .expect("mirror the Nevada pool");

    for (project_id, cents, tag) in [(first_id, first_cents, "a"), (second_id, second_cents, "b")] {
        store::projects::set_jurisdiction(&surreal, project_id, Some(jurisdiction.id))
            .await
            .expect("point the matter at Nevada");
        store::xero_invoices::upsert(
            &surreal,
            &store::xero_invoices::UpsertXeroInvoice {
                project_id,
                xero_invoice_id: format!("INV-ALLOC-{project_id}-{tag}"),
                reference: format!("INV-ALLOC-{project_id}-{tag}"),
                status: "AUTHORISED".into(),
                amount_cents: cents,
                currency: "USD".into(),
                issued_at: chrono::Utc::now(),
                due_at: None,
            },
        )
        .await
        .expect("mirror the invoice this line settles");
    }

    let applied = store::iolta_withdrawals::apply(
        &surreal,
        &store::iolta_withdrawals::WithdrawalInput {
            xero_transaction_id: format!("bt-withdrawal-{first_id}"),
            xero_account_id: account_id,
            total_cents: first_cents + second_cents,
            currency: "USD".into(),
            occurred_at: chrono::Utc::now(),
            lines: vec![
                store::iolta_withdrawals::AllocationInput {
                    invoice_reference: format!("INV-ALLOC-{first_id}-a"),
                    amount_cents: first_cents,
                },
                store::iolta_withdrawals::AllocationInput {
                    invoice_reference: format!("INV-ALLOC-{second_id}-b"),
                    amount_cents: second_cents,
                },
            ],
        },
    )
    .await
    .expect("apply the pooled withdrawal");
    assert!(
        matches!(applied, store::iolta_withdrawals::Applied::Posted { .. }),
        "expected the withdrawal to post, got {applied:?}"
    );
}

/// A single-matter draw from the California pool — the sibling of
/// `pooled_withdrawal` above, proving California's pool applies and reads
/// back independently of Nevada's.
#[given(regex = r#"^one California pooled withdrawal settles (\d+) cents for "([^"]+)"$"#)]
async fn california_pooled_withdrawal(world: &mut TrustWorld, cents: i64, project_name: String) {
    let surreal = features::shared_surreal().await;
    let project_id = world.project_id(&project_name);
    let jurisdiction = store::jurisdictions::find_or_create(
        &surreal,
        &store::jurisdictions::NewJurisdiction::new("California", "CA", "state"),
    )
    .await
    .expect("seed California");
    // Scoped to this scenario's matter, same reasoning as `xero-nv-features`
    // above: the account is UNIQUE per state, so found-or-kept rather than
    // re-created per scenario.
    let account_id = "xero-ca-features".to_string();
    let _ = store::iolta_accounts::upsert(
        &surreal,
        &store::iolta_accounts::UpsertIoltaAccount {
            jurisdiction_id: jurisdiction.id,
            xero_account_id: account_id.clone(),
            xero_account_code: Some("091".into()),
            name: "IOLTA CA — Trust".into(),
            currency: "USD".into(),
            balance_cents: 0,
            mirrored_at: chrono::Utc::now(),
        },
    )
    .await
    .expect("mirror the California pool");

    store::projects::set_jurisdiction(&surreal, project_id, Some(jurisdiction.id))
        .await
        .expect("point the matter at California");
    let invoice_reference = format!("INV-ALLOC-CA-{project_id}");
    store::xero_invoices::upsert(
        &surreal,
        &store::xero_invoices::UpsertXeroInvoice {
            project_id,
            xero_invoice_id: invoice_reference.clone(),
            reference: invoice_reference.clone(),
            status: "AUTHORISED".into(),
            amount_cents: cents,
            currency: "USD".into(),
            issued_at: chrono::Utc::now(),
            due_at: None,
        },
    )
    .await
    .expect("mirror the invoice this line settles");

    let applied = store::iolta_withdrawals::apply(
        &surreal,
        &store::iolta_withdrawals::WithdrawalInput {
            xero_transaction_id: format!("bt-ca-withdrawal-{project_id}"),
            xero_account_id: account_id,
            total_cents: cents,
            currency: "USD".into(),
            occurred_at: chrono::Utc::now(),
            lines: vec![store::iolta_withdrawals::AllocationInput {
                invoice_reference,
                amount_cents: cents,
            }],
        },
    )
    .await
    .expect("apply the California pooled withdrawal");
    assert!(
        matches!(applied, store::iolta_withdrawals::Applied::Posted { .. }),
        "expected the California withdrawal to post, got {applied:?}"
    );
}

/// One pooled Nevada withdrawal settling two invoices on the *same* matter
/// in a single transfer — the multi-invoice pooled draw, distinct from the
/// multi-matter split `pooled_withdrawal` above already covers.
#[given(
    regex = r#"^one pooled withdrawal settles two invoices of (\d+) cents and (\d+) cents for "([^"]+)"$"#
)]
async fn pooled_withdrawal_two_invoices_one_matter(
    world: &mut TrustWorld,
    first_cents: i64,
    second_cents: i64,
    project_name: String,
) {
    let surreal = features::shared_surreal().await;
    let project_id = world.project_id(&project_name);
    let jurisdiction = store::jurisdictions::find_or_create(
        &surreal,
        &store::jurisdictions::NewJurisdiction::new("Nevada", "NV", "state"),
    )
    .await
    .expect("seed Nevada");
    let account_id = "xero-nv-features".to_string();
    let _ = store::iolta_accounts::upsert(
        &surreal,
        &store::iolta_accounts::UpsertIoltaAccount {
            jurisdiction_id: jurisdiction.id,
            xero_account_id: account_id.clone(),
            xero_account_code: Some("090".into()),
            name: "IOLTA NV — Trust".into(),
            currency: "USD".into(),
            balance_cents: 0,
            mirrored_at: chrono::Utc::now(),
        },
    )
    .await
    .expect("mirror the Nevada pool");

    store::projects::set_jurisdiction(&surreal, project_id, Some(jurisdiction.id))
        .await
        .expect("point the matter at Nevada");

    for (tag, cents) in [("x", first_cents), ("y", second_cents)] {
        store::xero_invoices::upsert(
            &surreal,
            &store::xero_invoices::UpsertXeroInvoice {
                project_id,
                xero_invoice_id: format!("INV-ALLOC-{project_id}-{tag}"),
                reference: format!("INV-ALLOC-{project_id}-{tag}"),
                status: "AUTHORISED".into(),
                amount_cents: cents,
                currency: "USD".into(),
                issued_at: chrono::Utc::now(),
                due_at: None,
            },
        )
        .await
        .expect("mirror the invoice this line settles");
    }

    let applied = store::iolta_withdrawals::apply(
        &surreal,
        &store::iolta_withdrawals::WithdrawalInput {
            xero_transaction_id: format!("bt-multi-invoice-withdrawal-{project_id}"),
            xero_account_id: account_id,
            total_cents: first_cents + second_cents,
            currency: "USD".into(),
            occurred_at: chrono::Utc::now(),
            lines: vec![
                store::iolta_withdrawals::AllocationInput {
                    invoice_reference: format!("INV-ALLOC-{project_id}-x"),
                    amount_cents: first_cents,
                },
                store::iolta_withdrawals::AllocationInput {
                    invoice_reference: format!("INV-ALLOC-{project_id}-y"),
                    amount_cents: second_cents,
                },
            ],
        },
    )
    .await
    .expect("apply the multi-invoice pooled withdrawal");
    assert!(
        matches!(applied, store::iolta_withdrawals::Applied::Posted { .. }),
        "expected the multi-invoice withdrawal to post, got {applied:?}"
    );
}

#[when(regex = r#"^"([^"]+)" opens the detail page for "([^"]+)"$"#)]
async fn open_detail(world: &mut TrustWorld, email: String, project_name: String) {
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
        viewing_as_dri: None,
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

fn cents_to_decimal(cents: i64) -> String {
    let abs = cents.unsigned_abs();
    format!("{}.{:02}", abs / 100, abs % 100)
}

#[then(regex = r"^the response status is (\d+)$")]
async fn status_is(world: &mut TrustWorld, code: u16) {
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
async fn body_contains(world: &mut TrustWorld, needle: String) {
    assert!(
        world.last_body.contains(&needle),
        "expected body to contain {needle:?}; body was: {}",
        truncated(&world.last_body)
    );
}

/// The cross-matter leak check. A pooled account holds many clients' funds;
/// one client's page must not carry another matter's cents.
#[then(regex = r#"^the response body does not contain "([^"]+)"$"#)]
async fn body_does_not_contain(world: &mut TrustWorld, needle: String) {
    assert!(
        !world.last_body.contains(&needle),
        "another matter's trust amount {needle:?} reached this client's page; body was: {}",
        truncated(&world.last_body)
    );
}

#[then("the page shows no trust card")]
async fn no_trust_card(world: &mut TrustWorld) {
    // Match the closing tag so a Dioxus hydration comment between `>` and
    // the text cannot make this a vacuous check a wrongly-rendered card
    // would pass — the same reasoning as the invoice card's absence step.
    assert!(
        !world.last_body.contains("Funds we hold for you</h2>"),
        "expected no trust card on a matter that never held client funds; body was: {}",
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
    TrustWorld::cucumber()
        .run_and_exit("tests/features/portal_trust_position.feature")
        .await;
}
