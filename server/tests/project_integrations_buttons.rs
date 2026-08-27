//! Integration test: the two resource-bearing sections on
//! `GET /app/projects/:code` — the "Resources" panel (the matter's Slack
//! channels, Notion pages, Drive folder, and portal) and the "Integrations"
//! section that still carries Xero and the source repository.
//!
//! The audience split is what these assert. Xero and the repository stay
//! lawyer-only. A resource is firm-only or shared by *name*: a client sees the
//! shared Slack channel and shared Notion page and never the private ones,
//! because the private Notion page holds firm work product and the private
//! channel holds lawyer-only chatter.

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
    lawyer_cookie: String,
    client_cookie: String,
}

async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-project-integrations-test"))
            .await
            .unwrap(),
    );

    let client = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Libra", "libra@example.com", Role::Client),
    )
    .await
    .unwrap();
    let proj = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-integrations-{}", Uuid::now_v7()),
            name: "Libra integrations".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Lawyer Member", "lawyer@example.com", Role::Lawyer),
    )
    .await
    .unwrap();
    for (pid, participation) in [(lawyer.id, "lawyer"), (client.id, "client")] {
        store::projects::add_participation(&surreal, proj.id, pid, participation)
            .await
            .unwrap();
    }

    let sessions = SessionStore::new(KEY);
    let mut lawyer_session = SessionData::fresh("lawyer-sub", Role::Lawyer);
    lawyer_session.person_id = Some(lawyer.id);
    let lawyer_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&lawyer_session));
    let mut client_session = SessionData::fresh("client-sub", Role::Client);
    client_session.person_id = Some(client.id);
    let client_cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&client_session));

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
        project_id: proj.id,
        project_code: proj.code,
        lawyer_cookie,
        client_cookie,
    }
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn get_as(app: &axum::Router, project_code: &str, cookie: &str) -> String {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{project_code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_string(resp).await
}

/// A matter with neither Slack link set and no raised invoice shows no
/// "Integrations" section at all — nothing to point at, so no empty shell.
#[tokio::test]
async fn a_matter_with_no_integrations_set_has_no_integrations_section() {
    let f = build_fixture().await;
    let html = get_as(&f.app, &f.project_code, &f.lawyer_cookie).await;
    assert!(!html.contains("Integrations"), "no Xero and no repository");
    // The Resources panel is still here: the client portal is configured by the
    // matter existing rather than by a column, so it is the one row a matter
    // with nothing else set still carries.
    assert!(html.contains("Resources"));
    assert!(html.contains(r#"data-resource="client-portal""#));
    for unset in [
        "private-slack-channel",
        "private-notion-page",
        "private-drive-folder",
        "shared-slack-channel",
        "shared-notion-page",
    ] {
        assert!(
            !html.contains(&format!(r#"data-resource="{unset}""#)),
            "an unset resource must not render a slot: {unset}"
        );
    }
}

/// The private Slack channel renders when set; the shared one is genuinely
/// optional and only appears when the matter has one.
#[tokio::test]
async fn lawyer_sees_the_internal_slack_button_and_the_optional_external_one() {
    const INTERNAL: &str = "https://neonlaw.slack.com/archives/C0INTERNAL";
    let f = build_fixture().await;
    store::projects::update_project(
        &f.surreal,
        f.project_id,
        &store::projects::UpdateProjectCommand {
            name: Some("Libra integrations".into()),
            internal_slack_channel_url: Some(INTERNAL.into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let html = get_as(&f.app, &f.project_code, &f.lawyer_cookie).await;
    assert!(html.contains("Resources"));
    assert!(html.contains(INTERNAL));
    assert!(html.contains(r#"data-resource="private-slack-channel""#));
    assert!(html.contains("Private Slack channel"));
    assert!(
        !html.contains(r#"data-resource="shared-slack-channel""#),
        "no shared channel was set"
    );
}

#[tokio::test]
async fn lawyer_sees_the_external_slack_button_when_the_matter_has_one() {
    const EXTERNAL: &str = "https://neonlaw.slack.com/archives/C0EXTERNAL";
    let f = build_fixture().await;
    store::projects::update_project(
        &f.surreal,
        f.project_id,
        &store::projects::UpdateProjectCommand {
            name: Some("Libra integrations".into()),
            external_slack_channel_url: Some(EXTERNAL.into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let html = get_as(&f.app, &f.project_code, &f.lawyer_cookie).await;
    assert!(html.contains(EXTERNAL));
    assert!(html.contains(r#"data-resource="shared-slack-channel""#));
    assert!(html.contains("Shared Slack channel"));
}

/// The Xero button links straight to the matter's raised invoice, mirrored
/// locally in `xero_invoice` and keyed uniquely on `project_id` — the
/// invoice is already grouped per matter, so the button needs no fan-out.
#[tokio::test]
async fn lawyer_sees_the_xero_button_pointing_at_the_raised_invoice() {
    const XERO_ID: &str = "11111111-2222-3333-4444-555555555555";
    let f = build_fixture().await;
    store::xero_invoices::upsert(
        &f.surreal,
        &store::xero_invoices::UpsertXeroInvoice {
            project_id: f.project_id,
            xero_invoice_id: XERO_ID.into(),
            reference: format!("Matter {}", f.project_id),
            status: "AUTHORISED".into(),
            amount_cents: 50_000,
            currency: "USD".into(),
        },
    )
    .await
    .unwrap();

    let html = get_as(&f.app, &f.project_code, &f.lawyer_cookie).await;
    assert!(html.contains(&format!(
        "https://go.xero.com/AccountsReceivable/View.aspx?InvoiceID={XERO_ID}"
    )));
    assert!(html.contains(">Xero<"));
}

/// **The confidentiality boundary.** A client sees the resources shared with
/// them and none of the firm's, even with every one of them set — and never
/// Xero or the source repository, which stay wholly lawyer-only.
///
/// The private Notion page is firm work product and the private channel is
/// lawyer-only chatter, so this asserts on the URLs themselves: a filter that
/// dropped a row's label while leaving its `href` in the markup would still
/// have leaked the address.
#[tokio::test]
async fn a_client_sees_the_shared_resources_and_none_of_the_firms() {
    const PRIVATE_SLACK: &str = "https://neonlaw.slack.com/archives/C0PRIVATE";
    const SHARED_SLACK: &str = "https://neonlaw.slack.com/archives/C0SHARED";
    const PRIVATE_NOTION: &str = "https://www.notion.so/neonlaw/Private-abc123";
    const SHARED_NOTION: &str = "https://www.notion.so/neonlaw/Shared-def456";
    let f = build_fixture().await;
    store::projects::update_project(
        &f.surreal,
        f.project_id,
        &store::projects::UpdateProjectCommand {
            name: Some("Libra integrations".into()),
            internal_slack_channel_url: Some(PRIVATE_SLACK.into()),
            external_slack_channel_url: Some(SHARED_SLACK.into()),
            private_notion_page_url: Some(PRIVATE_NOTION.into()),
            shared_notion_page_url: Some(SHARED_NOTION.into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    store::projects::set_drive_folder_id(&f.surreal, f.project_id, Some("1QaBcDPRIVATE"))
        .await
        .unwrap();
    store::xero_invoices::upsert(
        &f.surreal,
        &store::xero_invoices::UpsertXeroInvoice {
            project_id: f.project_id,
            xero_invoice_id: "11111111-2222-3333-4444-555555555555".into(),
            reference: format!("Matter {}", f.project_id),
            status: "AUTHORISED".into(),
            amount_cents: 50_000,
            currency: "USD".into(),
        },
    )
    .await
    .unwrap();

    let html = get_as(&f.app, &f.project_code, &f.client_cookie).await;
    assert!(
        html.contains("Libra integrations"),
        "renders the client view"
    );

    // Shared with the client, so shown.
    assert!(html.contains(SHARED_SLACK), "the shared channel is theirs");
    assert!(html.contains(SHARED_NOTION), "the shared page is theirs");
    assert!(html.contains(r#"data-resource="client-portal""#));

    // The firm's side, and the lawyer-only integrations: none of it reaches the
    // page, by URL or by row.
    for firm_only in [
        PRIVATE_SLACK,
        PRIVATE_NOTION,
        "1QaBcDPRIVATE",
        "drive.google.com",
        "go.xero.com",
    ] {
        assert!(
            !html.contains(firm_only),
            "a client's page leaked `{firm_only}`"
        );
    }
    for firm_row in [
        "private-slack-channel",
        "private-notion-page",
        "private-drive-folder",
    ] {
        assert!(
            !html.contains(&format!(r#"data-resource="{firm_row}""#)),
            "a client's page rendered the firm-only row `{firm_row}`"
        );
    }
    assert!(!html.contains("Integrations"), "Xero stays lawyer-only");
    // A client configures nothing.
    assert!(!html.contains("Configure resources"));
}
