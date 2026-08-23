//! `/llms.txt` on **both** hosts, composed exactly as their binaries compose
//! them.
//!
//! An `llms.txt` is the same promise `/sitemap.xml` makes, to a different
//! reader: every URL in it is a document this host serves. The two brands merge
//! one shared crawler table (`host_crawler_and_legal_routes`), and while the
//! page list inside it was hardcoded that promise was broken on the firm's
//! host — it opened with the Foundation's name and sent an LLM crawler to `/`,
//! `/education` and `/attorneys` as the nonprofit's pages, which the firm does
//! not serve at all.
//!
//! So the assertion here is the promise itself, made against the real
//! composition rather than against a restated list: fetch each host's
//! `llms.txt`, then fetch every URL in it from that same host. A page a brand
//! advertises enters this gate by being advertised.

use axum::body::Body;
use axum::http::{Request, StatusCode};
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

/// Every URL the Foundation advertises is a document the Foundation host
/// serves — and serves to a stranger, which is the stricter half. A gated page
/// answers a redirect rather than `200`, so a crawler sent to one lands on the
/// login door.
#[tokio::test]
async fn the_foundation_llms_txt_advertises_only_documents_the_foundation_host_serves() {
    let app = app().await;
    for path in advertised_paths(&document(&app).await) {
        let (status, _) = get(&app, &path).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "the Foundation llms.txt advertises {path}, which its host answers with {status}"
        );
    }
}

/// The one index advertises both faces, each under the prefix that serves it.
///
/// The two hosts each carried their own document and neither could name the
/// other's pages; one binary means one `/llms.txt`, so the risk inverted. It is
/// no longer "the firm advertised a Foundation page it 404s" — every page here
/// resolves — it is that a crawler is told a Foundation page lives at the site
/// root, or a firm page under `/foundation`, and indexes the nonprofit's work
/// as the law firm's or the reverse.
#[tokio::test]
async fn the_index_files_each_face_under_its_own_prefix() {
    let advertised = advertised_paths(&document(&app().await).await);

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

    // No Foundation page may be advertised at the root, and no firm page
    // beneath the prefix. This is the misattribution the prefix exists to
    // prevent, and it is the assertion that replaces the old cross-host gate.
    for stray in ["/education", "/legal-aid", "/mission", "/notations"] {
        assert!(
            !advertised.iter().any(|path| path == stray),
            "{stray} is a retired Foundation URL and a `301`; advertising it \
             indexes a redirect as a document: {advertised:?}"
        );
    }
    for stray in [
        "/foundation/services",
        "/foundation/litigation",
        "/foundation/blog",
    ] {
        assert!(
            !advertised.iter().any(|path| path == stray),
            "{stray} would file the firm's practice under the nonprofit: {advertised:?}"
        );
    }
}

/// The index opens as the firm, and names the Foundation as a section of the
/// site rather than as its author.
///
/// One document now introduces two organizations, so the opening line has to
/// say which one publishes it. The firm holds the root and renders the legal
/// services, so the preamble is the firm's — and the Foundation appears in the
/// page list, where a reader learns it is part of the same site without being
/// told a 501(c)(3) wrote the fee schedule.
#[tokio::test]
async fn the_index_opens_as_the_firm_and_names_the_foundation_within() {
    let body = document(&app().await).await;
    assert!(
        body.starts_with(&format!("# {}\n", views::brand::FIRM_BRAND.site_name)),
        "the llms.txt opens as the firm: {body}"
    );
    assert!(
        body.contains("Neon Law Foundation"),
        "the Foundation is named in the document it now shares: {body}"
    );
}

/// The notes about the application underneath are the half both hosts share:
/// each mounts the same Navigator, so how an agent should work with it reads
/// identically on either domain.
#[tokio::test]
async fn both_hosts_carry_the_shared_navigator_notes() {
    for (brand, body) in [
        ("the firm", document(&app().await).await),
        ("the Foundation", document(&app().await).await),
    ] {
        for note in [
            "This is not legal advice; attorney review remains required.",
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
    assert!(
        advertised
            .iter()
            .any(|path| path == "/presentations/rust-in-peace.md"),
        "the raw-Markdown twin a crawler fetches must be indexed: {advertised:?}"
    );

    let body = document(&app().await).await;
    assert!(
        body.contains("## Presentation Corpus"),
        "the talks are indexed under their own heading: {body}"
    );
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

/// The document carries no empty section.
///
/// `## Presentation Corpus` renders only when talks are loaded; a heading with
/// nothing beneath it tells a crawler a corpus exists and then gives it none.
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
