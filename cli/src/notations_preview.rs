//! `navigator notation preview <FILE>` (LAW-29) — push one template as a
//! **draft** to the Project repository it sits in, and open that Project's
//! real portal at it.
//!
//! The published show page depends on `@neon-law-source-code/navigator-ux`
//! to draw a surface Navigator could serve directly, and a from-scratch
//! local imitation of that page has to be kept in step with it by hand —
//! easy to let drift, and once it drifts the one thing a preview exists to
//! prove (that the questionnaire steps the way an author wrote it) is
//! exactly what it can no longer show. So the default here is not a second
//! renderer: it is the production one. [`run`] reads the Project and the
//! deployment host out of `navigator.yaml`, the way `navigator project
//! gate` does — there is no `--project` flag, because a template is always
//! previewed as the Project whose repository it sits in — pushes the
//! template to that deployment as a **draft**, and opens the browser at it.
//!
//! A draft is stored and addressable, but explicitly **not run**: creating
//! one starts no workflow instance, journals no `intake_submitted`, and
//! renders no PDF — nothing a draft creates could be mistaken for an
//! executed instrument or a filed document (see `store::notation_drafts`).
//!
//! # `--offline`
//!
//! Outside a Project repository, or without a login, there is no
//! deployment to push a draft to. `--offline` covers that case, but it is
//! a **lint**, not a preview: a from-scratch local render of the same
//! [`portal::dioxus_app::notation_preview_router`] the firm's public site
//! mounts, with no store connection, no persistence, and no claim to be
//! the surface a client will see. Good for catching a malformed
//! questionnaire or workflow spec before it ever reaches a Project; not a
//! substitute for opening the real draft.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
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

/// `navigator notation preview <file>` (LAW-29).
///
/// Resolves `file` to a template, then either pushes it as a draft to the
/// Project repository it sits in and opens the real portal at it, or — with
/// `offline` — renders it locally as a lint. `host_override` is honored only
/// in the pushed-draft path, and only as an override of `navigator.yaml`'s
/// own `project.host`, never a replacement for it.
pub async fn run(file: &Path, offline: bool, port: u16, host_override: Option<&str>) -> Result<()> {
    let here = std::env::current_dir().context("reading the current directory")?;
    let found = resolve_template(&here, file)?;
    let slug = slug_for(&found.path);

    if offline {
        return run_offline_lint(&found, &slug, port).await;
    }

    let (project_code, manifest_host) = read_project_manifest(&here)?;
    let host = host_override.unwrap_or(&manifest_host);

    let doc = portal::notation_preview_doc::from_markdown(
        &slug,
        &found.path.display().to_string(),
        &found.src,
    );

    eprintln!("==> {}", doc.title);
    eprintln!(
        "    pushing a draft to `{project_code}` from {}{}",
        found.path.display(),
        if found.bundled {
            " (bundled in this binary)"
        } else {
            ""
        }
    );

    let (draft_id, url) =
        crate::remote::create_notation_draft(Some(host), &project_code, &slug, &found.src)
            .await
            .context("pushing the draft")?;

    eprintln!("    draft {draft_id} — not run: no notation, no workflow, no PDF");
    eprintln!("==> {url}");
    if open_browser(&url) {
        eprintln!("    opened in your browser.");
    } else {
        eprintln!("    open that URL in a browser to see the draft.");
    }
    Ok(())
}

/// Read the Project and the deployment host out of `navigator.yaml` two
/// directories up, the way `navigator project gate` does. Refuses outside a
/// Project repository rather than falling back to a local render — asking
/// the author to name the Project or the host invites naming the wrong one,
/// when the repository they are standing in already answers both.
fn read_project_manifest(root: &Path) -> Result<(String, String)> {
    let (project, host) = crate::document_sync::read_manifest(root).map_err(|error| {
        anyhow!(
            "`navigator notation preview` pushes a draft to the Project repository it runs \
             in, and this does not look like one: {error:#}. Run it from a Project repository \
             root, or pass `--offline` to lint the template locally instead."
        )
    })?;
    let host = host.ok_or_else(|| {
        anyhow!(
            "{} names a Project but no host to preview against — add `project.host` to \
             navigator.yaml, or pass `--host` to override it",
            crate::projects::manifest::FILE
        )
    })?;
    Ok((project, host))
}

/// Best-effort: open `url` in the reader's default browser. Returns whether
/// it worked, but opening is convenience, not a contract — the URL is
/// printed either way, so a failure here never loses the one thing the
/// reader actually needs.
///
/// macOS's `open`, Linux's `xdg-open`, and Windows' `cmd /c start` cover
/// every platform this binary ships for without a crate dependency. A
/// failure here (headless CI, a missing binary, no display) is silently
/// non-fatal — the printed URL is the real interface.
fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let opened = std::process::Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let opened = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .status();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let opened: std::io::Result<std::process::ExitStatus> =
        Err(std::io::Error::other("no known browser-open command"));

    opened.is_ok_and(|status| status.success())
}

/// Render `found`'s show page on `port` until the process is interrupted —
/// the offline lint. Named for what it is: a local render of the same
/// router the public site mounts, with no store connection and no claim to
/// be the surface a client will see. `port` `0` asks the OS for a free one,
/// so two lints can run at once.
async fn run_offline_lint(found: &Resolved, slug: &str, port: u16) -> Result<()> {
    let doc = portal::notation_preview_doc::from_markdown(
        slug,
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

    eprintln!("==> lint (offline): {title}");
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
    eprintln!("    this is a local render, not the Project's portal — nothing is pushed.");
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
        .context("serving the lint")?;
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
        None,
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
/// works too — `navigator notation preview onboarding` from any directory at
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
    use super::{normalized_stem, read_project_manifest, resolve_template, router, slug_for};
    use std::path::Path;

    const TEMPLATE: &str = "---\ntitle: Sample Letter\ncode: sample__letter\nquestionnaire:\n  \
                            BEGIN:\n    _: custom_text__client_name\n  \
                            custom_text__client_name:\n    _: END\n  END: \
                            {}\nprompts:\n  client_name: What \
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

    /// LAW-29's gate: outside a Project repository there is nothing to push
    /// a draft to, and the command refuses rather than falling back to a
    /// local render.
    #[test]
    fn preview_refuses_outside_a_project_repository() {
        let dir = tempfile::tempdir().expect("tempdir");

        let error = read_project_manifest(dir.path())
            .expect_err("a directory with no navigator.yaml is not a Project repository");
        let message = error.to_string();
        assert!(message.contains("navigator.yaml"), "{message}");
        assert!(message.contains("--offline"), "{message}");
    }

    /// A `navigator.yaml` that names a Project but no host is refused too —
    /// there is nowhere to push the draft.
    #[test]
    fn preview_refuses_a_manifest_with_no_host() {
        // The nested `project: {host, name}` shape requires `host` at parse
        // time, so a manifest missing it entirely has to use the deprecated
        // flat shape to reach `read_project_manifest`'s own host check
        // rather than `manifest::parse`'s.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "version: 26.9.17\nproject: acme\n",
        )
        .expect("write manifest");

        let error = read_project_manifest(dir.path())
            .expect_err("a manifest naming no host cannot resolve a deployment");
        assert!(error.to_string().contains("host"), "{error}");
        assert!(error.to_string().contains("navigator.yaml"), "{error}");
    }

    /// The success path: a real manifest resolves to the Project code and
    /// host it names, read the same way `navigator project gate` reads them.
    #[test]
    fn preview_reads_the_project_and_host_navigator_yaml_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("navigator.yaml"),
            "version: 26.9.17\nproject:\n  host: www.neonlaw.com\n  name: acme\n",
        )
        .expect("write manifest");

        let (project, host) = read_project_manifest(dir.path()).expect("manifest resolves");
        assert_eq!(project, "acme");
        assert_eq!(host, "www.neonlaw.com");
    }
}
