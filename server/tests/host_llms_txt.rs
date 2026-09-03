//! `/llms.txt`, composed exactly as the binary composes it.
//!
//! An `llms.txt` is the same promise `/sitemap.xml` makes, to a different
//! reader: every URL in it is a document this host serves. The crawler table
//! (`host_crawler_and_legal_routes`) is shared by every brand host, and while
//! the page list inside it was hardcoded that promise was broken — one
//! hardcoded list opens every host with one brand's name and sends an LLM
//! crawler at pages the others do not serve.
//!
//! So the assertion here is the promise itself, made against the real
//! composition rather than against a restated list: fetch the `llms.txt`, then
//! fetch every URL in it from the same host. A page a brand advertises enters
//! this gate by being advertised.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// An `AppState` carrying the content the corpus is built from. The shared
/// builder ships empty indexes, so a document built on it would advertise the
/// static pages and none of the talks — the half most likely to drift.
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
async fn app() -> Router {
    let state = state().await;
    let dioxus = neon::public_dioxus_routers(&state);
    compose(state, neon::public_routes(), neon::PUBLIC_PATHS, dioxus)
}

async fn get(app: &Router, path: &str) -> (StatusCode, String) {
    get_on_host(app, path, None).await
}

async fn get_on_host(app: &Router, path: &str, host: Option<&str>) -> (StatusCode, String) {
    let mut builder = Request::builder().uri(path);
    if let Some(host) = host {
        builder = builder.header(header::HOST, host);
    }
    let resp = app
        .clone()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn document(app: &Router) -> String {
    let (status, body) = get(app, "/llms.txt").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// The URL paths one host advertises, in the order the document lists them.
///
/// Every link in the document is `- [title](url): description`, and every URL
/// is absolute. Everything from the third `/` on is the path this host must
/// serve.
fn advertised_paths(body: &str) -> Vec<String> {
    let paths: Vec<String> = body
        .lines()
        .filter(|line| line.starts_with("- ["))
        .filter_map(|line| line.split_once("](").map(|(_, rest)| rest))
        .filter_map(|rest| rest.split(')').next())
        .map(|url| {
            let after_scheme = url.split_once("://").expect("an absolute URL").1;
            let path = after_scheme
                .find('/')
                .map_or("/", |slash| &after_scheme[slash..]);
            path.to_string()
        })
        .collect();
    assert!(!paths.is_empty(), "a host with pages advertises them");
    paths
}

/// Every URL the firm advertises is a document the firm host serves.
#[tokio::test]
async fn the_firm_llms_txt_advertises_only_documents_the_firm_host_serves() {
    let app = app().await;
    for path in advertised_paths(&document(&app).await) {
        let (status, _) = get(&app, &path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the firm llms.txt advertises {path}, which its host answers with {status}"
        );
    }
}

/// The `/services` entry claims no dollar figure the page does not carry.
///
/// The firm publishes no fee amounts on its public pages: a matter's card on
/// `/services` renders a price chip only once its fee is set, and every entry
/// is unset today. An index that told a crawler the page named its fees "in
/// dollars" would send it looking for numbers `/services` does not have, so
/// this checks the promise against the page rather than restating the
/// contract as a string match.
#[tokio::test]
async fn the_services_entry_does_not_overclaim_published_prices() {
    let app = app().await;
    let (status, services_body) = get(&app, "/services").await;
    assert_eq!(status, StatusCode::OK, "{services_body}");
    let services_publishes_a_price = services_body.contains("fm-chips");

    let llms_txt = document(&app).await;
    let services_line = llms_txt
        .lines()
        .find(|line| line.contains("](") && line.contains("/services)"))
        .expect("the /services entry is in the index");

    if !services_publishes_a_price {
        assert!(
            !services_line.contains('$'),
            "the index promises a dollar figure the page does not carry: {services_line}"
        );
        assert!(
            !services_line.to_lowercase().contains("in dollars"),
            "the index promises a dollar figure the page does not carry: {services_line}"
        );
    }
}

/// The index advertises the firm's pages.
#[tokio::test]
async fn the_index_advertises_the_firms_pages() {
    let advertised = advertised_paths(&document(&app().await).await);

    for firm_page in [
        "/",
        "/services",
        "/litigation",
        "/fractional-gc",
        "/navigator",
        "/notations",
        "/contact",
        "/blog",
        "/presentations",
    ] {
        assert!(
            advertised.iter().any(|path| path == firm_page),
            "the firm page {firm_page} must be advertised: {advertised:?}"
        );
    }
}

/// The index opens as the firm and names no other organization.
#[tokio::test]
async fn the_index_opens_as_the_firm_and_names_nobody_else() {
    let body = document(&app().await).await;
    assert!(
        body.starts_with(&format!("# {}\n", views::brand::FIRM_BRAND.site_name)),
        "the llms.txt opens as the firm: {body}"
    );
    assert!(
        !body.contains(&["Neon", "Law", "Foundation"].join(" ")),
        "no other organization is named in the index: {body}"
    );
}

/// The notes about the application underneath are the half every brand host
/// shares: each mounts the same Navigator, so how an agent should work with it
/// reads identically on any domain.
#[tokio::test]
async fn every_host_carries_the_shared_navigator_notes() {
    for (brand, body) in [("the firm", document(&app().await).await)] {
        for note in [
            "Nothing is legal advice without a signed retainer for an active project.",
            "A Template is a markdown file with YAML frontmatter",
            "`{{placeholders}}`",
            "ground questionnaire states and placeholders",
            "Use the Navigator CLI to validate templates",
        ] {
            assert!(
                body.contains(note),
                "{brand} llms.txt is missing the shared note {note}: {body}"
            );
        }
    }
}

/// The corpora expand over the materials loaded at boot rather than constants.
#[tokio::test]
async fn the_corpora_expand_over_the_loaded_materials() {
    let advertised = advertised_paths(&document(&app().await).await);

    let body = document(&app().await).await;
    assert!(
        body.contains("## Workshop Corpus"),
        "the workshops are indexed under their own heading: {body}"
    );
    assert!(
        advertised
            .iter()
            .any(|path| path == "/workshops/use-the-navigator.md"),
        "the workshop Markdown twin must be indexed: {advertised:?}"
    );
}

/// The talks are not part of the crawlable index.
///
/// `/presentations` still serves anonymously, but the firm does not curate a
/// Presentation Corpus section for `/llms.txt` — a crawler that wants the
/// talks finds them from `/presentations` itself.
#[tokio::test]
async fn the_index_carries_no_presentation_corpus() {
    let advertised = advertised_paths(&document(&app().await).await);
    assert!(
        !advertised
            .iter()
            .any(|path| path == "/presentations/rust-in-peace.md"),
        "the presentation corpus must not be advertised: {advertised:?}"
    );

    let body = document(&app().await).await;
    assert!(
        !body.contains("## Presentation Corpus"),
        "the index must not carry a Presentation Corpus section: {body}"
    );
}

/// The document carries no empty section.
///
/// A heading with nothing beneath it tells a crawler a corpus exists and then
/// gives it none.
#[tokio::test]
async fn no_section_renders_empty() {
    let body = document(&app().await).await;
    assert!(body.contains("## Pages"), "{body}");
    for heading in body.match_indices("## ") {
        let rest = &body[heading.0 + 3..];
        let section: String = rest
            .lines()
            .skip(1)
            .take_while(|l| !l.starts_with("## "))
            .collect();
        assert!(
            !section.trim().is_empty(),
            "a section header renders with nothing beneath it: {body}"
        );
    }
}

/// The house-brand host names itself, uses its own base URL, and does not
/// advertise Neon practice pages.
#[tokio::test]
async fn the_delete_your_data_llms_txt_is_that_brands_pages_under_its_host() {
    let app = app().await;
    let host = "staging.deleteyourdata.com";
    let (status, body) = get_on_host(&app, "/llms.txt", Some(host)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.starts_with("# DeleteYourData.com\n"), "{body}");
    assert!(body.contains(&format!("https://{host}/)")), "{body}");
    assert!(!body.contains("neonlaw.com"), "{body}");
    assert!(!body.contains("/litigation"), "{body}");
    for path in advertised_paths(&body) {
        let (status, _) = get_on_host(&app, &path, Some(host)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{host} llms.txt advertises {path}, which answers {status}"
        );
    }
}
