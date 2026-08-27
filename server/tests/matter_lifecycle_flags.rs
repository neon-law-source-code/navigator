#![allow(clippy::doc_markdown)]
//! Matter-lifecycle warning flags on the admin Projects list.
//!
//! The firm's lifecycle invariant: every matter opens on an onboarding
//! (`onboarding__*`) notation — the client's retainer — and a *closed*
//! matter carries an `offboarding__letter`. Neither is schema-enforced, so the
//! Projects list surfaces the gaps with a warning badge. These tests pin
//! both the pure rule (`store::projects::matter_flags`) and the rendered list.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use store::persons::Role;
use store::seed;
use tower::ServiceExt;
use uuid::Uuid;
// Keyed render assertions: the matter-flag badges are catalog copy.
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::test_support::mem_surreal;
use workflows::{InMemoryRuntime, StateMachineRuntime};

const SESSION_KEY: &str = "test-session-key-not-for-production";

// ---- the pure rule ----

#[test]
fn flags_an_open_matter_with_no_onboarding_as_missing_retainer() {
    assert_eq!(
        store::projects::matter_flags(false, "open", false),
        (true, false)
    );
}

#[test]
fn a_matter_with_an_onboarding_notation_is_not_missing_its_retainer() {
    assert_eq!(
        store::projects::matter_flags(true, "open", false),
        (false, false)
    );
}

#[test]
fn flags_a_closed_matter_with_no_closing_letter() {
    // Has its retainer, but closed without a closing letter.
    assert_eq!(
        store::projects::matter_flags(true, "closed", false),
        (false, true)
    );
}

#[test]
fn a_closed_matter_with_a_closing_letter_is_clean() {
    assert_eq!(
        store::projects::matter_flags(true, "closed", true),
        (false, false)
    );
}

#[test]
fn an_open_matter_never_owes_a_closing_letter() {
    // No closing letter on an open matter is fine — it is only owed at close.
    assert_eq!(
        store::projects::matter_flags(true, "open", false),
        (false, false)
    );
}

// ---- what counts as the matter-opening engagement ----

#[test]
fn an_onboarding_template_opens_a_matter_whatever_its_code() {
    // The flag keys off the declared `kind`, never the template's code.
    // Every onboarding template happens to use an `onboarding__*` code
    // today; a future one that does not must still count, or the badge
    // would lie about a matter that has its engagement.
    assert!(store::projects::template_opens_a_matter(Some("onboarding")));
}

#[test]
fn a_retired_retainer_kind_string_no_longer_opens_a_matter() {
    // `Kind::Retainer` was merged into `Kind::Onboarding` — every
    // `kind: retainer` template was reclassified `kind: onboarding`, so
    // the bare string `"retainer"` is no longer a recognized `Kind` at
    // all and must not open a matter by accident.
    assert!(!store::projects::template_opens_a_matter(Some("retainer")));
}

#[test]
fn work_inside_an_open_matter_does_not_open_one() {
    for kind in ["letter", "filing", "will", "trust", "directive", "memo"] {
        assert!(
            !store::projects::template_opens_a_matter(Some(kind)),
            "{kind} is work inside a matter, not the engagement that opens it"
        );
    }
}

#[test]
fn an_absent_or_unknown_kind_never_opens_a_matter() {
    // A template that declares no kind — or one this build does not
    // recognize — cannot be a matter's engagement. The badge stays on:
    // better to over-warn lawyer than to silently call a matter opened.
    assert!(!store::projects::template_opens_a_matter(None));
    assert!(!store::projects::template_opens_a_matter(Some("bogus")));
}

#[test]
fn the_badge_reads_every_kind_straight_from_the_classifier() {
    // The badge must answer "does this kind open a matter?" from
    // `rules::kind::Kind::opens_a_matter` and nothing else — re-keying it to
    // a template's `code` (the `onboarding__*` prefix it used to use) would
    // put a permanent "no engagement letter" warning on every matter opened
    // by a retainer template named otherwise. This walks `Kind::ALL` so a
    // new kind cannot slip through unclassified.
    //
    // Scope: this pins the *badge* to the classifier. The first-notation
    // gate in `workflows::notation_session` reads the same classifier, but
    // it is not exercised here — `an_onboarding_opens_a_matter_as_its_first_notation`
    // (`web/tests/project_notation_create.rs`) is what drives a real
    // `kind: onboarding` template through that gate.
    for kind in rules::kind::Kind::ALL {
        assert_eq!(
            store::projects::template_opens_a_matter(Some(kind.as_str())),
            kind.opens_a_matter(),
            "the badge must classify {} straight from `Kind::opens_a_matter`",
            kind.as_str()
        );
    }
}

// ---- what counts as the matter-closing offboarding letter ----

#[test]
fn an_offboarding_template_closes_a_matter_whatever_its_code() {
    // The flag keys off the declared `kind`, never the template's code —
    // the mirror of the engagement side above.
    assert!(store::projects::template_closes_a_matter(Some(
        "offboarding"
    )));
}

#[test]
fn an_ordinary_letter_does_not_close_a_matter() {
    // `kind: letter` is too broad to be the closing classifier — every
    // offboarding letter is a letter, but a demand, notice, or settlement
    // letter must not silently clear the badge.
    assert!(!store::projects::template_closes_a_matter(Some("letter")));
}

#[test]
fn work_inside_an_open_matter_does_not_close_it() {
    for kind in ["onboarding", "filing", "will", "trust", "directive", "memo"] {
        assert!(
            !store::projects::template_closes_a_matter(Some(kind)),
            "{kind} is work inside a matter, not the letter that closes it"
        );
    }
}

#[test]
fn an_absent_or_unknown_kind_never_closes_a_matter() {
    assert!(!store::projects::template_closes_a_matter(None));
    assert!(!store::projects::template_closes_a_matter(Some("bogus")));
}

#[test]
fn the_closing_badge_reads_every_kind_straight_from_the_classifier() {
    // The mirror of `the_badge_reads_every_kind_straight_from_the_classifier`:
    // the badge must answer "does this kind close a matter?" from
    // `rules::kind::Kind::closes_a_matter` and nothing else.
    for kind in rules::kind::Kind::ALL {
        assert_eq!(
            store::projects::template_closes_a_matter(Some(kind.as_str())),
            kind.closes_a_matter(),
            "the badge must classify {} straight from `Kind::closes_a_matter`",
            kind.as_str()
        );
    }
}

#[tokio::test]
async fn a_matter_closed_on_a_bespoke_offboarding_code_still_clears_the_flag() {
    // The assertion that would have caught the defect this issue fixes:
    // keying the closing badge off `t.code == "closing__letter"` badges a
    // matter offboarded on a differently-named template forever, even
    // though its template legitimately declares `kind: offboarding`.
    let (app, surreal) = build_app().await;
    let _ = app;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Bespoke", "bespoke@example.com"),
    )
    .await
    .unwrap();
    let matter = project(&surreal, "Bespoke closing matter", "closed").await;

    let bespoke = store::templates::save_version(
        &surreal,
        Some(matter),
        "practice__bespoke_closing",
        store::templates::Version {
            title: "Bespoke Closing Letter".into(),
            respondent_type: "person".into(),
            asset_id: None,
            form_code: None,
            kind: Some("offboarding".into()),
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();

    store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(bespoke.id, person.id, matter, "BEGIN"),
    )
    .await
    .unwrap();

    let projects = vec![store::projects::find_by_id(&surreal, matter)
        .await
        .unwrap()
        .expect("matter row")];
    let (_, has_closing) = store::projects::matter_lifecycle_sets(&surreal, &projects)
        .await
        .unwrap();
    assert!(
        has_closing.contains(&matter),
        "a bespoke-coded kind: offboarding template must still clear the closing-letter flag"
    );
}

// ---- the rendered list ----

async fn build_app() -> (axum::Router, store::surreal::SurrealDb) {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-matter-flags-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(
        workflows::DispatchingRuntime::new(runtime.clone(), email.clone(), storage.clone()),
    );
    let state = AppState {
        sessions: SessionStore::new(SESSION_KEY),
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (
        server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
    )
}

async fn project(surreal: &store::surreal::SurrealDb, name: &str, status: &str) -> Uuid {
    store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("test-{}", Uuid::now_v7()),
            name: name.into(),
            status: status.into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .id
}

async fn notation(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    template_code: &str,
) {
    let tmpl = store::templates::resolve(surreal, None, template_code)
        .await
        .unwrap()
        .expect("template seeded");
    store::notations::create(
        surreal,
        &store::notations::NewNotation::new(tmpl.id, person_id, project_id, "BEGIN"),
    )
    .await
    .unwrap();
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// An Admin who is firm-side on each of `projects`.
///
/// The matter surface and the matters list are both participation-scoped for
/// every tier now, so a test that wants to *see* matters has to put its actor
/// on them. The scoping itself is pinned in `store::access`.
async fn participating_admin(surreal: &store::surreal::SurrealDb, projects: &[Uuid]) -> Uuid {
    let admin = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(
            "Lifecycle Admin",
            "lifecycle-admin@neonlaw.com",
            Role::Admin,
        ),
    )
    .await
    .expect("seed the acting admin");
    for project_id in projects {
        store::projects::add_participation(surreal, *project_id, admin.id, "attorney")
            .await
            .expect("put the acting admin on the matter");
    }
    admin.id
}

/// The matter show page — the page lawyer land on straight after opening a
/// matter, when it legitimately has no engagement letter yet.
async fn get_project(app: &axum::Router, code: &str, admin_person: Uuid) -> String {
    let mut admin = SessionData::fresh("admin-sub", Role::Admin);
    // The matter surface scopes every tier by participation, so the acting
    // admin has to be on the matter — a bare session used to ride the bypass.
    admin.person_id = Some(admin_person);
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={}",
        SessionStore::new(SESSION_KEY).encode(&admin)
    );
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{code}"))
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_string(resp).await
}

#[tokio::test]
async fn a_matter_show_page_carries_no_engagement_letter_disclaimer() {
    // The lifecycle gap belongs to the matters *list*, where lawyers scan the
    // whole portfolio. The show page is the workbench for one matter a lawyer
    // already opened deliberately, so it carries no banner about the missing
    // engagement letter — not even on a fresh matter that has none yet.
    let (app, surreal) = build_app().await;
    let bare = project(&surreal, "Bare show matter", "open").await;
    let admin_person = participating_admin(&surreal, &[bare]).await;
    let bare_code = store::projects::find_by_id(&surreal, bare)
        .await
        .unwrap()
        .expect("bare matter")
        .code;

    let html = get_project(&app, &bare_code, admin_person).await;
    assert!(!html.contains("no retainer"), "{html}");
    assert!(!html.contains("has no engagement letter"), "{html}");
    assert!(!html.contains("notation create"), "{html}");
}

#[tokio::test]
async fn projects_list_flags_the_lifecycle_gaps_and_nothing_else() {
    let (app, surreal) = build_app().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Aries", "aries@example.com"),
    )
    .await
    .unwrap();

    // A: open, has its retainer → clean.
    let a = project(&surreal, "Has retainer open", "open").await;
    notation(&surreal, a, person.id, "onboarding__retainer").await;
    // B: open, no onboarding notation → missing retainer.
    let b = project(&surreal, "Bare open matter", "open").await;
    // C: closed, has its retainer but no offboarding letter → missing offboarding letter.
    let c = project(&surreal, "Closed no letter", "closed").await;
    notation(&surreal, c, person.id, "onboarding__estate").await;
    // D: closed, has both → clean.
    let d = project(&surreal, "Closed with letter", "closed").await;
    notation(&surreal, d, person.id, "onboarding__retainer").await;
    notation(&surreal, d, person.id, "offboarding__letter").await;

    // The matters list is participation-scoped for every tier since ENG-81,
    // so the acting admin is put on each seeded matter rather than relying on
    // a bypass to see the whole portfolio.
    let admin_person = participating_admin(&surreal, &[a, b, c, d]).await;
    let mut admin = SessionData::fresh("admin-sub", Role::Admin);
    admin.person_id = Some(admin_person);
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={}",
        SessionStore::new(SESSION_KEY).encode(&admin)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;

    // Scope each check to the matter's own table row (the canonical seed
    // also carries projects without onboarding notations, so a global
    // count would be polluted).
    let row_for = |name: &str| -> String {
        html.split("<tr")
            .find(|frag| frag.contains(name))
            .unwrap_or("")
            .to_string()
    };

    // `absent` reads as the negation of the `contains` checks below: a badge
    // the row must NOT carry.
    let absent = |row: &str, badge: &str| assert!(!row.contains(badge), "{row}");

    // B — bare open matter — is flagged as missing its retainer only.
    let b = row_for("Bare open matter");
    assert!(&b.contains("no retainer"));
    absent(&b, "no offboarding letter");

    // C — closed without a letter — is flagged for the closing letter only
    // (it has its onboarding__estate retainer).
    let c = row_for("Closed no letter");
    assert!(&c.contains("no offboarding letter"));
    absent(&c, "no retainer");

    // A and D are clean — no badge either way.
    let a = row_for("Has retainer open");
    absent(&a, "no retainer");
    absent(&a, "no offboarding letter");
    let d = row_for("Closed with letter");
    absent(&d, "no retainer");
    absent(&d, "no offboarding letter");
}

#[tokio::test]
async fn projects_list_reports_a_loader_failure_as_a_server_error() {
    // The lawyer-lens query runs against a handle with no engine behind it, so
    // it fails. The endpoint must surface that as a real 500 (the explicit
    // server error the retired `projects_index` handler returned), not a 200
    // with an error body — availability monitoring and HTTP clients must see the
    // matter directory as unavailable, not served.
    //
    // The lever is the Surreal handle because every row this listing reads is
    // Surreal-resident, so an unreachable Surreal handle is the only thing
    // that can make the listing fail.
    let surreal = store::surreal::SurrealDb::uninitialized();
    let state = AppState {
        sessions: SessionStore::new(SESSION_KEY),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    let app = server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));

    let admin = SessionData::fresh("admin-sub", Role::Admin);
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={}",
        SessionStore::new(SESSION_KEY).encode(&admin)
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/app/projects")
                .header("authorization", "Bearer dev")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let html = body_string(resp).await;
    assert!(
        html.contains("Failed to load projects."),
        "the failed load must still render the error state under the 500; got: {html}",
    );
}
