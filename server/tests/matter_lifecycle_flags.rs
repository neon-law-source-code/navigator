#![allow(clippy::doc_markdown)]
//! Matter-lifecycle warning flags on the admin Projects list.
//!
//! The firm's lifecycle invariant: every matter opens on an onboarding
//! (`onboarding__*`) notation — the client's retainer — and a *closed*
//! matter carries an `offboarding__letter`. Neither is schema-enforced, so the
//! Projects list surfaces the gaps: a missing onboarding notation folds into
//! the lifecycle status pill's "needs onboarding" state (no separate badge
//! duplicates it), and a missing offboarding letter still carries its own
//! warning badge. These tests pin both the pure rule
//! (`store::projects::matter_flags`) and the rendered list.
//!
//! **An artifact clears a flag; a walk never does.** A notation row means
//! somebody opened a questionnaire walk, not that it produced anything, so the
//! flag clears only on a classified `assets` row or on a walk that actually
//! rendered its instruments. The tests below keep those two apart
//! deliberately: `notation()` opens a walk and leaves it at `BEGIN`,
//! `walk_that_produced_an_instrument()` runs it far enough to file a draft.

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
fn flags_an_open_matter_with_no_onboarding_as_missing_onboarding() {
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

// ---- the yellow/green/red lifecycle indicator ----

#[test]
fn an_open_matter_missing_its_onboarding_letter_is_the_yellow_state() {
    assert_eq!(
        store::projects::matter_lifecycle("open", true, false),
        store::projects::MatterLifecycle::NeedsOnboarding
    );
}

#[test]
fn an_open_matter_with_its_onboarding_letter_is_the_green_on_file_state() {
    assert_eq!(
        store::projects::matter_lifecycle("open", false, false),
        store::projects::MatterLifecycle::OnboardingOnFile
    );
}

/// The green state names where the paperwork is, not what condition the
/// representation is in.
///
/// `matter_lifecycle_sets` matches an artifact by its declared kind and reads
/// no signature state, so the only fact established is that the paperwork
/// exists — never that anyone executed it. A status word here
/// ("live", "active", "in good standing") would tell a lawyer scanning the
/// Projects list that the matter is properly papered on evidence that shows
/// nothing of the kind, and the yellow state's converse would carry the same
/// over-claim. Pin the vocabulary so a future edit cannot quietly restore it.
#[test]
fn the_green_label_states_a_location_and_never_a_matter_status() {
    use store::projects::MatterLifecycle;

    assert_eq!(
        MatterLifecycle::OnboardingOnFile.label(),
        "onboarding on file"
    );

    for status_word in [
        "live",
        "active",
        "open",
        "good standing",
        "in good standing",
    ] {
        assert_ne!(
            MatterLifecycle::OnboardingOnFile.label(),
            status_word,
            "the green pill must name a location, not assert a matter status"
        );
    }
}

/// Every state's title says what the indicator did and did not verify, and the
/// green one carries the limit the one-line label cannot.
#[test]
fn every_lifecycle_state_carries_its_own_title_and_green_states_its_limit() {
    use store::projects::MatterLifecycle;
    let states = [
        MatterLifecycle::NeedsOnboarding,
        MatterLifecycle::OnboardingOnFile,
        MatterLifecycle::Closed,
    ];
    let titles: Vec<&str> = states.iter().map(|s| s.title()).collect();
    assert_eq!(
        titles
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "each state must carry its own title: {titles:?}"
    );
    assert!(
        MatterLifecycle::OnboardingOnFile
            .title()
            .contains("not verified as executed"),
        "the green title must state that filing is not execution: {}",
        MatterLifecycle::OnboardingOnFile.title()
    );
}

#[test]
fn a_closed_matter_with_its_offboarding_letter_is_the_red_state() {
    assert_eq!(
        store::projects::matter_lifecycle("closed", false, false),
        store::projects::MatterLifecycle::Closed
    );
}

#[test]
fn a_closed_matter_still_owing_its_offboarding_letter_is_still_the_red_state() {
    // Red covers both — the still-owed gap keeps surfacing through the
    // separate "no offboarding letter" badge, not a fourth colour.
    assert_eq!(
        store::projects::matter_lifecycle("closed", false, true),
        store::projects::MatterLifecycle::Closed
    );
}

#[test]
fn every_lifecycle_state_carries_its_own_class_and_a_distinct_text_label() {
    use store::projects::MatterLifecycle;
    let states = [
        MatterLifecycle::NeedsOnboarding,
        MatterLifecycle::OnboardingOnFile,
        MatterLifecycle::Closed,
    ];
    let classes: Vec<&str> = states.iter().map(|s| s.class()).collect();
    let labels: Vec<&str> = states.iter().map(|s| s.label()).collect();
    assert_eq!(
        classes
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "each state must render its own class: {classes:?}"
    );
    assert_eq!(
        labels
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3,
        "each state must carry its own text label — colour cannot be the only signal: {labels:?}"
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

    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(bespoke.id, person.id, matter, "BEGIN"),
    )
    .await
    .unwrap()
    .id;
    // The classifier, not the walk's progress, is what this test pins — so give
    // the walk its produced instrument and leave only the `kind` question open.
    drafted_instrument(&surreal, notation_id, "offboarding_letter").await;

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

// ---- the asset lane (an uploaded, lawyer-classified letter counts too) ----

async fn asset_storage(name: &str) -> Arc<dyn cloud::StorageService> {
    Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-matter-flags-{name}")))
            .await
            .unwrap(),
    )
}

async fn upload(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
    project_id: Uuid,
    filename: &str,
    kind: &str,
) {
    store::documents::ingest_bytes(
        surreal,
        storage,
        &store::documents::IngestArgs {
            project_id,
            source: store::documents::source::UPLOAD,
            filename,
            kind,
            content_type: "application/pdf",
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::INTERNAL,
        },
        b"uploaded bytes",
    )
    .await
    .unwrap();
}

async fn lifecycle_sets_for(surreal: &store::surreal::SurrealDb, project_id: Uuid) -> (bool, bool) {
    let projects = vec![store::projects::find_by_id(surreal, project_id)
        .await
        .unwrap()
        .expect("matter row")];
    let (has_engagement, has_closing) = store::projects::matter_lifecycle_sets(surreal, &projects)
        .await
        .unwrap();
    (
        has_engagement.contains(&project_id),
        has_closing.contains(&project_id),
    )
}

#[tokio::test]
async fn an_uploaded_onboarding_asset_clears_the_engagement_flag_with_no_notation_at_all() {
    let (_app, surreal) = build_app().await;
    let storage = asset_storage("asset-only-onboarding").await;
    let matter = project(&surreal, "Asset-only onboarding", "open").await;
    upload(&surreal, &storage, matter, "engagement.pdf", "onboarding").await;

    let (has_engagement, _) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        has_engagement,
        "an uploaded onboarding-kind asset must clear the engagement flag with no notation"
    );
}

#[tokio::test]
async fn an_uploaded_offboarding_asset_clears_the_closing_flag_with_no_notation_at_all() {
    let (_app, surreal) = build_app().await;
    let storage = asset_storage("asset-only-offboarding").await;
    let matter = project(&surreal, "Asset-only offboarding", "closed").await;
    upload(&surreal, &storage, matter, "closing.pdf", "offboarding").await;

    let (_, has_closing) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        has_closing,
        "an uploaded offboarding-kind asset must clear the closing flag with no notation"
    );
}

#[tokio::test]
async fn a_walk_that_produced_its_instruments_clears_the_flags_with_no_asset_present() {
    // The regression neither the asset fold nor the artifact predicate may
    // introduce: a matter opened and closed purely through the questionnaire
    // walk, with no uploaded asset at all, still clears — so long as the walk
    // actually produced something. A walk that drafts its instruments files
    // them as `review_documents` rows rather than one rendered letter, so the
    // asset lane cannot see them and this lane must.
    let (_app, surreal) = build_app().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Notation Only", "notation-only@example.com"),
    )
    .await
    .unwrap();
    let matter = project(&surreal, "Notation-only matter", "closed").await;
    walk_that_produced_an_instrument(&surreal, matter, person.id, "onboarding__letter")
        .await;
    walk_that_produced_an_instrument(&surreal, matter, person.id, "offboarding__letter").await;

    let (has_engagement, has_closing) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        has_engagement,
        "a matter-opening walk that produced its instrument must clear the engagement flag"
    );
    assert!(
        has_closing,
        "a matter-closing walk that produced its instrument must clear the closing flag"
    );
}

/// **The defect.** An onboarding walk opened and abandoned at `BEGIN` produced
/// no document, signed nothing, and filed nothing — so it must not clear the
/// engagement flag.
///
/// This was a silent false positive: the matter read as papered, and nothing
/// else on the screen contradicted it, because the document that would have
/// contradicted it did not exist. A lawyer scanning for matters that still
/// needed an engagement letter could not see this one.
///
/// Asserted on the flag values, never on the absence of a rendered string, so
/// a rename cannot leave this passing vacuously.
#[tokio::test]
async fn an_onboarding_walk_abandoned_at_begin_does_not_clear_the_engagement_flag() {
    let (_app, surreal) = build_app().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Abandoned", "abandoned@example.com"),
    )
    .await
    .unwrap();
    let matter = project(&surreal, "Abandoned onboarding walk", "open").await;
    notation(&surreal, matter, person.id, "onboarding__letter").await;

    let (has_engagement, has_closing) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        !has_engagement,
        "an onboarding walk abandoned at BEGIN produced no document, so it must not clear \
         the engagement flag — a never-papered matter reporting as papered is the defect"
    );
    assert!(
        !has_closing,
        "an abandoned onboarding walk must not clear the closing flag either"
    );
}

/// The mirror on the closing side: an offboarding walk opened and dropped
/// leaves the closed matter still owing its letter.
#[tokio::test]
async fn an_offboarding_walk_abandoned_at_begin_does_not_clear_the_closing_flag() {
    let (_app, surreal) = build_app().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Dropped", "dropped@example.com"),
    )
    .await
    .unwrap();
    let matter = project(&surreal, "Abandoned offboarding walk", "closed").await;
    notation(&surreal, matter, person.id, "offboarding__letter").await;

    let (_, has_closing) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        !has_closing,
        "an offboarding walk abandoned at BEGIN produced no letter, so the closed matter \
         must still report the gap"
    );
}

/// An abandoned walk does **not** re-break the upload lane. A matter whose
/// walk was dropped but whose signed letter was uploaded and classified still
/// clears — the fix narrows what a *walk* proves, never what an upload proves.
#[tokio::test]
async fn an_upload_still_clears_the_flag_on_a_matter_whose_walk_was_abandoned() {
    let (_app, surreal) = build_app().await;
    let storage = asset_storage("abandoned-walk-plus-upload").await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Both", "both@example.com"),
    )
    .await
    .unwrap();
    let matter = project(&surreal, "Abandoned walk with upload", "open").await;
    notation(&surreal, matter, person.id, "onboarding__letter").await;
    upload(&surreal, &storage, matter, "engagement.pdf", "onboarding").await;

    let (has_engagement, _) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        has_engagement,
        "a classified upload must clear the engagement flag regardless of the walk's state"
    );
}

#[tokio::test]
async fn neither_a_notation_nor_an_asset_leaves_the_matter_badged() {
    let (_app, surreal) = build_app().await;
    let matter = project(&surreal, "Nothing filed", "closed").await;

    let (has_engagement, has_closing) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        !has_engagement && !has_closing,
        "a matter with neither a notation nor an asset must not clear either flag"
    );
}

#[tokio::test]
async fn an_asset_of_a_non_opening_kind_does_not_clear_the_engagement_flag() {
    // A lawyer-classified upload only counts when its kind actually opens
    // (or closes) a matter — an ordinary letter uploaded to the matter must
    // not silently satisfy the engagement badge.
    let (_app, surreal) = build_app().await;
    let storage = asset_storage("asset-wrong-kind").await;
    let matter = project(&surreal, "Wrong-kind upload", "open").await;
    upload(&surreal, &storage, matter, "demand.pdf", "letter").await;

    let (has_engagement, has_closing) = lifecycle_sets_for(&surreal, matter).await;
    assert!(
        !has_engagement && !has_closing,
        "an ordinary-letter upload must not clear either lifecycle flag"
    );
}

#[tokio::test]
async fn an_uploaded_engagement_letter_clears_the_lifecycle_pill_on_the_rendered_projects_list() {
    let (app, surreal) = build_app().await;
    let storage = asset_storage("rendered-list-upload").await;
    let matter = project(&surreal, "Uploaded engagement matter", "open").await;
    upload(&surreal, &storage, matter, "engagement.pdf", "onboarding").await;
    let matter_code = store::projects::find_by_id(&surreal, matter)
        .await
        .unwrap()
        .expect("matter row")
        .code;

    let admin_person = participating_admin(&surreal, &[matter]).await;
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
    let row = html
        .split("<tr")
        .find(|frag| frag.contains(&matter_code))
        .unwrap_or_default()
        .to_string();
    assert!(
        !row.contains("needs onboarding"),
        "an uploaded engagement letter must clear the lifecycle pill's onboarding-missing state: {row}"
    );
    assert!(
        row.contains("onboarding on file"),
        "an uploaded engagement letter must flip the lifecycle pill to onboarding-on-file: {row}"
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

/// An onboarding or offboarding walk **opened and left at `BEGIN`** — the
/// shape that used to turn a matter green on its own. Returns the notation id
/// so a caller can go on to give the walk an artifact.
async fn notation(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    template_code: &str,
) -> Uuid {
    let tmpl = store::templates::resolve(surreal, None, template_code)
        .await
        .unwrap()
        .expect("template seeded");
    store::notations::create(
        surreal,
        &store::notations::NewNotation::new(tmpl.id, person_id, project_id, "BEGIN"),
    )
    .await
    .unwrap()
    .id
}

/// The instrument a walk that drafts its instruments actually produces: one
/// drafted `review_documents` row on the notation. This — not the notation
/// row — is what makes the walk evidence that the matter was papered.
async fn drafted_instrument(surreal: &store::surreal::SurrealDb, notation_id: Uuid, kind: &str) {
    store::review_documents::upsert_draft(
        surreal,
        &store::review_documents::NewReviewDocument {
            notation_id,
            kind,
            title: "Synthetic instrument",
            body_html: "<p>Synthetic draft body.</p>",
        },
    )
    .await
    .unwrap();
}

/// A walk that ran far enough to produce an instrument — the notation plus the
/// draft it rendered.
async fn walk_that_produced_an_instrument(
    surreal: &store::surreal::SurrealDb,
    project_id: Uuid,
    person_id: Uuid,
    template_code: &str,
) {
    let id = notation(surreal, project_id, person_id, template_code).await;
    drafted_instrument(surreal, id, "will").await;
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
    assert!(!html.contains("no onboarding"), "{html}");
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

    // A: open, onboarding walk produced its instrument → clean.
    let a = project(&surreal, "Has retainer open", "open").await;
    walk_that_produced_an_instrument(&surreal, a, person.id, "onboarding__letter").await;
    // B: open, no onboarding walk at all → missing retainer.
    let b = project(&surreal, "Bare open matter", "open").await;
    // C: closed, onboarded but no offboarding letter → missing offboarding letter.
    let c = project(&surreal, "Closed no letter", "closed").await;
    walk_that_produced_an_instrument(&surreal, c, person.id, "onboarding__letter").await;
    // D: closed, both walks produced their instruments → clean.
    let d = project(&surreal, "Closed with letter", "closed").await;
    walk_that_produced_an_instrument(&surreal, d, person.id, "onboarding__letter").await;
    walk_that_produced_an_instrument(&surreal, d, person.id, "offboarding__letter").await;

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

    // `absent` reads as the negation of the `contains` checks below: text
    // the row must NOT carry.
    let absent = |row: &str, badge: &str| assert!(!row.contains(badge), "{row}");

    // B — bare open matter — is missing its onboarding: the lifecycle pill
    // says so (there is no second, duplicate "no onboarding" badge next to
    // the name), and it carries no offboarding-letter badge (it is open).
    let b = row_for("Bare open matter");
    assert!(&b.contains("needs onboarding"));
    absent(&b, "no offboarding letter");

    // C — closed without a letter — is flagged for the offboarding letter
    // only (its onboarding letter produced an instrument, so the pill itself
    // reads "closed" rather than the onboarding-missing state).
    let c = row_for("Closed no letter");
    assert!(&c.contains("no offboarding letter"));
    absent(&c, "needs onboarding");

    // A and D are clean — no offboarding badge and no "needs onboarding" pill.
    let a = row_for("Has retainer open");
    assert!(&a.contains("onboarding on file"));
    absent(&a, "no offboarding letter");
    let d = row_for("Closed with letter");
    absent(&d, "needs onboarding");
    absent(&d, "no offboarding letter");

    // The badge vocabulary is the codebase's, not the conversational one: the
    // row says "needs onboarding"/"no offboarding letter", never "no retainer".
    assert!(
        !html.contains("no retainer"),
        "the warning badge must use the onboarding vocabulary: {html}"
    );
}

#[tokio::test]
async fn projects_list_renders_each_lifecycle_state_with_its_own_class() {
    let (app, surreal) = build_app().await;
    let person = store::persons::create(
        &surreal,
        &store::persons::NewPerson::new("Lifecycle Row", "lifecycle-row@example.com"),
    )
    .await
    .unwrap();

    // Yellow: open, no onboarding walk at all.
    let yellow = project(&surreal, "Yellow lifecycle matter", "open").await;
    // Yellow too: open, onboarding walk opened and abandoned at BEGIN. This row
    // is the defect's user-facing face — it used to render green.
    let abandoned = project(&surreal, "Abandoned lifecycle matter", "open").await;
    notation(
        &surreal,
        abandoned,
        person.id,
        "onboarding__letter",
    )
    .await;
    // Green: open, onboarding walk produced its instrument.
    let green = project(&surreal, "Green lifecycle matter", "open").await;
    walk_that_produced_an_instrument(&surreal, green, person.id, "onboarding__letter")
        .await;
    // Red: closed.
    let red = project(&surreal, "Red lifecycle matter", "closed").await;

    let admin_person = participating_admin(&surreal, &[yellow, abandoned, green, red]).await;
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

    assert!(
        row_for("Yellow lifecycle matter").contains("matter-lifecycle--yellow"),
        "an open matter missing its onboarding letter must render the yellow class"
    );
    assert!(
        row_for("Abandoned lifecycle matter").contains("matter-lifecycle--yellow"),
        "an open matter whose onboarding walk was abandoned at BEGIN produced no document, \
         so it must render the yellow class — asserted on the class, never on the absence \
         of a string, so a rename cannot leave this passing vacuously"
    );
    assert!(
        row_for("Green lifecycle matter").contains("matter-lifecycle--green"),
        "an open matter whose onboarding walk produced its instrument must render green"
    );
    assert!(
        row_for("Red lifecycle matter").contains("matter-lifecycle--red"),
        "a closed matter must render the red class"
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
