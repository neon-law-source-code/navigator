//! `navigator notations preview <FILE>` — serve one template's
//! `/notations/{slug}` show page on a local bind, so an author can read what
//! they are writing as a reader will read it.
//!
//! The page is not a second rendering of the template. It is
//! [`portal::dioxus_app::notation_preview_router`] — the same axum router the
//! firm's public site mounts — fed by
//! [`portal::notation_preview_doc::from_markdown`], the same projection that
//! builds the published page. So the questionnaire section walks the
//! template's own declared question order with Navigator's real field
//! controls, and the workflow section draws the template's own declared state
//! machine. A template that reads badly here reads badly published.
//!
//! **Nothing is persisted and nothing is bound.** There is no Notation row,
//! no Answer row, no runtime signal, no store connection at all — the two
//! demo sections are client-side-only by construction (see
//! `webapp::notation_demo` and `webapp::notation_workflow`). That is what
//! lets this run offline against a file that has never been imported. To
//! walk the *real* questionnaire runtime and watch the post-questionnaire
//! workflow actually start, that is a different command and a different
//! machine (ENG-688).
//!
//! Two things the page needs beyond the router, and both are resolved here
//! rather than assumed:
//!
//! * **Stylesheets and scripts.** The show page hoists `/public/css/*` and
//!   `/public/js/*`. An author previewing a template stands in a matter's
//!   Project repository, which has no `server/public` directory, so those
//!   bytes are compiled into this binary and served from it.
//! * **The Dioxus client bundle.** Stepping the questionnaire is hydration,
//!   and hydration needs the wasm the `dx` build emits. When it is present
//!   the page steps; when it is not, `dioxus-server` degrades to an SSR-only
//!   shell and the questions still render, so a missing bundle costs
//!   interactivity rather than the preview. The command says which of the
//!   two the reader is looking at rather than leaving them to wonder why
//!   "Next" does nothing.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::extract::Path as PathParam;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use include_dir::{include_dir, Dir};

/// The show page's stylesheets, compiled in so the preview is styled in a
/// Project repository that has no `server/public` of its own.
static PUBLIC_CSS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../server/public/css");

/// The show page's scripts — the paragraph-stepping stage's key handling
/// among them.
static PUBLIC_JS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../server/public/js");

/// The environment variable `dioxus-server` reads to find the built client
/// bundle. Set from [`client_bundle_dir`] before the router is built:
/// `ServeConfig::new` resolves the index template once, at construction.
const PUBLIC_PATH_ENV: &str = "DIOXUS_PUBLIC_PATH";

/// Serve `file`'s show page on `port` until the process is interrupted.
///
/// `port` `0` asks the OS for a free one, which is what makes two previews
/// in two checkouts able to run at once; the bound port is printed either
/// way, so the reader never has to guess.
pub async fn run(file: &Path, port: u16) -> Result<()> {
    let here = std::env::current_dir().context("reading the current directory")?;
    let found = resolve_template(&here, file)?;
    let slug = slug_for(&found.path);
    let doc = portal::notation_preview_doc::from_markdown(
        &slug,
        &found.path.display().to_string(),
        &found.src,
    );

    let questions = doc.demo_questions.len();
    let states = doc.demo_workflow.len();
    let title = doc.title.clone();

    let bundle = client_bundle_dir();
    if let Some(dir) = &bundle {
        // Safe on this edition, and set before the router is built because
        // `ServeConfig::new` reads it exactly once, at construction.
        std::env::set_var(PUBLIC_PATH_ENV, dir);
    }
    let app = router(doc);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let bound = listener.local_addr().context("reading the bound port")?;

    eprintln!("==> {title}");
    eprintln!(
        "    {} question(s), {} workflow state(s), from {}{}",
        questions,
        states,
        found.path.display(),
        if found.bundled {
            " (bundled in this binary)"
        } else {
            ""
        }
    );
    match &bundle {
        Some(dir) => eprintln!("    client bundle: {}", dir.display()),
        None => eprintln!(
            "    client bundle: none — the page renders but the questionnaire will not step. \
             Build one with `navigator dev build-webapp`."
        ),
    }
    eprintln!("==> http://{bound}/notations/{slug}");
    eprintln!("    Ctrl-C to stop.");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serving the preview")?;
    Ok(())
}

/// The preview surface: the real show-page router, the client bundle's own
/// assets when there is a bundle, this binary's compiled-in stylesheets and
/// scripts, and a root that lands the reader on the page rather than a 404.
fn router(doc: webapp::notation_preview::PreviewDoc) -> Router {
    let slug = doc.slug.clone();
    let mut app = portal::dioxus_app::notation_preview_router(
        vec![doc],
        webapp::notation_preview::NotationPreviewMode::Local,
    );
    // Mounts the wasm and the wasm-bindgen glue at the paths the bundle's own
    // `index.html` references. `None` when no bundle is staged, which is the
    // SSR-only case the banner above already reported.
    if let Some(bundle) = portal::dioxus_app::router() {
        app = app.merge(bundle);
    }
    let home = format!("/notations/{slug}");
    app.route(
        "/",
        get(move || {
            let home = home.clone();
            async move { Redirect::temporary(&home) }
        }),
    )
    .route("/public/css/{*path}", get(css_asset))
    .route("/public/js/{*path}", get(js_asset))
}

async fn css_asset(PathParam(path): PathParam<String>) -> Response {
    embedded(&PUBLIC_CSS, &path, "text/css; charset=utf-8")
}

async fn js_asset(PathParam(path): PathParam<String>) -> Response {
    embedded(&PUBLIC_JS, &path, "text/javascript; charset=utf-8")
}

/// Serve one compiled-in asset. A path that escapes the directory resolves
/// to nothing in `include_dir` (it indexes by the exact relative path it
/// compiled), so a traversal attempt is a `404` rather than a read.
fn embedded(dir: &Dir<'static>, path: &str, content_type: &str) -> Response {
    match dir.get_file(path) {
        Some(file) => (
            [(header::CONTENT_TYPE, content_type)],
            file.contents().to_vec(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such asset").into_response(),
    }
}

/// The built Dioxus client bundle, or `None` when there is none to serve.
///
/// An explicit `DIOXUS_PUBLIC_PATH` wins, so a caller can point at a bundle
/// built anywhere. Otherwise this looks where `navigator dev build-webapp`
/// stages it, which only exists in a Navigator checkout — a Project
/// repository has no bundle and takes the SSR-only path.
fn client_bundle_dir() -> Option<PathBuf> {
    let staged = std::env::var_os(PUBLIC_PATH_ENV)
        .map(PathBuf::from)
        .or_else(|| Some(workspace_public_dir()?.join("dioxus")))?;
    staged.join("index.html").is_file().then_some(staged)
}

/// This binary's own `server/public`, when it is running out of a Navigator
/// checkout. `None` for an installed binary, whose `server/public` does not
/// exist on the reader's disk — which is why the stylesheets above are
/// compiled in rather than read from it.
fn workspace_public_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../server/public"));
    dir.is_dir().then_some(dir)
}

/// A template the preview will serve, and where its bytes came from.
#[derive(Debug)]
struct Resolved {
    /// What the slug and the printed provenance line derive from: a path on
    /// disk, or the repository-relative path of a bundled file.
    path: PathBuf,
    src: String,
    /// True when the bytes came out of this binary rather than a checkout.
    bundled: bool,
}

/// Resolve what the author typed to a template.
///
/// A path is taken as given. A bare name is looked up the three ways a name
/// is written: a Project repository's flat `templates/<code>.md`, then
/// Navigator's own nested `templates/notations/**/<code>.md`, then the
/// catalog compiled into this binary. Underscores and hyphens are treated as
/// the same character throughout, because a notation's `code` is written with
/// underscores and its URL slug with hyphens, and an author reaching for
/// either means the same file.
///
/// The bundled tier is last so a checkout always wins: an author editing a
/// template previews the file under their cursor, never the copy frozen into
/// the binary they happen to be running. It exists so the other direction
/// works too — `navigator notations preview onboarding` from any directory at
/// all, which is what makes the binary worth handing to someone.
fn resolve_template(base: &Path, file: &Path) -> Result<Resolved> {
    if file.is_file() {
        let src = std::fs::read_to_string(file)
            .with_context(|| format!("reading the template at {}", file.display()))?;
        return Ok(Resolved {
            path: file.to_path_buf(),
            src,
            bundled: false,
        });
    }
    let wanted = normalized_stem(file);
    if wanted.is_empty() {
        anyhow::bail!("{} names no template", file.display());
    }
    let mut searched = Vec::new();
    for root in ["templates", "templates/notations"] {
        let root = base.join(root);
        if !root.is_dir() {
            continue;
        }
        searched.push(root.display().to_string());
        let found = walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .find(|entry| {
                entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|e| e == "md")
                    && normalized_stem(entry.path()) == wanted
            });
        if let Some(entry) = found {
            let path = entry.into_path();
            let src = std::fs::read_to_string(&path)
                .with_context(|| format!("reading the template at {}", path.display()))?;
            return Ok(Resolved {
                path,
                src,
                bundled: false,
            });
        }
    }
    if let Some(found) = bundled_template(&wanted) {
        return Ok(found);
    }
    searched.push("the catalog bundled in this binary".to_string());
    anyhow::bail!(
        "no template named `{}` in {} — pass a path instead",
        wanted,
        searched.join(", or ")
    )
}

/// The bundled template whose stem matches `wanted`, read out of the
/// catalog [`portal::template_api::bundled_files`] compiled into this binary.
///
/// Only Markdown is a candidate: a form's `.fields` manifest travels with the
/// catalog for `export`, but it is not a template anyone previews.
fn bundled_template(wanted: &str) -> Option<Resolved> {
    portal::template_api::bundled_files()
        .into_iter()
        .filter(|(path, _)| {
            Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        })
        .find(|(path, _)| normalized_stem(Path::new(path)) == wanted)
        .and_then(|(path, bytes)| {
            Some(Resolved {
                path: PathBuf::from(path),
                src: String::from_utf8(bytes.to_vec()).ok()?,
                bundled: true,
            })
        })
}

/// A file stem reduced to its lookup key: every separator becomes a hyphen
/// and a run of them collapses to one.
///
/// A notation's `code` doubles its separator between the shelf and the
/// document (`us__form_990`, `sample__letter`) and its URL slug keeps both
/// (`us--form-990`), but an author naming a template at a prompt writes one
/// (`us-form-990`). Collapsing here is what lets any of those three spellings
/// name the same file; it never touches the slug the page is served at, which
/// stays the canonical [`views::slug::to_url`] form.
fn normalized_stem(path: &Path) -> String {
    let flattened = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
        .replace('_', "-");
    let mut key = String::with_capacity(flattened.len());
    for part in flattened.split('-').filter(|part| !part.is_empty()) {
        if !key.is_empty() {
            key.push('-');
        }
        key.push_str(part);
    }
    key
}

/// The `{slug}` this template is served at — its filename in the kebab-case
/// every file-derived URL segment uses ([`views::slug::to_url`]), so the
/// local URL is the shape the published one would be.
fn slug_for(path: &Path) -> String {
    views::slug::to_url(
        &path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase(),
    )
}

#[cfg(test)]
mod tests {
    use super::{normalized_stem, resolve_template, router, slug_for};
    use std::path::Path;

    const TEMPLATE: &str = "---\ntitle: Sample Letter\ncode: sample__letter\nquestionnaire:\n  \
                            BEGIN:\n    _: custom_text__client_name\n  \
                            custom_text__client_name:\n    _: END\n  END: \
                            {}\ncustom_questions:\n  client_name:\n    prompt: What \
                            is your name?\nworkflow:\n  BEGIN:\n    intake_submitted: \
                            lawyer_review\n  lawyer_review:\n    approved: END\n  END: \
                            {}\n---\n\n# Sample Letter\n\nBody prose.\n";

    /// The underscore `code` an author writes in frontmatter becomes the
    /// kebab-case `{slug}` the page is served at, so the local URL has the
    /// shape the published one would.
    #[test]
    fn slug_is_the_kebab_form_of_the_filename() {
        // Each separator maps across one-for-one, which is what makes the
        // local URL the same shape the published page would carry.
        assert_eq!(
            slug_for(Path::new("templates/us__form_990.md")),
            "us--form-990"
        );
        assert_eq!(
            slug_for(Path::new("/x/Onboarding_Letter.md")),
            "onboarding-letter"
        );
    }

    /// A name written either way names the same file, because a notation's
    /// `code` uses underscores and its slug uses hyphens.
    #[test]
    fn a_name_matches_whichever_separator_the_author_reached_for() {
        // A `code` doubles its separator and a typed name does not, so the
        // lookup key collapses the run: all three spellings name one file.
        assert_eq!(
            normalized_stem(Path::new("a/us__form_990.md")),
            "us-form-990"
        );
        assert_eq!(
            normalized_stem(Path::new("a/us--form-990.md")),
            normalized_stem(Path::new("b/us__form_990.md"))
        );
        assert_eq!(
            normalized_stem(Path::new("c/us-form-990.md")),
            normalized_stem(Path::new("b/us__form_990.md"))
        );
    }

    /// An existing path is taken as given, and a name that matches nothing
    /// fails with what was looked for rather than serving an empty page.
    #[test]
    fn resolves_a_path_and_refuses_an_unknown_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample__letter.md");
        std::fs::write(&file, TEMPLATE).expect("write");

        assert_eq!(
            resolve_template(dir.path(), &file)
                .expect("the path resolves")
                .path,
            file
        );

        let err = resolve_template(dir.path(), Path::new("no-such-notation"))
            .expect_err("an unknown name is refused");
        assert!(
            err.to_string().contains("no-such-notation"),
            "the error names what was looked for: {err}"
        );
    }

    /// A name that names nothing on disk falls through to the catalog in
    /// the binary, so the preview works outside a checkout — the whole point
    /// of shipping the templates inside `navigator`.
    #[test]
    fn a_name_falls_back_to_the_catalog_bundled_in_the_binary() {
        let empty = tempfile::tempdir().expect("tempdir");

        let found = resolve_template(empty.path(), Path::new("onboarding"))
            .expect("the bundled onboarding letter resolves with no templates/ in sight");

        assert!(found.bundled);
        assert_eq!(found.path, Path::new("notations/neon_law/onboarding.md"));
        assert!(found.src.contains("code: onboarding__letter"));
    }

    /// The checkout wins. An author editing a template previews the file
    /// under their cursor, never the copy frozen into the binary.
    #[test]
    fn a_checkout_outranks_the_bundled_copy_of_the_same_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("templates/notations")).expect("mkdir");
        let file = dir.path().join("templates/notations/onboarding.md");
        std::fs::write(&file, TEMPLATE).expect("write");

        let found = resolve_template(dir.path(), Path::new("onboarding")).expect("resolves");

        assert!(!found.bundled);
        assert_eq!(found.path, file);
        assert!(found.src.contains("code: sample__letter"));
    }

    /// The failure still names what was looked for, and now says the bundled
    /// catalog was consulted too, so the reader knows the search was total.
    #[test]
    fn an_unknown_name_reports_every_tier_it_searched() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("templates")).expect("mkdir");

        let err = resolve_template(dir.path(), Path::new("no-such-notation"))
            .expect_err("an unknown name is refused");

        let message = err.to_string();
        assert!(message.contains("no-such-notation"), "{message}");
        assert!(message.contains("bundled in this binary"), "{message}");
    }

    /// The preview mounts the show page at the template's own slug and lands
    /// the root there, so the printed URL and a bare `localhost:<port>` reach
    /// the same page.
    #[tokio::test]
    async fn root_redirects_to_the_template_page() {
        use tower::ServiceExt;

        let doc = portal::notation_preview_doc::from_markdown(
            "sample-letter",
            "/tmp/sample__letter.md",
            TEMPLATE,
        );
        let app = router(doc);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(
            response.status(),
            axum::http::StatusCode::TEMPORARY_REDIRECT
        );
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::LOCATION)
                .and_then(|v| v.to_str().ok()),
            Some("/notations/sample-letter")
        );
    }

    /// The local authoring server renders the notation alone while retaining
    /// the actual document, questionnaire, and workflow surfaces.
    #[tokio::test]
    async fn local_preview_omits_global_chrome_and_keeps_notation_surfaces() {
        use axum::body::to_bytes;
        use tower::ServiceExt;

        let doc = portal::notation_preview_doc::from_markdown(
            "sample-letter",
            "/tmp/sample__letter.md",
            TEMPLATE,
        );
        let response = router(doc)
            .oneshot(
                axum::http::Request::builder()
                    .uri("/notations/sample-letter")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body")
                .to_vec(),
        )
        .expect("UTF-8 HTML");
        assert!(!html.contains("site-header"), "header chrome: {html}");
        assert!(
            !html.contains("site-footer__legal"),
            "footer chrome: {html}"
        );
        assert!(html.contains("Sample Letter"), "document: {html}");
        assert!(html.contains("Try answering this"), "questionnaire: {html}");
        assert!(html.contains("notation-workflow"), "workflow: {html}");
    }

    /// The stylesheets the page hoists are served out of this binary, so a
    /// preview run inside a Project repository — which has no
    /// `server/public` — is styled rather than bare markup.
    #[tokio::test]
    async fn stylesheets_are_served_from_the_compiled_in_copy() {
        use tower::ServiceExt;

        let doc = portal::notation_preview_doc::from_markdown("s", "/tmp/s.md", TEMPLATE);
        let app = router(doc);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    // The page requests it cache-busted; the query is not part
                    // of the match.
                    .uri("/public/css/theme.css?v=26.9.13-7")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/css; charset=utf-8")
        );
    }

    /// A template the author names rather than paths to is found under a
    /// Project repository's flat `templates/`.
    #[test]
    fn finds_a_named_template_under_a_project_repositorys_flat_templates() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("templates")).expect("mkdir");
        let file = dir.path().join("templates/sample__letter.md");
        std::fs::write(&file, TEMPLATE).expect("write");

        // The author types the single-separator name; the file on disk carries
        // the `code`'s doubled one.
        let found = resolve_template(dir.path(), Path::new("sample-letter"));

        let found = found.expect("the name resolves");
        assert_eq!(found.path, dir.path().join("templates/sample__letter.md"));
        assert!(!found.bundled, "a checkout outranks the bundled catalog");
    }
}
