//! `GET /app/projects/{code}/portal` on the assembled router.
//!
//! Three claims. The first is the reason the mount has a `portal` segment at
//! all: `/app/projects/{code}` must keep resolving to Navigator's own matter show
//! page. Asserting both in one test is deliberate — reasoning about which of two
//! overlapping routes Axum would pick is exactly the mistake this replaces.
//!
//! The second is that a participant with a published bundle actually streams it:
//! the bare mount redirects to the trailing slash, the entrypoint is served
//! `no-store`, a content-hashed asset is served immutable, and an unmatched deep
//! link falls back to `index.html` so a client-side route survives a refresh.
//!
//! The third is that every refusal is the same non-disclosing 404. A 403 would
//! confirm to a nonparticipant that a Project with this code exists, so "no such
//! Project", "not your Project", and "nothing published here" are one response.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

/// A Project with a published bundle, its client participant, a second Project
/// with no bundle, and one person on no matter at all.
struct Fixture {
    app: axum::Router,
    project_code: String,
    unpublished_code: String,
    participant_cookie: String,
    unpublished_participant_cookie: String,
    outsider_cookie: String,
}

async fn seed_project(surreal: &store::surreal::SurrealDb, code: &str) -> store::projects::Project {
    store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: code.into(),
            name: code.into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

async fn seed_participant(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
    project_id: Uuid,
) -> Uuid {
    let person = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, Role::Client),
    )
    .await
    .unwrap();
    store::projects::add_participation(surreal, project_id, person.id, "client")
        .await
        .unwrap();
    person.id
}

async fn fixture() -> Fixture {
    let surreal = mem_surreal().await;

    let project = seed_project(&surreal, "libra-formation").await;
    let unpublished = seed_project(&surreal, "aries-eviction").await;

    let participant = seed_participant(&surreal, "Libra", "libra@example.com", project.id).await;
    let unpublished_participant =
        seed_participant(&surreal, "Cancer", "cancer@example.com", unpublished.id).await;

    // A client on no matter. Same tier as the participants, so what separates
    // them is participation and nothing else.
    let outsider = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Aries", "aries@example.com", Role::Client),
    )
    .await
    .unwrap();

    // Publish a bundle for the first Project only, into the exact handle the
    // router streams from.
    let applications = portal::test_support::empty_applications_bucket().await;
    portal::test_support::publish_portal_object(
        &applications,
        &project.code,
        "index.html",
        "text/html; charset=utf-8",
        b"<!doctype html><html><head><title>Libra portal</title></head><body>\
          <div id=\"root\"></div><script type=module src=\"./assets/app.js\"></script>\
          </body></html>",
    )
    .await;
    portal::test_support::publish_portal_object(
        &applications,
        &project.code,
        "assets/app.js",
        "text/javascript; charset=utf-8",
        b"console.log('libra');",
    )
    .await;
    // A directory index, the shape a multi-page build publishes. `rsync
    // --recursive` lands it under exactly this key, so this is the object a
    // trailing-slash path must find before the root entrypoint is reached for.
    portal::test_support::publish_portal_object(
        &applications,
        &project.code,
        "guide/index.html",
        "text/html; charset=utf-8",
        b"<!doctype html><title>Libra guide</title>",
    )
    .await;

    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let cookie_for = |sub: &str, person_id: Uuid| {
        let mut session = portal::SessionData::fresh(sub, Role::Client);
        session.person_id = Some(person_id);
        format!(
            "{}={}",
            portal::session::SESSION_COOKIE_NAME,
            sessions.encode(&session)
        )
    };

    let state =
        portal::test_support::app_state_with_applications(surreal.clone(), applications).await;
    Fixture {
        app: portal::router(state),
        project_code: project.code,
        unpublished_code: unpublished.code,
        participant_cookie: cookie_for("libra-sub", participant),
        unpublished_participant_cookie: cookie_for("cancer-sub", unpublished_participant),
        outsider_cookie: cookie_for("aries-sub", outsider.id),
    }
}

async fn send(app: &axum::Router, uri: &str, cookie: &str) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn header_value(response: &axum::response::Response, name: header::HeaderName) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The collision the `portal` segment exists to prevent.
///
/// `/app/projects/{code}` still renders the matter show page while
/// `/app/projects/{code}/portal` reaches the portal route. Neither shadows the
/// other, and no registration order decides it: the two differ in path shape.
#[tokio::test]
async fn the_portal_mount_and_the_matter_show_page_both_resolve() {
    let f = fixture().await;

    let matter = send(
        &f.app,
        &format!("/app/projects/{}", f.project_code),
        &f.participant_cookie,
    )
    .await;
    assert_eq!(
        matter.status(),
        StatusCode::OK,
        "the matter show page must survive the portal mount"
    );
    assert!(
        body_string(matter).await.contains("class=\"app-footer\""),
        "every /app page carries the minimal footer"
    );

    // The bare mount redirects to the trailing-slash form the Vite base joins
    // asset URLs onto.
    let bare = send(
        &f.app,
        &format!("/app/projects/{}/portal", f.project_code),
        &f.participant_cookie,
    )
    .await;
    assert_eq!(bare.status(), StatusCode::MOVED_PERMANENTLY);
    assert_eq!(
        header_value(&bare, header::LOCATION),
        format!("/app/projects/{}/portal/", f.project_code),
    );
}

/// A participant streams the published bundle.
///
/// The entrypoint is served `no-store` as `text/html`, a content-hashed asset
/// is served immutable as `text/javascript`, and an unmatched deep link falls
/// back to the entrypoint so a client-side route survives a refresh.
#[tokio::test]
async fn a_participant_streams_the_published_bundle() {
    let f = fixture().await;
    let root = format!("/app/projects/{}/portal/", f.project_code);

    let index = send(&f.app, &root, &f.participant_cookie).await;
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(
        header_value(&index, header::CONTENT_TYPE),
        "text/html; charset=utf-8"
    );
    assert_eq!(header_value(&index, header::CACHE_CONTROL), "no-store");
    assert!(
        header_value(&index, header::CONTENT_SECURITY_POLICY).contains("connect-src 'self'"),
        "the bundle carries its own portal CSP, not Navigator's nonce policy"
    );
    assert!(body_string(index).await.contains("Libra portal"));

    let asset = send(
        &f.app,
        &format!("{root}assets/app.js"),
        &f.participant_cookie,
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        header_value(&asset, header::CONTENT_TYPE),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        header_value(&asset, header::CACHE_CONTROL),
        "private, max-age=31536000, immutable"
    );
    assert!(body_string(asset).await.contains("console.log('libra')"));

    // A deep link with no published object falls back to the entrypoint.
    let deep = send(
        &f.app,
        &format!("{root}dashboard/matters"),
        &f.participant_cookie,
    )
    .await;
    assert_eq!(deep.status(), StatusCode::OK);
    assert_eq!(
        header_value(&deep, header::CONTENT_TYPE),
        "text/html; charset=utf-8"
    );
    assert!(body_string(deep).await.contains("Libra portal"));
}

/// The entrypoint carries a way back to the matter.
///
/// A participant embedded in a Project's own bundle has no Navigator chrome
/// around it otherwise — nothing on the page can get them back to
/// `/app/projects/{code}`. The banner is entrypoint-only: a content-hashed
/// asset streams unmodified, because the bundle's own script/style bytes must
/// stay byte-for-byte what was published.
#[tokio::test]
async fn the_entrypoint_carries_a_link_back_to_the_matter() {
    let f = fixture().await;
    let root = format!("/app/projects/{}/portal/", f.project_code);

    let index = send(&f.app, &root, &f.participant_cookie).await;
    let body = body_string(index).await;
    assert!(
        body.contains(&format!("href=\"/app/projects/{}\"", f.project_code)),
        "{body}"
    );
    assert!(body.contains("Neon Law"), "{body}");
    // Still the same document otherwise: the banner is inserted, not replacing
    // the bundle's own markup.
    assert!(body.contains("Libra portal"), "{body}");

    let asset = send(
        &f.app,
        &format!("{root}assets/app.js"),
        &f.participant_cookie,
    )
    .await;
    assert!(
        !body_string(asset).await.contains("Neon Law"),
        "a content-hashed asset must stream unmodified"
    );
}

/// A scope miss is 404, never 403 — and so is a Project with nothing published.
///
/// 403 would confirm that a Project with this code exists to somebody who is not
/// on it. The nonparticipant, the unknown code, the reserved code, and the
/// participant of an unpublished Project all get the identical response, so the
/// status carries no information about which.
#[tokio::test]
async fn every_refusal_is_the_same_non_disclosing_response() {
    let f = fixture().await;

    for (uri, cookie, why) in [
        (
            format!("/app/projects/{}/portal/", f.project_code),
            &f.outsider_cookie,
            "a nonparticipant must not learn that this Project exists",
        ),
        (
            format!("/app/projects/{}/portal/", f.unpublished_code),
            &f.unpublished_participant_cookie,
            "a participant of a Project with no published bundle gets nothing to disclose",
        ),
        (
            "/app/projects/no-such-project/portal/".to_string(),
            &f.outsider_cookie,
            "a code naming no Project",
        ),
        (
            "/app/projects/new/portal/".to_string(),
            &f.outsider_cookie,
            "`new` is a route Navigator serves itself and cannot be a Project",
        ),
        (
            "/app/projects/Not_A_Code/portal/".to_string(),
            &f.outsider_cookie,
            "a malformed code cannot name a Project",
        ),
    ] {
        assert_eq!(
            send(&f.app, &uri, cookie).await.status(),
            StatusCode::NOT_FOUND,
            "{why}"
        );
    }
}

/// An anonymous caller never reaches the handler.
///
/// The route rides the same session boundary as every other `/app` page, so the
/// answer is the login redirect rather than a 404 — which is what
/// `portal/tests/router_contract.rs` classifies it as.
#[tokio::test]
async fn an_anonymous_caller_is_sent_through_the_login_door() {
    let f = fixture().await;
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/app/projects/{}/portal/", f.project_code))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = header_value(&response, header::LOCATION);
    assert!(location.starts_with("/auth/login"), "{location}");
}

/// A client-side route written with a trailing slash resolves.
///
/// This is the shape a portal's own navigation actually emits: a section link
/// is `${base}${slug}/`, so every in-app link below the mount arrives with a
/// trailing slash. It reached [`asset_path_is_safe`] as a path whose final
/// segment is empty and was refused as a traversal, so every section of every
/// published portal answered 404 while the bare mount served fine — the one
/// failure a `dist/` with a single `index.html` cannot produce on its own.
///
/// A trailing slash cannot climb out of the `<code>/portal/` prefix, so it is
/// safe; what it names is a client-side route, and the entrypoint is what
/// serves it.
#[tokio::test]
async fn a_trailing_slash_client_route_falls_back_to_the_entrypoint() {
    let f = fixture().await;
    let root = format!("/app/projects/{}/portal/", f.project_code);

    let section = send(&f.app, &format!("{root}engagement/"), &f.participant_cookie).await;
    assert_eq!(
        section.status(),
        StatusCode::OK,
        "a portal's own section link must not read as a traversal"
    );
    assert_eq!(
        header_value(&section, header::CONTENT_TYPE),
        "text/html; charset=utf-8"
    );
    assert_eq!(header_value(&section, header::CACHE_CONTROL), "no-store");
    assert!(body_string(section).await.contains("Libra portal"));

    // Nested just as deep as a client router goes, and still the entrypoint.
    let nested = send(
        &f.app,
        &format!("{root}matters/open/filings/"),
        &f.participant_cookie,
    )
    .await;
    assert_eq!(nested.status(), StatusCode::OK);
    assert!(body_string(nested).await.contains("Libra portal"));
}

/// The portal subtree is exempt from the global trailing-slash redirect.
///
/// A blanket "strip the trailing slash" rule would collapse `.../portal/`
/// back down to `.../portal`, which itself redirects to `.../portal/` —
/// an infinite loop — and would 404 every section link a published bundle's
/// own client-side router emits (see
/// `a_trailing_slash_client_route_falls_back_to_the_entrypoint`). Both the
/// root and a nested section link must answer as real content, never a
/// `301`.
#[tokio::test]
async fn a_trailing_slash_within_the_portal_bundle_is_never_stripped() {
    let f = fixture().await;
    let root = format!("/app/projects/{}/portal/", f.project_code);

    let index = send(&f.app, &root, &f.participant_cookie).await;
    assert_ne!(
        index.status(),
        StatusCode::MOVED_PERMANENTLY,
        "the portal root's trailing slash is the route itself, not a redirect target"
    );

    let section = send(&f.app, &format!("{root}engagement/"), &f.participant_cookie).await;
    assert_ne!(
        section.status(),
        StatusCode::MOVED_PERMANENTLY,
        "a portal's own client-side section link must not be redirected"
    );
}

/// A published directory index serves its own path, not the root entrypoint.
///
/// A portal built as multiple pages rather than one bundle publishes
/// `<section>/index.html`, and `rsync --recursive` lands it under exactly that
/// key. Reaching for it before the root entrypoint is what keeps the
/// entrypoint fallback a *fallback*: without it, every page of a multi-page
/// build would answer 200 with the wrong document, which is worse than the 404
/// it replaced.
///
/// It is served `no-store` for the same reason the root entrypoint is: an
/// `index.html` names the build's hashed assets and is never content-hashed
/// itself, so caching one for a year pins a page at assets a later publish has
/// aged out.
#[tokio::test]
async fn a_published_directory_index_is_served_for_its_own_path() {
    let f = fixture().await;
    let root = format!("/app/projects/{}/portal/", f.project_code);

    let page = send(&f.app, &format!("{root}guide/"), &f.participant_cookie).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(
        header_value(&page, header::CONTENT_TYPE),
        "text/html; charset=utf-8"
    );
    assert_eq!(
        header_value(&page, header::CACHE_CONTROL),
        "no-store",
        "an index.html is never content-hashed, so it must not cache for a year"
    );
    assert!(
        body_string(page).await.contains("Libra guide"),
        "the published page serves itself rather than the root entrypoint"
    );

    // The slashless form of the same path finds it too, so a hand-typed link
    // and a copied one land on the same document.
    let slashless = send(&f.app, &format!("{root}guide"), &f.participant_cookie).await;
    assert_eq!(slashless.status(), StatusCode::OK);
    assert!(body_string(slashless).await.contains("Libra guide"));
}

/// A traversal is still refused, trailing slash or not.
#[tokio::test]
async fn a_traversal_is_refused_even_with_a_trailing_slash() {
    let f = fixture().await;
    let root = format!("/app/projects/{}/portal/", f.project_code);

    for path in ["..%2Fsecret/", "a//b/", "assets/../../etc/passwd"] {
        assert_eq!(
            send(&f.app, &format!("{root}{path}"), &f.participant_cookie)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "{path} must not resolve"
        );
    }
}
