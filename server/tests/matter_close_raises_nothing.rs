//! Closing a matter records legal work. It raises no money — for **any**
//! matter.
//!
//! Accounting originates in Xero: lawyers agree a matter's price with the
//! client and raise the invoice there directly. So countersigning a
//! `offboarding__letter` must close the matter and touch the accounting seam
//! **not at all** — no `ensure_contact`, no `create_invoice`, and no
//! `xero_invoice` mirror row.
//!
//! The close path is deliberately shape-agnostic: `close_matter_post`
//! reads the project, the `offboarding__letter` template, and the project's
//! client participation, and nothing else. It never consults a product, a
//! catalog price, or the matter's originating work. This test pins that by
//! closing several unrelated matters through the same real HTTP walk —
//! including a matter still tagged with a priced product code, and a matter
//! with no originating work at all — and asserting all of them close and
//! none of them bills.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use portal::billing::StubBillingProvider;
use portal::AppState;
use store::seed;
use store::surreal::SurrealDb;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

/// Percent-encode a form value — the closing walk answers are posted as
/// `application/x-www-form-urlencoded`.
fn urlencoding(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ' ' => "+".to_string(),
            c if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') => c.to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}

/// One matter shape to close. The point of the table is that none of these
/// distinctions reaches the close path.
struct Matter {
    /// What makes this shape worth closing separately.
    what: &'static str,
    /// The matter's originating work notation, if any. `None` closes a
    /// matter that carries nothing but its closing letter.
    originating_template: Option<&'static str>,
}

/// The six closing-letter answers, in walk order.
const CLOSING_ANSWERS: [&str; 6] = [
    "Capricorn",
    "The engagement",
    "Wound up the engagement",
    "paid_in_full",
    "Returned on request, kept 7 years",
    "None",
];

/// Seed one matter of the given shape with its client, returning
/// `(project_id, project_code, client_id)` — the id for the assertions and the
/// code the `/app/projects/{project_code}/close` route is keyed by.
async fn seed_matter(surreal: &SurrealDb, matter: &Matter) -> (Uuid, String, Uuid) {
    let client = store::persons::create(
        surreal,
        &store::persons::NewPerson::new(
            "Capricorn",
            format!("capricorn-{}@example.com", Uuid::now_v7()),
        ),
    )
    .await
    .unwrap();
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("matter-{}", Uuid::now_v7()),
            name: matter.what.into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::add_participation(surreal, project.id, client.id, "client")
        .await
        .unwrap();

    if let Some(code) = matter.originating_template {
        let template = store::templates::resolve(surreal, None, code)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{code} is seeded"));
        store::notations::create(
            surreal,
            &store::notations::NewNotation::new(
                template.id,
                client.id,
                project.id,
                "signature_received",
            ),
        )
        .await
        .unwrap();
    }
    (project.id, project.code, client.id)
}

/// Drive the whole closing walk over real HTTP: open it, then answer every
/// question. The final answer countersigns the letter, which closes the
/// matter — and must still land on the firm dashboard rather than an error,
/// since the closing flow's success path is unchanged by the fee's removal.
async fn walk_the_close(app: &Router, project_code: &str, what: &str) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/app/projects/{project_code}/close"))
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SEE_OTHER,
        "{what}: close should start the walk"
    );
    let closing_id: Uuid = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_else(|| panic!("{what}: close must redirect"))
        .trim_start_matches("/app/lawyer/notations/")
        .trim_end_matches("/step")
        .parse()
        .unwrap_or_else(|e| panic!("{what}: redirect carries a notation id: {e}"));

    for (i, value) in CLOSING_ANSWERS.iter().enumerate() {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/app/lawyer/notations/{closing_id}/step"))
                    .header(
                        "authorization",
                        portal::test_support::lawyer_bearer_header(),
                    )
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(format!("value={}", urlencoding(value))))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "{what}: answer {i}={value}"
        );
        if i == CLOSING_ANSWERS.len() - 1 {
            assert_eq!(
                resp.headers().get("location").and_then(|v| v.to_str().ok()),
                Some("/app/lawyer"),
                "{what}: the final answer returns to /app/lawyer"
            );
        }
    }
}

#[tokio::test]
async fn any_matter_closes_and_none_of_them_raises_money() {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-close-no-fee-storage"))
            .await
            .unwrap(),
    );
    // The canonical seed brings the `offboarding__letter` template and the
    // originating templates below.
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    // A dispatching runtime backed by the store so the firm-signature
    // transition runs its `close_matter` side effect in-process, the same
    // path the dev binary and the worker take.
    let inner = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        DispatchingRuntime::new(inner.clone(), email.clone(), storage.clone())
            .with_store(surreal.clone()),
    );
    // One concrete billing handle across every close, so the final
    // assertion covers the whole run rather than one matter.
    let billing = Arc::new(StubBillingProvider::new());
    let state = AppState {
        storage: storage.clone(),
        workflow_runtime,
        questionnaire_runtime: inner,
        email,
        billing_provider: billing.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let matters = [
        Matter {
            what: "a bare matter — no originating work",
            originating_template: None,
        },
        Matter {
            what: "an engagement letter",
            originating_template: Some("onboarding__letter"),
        },
        Matter {
            what: "an LLC formation",
            originating_template: Some("nv__llc_formation"),
        },
    ];

    for matter in &matters {
        let (project_id, project_code, client_id) = seed_matter(&surreal, matter).await;
        walk_the_close(&app, &project_code, matter.what).await;

        // The matter is closed …
        let row = store::projects::find_by_id(&surreal, project_id)
            .await
            .unwrap()
            .expect("project exists");
        assert_eq!(row.status, "closed", "{}: must close", matter.what);

        // … and no invoice was mirrored against it.
        assert!(
            store::xero_invoices::for_projects(&surreal, &[project_id])
                .await
                .unwrap()
                .is_empty(),
            "{}: must not write an invoice mirror row",
            matter.what
        );

        // The client keeps no cached Xero contact id either — that cache
        // was only ever written by the raise path.
        let payer = store::persons::find_by_id(&surreal, client_id)
            .await
            .unwrap()
            .expect("client exists");
        assert_eq!(
            payer.xero_contact_id, None,
            "{}: must not cache a Xero contact id on the client",
            matter.what
        );
    }

    // Across every matter shape above, Navigator never reached the
    // accounting seam at all.
    assert!(
        billing.contact_calls().is_empty(),
        "closing a matter must not resolve a Xero contact, got {:?}",
        billing.contact_calls()
    );
    assert!(
        billing.calls().is_empty(),
        "closing a matter must not raise a Xero invoice, got {:?}",
        billing.calls()
    );
}
