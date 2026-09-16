#![allow(clippy::doc_markdown)]
//! ENG-720: the lawyer Projects list (`GET /app/lawyer`) carries State and
//! Next columns per matter, read from `workflows::client_phrase_for` against
//! whichever of the matter's Notations `store::notations::furthest_along_state`
//! picks as furthest along.
//!
//! Seeds the sample-matter fixture rather than an ad hoc project, so this
//! route test never invents a synthetic matter name or code of its own — a
//! real Project code is a client identifier (see `docs/agent-workflows.md`),
//! and the seeded sample matters are the one fixture already cleared to
//! appear in a test's assertions.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::SESSION_COOKIE_NAME;
use portal::test_support::TEST_SESSION_KEY;
use portal::{AppState, SessionData, SessionStore};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The `<tr>...</tr>` fragment naming `needle`, e.g. one row of the KPI
/// table. The full app renders each dynamic text node wrapped in
/// `data-node-hydration` attributes and `<!--node-idN-->...<!--#-->`
/// hydration-marker comments, so a row can't be matched as one literal
/// string the way a static `dioxus_ssr::render_element` fragment can — this
/// isolates the row first, and [`visible_text`] strips those markers back
/// out of it.
fn row_containing<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("expected to find `{needle}` in the rendered page:\n{html}"));
    let start = html[..at].rfind("<tr").expect("row start");
    let end = html[at..]
        .find("</tr>")
        .map(|i| at + i + "</tr>".len())
        .expect("row end");
    &html[start..end]
}

/// The plain text a reader would see in an HTML fragment: comments and tags
/// stripped, so a hydration marker or a `data-node-hydration` attribute
/// can't masquerade as rendered content.
fn visible_text(fragment: &str) -> String {
    let mut without_comments = String::with_capacity(fragment.len());
    let mut rest = fragment;
    while let Some(start) = rest.find("<!--") {
        without_comments.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => break,
        };
    }
    without_comments.push_str(rest);

    let mut text = String::with_capacity(without_comments.len());
    let mut in_tag = false;
    for ch in without_comments.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text
}

/// A cookie for the fixture Lawyer, carrying the linked `person_id` the
/// dashboard's loader reads to scope the matter list.
fn lawyer_cookie(person_id: uuid::Uuid) -> String {
    let sessions = SessionStore::new(TEST_SESSION_KEY);
    let mut session = SessionData::fresh("eng-720-columns-test", Role::Lawyer);
    session.person_id = Some(person_id);
    format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&session))
}

#[tokio::test]
async fn the_lawyer_projects_list_renders_state_and_next_for_a_live_notation_and_empty_for_none() {
    let surreal = mem_surreal().await;
    let state: AppState = portal::test_support::app_state(surreal.clone()).await;
    let storage: Arc<dyn cloud::StorageService> = state.storage.clone();
    store::seed::seed_canonical(&surreal, &storage)
        .await
        .unwrap();
    store::seed::seed_sample_portfolio(&surreal, &storage, store::DeploymentEnvironment::Dev)
        .await
        .unwrap();

    let lawyer = store::persons::find_by_email_ci(&surreal, "lawyer@neonlaw.com")
        .await
        .unwrap()
        .expect("the sample-portfolio fixture seeds the fixture Lawyer");

    // `sample-litigation` gets a live Notation below, so its row must read a
    // non-generic State/Next; `sample-estate` gets none, so its row must
    // render both columns empty rather than a generic placeholder phrase.
    let with_notation = store::projects::find_by_code(&surreal, "sample-litigation")
        .await
        .unwrap()
        .expect("the sample-matter fixture seeds sample-litigation");
    let without_notation = store::projects::find_by_code(&surreal, "sample-estate")
        .await
        .unwrap()
        .expect("the sample-matter fixture seeds sample-estate");

    let template = store::templates::save_version(
        &surreal,
        None,
        "test__eng_720_columns",
        store::templates::Version {
            title: "ENG-720 columns fixture".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();

    store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(
            template.id,
            lawyer.id,
            with_notation.id,
            "lawyer_review",
        ),
    )
    .await
    .unwrap();

    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/lawyer")
                .header(header::COOKIE, lawyer_cookie(lawyer.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /app/lawyer should render"
    );
    let html = body_string(resp).await;

    assert!(html.contains(r#"<th scope="col">State</th>"#), "{html}");
    assert!(html.contains(r#"<th scope="col">Next</th>"#), "{html}");

    // The live notation's row reads `workflows`' real phrase for
    // `lawyer_review`, not a generic placeholder.
    let phrase = workflows::client_phrase_for("lawyer_review");
    let live_row = visible_text(row_containing(
        &html,
        &format!("/app/projects/{}", with_notation.code),
    ));
    assert!(live_row.contains(&with_notation.name), "{live_row}");
    assert!(live_row.contains(phrase.where_this_is), "{live_row}");
    assert!(live_row.contains(phrase.whats_next), "{live_row}");

    // The matter with no Notation renders empty State/Next cells rather than
    // a generic phrase: its row's only visible text is the matter's own name.
    let empty_row = visible_text(row_containing(
        &html,
        &format!("/app/projects/{}", without_notation.code),
    ));
    assert_eq!(empty_row, without_notation.name, "{empty_row}");
}
