//! `/sitemap.xml` on **both** hosts, composed exactly as their binaries compose
//! them.
//!
//! A sitemap is a promise: every URL in it is a page this host serves. The two
//! brands merge one shared crawler table (`host_crawler_and_legal_routes`), and
//! while the path list inside it was hardcoded that promise was broken in both
//! directions — the Foundation advertised `/litigation` and `/blog`, the firm
//! advertised `/education`, and each is a `404` on the host that offered it.
//! Google reports those as errors and spends crawl budget rediscovering them.
//!
//! So the assertion here is the promise itself, made against the real
//! composition rather than against a restated list: fetch each host's sitemap,
//! then fetch every URL in it from that same host. A path a brand adds enters
//! this gate by being advertised.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// An `AppState` carrying the content the sitemap is built from. The shared
/// builder ships empty indexes, so a sitemap built on it would advertise the
/// static pages and none of the posts or talks — the half most likely to drift.
async fn state() -> portal::AppState {
    let mut state = portal::test_support::app_state(mem_surreal().await).await;
    state.blog = portal::blog::load_dir(std::path::Path::new(portal::DEFAULT_BLOG_DIR))
        .expect("load the bundled blog posts");
    state.workshops = portal::WorkshopIndex::new(
        portal::workshops::loader::load_navigator(std::path::Path::new(
            portal::DEFAULT_WORKSHOPS_DIR,
        ))
        .expect("load the bundled workshop materials"),
    );
    state
}

fn compose(
    state: portal::AppState,
    public: Router<portal::AppState>,
    public_paths: &'static [&'static str],
    dioxus: Vec<Router>,
) -> Router {
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        public,
        public_paths,
        dioxus,
    )
    .expect("brand public routes must not collide with Navigator")
}

/// The site, composed exactly as the binary composes it.
///
/// One helper where there were two. `firm_app` and `foundation_app` named the
/// two hosts while they were separate deployments; they became the same
/// expression when the crates merged, and keeping both would have implied a
/// separation the router no longer has.
async fn app() -> Router {
    let state = state().await;
    let dioxus = neon::public_dioxus_routers(&state);
    compose(state, neon::public_routes(), neon::PUBLIC_PATHS, dioxus)
}

async fn get(app: &Router, path: &str) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// The URL paths one host advertises, in the order the document lists them.
async fn advertised_paths(app: &Router) -> Vec<String> {
    let (status, body) = get(app, "/sitemap.xml").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let paths: Vec<String> = body
        .split("<loc>")
        .skip(1)
        .filter_map(|rest| rest.split("</loc>").next())
        .map(|loc| {
            // Every `<loc>` is absolute. Everything from the third `/` on is
            // the path this host must serve.
            let after_scheme = loc.split_once("://").expect("an absolute URL").1;
            let path = after_scheme
                .find('/')
                .map_or("/", |slash| &after_scheme[slash..]);
            path.to_string()
        })
        .collect();
    assert!(!paths.is_empty(), "a host with pages advertises them");
    paths
}

/// Does `path` fall under a declared route pattern such as
/// `/presentations/{slug}/step/{step}`?
///
/// A brand's path table is the declaration Axum cannot be asked for, so it is
/// what a concrete URL is checked against. `{*rest}` swallows the remainder;
/// every other `{…}` matches exactly one segment.
fn matches(pattern: &str, path: &str) -> bool {
    let declared: Vec<&str> = pattern.split('/').collect();
    let actual: Vec<&str> = path.split('/').collect();
    for (index, segment) in declared.iter().enumerate() {
        if segment.starts_with("{*") {
            return actual.len() > index;
        }
        let Some(candidate) = actual.get(index) else {
            return false;
        };
        if segment.starts_with('{') {
            if candidate.is_empty() {
                return false;
            }
        } else if segment != candidate {
            return false;
        }
    }
    declared.len() == actual.len()
}

/// Every URL the firm advertises is a page the firm host serves.
#[tokio::test]
async fn the_firm_sitemap_advertises_only_pages_the_firm_host_serves() {
    let app = app().await;
    for path in advertised_paths(&app).await {
        let (status, _) = get(&app, &path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the firm sitemap advertises {path}, which its host answers with {status}"
        );
    }
}

/// Every URL the Foundation advertises is a page the Foundation host serves —
/// and serves to a stranger, which is the stricter half. A gated page answers
/// a redirect rather than `200`, so a crawler sent to one lands on the login
/// door.
#[tokio::test]
async fn the_foundation_sitemap_advertises_only_pages_the_foundation_host_serves() {
    let app = app().await;
    for path in advertised_paths(&app).await {
        let (status, _) = get(&app, &path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the Foundation sitemap advertises {path}, which its host answers with {status}"
        );
    }
}

/// Every advertised URL falls under a path the brand declares.
///
/// The `200` gates above prove the pages answer; this proves they answer
/// *because the brand registered them*, not through a Navigator-owned route
/// that happens to overlap.
#[tokio::test]
async fn every_advertised_url_falls_under_a_declared_brand_path() {
    for (brand, app, declared) in [
        ("the firm", app().await, neon::PUBLIC_PATHS),
        ("the Foundation", app().await, neon::PUBLIC_PATHS),
    ] {
        for path in advertised_paths(&app).await {
            assert!(
                declared.iter().any(|pattern| matches(pattern, &path)),
                "{brand} advertises {path}, which is not in its declared path table"
            );
        }
    }
}

/// The sitemap files each face under the prefix that serves it.
///
/// One binary means one sitemap, so the old cross-host gate — "the Foundation
/// must not advertise a firm page" — no longer describes a failure that can
/// happen: every page here resolves. What can still go wrong is the prefix. A
/// Foundation page advertised at the site root, or a firm page under
/// `/foundation`, tells a crawler the nonprofit published the fee schedule or
/// that the law firm runs the grant programme.
#[tokio::test]
async fn the_sitemap_files_each_face_under_its_own_prefix() {
    let advertised = advertised_paths(&app().await).await;

    for firm_page in [
        "/",
        "/services",
        "/litigation",
        "/fractional-gc",
        "/navigator",
        "/contact",
        "/blog",
        "/presentations",
    ] {
        assert!(
            advertised.iter().any(|path| path == firm_page),
            "the firm page {firm_page} must be advertised: {advertised:?}"
        );
    }

    for foundation_page in [
        "/foundation",
        "/foundation/education",
        "/foundation/attorneys",
    ] {
        assert!(
            advertised.iter().any(|path| path == foundation_page),
            "the Foundation page {foundation_page} must be advertised: {advertised:?}"
        );
    }

    // A retired URL is a `301`, not a document. Advertising one asks a crawler
    // to index a redirect and splits the page's authority across two URLs.
    for retired in [
        "/education",
        "/legal-aid",
        "/attorneys",
        "/mission",
        "/notations",
    ] {
        assert!(
            !advertised.iter().any(|path| path == retired),
            "{retired} is a retired Foundation URL and must not be advertised: {advertised:?}"
        );
    }
}

/// The sitemap expands over the content loaded at boot rather than a constant.
#[tokio::test]
async fn the_sitemap_expands_over_loaded_content() {
    let advertised = advertised_paths(&app().await).await;
    for expected in [
        "/blog/thanks-apple",
        "/presentations/rust-in-peace",
        // The raw-Markdown twin, which is what an LLM crawler fetches.
        "/presentations/rust-in-peace.md",
        "/presentations/rust-in-peace/step/1",
        "/workshops",
        "/workshops/use-the-navigator",
        "/workshops/use-the-navigator.md",
        "/workshops/use-the-navigator/step/1",
        "/privacy",
        "/llms.txt",
    ] {
        assert!(
            advertised.iter().any(|path| path == expected),
            "the sitemap is missing {expected}: {advertised:?}"
        );
    }
}
