#![allow(clippy::doc_markdown)]
//! Matter-lifecycle warning flags on the admin Projects list.
//!
//! The firm's lifecycle invariant: every matter opens on an engagement the
//! client agreed to — the retainer — and a *closed* matter carries a
//! closing letter. Neither is schema-enforced, so the Projects list
//! surfaces the gaps with a warning badge.
//!
//! The engagement reaches a matter down either of two lanes, and the badge
//! reads both: the questionnaire walk creates a **notation** bound to a
//! template, and a letter signed on paper (or returned from an e-signature
//! provider) is uploaded as an **asset**. A matter papered the second way
//! used to read `no retainer` forever, with no action available that would
//! clear it, so these tests pin the asset lane as explicitly as the
//! notation one.
//!
//! Pinned here: the pure rule (`store::projects::matter_flags`), both
//! classifiers, the upload form's constrained vocabulary, and the rendered
//! list.

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
fn a_retainer_template_opens_a_matter_whatever_its_code() {
    // The flag keys off the declared `kind`, never the template's code.
    // Every `kind: retainer` template happens to use an `onboarding__*`
    // code today; a future one that does not must still count, or the
    // badge would lie about a matter that has its engagement.
    assert!(store::projects::template_opens_a_matter(Some("retainer")));
}

#[test]
fn an_onboarding_engagement_opens_a_matter() {
    // `onboarding__estate` / `onboarding__nexus` are `kind: onboarding`,
    // not `retainer` — the intake-driven engagements that open a bundle
    // of instruments. They open a matter just as a retainer does.
    assert!(store::projects::template_opens_a_matter(Some("onboarding")));
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

// ---- what counts as the matter-opening engagement, on the upload lane ----

#[test]
fn an_uploaded_engagement_letter_opens_a_matter() {
    // The two kinds a lawyer can file an engagement letter under. This is
    // the whole point of the asset lane: a letter signed on paper and
    // uploaded opens the matter exactly as a generated notation does.
    assert!(store::projects::asset_opens_a_matter("retainer"));
    assert!(store::projects::asset_opens_a_matter("onboarding"));
}

#[test]
fn an_uploaded_document_that_is_not_the_engagement_does_not_open_a_matter() {
    // Every other thing a lawyer files on a matter. `unclassified` matters
    // most: it is the upload form's default, so a lawyer who uploads
    // without classifying must not clear the badge by accident.
    for kind in [
        "unclassified",
        "letter",
        "filing",
        "will",
        "trust",
        "directive",
        "agreement",
        "memo",
        "transcript",
        "inbound_contract",
        "certificate_of_naturalization",
    ] {
        assert!(
            !store::projects::asset_opens_a_matter(kind),
            "an uploaded `{kind}` is not the engagement that opens a matter"
        );
    }
}

#[test]
fn an_unknown_uploaded_kind_never_opens_a_matter() {
    // A string this build does not recognize cannot be an engagement. The
    // badge stays on: over-warning beats silently calling a matter opened.
    assert!(!store::projects::asset_opens_a_matter(""));
    assert!(!store::projects::asset_opens_a_matter("bogus"));
    // The free-text values this workspace filed before the vocabulary was
    // enforced at ingest. None of them was ever a `Kind`.
    assert!(!store::projects::asset_opens_a_matter("intake"));
    assert!(!store::projects::asset_opens_a_matter("retainer_pdf"));
    assert!(!store::projects::asset_opens_a_matter("engagement"));
}

#[test]
fn both_lanes_read_the_same_classifier_for_every_kind() {
    // The two lanes must not drift into disagreeing about what an
    // engagement letter is: a matter papered by upload and a matter
    // papered by the walk have to badge identically. Walking `Kind::ALL`
    // means a new kind cannot slip through classified one way and not the
    // other.
    for kind in rules::kind::Kind::ALL {
        assert_eq!(
            store::projects::asset_opens_a_matter(kind.as_str()),
            store::projects::template_opens_a_matter(Some(kind.as_str())),
            "the two lanes disagree about {}",
            kind.as_str()
        );
        assert_eq!(
            store::projects::asset_opens_a_matter(kind.as_str()),
            kind.opens_a_matter(),
            "the upload lane must classify {} straight from `Kind::opens_a_matter`",
            kind.as_str()
        );
    }
}

// ---- the upload form's vocabulary ----

#[test]
fn asset_kind_choices_are_exactly_the_asset_lane() {
    // The upload form offers a literal list rather than calling into
    // `rules`, because that module compiles into the wasm client bundle
    // (see `ASSET_KIND_CHOICES`). This is what keeps the two honest — the
    // same discipline `valid_strings_are_exactly_the_template_lane`
    // applies to `rules::kind::VALID`.
    //
    // Drift either way is a real defect: an extra value offers a lawyer a
    // classification `store::documents::ingest_bytes` would refuse, and a
    // missing one hides a classification they are entitled to file under.
    let offered: std::collections::BTreeSet<&str> =
        webapp::lawyer_project_detail::ASSET_KIND_CHOICES
            .iter()
            .map(|(value, _)| *value)
            .collect();
    let lane: std::collections::BTreeSet<&str> = rules::kind::Kind::ALL
        .iter()
        .filter(|k| k.valid_for(rules::kind::Lane::Asset))
        .map(|k| k.as_str())
        .collect();
    assert_eq!(offered, lane);
}

#[test]
fn the_upload_form_defaults_to_unclassified() {
    // `unclassified` leads the list and is the field's selected value, so
    // the default upload asserts nothing about the matter's lifecycle.
    assert_eq!(
        webapp::lawyer_project_detail::ASSET_KIND_CHOICES
            .first()
            .map(|(value, _)| *value),
        Some("unclassified")
    );
}

#[test]
fn every_offered_choice_carries_a_label() {
    // The select is the only documentation a lawyer gets for what these
    // values mean, so a bare enum string is not acceptable copy.
    for (value, label) in webapp::lawyer_project_detail::ASSET_KIND_CHOICES {
        assert!(
            label.len() > value.len(),
            "`{value}` needs a lawyer-facing label, got {label:?}"
        );
    }
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

/// File a document on the matter, classified as `kind` — the upload lane.
///
/// Goes through `store::documents::ingest_bytes`, the same door
/// `portal::project_documents::upload` uses, so the row this writes is the
/// row a lawyer's upload writes. `filename` varies per call because ingest
/// dedups identical bytes within a matter.
async fn upload(surreal: &store::surreal::SurrealDb, project_id: Uuid, kind: &str) {
    let tmp = std::env::temp_dir().join(format!("navigator-lifecycle-upload-{}", Uuid::now_v7()));
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(tmp)
            .await
            .expect("a temp storage root for the uploaded document"),
    );
    let filename = format!("{kind}-{}.pdf", Uuid::now_v7());
    store::documents::ingest_bytes(
        surreal,
        &storage,
        &store::documents::IngestArgs {
            project_id,
            source: store::documents::source::UPLOAD,
            filename: &filename,
            kind,
            content_type: "application/pdf",
            description: Some("countersigned, returned on paper"),
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        filename.as_bytes(),
    )
    .await
    .unwrap_or_else(|e| panic!("filing a `{kind}` document must succeed: {e}"));
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
    // C: closed, has its retainer but no closing letter → missing closing letter.
    let c = project(&surreal, "Closed no letter", "closed").await;
    notation(&surreal, c, person.id, "onboarding__estate").await;
    // D: closed, has both → clean.
    let d = project(&surreal, "Closed with letter", "closed").await;
    notation(&surreal, d, person.id, "onboarding__retainer").await;
    notation(&surreal, d, person.id, "closing__letter").await;

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
    absent(&b, "no closing letter");

    // C — closed without a letter — is flagged for the closing letter only
    // (it has its onboarding__estate retainer).
    let c = row_for("Closed no letter");
    assert!(&c.contains("no closing letter"));
    absent(&c, "no retainer");

    // A and D are clean — no badge either way.
    let a = row_for("Has retainer open");
    absent(&a, "no retainer");
    absent(&a, "no closing letter");
    let d = row_for("Closed with letter");
    absent(&d, "no retainer");
    absent(&d, "no closing letter");
}

/// The defect this issue exists to fix, at the surface a lawyer reads.
///
/// A matter whose engagement letter arrived as a signed PDF — not through
/// the questionnaire walk — used to read `no retainer` on `/app/projects`
/// permanently, with no action available that would clear it, while the
/// countersigned letter sat in the matter's own document list. This is the
/// mutation check for the asset arm of `store::projects::matter_lifecycle_sets`:
/// remove that arm and the first assertion here fails.
#[tokio::test]
async fn an_uploaded_engagement_letter_clears_the_retainer_badge() {
    let (app, surreal) = build_app().await;

    // E: open, engagement letter uploaded as a PDF, no notation → clean.
    let e = project(&surreal, "Uploaded retainer open", "open").await;
    upload(&surreal, e, "retainer").await;
    // F: open, an uploaded document that is not the engagement → still
    // missing its retainer. An upload must not clear the badge merely by
    // existing; only its classification can.
    let f = project(&surreal, "Uploaded exhibit open", "open").await;
    upload(&surreal, f, "filing").await;
    upload(&surreal, f, "unclassified").await;
    // G: open, engagement letter uploaded under the other opening kind.
    let g = project(&surreal, "Uploaded onboarding open", "open").await;
    upload(&surreal, g, "onboarding").await;

    let admin_person = participating_admin(&surreal, &[e, f, g]).await;
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

    let row_for = |name: &str| -> String {
        html.split("<tr")
            .find(|frag| frag.contains(name))
            .unwrap_or("")
            .to_string()
    };

    let e = row_for("Uploaded retainer open");
    assert!(
        !e.contains("no retainer"),
        "an uploaded `retainer` is this matter's engagement letter; the badge must clear: {e}"
    );
    let g = row_for("Uploaded onboarding open");
    assert!(
        !g.contains("no retainer"),
        "an uploaded `onboarding` engagement must clear the badge too: {g}"
    );
    let f = row_for("Uploaded exhibit open");
    assert!(
        f.contains("no retainer"),
        "a filing and an unclassified upload are not an engagement letter;          the badge must stay: {f}"
    );
}

/// An uploaded engagement letter clears the badge on the *sets* function
/// directly, without the render in the way.
///
/// The rendered test above is what a lawyer sees; this is the invariant
/// underneath it, and it also pins the part the render cannot show — that
/// the notation lane still works unchanged, and that the two lanes union
/// rather than one overwriting the other.
#[tokio::test]
async fn the_lifecycle_sets_union_both_document_lanes() {
    let (_app, surreal) = build_app().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();

    // Notation only — the walk path, unchanged.
    let walked = project(&surreal, "Walked", "open").await;
    notation(&surreal, walked, person.id, "onboarding__retainer").await;
    // Upload only — the path this change adds.
    let uploaded = project(&surreal, "Uploaded", "open").await;
    upload(&surreal, uploaded, "retainer").await;
    // Both lanes on one matter — counted once, not double-counted into
    // some other state.
    let both = project(&surreal, "Both", "open").await;
    notation(&surreal, both, person.id, "onboarding__estate").await;
    upload(&surreal, both, "onboarding").await;
    // Neither lane.
    let neither = project(&surreal, "Neither", "open").await;
    // An upload that classifies as something else entirely.
    let other = project(&surreal, "Other", "open").await;
    upload(&surreal, other, "transcript").await;

    let ids = [walked, uploaded, both, neither, other];
    let mut matters = Vec::new();
    for id in ids {
        matters.push(
            store::projects::find_by_id(&surreal, id)
                .await
                .unwrap()
                .expect("seeded matter"),
        );
    }
    let (has_engagement, _has_closing) = store::projects::matter_lifecycle_sets(&surreal, &matters)
        .await
        .expect("the lifecycle sets load");

    assert!(has_engagement.contains(&walked), "notation lane");
    assert!(has_engagement.contains(&uploaded), "upload lane");
    assert!(has_engagement.contains(&both), "both lanes");
    assert!(!has_engagement.contains(&neither), "neither lane");
    assert!(
        !has_engagement.contains(&other),
        "a transcript is not an engagement letter"
    );
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
