//! ENG-84: the full `/app` boundary, derived from source rather than
//! hand-copied.
//!
//! `portal/tests/router_contract.rs::CONTRACT` pins a curated, representative
//! sample of the shared surface. This file is the exhaustive complement for
//! one narrower claim: every path this crate registers under `/app` — every
//! [`portal::dioxus_app`] route constant, every `/app/api/*` operation in
//! [`portal::api::documented_api_operations`], and a representative sample of
//! `portal::admin`'s own native-form registrations across each of its four
//! registration functions — denies an anonymous caller. A page is redirected
//! to the login door; an API operation gets the structured `401`. Deriving the
//! list from the actual constants and tables means a newly added page or
//! operation is covered the moment its constant exists here, without anyone
//! remembering to add a case by hand.
//!
//! The exceptions are exactly `/app/health`, `/app/readyz` (proven separately
//! in `server/tests/routes.rs`, since they need a special store-down setup),
//! and `/app/mcp` (Bearer-only, not session-cookie gated — proven separately
//! in `server/tests/mcp_embedded.rs` because its anonymous failure shape is a
//! bare `401`, not the structured JSON body every other `/app/api/*` surface
//! answers with). None of the three appear in the derived list below, because
//! none of them has a `dioxus_app` route constant or an `api` operation-table
//! entry — the admin-registration sample doesn't name them either.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::dioxus_app as pages;
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// Replace every `{segment}` path parameter with a fixed placeholder. The
/// auth boundary runs before any handler reads the parameter, so its value
/// never matters for this test.
fn fill(template: &str) -> String {
    let mut out = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c == '{' {
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    break;
                }
            }
            out.push('x');
        } else {
            out.push(c);
        }
    }
    out
}

/// Every `/app`-prefixed page this crate declares a route constant for, in
/// `portal::dioxus_app`. Excludes the constants that are not full paths
/// (`DOCS_INDEX_SLUG`) and the ones outside `/app` (`DOCS_PATH` and kin,
/// `/design`, `/templates`, `/blog`, …), which `router_contract.rs::CONTRACT`
/// already covers under their own anonymous/host-public classification.
fn dioxus_app_pages() -> Vec<String> {
    [
        pages::LAWYER_ENTITY_TYPES_PATH,
        pages::PROJECTS_PATH,
        pages::LAWYER_DASHBOARD_PATH,
        pages::APP_OUTLINE_PATH,
        pages::NOTATION_OUTLINE_PATH,
        pages::APP_FORMS_PATH,
        pages::PROJECT_DETAIL_PATH,
        pages::LAWYER_PROJECT_NEW_PATH,
        pages::LAWYER_PROJECT_EDIT_PATH,
        pages::LAWYER_PARTICIPATION_NEW_PATH,
        pages::LAWYER_PROJECT_NOTATION_NEW_PATH,
        pages::LAWYER_PARTICIPATION_EDIT_PATH,
        pages::PROJECT_DOCUMENT_PATH,
        pages::CONVERSATION_PATH,
        pages::PORTAL_INTAKE_PATH,
        pages::LAWYER_CLAUSES_PATH,
        pages::LAWYER_WALKER_STEP_PATH,
        pages::LAWYER_INTAKE_REVIEW_PATH,
        pages::LAWYER_REASK_PATH,
        pages::REVIEW_PATH,
        pages::LAWYER_EXPUNGE_QUEUE_PATH,
        pages::LAWYER_DOCUMENT_EXPUNGE_PATH,
        pages::LAWYER_CONTRACT_REVIEW_PATH,
        pages::LAWYER_ENTITY_NEW_PATH,
        pages::ADMIN_PEOPLE_NEW_PATH,
        pages::ADMIN_PERSON_PATH,
        pages::ADMIN_PERSON_EDIT_PATH,
        pages::LAWYER_ENTITY_EDIT_PATH,
        pages::LAWYER_RETAINER_NEW_PATH,
        pages::LAWYER_SCHEDULES_PATH,
        pages::ADMIN_ANALYTICS_PATH,
        pages::ADMIN_MATTER_DIRECTORY_PATH,
        pages::LAWYER_ENTITIES_PATH,
        pages::ADMIN_PEOPLE_PATH,
        pages::LAWYER_JURISDICTIONS_PATH,
        pages::LAWYER_GIT_REPOSITORIES_PATH,
        pages::LAWYER_PERSON_ENTITY_ROLES_PATH,
        pages::LAWYER_NOTATIONS_PATH,
        pages::LAWYER_ANSWERS_PATH,
        pages::LAWYER_ADDRESSES_PATH,
        pages::LAWYER_ASSETS_PATH,
        pages::LAWYER_PERSON_PROJECT_ROLES_PATH,
        pages::LAWYER_DISCLOSURES_PATH,
        pages::LAWYER_RELATIONSHIP_LOGS_PATH,
        pages::LAWYER_MAILROOMS_PATH,
        pages::LAWYER_LETTERS_PATH,
        pages::LAWYER_EMAIL_LOG_PATH,
        pages::LAWYER_LETTER_DETAIL_PATH,
        pages::ADMIN_LANDING_PATH,
        pages::LAWYER_PLAYBOOKS_PATH,
        pages::LAWYER_PLAYBOOK_NEW_PATH,
        pages::LAWYER_PLAYBOOK_EDIT_PATH,
        pages::LAWYER_TEMPLATES_PATH,
        pages::LAWYER_QUESTIONS_PATH,
        pages::APP_DOCUMENTS_PATH,
        pages::APP_DOCUMENT_PATH,
        pages::APP_TEAM_PATH,
        pages::APP_BRANDS_PATH,
        pages::APP_OWNER_PATH,
        pages::APP_PROFILE_PATH,
        pages::FIRM_SHOW_PATH,
    ]
    .iter()
    .map(|template| fill(template))
    .collect()
}

/// A representative sample of `portal::admin`'s own native-form
/// registrations — the paths with no `dioxus_app` route constant, because
/// they are POST-only mutations or file downloads rather than a Dioxus page.
/// One or more per registration function (`admin::routes` itself,
/// `register_firm_matter_routes`, `register_firm_admin_routes`,
/// `register_project_routes`), so a boundary gap in any one of the four is
/// covered by at least one path here.
fn admin_registration_sample() -> Vec<String> {
    [
        // admin::routes() top level.
        "/app/admin/people/x/avatar",
        "/app/admin/people/x/welcome",
        "/app/admin/people/x/delete",
        "/app/avatar",
        "/app/profile/avatar",
        "/app/people/x/avatar",
        "/app/team/fonts/gorp-serif.zip",
        "/app/notations/x/documents/x",
        "/app/forms/x",
        // register_firm_matter_routes (/app/lawyer).
        "/app/lawyer/notations/x/transcript",
        "/app/lawyer/notations/x/sign",
        "/app/lawyer/expunge-requests/x/authorize",
        "/app/lawyer/contract-reviews/x/approve",
        "/app/view-as-client/stop",
        // register_firm_admin_routes (/app/admin).
        "/app/admin/people.csv",
        "/app/admin/entities.csv",
        "/app/admin/schedules/x/run",
        // register_project_routes (/app/projects).
        "/app/projects.csv",
        "/app/projects/x/documents/upload",
        "/app/projects/x/close",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

async fn anonymous_get(app: &axum::Router, path: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// Every derived `/app` page redirects an anonymous browser to the login
/// door — the same `ProtectedHuman` shape `router_contract.rs::CONTRACT`
/// pins for its representative sample, just exhaustively over every declared
/// route constant plus a sample of the native-form registrations.
#[tokio::test]
async fn every_declared_app_page_denies_an_anonymous_browser() {
    let state = portal::test_support::app_state(mem_surreal().await).await;
    let app = portal::router(state);

    for path in dioxus_app_pages()
        .into_iter()
        .chain(admin_registration_sample())
    {
        let response = anonymous_get(&app, &path).await;
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "{path} must send an anonymous browser to the login door"
        );
        let location = response
            .headers()
            .get(axum::http::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            location.starts_with("/auth/login?return_to="),
            "{path} must redirect through /auth/login, got {location}"
        );
    }
}

/// Every `/app/api/*` operation this crate registers — derived from the same
/// table `portal::api::routes` builds the real router from — refuses an
/// anonymous machine caller with the structured `401`, never a login
/// redirect.
#[tokio::test]
async fn every_declared_app_api_operation_denies_an_anonymous_caller() {
    let state = portal::test_support::app_state(mem_surreal().await).await;
    let app = portal::router(state);

    for (_method, path) in portal::api::documented_api_operations() {
        let response = anonymous_get(&app, path).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{path} must refuse a machine caller with a status, not a redirect"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let document: serde_json::Value = serde_json::from_slice(&body)
            .unwrap_or_else(|e| panic!("{path} must answer with JSON: {e}"));
        assert_eq!(
            document.get("error").and_then(serde_json::Value::as_str),
            Some("unauthenticated"),
            "{path} must keep the structured unauthenticated shape"
        );
    }
}
