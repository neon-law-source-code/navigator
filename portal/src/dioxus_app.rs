//! Phase 0 Dioxus mount (issue #641).
//!
//! Mounts the [`webapp`] crate's `App` component into the existing `web` axum
//! router on one low-risk page — [`DIOXUS_DEMO_PATH`] — server-side rendered by
//! `dioxus-server` and hydrated in the browser by a same-origin WebAssembly
//! bundle. `navigator dev build-webapp` drives `dx` to build that bundle into a
//! directory; `web`'s `main` points `DIOXUS_PUBLIC_PATH` at it (defaulting to
//! `server/public/dioxus`).
//!
//! The mount is deliberately *constrained*: `dioxus-server`'s
//! `serve_dioxus_application` installs a global fallback that would answer every
//! unmatched route, so this module never calls it. Instead it serves the client
//! bundle plus exactly [`DIOXUS_DEMO_PATH`], leaving `web`'s own fallback and
//! all its routes — the JSON API, MCP, A2A, git smart-HTTP, OIDC, webhooks, and
//! every marketing page — untouched.
//!
//! When no bundle directory is present (the default in unit tests and any
//! deploy that has not built the bundle), [`router`] returns `None` and the
//! demo page is simply absent — nothing else changes. This also sidesteps
//! `serve_static_assets`, which panics on a missing public directory.
//!
//! ## CSP and hydration (a Phase 0 finding)
//!
//! Dioxus 0.7 serializes its hydration data into **inline** `<script>` elements
//! (`window.initial_dioxus_hydration_data = …`, `window.dx_hydrate(…)`), which a
//! strict `script-src 'self'` blocks — so the page renders but never hydrates.
//! The epic's CSP table anticipated only `'wasm-unsafe-eval'`; this is the extra
//! allowance SSR hydration actually needs. Rather than weaken the policy with a
//! blanket `'unsafe-inline'`, [`dioxus_document_head`] tags Dioxus's own inline
//! scripts with a fresh per-response nonce and scopes a `script-src 'self'
//! 'nonce-…' 'wasm-unsafe-eval'` policy to this one route. A nonce is strictly
//! stronger than `'unsafe-inline'`: only Dioxus's first-party scripts run, an
//! injected `<script>` still cannot. No CDN host ever enters the policy.
//!
//! ## Brand typography
//!
//! The same middleware also declares the licensed GORP Serif faces. `webapp`'s
//! `index.html` is a fixed template and `webapp` itself compiles to
//! `wasm32-unknown-unknown`, so neither can read the deployment asset origin
//! that the WOFF2 files live behind — the repository ships the declaration, not
//! the bytes. Resolving the two face URLs here, on the server, is what keeps a
//! Dioxus page set in the firm's typeface instead of the browser's default
//! serif, and it applies to every current and future Dioxus route without each
//! one remembering to opt in. `font-src` widens to the same asset origin the
//! site-wide policy admits, so the faces are actually fetchable.

use std::path::PathBuf;

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::{from_fn, from_fn_with_state, Next},
    response::{IntoResponse, Response},
    routing::get,
    RequestExt, Router,
};
use base64::Engine as _;
use dioxus_server::{render_handler, DioxusRouterExt, FullstackState, ServeConfig};

/// The single low-risk page Phase 0 renders through Dioxus.
pub const DIOXUS_DEMO_PATH: &str = "/dioxus-demo";

/// The lawyer entity-types directory — the first admin list page migrated to
/// Dioxus (#641 Phase 3). Kept at its existing path so the embedded Rego policy (keyed on
/// the request path) continues to gate it to lawyer/admin.
pub const LAWYER_ENTITY_TYPES_PATH: &str = "/lawyer/entity-types";

/// The living design system. Renders the Dioxus Components — the theme, icons,
/// cards, and toasts — so the gallery shows the real components the pages use.
pub const DESIGN_PATH: &str = "/design";

/// The firm host's `/blog` index path (#641 / #730 PR6).
pub const BLOG_PATH: &str = "/blog";

/// The firm blog post route (one post per `{slug}`), served by the Dioxus SSR
/// port. Firm-scoped and English-only.
pub const BLOG_POST_PATH: &str = "/blog/{slug}";

/// The template gallery index.
pub const TEMPLATES_PATH: &str = "/templates";

/// One template's detail page, or its `/download` raw markdown.
pub const TEMPLATE_ENTRY_PATH: &str = "/templates/{*path}";

/// The workspace-documentation hub, which renders the `index` doc.
pub const DOCS_PATH: &str = "/docs";

/// The slug that [`DOCS_PATH`] renders — the hub has no path parameter.
pub const DOCS_INDEX_SLUG: &str = "index";

/// One workspace doc, served by the Dioxus SSR port.
pub const DOC_PATH: &str = "/docs/{slug}";

/// The environment variable naming the built client-bundle directory. Read by
/// `dioxus-server`'s `ServeConfig::new` (for the `index.html` template) and
/// `serve_static_assets` (for the wasm + glue), so this module points both at
/// the same directory by reading it here first.
const PUBLIC_PATH_ENV: &str = "DIOXUS_PUBLIC_PATH";

/// The bundle directory to serve, or `None` when it is unset or has no
/// `index.html` (so the Dioxus page must not mount).
fn bundle_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(PUBLIC_PATH_ENV)?);
    dir.join("index.html").is_file().then_some(dir)
}

/// Build the constrained Dioxus sub-router, or `None` when no client bundle is
/// available.
///
/// The returned `Router<()>` serves the client bundle (wasm + wasm-bindgen
/// glue) at the same-origin paths `webapp`'s `index.html` references and renders
/// [`webapp::App`] at [`DIOXUS_DEMO_PATH`], with [`dioxus_document_head`] scoped
/// to the rendered page. `.merge()` it into the main router after its
/// `.with_state(...)`.
#[must_use]
pub fn router() -> Option<Router> {
    // Only mount when a real bundle directory exists: `serve_static_assets`
    // reads the directory eagerly and panics if it is missing, and a demo page
    // with no wasm to hydrate it is not worth serving.
    let _dir = bundle_dir()?;
    Some(
        Router::<FullstackState>::new()
            .serve_static_assets()
            .route(
                DIOXUS_DEMO_PATH,
                get(render_handler).layer(from_fn(dioxus_document_head)),
            )
            .with_state(FullstackState::new(ServeConfig::new(), webapp::App)),
    )
}

/// Middleware for the rendered page. It owns the head-level concerns the
/// SSR'd component tree cannot express itself:
///
/// - give Dioxus's inline hydration scripts a per-response nonce and emit a
///   matching route-scoped CSP, so hydration runs under `script-src 'self'
///   'nonce-…' 'wasm-unsafe-eval'` without ever admitting blanket
///   `'unsafe-inline'` or a CDN host;
/// - declare the licensed GORP Serif faces against the deployment asset origin,
///   so the page is set in the firm's typeface (see the module docs); and
/// - boot the support-chat widget on public pages, when the deployment carries
///   one ([`crate::chatwoot`]). This is the one thing the page renders from a
///   third-party origin, so the CSP widening travels with the injection rather
///   than being declared unconditionally: a deployment with no widget keeps a
///   byte-identical same-origin policy.
///
/// Only the HTML render response is rewritten; the wasm/glue assets pass
/// through untouched.
async fn dioxus_document_head(req: Request, next: Next) -> Response {
    // Navigator publishes one language. The `PageLayout` set `<html lang>`
    // and the Dioxus port has no `PageLayout`, so it is stamped onto the SSR
    // shell's opening `<html>` tag here — whether that shell is bare or already
    // carries the template's `lang="en"`.
    let lang = "en";
    // Captured before `next.run` consumes `req`: the footer below is gated on
    // the request path, not on anything the render produced.
    let path = req.uri().path().to_string();
    let response = next.run(req).await;

    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|content_type| content_type.starts_with("text/html"));
    if !is_html {
        return response;
    }

    let nonce = generate_nonce();
    let (mut parts, body) = response.into_parts();
    // A render that errors mid-stream or overruns the buffer is a real failure,
    // not a blank page: surface it as a 500 rather than a successful empty body.
    let Ok(bytes) = axum::body::to_bytes(body, MAX_RENDER_BYTES).await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    // Dioxus emits its hydration data as bare `<script>…</script>` elements; the
    // external wasm loader is `<script type="module" … src=…>` and is left
    // alone (it is same-origin, already allowed by `'self'`). Tagging only the
    // bare tag nonces exactly Dioxus's first-party inline scripts.
    // The faces go at the *end* of the head, not the start: `<meta charset>`
    // must stay within the document's first 1024 bytes, and a face block
    // carrying two absolute bucket URLs is large enough to push it out.
    let rendered = String::from_utf8_lossy(&bytes);
    let html = stamp_html_lang(&rendered, lang)
        .replace("<script>", &format!("<script nonce=\"{nonce}\">"))
        .replacen("</head>", &format!("{}</head>", *GORP_HEAD), 1);
    let html = if *SAMPLE_MATTERS {
        open_with_banner(&html, &SAMPLE_MATTERS_BANNER)
    } else {
        html
    };

    // The widget rides public pages only, while the authenticated `/app` and
    // `/lawyer` surfaces render `NavigatorShell` and are left alone, so the
    // pages that display a client's matter keep the strict same-origin policy.
    let chat = CHATWOOT.as_ref().filter(|_| is_public_page(&html));
    let html = match chat {
        Some(widget) => close_with_script(&html, &widget.script_tags()),
        None => html,
    };

    // The minimal `/app` footer — a centered copyright line, nothing else.
    // Gated on the request path rather than on the rendered shell: unlike the
    // public/authenticated split above, the eight real `/app` pages render
    // their navbar directly rather than through a shared `NavigatorShell`, so
    // there is no shell marker to key off. See `webapp::components::AppFooter`.
    let html = if renders_app_footer(&path) {
        close_with_script(&html, &APP_FOOTER)
    } else {
        html
    };

    if let Ok(csp) = HeaderValue::from_str(&csp_with_nonce(&nonce, crate::asset_csp_origin(), chat))
    {
        parts.headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    // The body length changed; drop any stale content-length so axum recomputes.
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(html))
}

/// Whether this deployment's matters are sample, read once at startup.
///
/// Read once rather than per request because it cannot change while the
/// process runs, and because a per-request `std::env::var` on the one
/// middleware that touches every HTML response is a cost paid on every page.
/// An unparsable value resolves to `false`: `web` boot already validates the
/// pair through `store::config::sample_matters` and fails loudly, so by the
/// time a request is served this can only be the value boot accepted.
static SAMPLE_MATTERS: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    store::DeploymentEnvironment::from_env()
        .and_then(|environment| {
            store::sample_matters(environment).map_err(|_| {
                store::DeploymentEnvironmentError::Invalid(String::from("sample-matters"))
            })
        })
        .unwrap_or(false)
});

/// This deployment's support-chat widget, resolved once at startup.
///
/// Read once for the same reason [`SAMPLE_MATTERS`] is: it cannot change while
/// the process runs, and this middleware touches every HTML response. `None`
/// is the default and the answer everywhere the deployment names no Chatwoot
/// inbox — local KIND and the staging release ring included.
static CHATWOOT: std::sync::LazyLock<Option<crate::chatwoot::ChatwootWidget>> =
    std::sync::LazyLock::new(crate::chatwoot::ChatwootWidget::from_env);

/// Whether this rendered document is a public page — the surface the
/// support-chat widget rides.
///
/// Decided from the shell the page rendered rather than from its path: the
/// public shell is what "public page" means, one binary serves both brand faces
/// through it, and a path allow-list would have to be extended by hand every
/// time a public route is added. The match is on the whole class attribute, not
/// a substring: the authenticated shell's root carries the same `nav-theme`
/// class in the other order.
fn is_public_page(html: &str) -> bool {
    html.contains(&format!(
        "class=\"{}\"",
        webapp::components::PUBLIC_SHELL_MARKER
    ))
}

/// Insert `script` as the last thing before `</body>`.
///
/// Last, not first: the support-chat widget this was written for is chrome
/// layered over a page that must already be readable without it, so it loads
/// after the document's own content rather than competing with first paint.
/// The same shape serves the `/app` footer's injection — despite the name,
/// the content inserted is an arbitrary HTML fragment, not necessarily a
/// `<script>`.
///
/// A document with no `</body>` is returned untouched, matching
/// [`open_with_banner`]: an HTML response that is a fragment rather than a
/// document has no body to close, and dropping the widget from it is better
/// than a 500.
fn close_with_script(html: &str, script: &str) -> String {
    match html.rfind("</body>") {
        Some(at) => format!("{}{script}{}", &html[..at], &html[at..]),
        None => html.to_string(),
    }
}

/// The rendered sample-matter banner, built once.
///
/// The component carries no props and no brand-dependent copy, so its markup
/// is the same on every page of both faces and there is nothing to re-render
/// per request.
static SAMPLE_MATTERS_BANNER: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(webapp::components::render_sample_matters_banner);

/// The rendered `/app` footer, built once.
///
/// `views::brand::FIRM_BRAND` is a process-wide constant, so — like
/// [`SAMPLE_MATTERS_BANNER`] — there is nothing to re-render per request.
static APP_FOOTER: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(webapp::components::render_app_footer);

/// Whether the rendered document is an authenticated `/app` page, which is
/// what [`APP_FOOTER`] renders onto.
///
/// Decided from the request path rather than the rendered shell: unlike
/// [`is_public_page`], the eight real `/app` pages render `AppNavbar` and
/// their own `main` directly rather than through a shared `NavigatorShell`
/// (issue tracked, not yet adopted), so there is no shell marker in the HTML
/// to key off. `/app/` is a prefix, not an exact match, so it covers every
/// page this middleware layers onto — `/app/projects`, `/app/projects/{code}`,
/// `/app/team`, and the rest — without naming each one.
fn renders_app_footer(path: &str) -> bool {
    path.starts_with("/app/")
}

/// Insert `banner` as the first child of the document body.
///
/// First child, not last: it is an advisory about everything below it, and a
/// reader who meets it after the page has nothing left to warn them about.
/// That position is safe for hydration because `dioxus-web` resolves its mount
/// by `document.getElementById("main")` rather than by walking the body's
/// children, so a preceding sibling shifts nothing it looks at.
///
/// A document with no `<body>` is returned untouched. That is not a case worth
/// failing on: an HTML response that is a fragment rather than a document has
/// no body to open, and dropping the banner from it is better than a 500.
fn open_with_banner(html: &str, banner: &str) -> String {
    let Some(start) = html.find("<body") else {
        return html.to_string();
    };
    let Some(end) = html[start..].find('>').map(|offset| start + offset + 1) else {
        return html.to_string();
    };
    format!("{}{banner}{}", &html[..end], &html[end..])
}

/// The GORP Serif head fragment, built once from the process asset origin: a
/// preload for the reading face (so first paint is not a fallback-serif flash)
/// and the two `@font-face` declarations `theme.css`'s `--nav-font-family`
/// resolves against. `views` owns the declaration text consumed by the Dioxus
/// browser surface.
static GORP_HEAD: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    gorp_head_fragment(
        &views::assets::asset_url("fonts/gorp-serif/GORPSerif-Regular.woff2"),
        &views::assets::asset_url("fonts/gorp-serif/GORPSerif-Bold.woff2"),
    )
});

/// Pure builder behind [`GORP_HEAD`], so tests exercise every asset-origin
/// shape without stomping the process-wide env var. The preload `href` is
/// HTML-escaped; the stylesheet body arrives already CSS-string-escaped from
/// `views::layout`.
fn gorp_head_fragment(regular_url: &str, bold_url: &str) -> String {
    let faces = views::assets::gorp_font_face_css(regular_url, bold_url);
    format!(
        "<link rel=\"preload\" as=\"font\" type=\"font/woff2\" crossorigin href=\"{}\"><style>{faces}</style>",
        webapp::html_escape::escape_attr(regular_url),
    )
}

/// Cap on the buffered render size — the demo page is a few KB; this is a
/// generous ceiling that still refuses a pathological body.
const MAX_RENDER_BYTES: usize = 4 * 1024 * 1024;

/// A fresh 128-bit base64 nonce for one response.
fn generate_nonce() -> String {
    let bytes: [u8; 16] = rand::random();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The route-scoped CSP for the rendered Dioxus page: same-origin everything,
/// plus the per-response nonce for Dioxus's inline hydration scripts and
/// `'wasm-unsafe-eval'` for the client bundle. No CDN host, no `'unsafe-inline'`
/// for scripts.
///
/// `asset_origin` is the deployment's `NAVIGATOR_ASSET_BASE_URL` origin when it
/// is off-origin (`None` for the same-origin `/public` default). It widens
/// `img-src`, `font-src`, and `media-src` exactly as the site-wide policy does,
/// because the licensed GORP faces this route declares live there — a
/// `font-src 'self'` would block them and silently drop the page back to a
/// fallback serif. `media-src` matters here in particular: Catalog slides render
/// through this route, so a slide's video is governed by this policy and not the
/// site-wide one.
///
/// `chat` is the support-chat widget when this response carries one, and is the
/// only thing that admits a third-party script origin. It widens four
/// directives, because the widget needs all four to work rather than to merely
/// appear: `script-src` for the vendor `sdk.js`, `frame-src` for the
/// conversation iframe, `img-src` for agent avatars and attachment thumbnails,
/// and `connect-src` for the socket that delivers replies. The last is the one a
/// partial allowance gets wrong — `connect-src` is not declared at all
/// otherwise, so it inherits `default-src 'self'` and a bubble that opens stays
/// permanently silent.
fn csp_with_nonce(
    nonce: &str,
    asset_origin: Option<String>,
    chat: Option<&crate::chatwoot::ChatwootWidget>,
) -> String {
    let asset_extra = asset_origin
        .map(|origin| format!(" {origin}"))
        .unwrap_or_default();
    // Appended to `img-src` and `script-src`, which are declared either way.
    let chat_extra = chat
        .map(|widget| format!(" {}", widget.origin()))
        .unwrap_or_default();
    // `connect-src` and `frame-src` are named only when there is a widget, so a
    // deployment without one emits exactly the policy it emitted before the
    // widget existed rather than two directives restating `default-src`.
    let chat_directives = chat
        .map(|widget| {
            format!(
                "; connect-src 'self' {origin} {socket}; frame-src 'self' {origin}",
                origin = widget.origin(),
                socket = widget.websocket_origin(),
            )
        })
        .unwrap_or_default();
    format!(
        "default-src 'self'; base-uri 'self'; object-src 'none'; \
         frame-ancestors 'none'; img-src 'self' data:{asset_extra}{chat_extra}; \
         font-src 'self'{asset_extra}; media-src 'self'{asset_extra}; \
         style-src 'self' 'unsafe-inline'; \
         script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval'{chat_extra}; \
         form-action 'self'{chat_directives}"
    )
}

/// Reject a `?sort=` that targets a field the people page does not advertise,
/// returning `400` before the Dioxus render runs — preserving the JSON:API
/// `SortSpec::validated` contract (the people page advertises `name` and
/// `email`). Layered onto the `/admin/people` Dioxus route by
/// [`admin_people_router`].
async fn reject_unadvertised_sort(request: Request, next: Next) -> Response {
    use std::collections::{HashMap, HashSet};

    let params = axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|query| query.0)
        .unwrap_or_default();
    let allowed: HashSet<&str> = ["name", "email"].into_iter().collect();
    match views::components::SortSpec::parse(params.get("sort").map(String::as_str))
        .validated(&allowed)
    {
        Ok(_) => next.run(request).await,
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// Derive the viewer's tier from the request session (inserted by
/// [`crate::policy::require_policy`], which this layer sits inside) and stash it
/// in the request extensions for a page's `#[server]` function, so the
/// server-rendered page shows the same role-appropriate nav chrome the native
/// route carried. Absent a session it defaults to the least-privileged tier.
async fn inject_viewer_role(mut req: Request, next: Next) -> Response {
    let role = req
        .extensions()
        .get::<crate::session::SessionData>()
        .map_or(webapp::people::ViewerRole::Client, |session| {
            viewer_role(session.role)
        });
    req.extensions_mut().insert(role);
    next.run(req).await
}

/// Inject the deploy's brand identity as the wasm-safe
/// [`webapp::app_chrome::AppBrandMark`] request extension, so an `/app` page's
/// navbar renders the mounted brand's logo and its copy names the mounted firm.
///
/// Resolved here, on the request task, where the brand `tokio::task_local`
/// (`scope_branding`) is live: a Dioxus server function runs on a task that does
/// not inherit it, so `app_logo_from_context` resolving the brand itself would
/// publish the DEFAULT mark under a mounted white-label bundle — the same reason
/// [`inject_public_utility`] resolves the public chrome here rather than there.
async fn inject_app_brand_mark(mut req: Request, next: Next) -> Response {
    req.extensions_mut()
        .insert(webapp::app_chrome::firm_brand_mark());
    next.run(req).await
}

/// Inject the session's linked `persons.id` as the wasm-safe
/// [`webapp::portal_project_list::PersonId`] request extension, so a Dioxus
/// portal `#[server]` function can scope rows to the signed-in person — the id
/// lives on `portal`'s `SessionData`, which `webapp` cannot see. `None` when the
/// session has no linked person (fail-closed: the loader then sees nothing).
async fn inject_person_id(mut req: Request, next: Next) -> Response {
    let person_id = req
        .extensions()
        .get::<crate::session::SessionData>()
        .and_then(|session| session.person_id)
        .map(|id| id.to_string());
    req.extensions_mut()
        .insert(webapp::portal_project_list::PersonId(person_id));
    next.run(req).await
}

/// Inject the session's impersonation state as the wasm-safe
/// [`webapp::components::ImpersonationView`] request extension, so a Dioxus page
/// can render the banner that says who the viewer is acting as and offer the way
/// out. `SessionData` lives in `portal` and `webapp` cannot see it, so the
/// already-decided values cross as their own type — a component never infers
/// that a session is impersonating.
///
/// The extension is always inserted; `None` is the ordinary case and renders no
/// banner.
async fn inject_impersonation(mut req: Request, next: Next) -> Response {
    let view = req
        .extensions()
        .get::<crate::session::SessionData>()
        .and_then(|session| {
            session
                .impersonation
                .as_ref()
                .map(|i| webapp::components::ImpersonationView {
                    target_name: i.target_name.clone(),
                    target_email: i.target_email.clone(),
                    csrf_token: session.csrf_token.clone(),
                })
        });
    req.extensions_mut()
        .insert(webapp::components::Impersonating(view));
    next.run(req).await
}

/// Inject the session's CSRF token as the wasm-safe [`webapp::csrf::CsrfToken`]
/// request extension, so a Dioxus CRUD page's `#[server]` function can thread it
/// into its native `POST` action forms — the token itself lives on `portal`'s
/// `SessionData`, which `webapp` cannot see. Empty when there is no session.
async fn inject_csrf_token(mut req: Request, next: Next) -> Response {
    let token = req
        .extensions()
        .get::<crate::session::SessionData>()
        .map_or_else(String::new, |session| session.csrf_token.clone());
    req.extensions_mut().insert(webapp::csrf::CsrfToken(token));
    next.run(req).await
}

/// Build a public page's auth-aware header utility links, matching the
/// navbar's auth block (`views::layout`): for a signed-in viewer, Portal plus
/// the role-gated Clerk / Lawyer / Admin workspace links and Sign out; for an
/// anonymous visitor, Sign in. Labels are resolved in `locale` (the app-route
/// hrefs — `/app/projects`, `/auth/login` — are not localized). Pure, so the
/// role→links mapping is unit-tested directly.
///
/// Sign in is offered on every property, not just the firm's own host: every
/// property signs into the one Navigator portal, so a visitor who lands on a
/// white-label tenant still needs a door in.
fn public_utility_links(
    session: Option<&crate::session::SessionData>,
) -> Vec<webapp::public_chrome::ChromeNavLink> {
    let link = |label: &str, href: &str| webapp::public_chrome::ChromeNavLink {
        label: label.to_string(),
        href: href.to_string(),
    };
    if session.is_none() {
        return vec![link("Sign in", "/auth/login")];
    }
    // One destination for every tier: the matter surface. The nav used to name
    // the viewer — Clerk, Lawyer, Admin — which is the same mistake the URL
    // prefixes made. What a caller can do is decided by their role at the
    // handler; it does not need to be advertised as a separate door.
    vec![
        link("Projects", "/app/projects"),
        link("Sign out", "/auth/logout"),
    ]
}

/// Stamp `lang` onto the opening `<html>` tag of an SSR document, making that
/// tag the single source of the document language.
///
/// The whole opening tag is rewritten so the stamp lands whatever the SSR
/// template emitted: dioxus-server's `ServeConfig::new` falls back to an
/// `ssr_only` default whose shell is a bare `<html>` (what the unit and firm
/// tests exercise), but the bundled `index.html` (`webapp/index.html`, served
/// in production and by the `web` SSR tests via `DIOXUS_PUBLIC_PATH`) opens with
/// `<html lang="en">`. A literal `<html>` search matches only the former, so
/// rewriting the tag covers both. The template carries no other `<html>`
/// attributes, so none are lost.
fn stamp_html_lang(html: &str, lang: &str) -> String {
    let Some(start) = html.find("<html") else {
        return html.to_string();
    };
    let Some(end) = html[start..].find('>').map(|offset| start + offset + 1) else {
        return html.to_string();
    };
    format!("{}<html lang=\"{lang}\">{}", &html[..start], &html[end..])
}

/// Inject the public page's auth-aware header utility links (see
/// [`public_utility_links`]) as wasm-safe request extensions, so a public page's
/// `#[server]` view function renders the signed-in overlay the navbar
/// carried. Anonymous requests resolve to the Sign-in link, so the marketing
/// pages stay anonymous.
async fn inject_public_utility(mut req: Request, next: Next) -> Response {
    let utility = public_utility_links(req.extensions().get::<crate::session::SessionData>());
    // Resolve the full public chrome here, on the request task, where the brand
    // `tokio::task_local` (`scope_branding`) is live: a Dioxus server-fn runs on
    // a task that does not inherit it, so `firm_public_chrome_from_context`
    // building the chrome there would render the DEFAULT brand under a mounted
    // white-label bundle (the header logo, wordmark, and footer). Inject the
    // resolved chrome for the server-fn to read back.
    req.extensions_mut()
        .insert(webapp::public_chrome::firm_public_chrome(utility.clone()));
    req.extensions_mut()
        .insert(webapp::public_chrome::PublicUtility(utility));
    next.run(req).await
}

/// Map the session's system role onto the wasm-safe tier the Dioxus page reads.
fn viewer_role(role: store::persons::Role) -> webapp::people::ViewerRole {
    use store::persons::Role;
    match role {
        Role::Owner => webapp::people::ViewerRole::Owner,
        Role::Admin => webapp::people::ViewerRole::Admin,
        Role::Lawyer => webapp::people::ViewerRole::Lawyer,
        Role::Clerk => webapp::people::ViewerRole::Clerk,
        Role::Client => webapp::people::ViewerRole::Client,
    }
}

/// The matter list. One path for every tier; the page picks the firm or client
/// lens from the caller's role. `/app/projects/new` (create) stays on the
/// router.
pub const PROJECTS_PATH: &str = "/app/projects";

/// Reject a `?sort=` targeting a field the projects list does not advertise (it
/// advertises `code` / `name` / `status` / `entity_name`), returning `400`
/// before the render runs — the JSON:API `SortSpec::validated` contract.
async fn reject_unadvertised_projects_sort(request: Request, next: Next) -> Response {
    use std::collections::{HashMap, HashSet};
    let Ok(params) = axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|query| query.0)
    else {
        return (StatusCode::BAD_REQUEST, "malformed query string").into_response();
    };
    let allowed: HashSet<&str> = ["code", "name", "status", "entity_name"]
        .into_iter()
        .collect();
    match views::components::SortSpec::parse(params.get("sort").map(String::as_str))
        .validated(&allowed)
    {
        Ok(_) => next.run(request).await,
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// The gated Dioxus matter list — one mount for every tier, and the client's
/// post-login landing.
///
/// It carries the union of what the two former mounts needed: the sort
/// pre-handler (the `400` contract the sortable firm directory advertises), the
/// injected `person_id` (to scope the visible matters), and impersonation, on
/// top of the auth + embedded Rego policy gate and nonce CSP.
///
/// The render adapts to the caller's tier (a client sees their own matters, a
/// firm tier the workbench), so the retired `/portal` client landing folded into
/// this one mount rather than needing a second, client-pinned copy.
pub fn projects_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    let page = || {
        get(render_handler)
            .layer(from_fn(inject_viewer_role))
            .layer(from_fn(inject_app_brand_mark))
            .layer(from_fn(inject_person_id))
            .layer(from_fn(inject_impersonation))
            .layer(from_fn(dioxus_document_head))
            .layer(from_fn(reject_unadvertised_projects_sort))
    };

    Router::<FullstackState>::new()
        .route(PROJECTS_PATH, page())
        .with_state(FullstackState::new(cfg, webapp::matter_surface::Projects))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The firm dashboard. Moved off `/lawyer` onto `/app/lawyer` with the rest of
/// the collapse; the tier gate is the handler's, not the prefix's. Its
/// `?sort=`/`?dir=`/`?status=`/`?page=` query is deliberately lenient (an
/// unrecognised value falls back to the default), so unlike the sortable
/// listings this route carries no `400` pre-handler.
pub const LAWYER_DASHBOARD_PATH: &str = "/app/lawyer";

/// The gated Dioxus lawyer workbench (#956 Phase 4). Shaped like
/// [`projects_router`] — it carries the injected `person_id` so the
/// `#[server]` loader scopes the matter counts and list to the caller's
/// workload — but without the sort pre-handler, because this page's sort query
/// is lenient rather than validated.
pub fn lawyer_dashboard_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_DASHBOARD_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_impersonation))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::lawyer_dashboard::LawyerDashboard,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The blank government-forms index (#956 Phase 4). The download route under it
/// (`/app/forms/{file}`) stays Axum-side; axum routes it by its deeper path.
pub const APP_FORMS_PATH: &str = "/app/forms";

/// The gated Dioxus blank-forms index. The vendored-forms registry lives on
/// `portal`'s router state as `Arc<Vec<forms::FormMeta>>`, which `webapp` cannot
/// see, so it is shaped once here and injected as
/// [`webapp::gov_forms::GovFormRows`] — the same seam the other app state
/// uses. The registry is process-lifetime data, so shaping it per request costs
/// a clone of a handful of short strings.
pub fn app_forms_router(
    surreal: store::surreal::SurrealDb,
    forms_registry: &[forms::FormMeta],
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    let rows: Vec<webapp::gov_forms::GovFormRow> = forms_registry
        .iter()
        .map(|f| webapp::gov_forms::GovFormRow {
            code: f.code.to_string(),
            title: f.title.to_string(),
            jurisdiction: f.jurisdiction.to_string(),
            origin_url: f.origin_url.to_string(),
        })
        .collect();
    let rows = webapp::gov_forms::GovFormRows(rows);

    Router::<FullstackState>::new()
        .route(
            APP_FORMS_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_impersonation))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(cfg, webapp::gov_forms::GovForms))
        .layer(axum::Extension(rows))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The matter-detail path. One page for every tier — the firm workbench and
/// the client view of the same matter — with the lens picked from the caller's
/// role. The mutation routes under it (`/contract-review`, `/documents/*`,
/// `/review/*`, `/conversation`, `/approve-plan`, …) and the edit-save `POST`
/// on this path stay on the router; axum merges the same-path methods and
/// routes the deeper paths.
pub const PROJECT_DETAIL_PATH: &str = "/app/projects/{project_code}";

/// Compute the estate "Approve my plan" decision for the matter in the request
/// path and inject it as [`webapp::portal_project_detail::ShowApprovePlan`]. The
/// decision is estate/`workflows`-specific — a transcript-driven notation parked
/// at `client_review` with every released draft still awaiting the client — and
/// lives in `crate::estate`, which `webapp` cannot see, so the portal router
/// resolves it here and injects the plain boolean the Dioxus page renders. A
/// missing project code or any read failure yields `false` (the control hides);
/// the `#[server]` loader is the authority on visibility and the 404.
async fn inject_show_approve_plan(
    axum::extract::State(surreal): axum::extract::State<store::surreal::SurrealDb>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let show = match project_id_from_path(&surreal, &path).await {
        Some(id) => show_approve_plan(&surreal, id).await,
        None => false,
    };
    req.extensions_mut()
        .insert(webapp::portal_project_detail::ShowApprovePlan(show));
    next.run(req).await
}

/// The estate "Approve my plan" predicate the `portal::projects::detail`
/// computed inline: a transcript-driven notation is parked at `client_review`
/// and every released review draft is still awaiting the client (none approved
/// yet — approve once).
async fn show_approve_plan(surreal: &store::surreal::SurrealDb, project_id: uuid::Uuid) -> bool {
    let awaiting_client = crate::estate::transcript_driven_notation(surreal, project_id)
        .await
        .is_some_and(|n| n.state == "client_review");
    if !awaiting_client {
        return false;
    }
    let review_docs = store::review_documents::client_visible_for_project(surreal, project_id)
        .await
        .unwrap_or_default();
    !review_docs.is_empty()
        && review_docs
            .iter()
            .all(|d| d.status == store::review_documents::STATUS_PENDING_REVIEW)
}

/// The gated Dioxus client portal matter-detail page (#641 Phase 3). Like
/// [`projects_router`] but for a single matter: it carries the matter id
/// in the path, injects the CSRF token (the approve-plan form) and the estate
/// approve-plan decision, and provides the object-store handle into the render
/// context (the loader probes which of each notation's PDFs exist), on top of
/// the auth + embedded Rego policy gate, person-id scoping, and nonce CSP.
pub fn project_detail_router(
    surreal: store::surreal::SurrealDb,
    storage: std::sync::Arc<dyn cloud::StorageService>,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let detail_stores = surreal.clone();
    let estate_stores = surreal.clone();
    let repository_surreal = surreal.clone();
    // Both lenses render from this one mount, so it provides the union of what
    // either needs. `storage` is the client view's; the firm view ignores it.
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
        Box::new(move || Box::new(storage.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            PROJECT_DETAIL_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn_with_state(detail_stores, inject_show_approve_plan))
                .layer(from_fn_with_state(
                    repository_surreal,
                    inject_project_repository_pointer,
                ))
                .layer(from_fn_with_state(estate_stores, inject_lawyer_estate))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::matter_surface::ProjectDetail,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// Resolve the trailing project code of `/app/projects/{code}` to its internal id.
async fn project_id_from_path(
    surreal: &store::surreal::SurrealDb,
    path: &str,
) -> Option<uuid::Uuid> {
    let code = path.rsplit('/').next().filter(|seg| !seg.is_empty())?;
    store::projects::find_by_code(surreal, code)
        .await
        .ok()
        .flatten()
        .map(|project| project.id)
}

pub(crate) async fn project_show_path(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
) -> String {
    store::projects::find_by_id(surreal, project_id)
        .await
        .ok()
        .flatten()
        .map_or_else(
            || "/app/projects".to_string(),
            |project| format!("/app/projects/{}", project.code),
        )
}

/// Resolve this matter's recorded repository URL and inject it as
/// [`webapp::lawyer_project_detail::ProjectRepositoryPointer`].
///
/// This has no forge client and composes nothing: the URL is stored on the
/// Project, so it may name any forge and any organization. A missing id, a
/// missing matter, or a Project with no repository recorded leaves the pointer
/// absent rather than failing the lawyer page. Navigator never verifies the
/// URL — it provisions no repositories, so the target may not exist.
async fn inject_project_repository_pointer(
    axum::extract::State(surreal): axum::extract::State<store::surreal::SurrealDb>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let pointer = match project_id_from_path(&surreal, &path).await {
        Some(id) => project_repository_pointer(&surreal, id).await,
        None => None,
    };
    req.extensions_mut()
        .insert(webapp::lawyer_project_detail::ProjectRepositoryPointer(
            pointer,
        ));
    next.run(req).await
}

/// One Project, one repository, recorded as a whole URL on the Project.
async fn project_repository_pointer(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
) -> Option<String> {
    store::projects::find_by_id(surreal, project_id)
        .await
        .ok()??
        .repository_url
}

/// Compute the transcript-driven estate view (if any) for the matter and inject
/// it as [`webapp::lawyer_project_detail::LawyerEstate`]. The detection is
/// `workflows`/estate-coupled and lives in `crate::estate`, which `webapp`
/// cannot see, so the portal router resolves it here.
async fn inject_lawyer_estate(
    axum::extract::State(surreal): axum::extract::State<store::surreal::SurrealDb>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let estate = match project_id_from_path(&surreal, &path).await {
        Some(id) => lawyer_estate(&surreal, id).await,
        None => None,
    };
    req.extensions_mut()
        .insert(webapp::lawyer_project_detail::LawyerEstate(estate));
    next.run(req).await
}

async fn lawyer_estate(
    surreal: &store::surreal::SurrealDb,
    project_id: uuid::Uuid,
) -> Option<webapp::lawyer_project_detail::EstateData> {
    let (notation_id, state) = crate::estate::transcript_driven_notation(surreal, project_id)
        .await
        .map(|n| (n.id, n.state))?;
    let drafts = store::review_documents::for_notation(surreal, notation_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|d| webapp::lawyer_project_detail::EstateDraft {
            title: d.title,
            kind: d.kind,
            status: d.status,
        })
        .collect();
    Some(webapp::lawyer_project_detail::EstateData {
        notation_id: notation_id.to_string(),
        state,
        drafts,
    })
}

/// The lawyer matter-open form path (#956 Phase 4) — `Add project`, plus the two
/// inline "New entity" / "New client" creates that share the page. All three
/// `POST`s (`/app/projects`, `…/new/entity`, `…/new/client`) stay on the
/// admin router as native form handlers; axum routes them by method and path.
pub const LAWYER_PROJECT_NEW_PATH: &str = "/app/projects/new";

/// The lawyer descriptive matter-edit form path (#956 Phase 4). `POST
/// /app/projects/{project_code}` (the save) stays on the admin router.
pub const LAWYER_PROJECT_EDIT_PATH: &str = "/app/projects/{project_code}/edit";

/// The add-participation form path (#956 Phase 4) — admin-only. `POST
/// /app/projects/{project_code}/people` (the create) stays on the admin router.
pub const LAWYER_PARTICIPATION_NEW_PATH: &str = "/app/projects/{project_code}/people/new";

/// The edit-participation form path (#956 Phase 4) — admin-only. The `POST` on
/// this same path (the update) stays on the admin router; axum merges the
/// same-path methods.
pub const LAWYER_PARTICIPATION_EDIT_PATH: &str =
    "/app/projects/{project_code}/people/{role_id}/edit";

/// One filed matter document. The `…/download` route under it stays on the
/// router. The tier decides the lens, so an `internal` asset is still not found
/// for a client — the guard moved from the mount into the loader.
pub const PROJECT_DOCUMENT_PATH: &str = "/app/projects/{project_code}/documents/{doc_id}";

/// The gated Dioxus lawyer project forms (#956 Phase 4): matter-open, the
/// descriptive edit, and the two participation forms. Each is a native `POST`
/// carrying the session CSRF token, so they all mount through the shared
/// [`csrf_page_router`].
pub fn lawyer_project_forms_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    csrf_page_router(
        LAWYER_PROJECT_NEW_PATH,
        webapp::project_new::LawyerProjectNew,
        surreal.clone(),
        sessions.clone(),
        policy.clone(),
        auth.clone(),
    )
    .merge(csrf_page_router(
        LAWYER_PROJECT_EDIT_PATH,
        webapp::project_edit::LawyerProjectEdit,
        surreal.clone(),
        sessions.clone(),
        policy.clone(),
        auth.clone(),
    ))
    .merge(csrf_page_router(
        LAWYER_PARTICIPATION_NEW_PATH,
        webapp::project_participation::LawyerParticipationNew,
        surreal.clone(),
        sessions.clone(),
        policy.clone(),
        auth.clone(),
    ))
    .merge(csrf_page_router(
        LAWYER_PARTICIPATION_EDIT_PATH,
        webapp::project_participation::LawyerParticipationEdit,
        surreal,
        sessions,
        policy,
        auth,
    ))
}

/// Build a gated Dioxus router for one filed-document page. Read-only (no CSRF
/// token), but person-scoped: the loader runs `can_see_project` against the
/// session's linked `persons.id`, so a lawyer off the matter and a client
/// on someone else's matter both see nothing.
fn project_document_page_router<C, M>(
    path: &'static str,
    component: C,
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router
where
    C: dioxus_core::ComponentFunction<(), M> + Send + Sync + 'static,
    M: 'static,
{
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_person_id))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(cfg, component))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The gated Dioxus filed-document page — one mount. A client still cannot
/// reach the firm view by rewriting a URL; they would have to change their tier,
/// which is not something a request can assert.
pub fn project_document_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    project_document_page_router(
        PROJECT_DOCUMENT_PATH,
        webapp::project_document_detail::ProjectDocument,
        surreal,
        sessions,
        policy,
        auth,
    )
}

/// The matter conversation path — one mount for both sides of the thread. The
/// loader scopes it by the caller's tier (firm-internal notes are firm-only),
/// which is a fact about the person, not about the URL they typed.
pub const CONVERSATION_PATH: &str = "/app/projects/{project_code}/conversation";

/// The gated Dioxus matter conversation page. The `POST
/// …/conversation/messages` handler stays on Axum because it already redirects
/// for a native form using PRG.
pub fn conversation_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    let gated = || {
        get(render_handler)
            .layer(from_fn(inject_viewer_role))
            .layer(from_fn(inject_app_brand_mark))
            .layer(from_fn(inject_person_id))
            .layer(from_fn(inject_csrf_token))
            .layer(from_fn(dioxus_document_head))
    };

    Router::<FullstackState>::new()
        .route(CONVERSATION_PATH, gated())
        .with_state(FullstackState::new(cfg, webapp::conversation::Conversation))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The client self-serve intake path (#956 Phase 4) — the magic-link surface
/// where a client answers the client-facing questions on a notation. The save
/// (`POST` on the same path) stays on the existing handler, which now redirects
/// back here; axum merges the two same-path method routes.
pub const PORTAL_INTAKE_PATH: &str = "/app/projects/{project_code}/intake/{notation_id}";

/// Resolve the client's current intake step and inject it as
/// [`webapp::client_intake::InjectedIntake`].
///
/// Resolving a step means calling `workflows::notation_session`, which `webapp`
/// does not depend on, so the work happens here and only the wasm-safe result
/// crosses — the same seam [`inject_show_approve_plan`] uses. This layer also
/// owns the refusal: an unknown or unauthorised notation gets the `404` here
/// (never a `403`, which would confirm the matter exists) and never reaches the
/// render.
async fn inject_client_intake(
    axum::extract::State(state): axum::extract::State<crate::admin::AdminState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some((project_code, notation_id)) = intake_ids_from_path(&req) else {
        return crate::intake::client_intake_not_found();
    };
    let Some(project_id) = store::projects::id_for_code(&state.surreal, &project_code).await else {
        return crate::intake::client_intake_not_found();
    };
    let session = req
        .extensions()
        .get::<crate::session::SessionData>()
        .cloned();
    match crate::intake::resolve_intake_state(&state, session.as_ref(), project_id, notation_id)
        .await
    {
        Ok(intake) => {
            req.extensions_mut()
                .insert(webapp::client_intake::InjectedIntake(intake));
            next.run(req).await
        }
        Err(response) => response,
    }
}

/// Parse `(project_code, notation_id)` out of
/// `/app/projects/{project_code}/intake/{notation_id}`. The layer runs before
/// the route's own path extraction, so it reads the segments itself.
///
/// The matter arrives as its code, not its row id — the client follows this
/// link from an email and may well read it aloud. Turning that code into an id
/// is `project_code_path::resolve`'s job, and it happens in the caller, which
/// has the store handle this parser does not.
fn intake_ids_from_path(req: &Request) -> Option<(String, uuid::Uuid)> {
    let mut segments = req.uri().path().rsplit('/');
    let notation_id = segments.next()?.parse().ok()?;
    let _intake = segments.next()?;
    let project_code = segments.next()?;
    Some((project_code.to_string(), notation_id))
}

/// The gated Dioxus client self-serve intake page (#956 Phase 4). Client-lens
/// and write-bearing (the answer form posts back), so it carries the CSRF token
/// on top of the auth + embedded Rego policy gate and nonce CSP, plus the step resolver above.
pub fn client_intake_router(
    state: crate::admin::AdminState,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let surreal = state.surreal.clone();
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            PORTAL_INTAKE_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn_with_state(state, inject_client_intake))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::client_intake::ClientIntake,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The per-notation clause editor path (#956 Phase 4) — the custom paragraphs
/// spliced into one matter's assembled document. Every mutation (`POST` on this
/// path and on `…/{cid}/edit|move|delete`) stays on the existing handlers, which
/// already redirect back here.
pub const LAWYER_CLAUSES_PATH: &str = "/lawyer/notations/{id}/clauses";

/// Answer `?format=json` on the clause-editor path before the render runs.
///
/// The same path serves the lawyer HTML editor and a thin JSON list the
/// `navigator retainer clause list` CLI consumes. axum cannot register two `GET`
/// handlers on one path, so the JSON branch runs as this pre-layer and the
/// Dioxus render is what happens when the query is absent.
async fn clause_editor_json_or_render(
    axum::extract::State(state): axum::extract::State<crate::admin::AdminState>,
    request: Request,
    next: Next,
) -> Response {
    let format = axum::extract::Query::<std::collections::HashMap<String, String>>::try_from_uri(
        request.uri(),
    )
    .ok()
    .and_then(|query| query.0.get("format").cloned())
    .unwrap_or_default();
    let Some(notation_id) = notation_id_from_path(&request) else {
        return next.run(request).await;
    };
    match crate::clauses::clauses_json(&state, notation_id, &format).await {
        Some(response) => response,
        None => next.run(request).await,
    }
}

/// Parse the notation id out of `/lawyer/notations/{id}/<leaf>`. These layers run
/// before the route's own path extraction, so they read the segment themselves.
fn notation_id_from_path(req: &Request) -> Option<uuid::Uuid> {
    req.uri()
        .path()
        .rsplit('/')
        .nth(1)
        .and_then(|seg| seg.parse().ok())
}

/// The gated Dioxus clause editor (#956 Phase 4). Write-bearing (add / edit /
/// reorder / delete all post from here), so it carries the CSRF token on top of
/// the auth + embedded Rego policy gate and nonce CSP, plus the `?format=json` pre-layer above.
pub fn clause_editor_router(
    state: crate::admin::AdminState,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let surreal = state.surreal.clone();
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_CLAUSES_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn_with_state(state, clause_editor_json_or_render)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::clause_editor::ClauseEditor,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The lawyer questionnaire walker path (#956 Phase 4) — one question at a time
/// on a notation. The answer `POST` on the same path stays on the existing
/// handler, which already redirects; axum merges the two method routes.
pub const LAWYER_WALKER_STEP_PATH: &str = "/lawyer/notations/{id}/step";

/// Resolve the current walker step and inject it as
/// [`webapp::walker_step::InjectedWalkerStep`].
///
/// Resolving a step means reading the questionnaire runtime through
/// `workflows::notation_session`, which `webapp` does not depend on, so the work
/// happens here and only the wasm-safe result crosses. This layer also returns
/// instead of rendering for the three non-page outcomes: the `?format=json` CLI
/// surface on this same path, the redirect once the questionnaire is complete,
/// and the `404` for a notation that is gone.
async fn inject_walker_step(
    axum::extract::State(state): axum::extract::State<crate::admin::AdminState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(notation_id) = notation_id_from_path(&req) else {
        return (StatusCode::NOT_FOUND, "notation not found").into_response();
    };
    let format =
        axum::extract::Query::<std::collections::HashMap<String, String>>::try_from_uri(req.uri())
            .ok()
            .and_then(|query| query.0.get("format").cloned());
    match crate::retainer_walk::resolve_walker_step(&state, notation_id, format.as_deref()).await {
        Ok(step) => {
            req.extensions_mut()
                .insert(webapp::walker_step::InjectedWalkerStep(step));
            next.run(req).await
        }
        Err(response) => response,
    }
}

/// The gated Dioxus lawyer walker step (#956 Phase 4). Write-bearing (the answer
/// form and the client hand-off both post from here), so it carries the CSRF
/// token on top of the auth + embedded Rego policy gate and nonce CSP, plus the step resolver
/// above.
pub fn walker_step_router(
    state: crate::admin::AdminState,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let surreal = state.surreal.clone();
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_WALKER_STEP_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn_with_state(state, inject_walker_step)),
        )
        .with_state(FullstackState::new(cfg, webapp::walker_step::WalkerStep))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The notation review-and-send path (#956 Phase 4) — where an attorney reads
/// the assembled document and decides. Every action on it (`approve-send`,
/// `send`, `request-changes`) stays a `POST` on its own path and redirects back
/// here.
pub const LAWYER_INTAKE_REVIEW_PATH: &str = "/lawyer/notations/{id}/review";

/// Resolve the review screen and inject it as
/// [`webapp::intake_review::InjectedIntakeReview`].
///
/// Assembling the document means reaching `workflows` and object storage, which
/// `webapp` does not depend on, so the work happens here and only the wasm-safe
/// result crosses. This layer also returns instead of rendering for the
/// `?format=json` status surface `navigator notation status` reads on this same
/// path, and for the `404`.
async fn inject_intake_review(
    axum::extract::State(state): axum::extract::State<crate::admin::AdminState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(notation_id) = notation_id_from_path(&req) else {
        return (StatusCode::NOT_FOUND, "notation not found").into_response();
    };
    let format =
        axum::extract::Query::<std::collections::HashMap<String, String>>::try_from_uri(req.uri())
            .ok()
            .and_then(|query| query.0.get("format").cloned());
    match crate::retainer_walk::resolve_intake_review(&state, notation_id, format.as_deref()).await
    {
        Ok(data) => {
            req.extensions_mut()
                .insert(webapp::intake_review::InjectedIntakeReview(data));
            next.run(req).await
        }
        Err(response) => response,
    }
}

/// The gated Dioxus notation review-and-send screen (#956 Phase 4). This is the
/// page a **binding** envelope goes out from, so it carries the CSRF token for
/// its approve / send / request-changes forms on top of the auth + embedded Rego policy gate and
/// nonce CSP, plus the resolver above.
pub fn intake_review_router(
    state: crate::admin::AdminState,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let surreal = state.surreal.clone();
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_INTAKE_REVIEW_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn_with_state(state, inject_intake_review)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::intake_review::IntakeReview,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The lawyer-on-behalf re-ask path (#956 Phase 4) — re-collect the answers a
/// review flagged. The save `POST` on the same path stays on the existing
/// handler, which already redirects; axum merges the two method routes.
pub const LAWYER_REASK_PATH: &str = "/lawyer/notations/{id}/reask";

/// Resolve the re-ask screen and inject it as [`webapp::reask::InjectedReask`].
///
/// Reading the change request and resolving each flagged code's label needs
/// `store::reask` and the questionnaire set, so the work happens here. This
/// layer also owns the two non-page outcomes: the `404` for a notation that is
/// gone, and the redirect to the review screen when nothing is parked for
/// re-collection.
async fn inject_reask(
    axum::extract::State(state): axum::extract::State<crate::admin::AdminState>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(notation_id) = notation_id_from_path(&req) else {
        return (StatusCode::NOT_FOUND, "notation not found").into_response();
    };
    match crate::retainer_walk::resolve_reask(&state, notation_id).await {
        Ok(data) => {
            req.extensions_mut()
                .insert(webapp::reask::InjectedReask(data));
            next.run(req).await
        }
        Err(response) => response,
    }
}

/// The gated Dioxus lawyer re-ask screen (#956 Phase 4). Write-bearing (the
/// corrected answers post from here), so it carries the CSRF token on top of the
/// auth + embedded Rego policy gate and nonce CSP, plus the resolver above.
pub fn reask_router(
    state: crate::admin::AdminState,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let surreal = state.surreal.clone();
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_REASK_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn_with_state(state, inject_reask)),
        )
        .with_state(FullstackState::new(cfg, webapp::reask::Reask))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The host's legal-document paths (#956 Phase 4). Host-level rather than
/// brand-scoped: every host that serves [`crate::host_crawler_and_legal_routes`]
/// serves these two.
pub const PRIVACY_PATH: &str = "/privacy";
pub const TERMS_PATH: &str = "/terms";

/// A gated-nothing Dioxus router for one legal document (#956 Phase 4). Public,
/// so it carries the public-utility injection (the auth-aware header group) on
/// top of the nonce CSP — the same shape as [`contact_router`]. The
/// rendered body is resolved once at construction and injected, so the render
/// never parses markdown.
pub fn legal_page_router(path: &'static str, content: webapp::legal_page::LegalContent) -> Router {
    let injected = webapp::legal_page::InjectedLegal(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(cfg, webapp::legal_page::LegalPage))
}

/// Build a host's two legal-document routers (`/privacy`, `/terms`) from that
/// host's own `CommonMark` bodies.
///
/// The mechanics — the Dioxus config, the title, the `<meta>` description — are
/// shared, but the *copy* is not: each deployment owns its own privacy and
/// terms text (`neon/content/*.md`) and passes it here, so a change to the
/// firm's policy never edits a white-label tenant's and vice versa.
/// `brand_name` names the deployment in the `<meta>` description; the `<title>`
/// is assembled from the request-scoped brand by `webapp::legal_page`.
#[must_use]
pub fn legal_dioxus_routers(brand_name: &str, privacy_body: &str, terms_body: &str) -> Vec<Router> {
    vec![
        legal_page_router(
            PRIVACY_PATH,
            webapp::legal_page::LegalContent {
                title: "Privacy Policy".to_string(),
                description: format!("Privacy Policy for {brand_name}."),
                body_html: views::markdown::render(privacy_body),
            },
        ),
        legal_page_router(
            TERMS_PATH,
            webapp::legal_page::LegalContent {
                title: "Terms of Service".to_string(),
                description: format!("Terms of Service for {brand_name}."),
                body_html: views::markdown::render(terms_body),
            },
        ),
    ]
}

/// The comment-only client document-review path (#641 Phase 3, Northstar Phase
/// A). The comment `GET`/`POST` stays on the Axum data API used by the custom
/// element.
pub const REVIEW_PATH: &str = "/app/projects/{project_code}/review/{doc_id}";

/// The gated Dioxus client document-review page (#641 Phase 3). Client-lens and
/// read-only-ish (a comment is the only write, via the custom element's own data
/// API), so it carries person-id scoping + the CSRF token on top of the auth +
/// embedded Rego policy gate and nonce CSP.
pub fn review_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            REVIEW_PATH,
            get(render_handler)
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(cfg, webapp::review::Review))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// Reject a `?sort=` that targets a field the entity-types list does not
/// advertise (it advertises only `name`), returning `400` before the render
/// runs — the same JSON:API `SortSpec::validated` contract the people route
/// applies. Layered onto `/lawyer/entity-types` by [`entity_types_router`].
async fn reject_unadvertised_entity_types_sort(request: Request, next: Next) -> Response {
    use std::collections::{HashMap, HashSet};

    // A malformed query (e.g. `?sort=%ZZ`) fails to parse entirely. Reject it
    // with a 400 rather than defaulting to "no sort" and rendering a 200 with
    // default ordering; the URL contract answers a bad `?sort=` with a 400, the
    // same guard `reject_unadvertised_design_sort` applies.
    let Ok(params) = axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|query| query.0)
    else {
        return (StatusCode::BAD_REQUEST, "malformed query string").into_response();
    };
    let allowed: HashSet<&str> = ["name"].into_iter().collect();
    match views::components::SortSpec::parse(params.get("sort").map(String::as_str))
        .validated(&allowed)
    {
        Ok(_) => next.run(request).await,
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// The gated Dioxus lawyer entity-types directory (#641 Phase 3, admin cluster).
/// Mounted unconditionally so it server-side renders even without a client
/// bundle, with the same authentication + embedded Rego policy gate the route carried. The
/// database handle is injected for the `list_entity_types` server function; the
/// sort pre-handler enforces the 400 contract; the nonce CSP allows hydration.
pub fn entity_types_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_ENTITY_TYPES_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(reject_unadvertised_entity_types_sort)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::entity_types::LawyerEntityTypes,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The lawyer expunge-request queue path (#641 Phase 3) — the read view; the
/// authorize/deny mutations keep their own `POST` routes.
pub const LAWYER_EXPUNGE_QUEUE_PATH: &str = "/lawyer/expunge-requests";

/// The gated Dioxus lawyer expunge-request queue (#641 Phase 3) — the first
/// row-action page. Its rows post to the existing authorize/deny handlers
/// through native forms, so the render carries the session CSRF token via the
/// extra [`inject_csrf_token`] layer on top of the usual auth + embedded Rego policy gate and
/// nonce CSP.
pub fn expunge_queue_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_EXPUNGE_QUEUE_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::expunge_requests::LawyerExpungeQueue,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The admin governed-expunge document path (#956 Phase 4) — the confirmation
/// form and, after the `POST` redirects back with `?record=`, the audit-row
/// result. The `POST` itself stays on the admin router; axum merges the
/// same-path methods.
pub const LAWYER_DOCUMENT_EXPUNGE_PATH: &str = "/lawyer/documents/{doc_id}/expunge";

/// The attorney contract-review path (#956 Phase 4) — the read screen. Its four
/// mutations (`…/findings/{idx}`, `…/summary`, `…/approve`, `…/reject`) stay on
/// the admin router; axum routes them by their deeper paths.
pub const LAWYER_CONTRACT_REVIEW_PATH: &str = "/lawyer/contract-reviews/{id}";

/// The gated Dioxus attorney contract-review screen (#956 Phase 4). Like
/// [`csrf_page_router`] — its findings, summary, approve, and reject forms are
/// native `POST`s carrying the session CSRF token — plus [`inject_person_id`],
/// because the loader enforces the per-matter lawyer row scope
/// (`can_see_project_as_lawyer`) that keeps an unrelated lawyer out.
pub fn contract_review_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_CONTRACT_REVIEW_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::contract_review::LawyerContractReview,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The lawyer "add entity" form path (#641 Phase 3) — a CRUD create form.
pub const LAWYER_ENTITY_NEW_PATH: &str = "/lawyer/entities/new";
/// The admin "add person" form path (#641 Phase 3) — an admin-only CRUD create
/// form; embedded Rego policy (default-deny + admin-bypass) gates `/admin/*` to admin, and the
/// page's `#[server]` function re-checks the admin role.
pub const ADMIN_PEOPLE_NEW_PATH: &str = "/admin/people/new";
/// The admin console person show/edit page path (#641 Phase 3) — a CRUD edit
/// form keyed by the person `{id}`, with a `/edit` alias (see
/// [`ADMIN_PERSON_EDIT_PATH`]). Both render the same component; it posts to the
/// native `POST /admin/person/{id}` update route.
pub const ADMIN_PERSON_PATH: &str = "/admin/person/{id}";
/// The `/edit` alias of [`ADMIN_PERSON_PATH`] — the surface served the
/// same show/edit render under both, so the Dioxus successor keeps the alias.
pub const ADMIN_PERSON_EDIT_PATH: &str = "/admin/person/{id}/edit";
/// The lawyer "edit entity" form path (#641 Phase 3) — a CRUD edit form keyed by
/// the entity `{id}`; it posts to the `POST /lawyer/entities/{id}` update handler.
pub const LAWYER_ENTITY_EDIT_PATH: &str = "/lawyer/entities/{id}/edit";
/// The "start a retainer walk" form path (#956 Phase 4) — the lawyer on-ramp that
/// opens a matter. `POST /lawyer/retainers/new` (the create) stays on the
/// `retainer_walk` handler; axum merges the two same-path method routes.
pub const LAWYER_RETAINER_NEW_PATH: &str = "/lawyer/retainers/new";

/// Build a gated Dioxus router for a CRUD page that renders `POST` forms (#641
/// Phase 3): like [`admin_listing_router`] but with the extra `inject_csrf_token`
/// layer so the page's `#[server]` function can carry the session CSRF token into
/// its forms. `component` is the page's Dioxus component; the route keeps the
/// same auth + embedded Rego policy gate and nonce CSP. Non-sortable — CRUD forms have no
/// `?sort=`.
pub fn csrf_page_router<C, M>(
    path: &'static str,
    component: C,
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router
where
    C: dioxus_core::ComponentFunction<(), M> + Send + Sync + 'static,
    M: 'static,
{
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                // The matter forms mounted here are part of the matter surface,
                // so their loaders run the participation gate — which fails
                // closed without a linked person. Injecting it is what makes
                // that gate a scope check rather than a blanket 404.
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(cfg, component))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The lawyer cron-schedules page path (#956 Phase 4) — the declared `CronJob`
/// reference plus a per-row "Run now" form. `POST
/// /lawyer/schedules/{job}/run` stays on `cron_schedules`, which already
/// redirects back here with a `?notice=` flash.
pub const LAWYER_SCHEDULES_PATH: &str = webapp::schedules::SCHEDULES_PATH;

/// The admin visitor-analytics page path (#956 Phase 4) — a read-only
/// aggregate dashboard. Admin-only; no form and no `POST`.
pub const ADMIN_ANALYTICS_PATH: &str = webapp::analytics::ANALYTICS_PATH;

/// The Owner/Admin matter directory path (ENG-221) — every matter's code,
/// name, status, and accountable lawyer, and nothing a matter contains.
///
/// Admission is the Owner/Admin route bypass at the top of
/// `portal/policy/navigator.rego`, exactly as `/app/admin` itself is admitted;
/// the path gets no rule of its own, because an `is_lawyer` rule here would hand
/// the whole directory to every lawyer. The page's loader re-checks the tier.
pub const ADMIN_MATTER_DIRECTORY_PATH: &str = webapp::matter_directory::MATTER_DIRECTORY_PATH;
/// The `?sort=` keys [`ADMIN_MATTER_DIRECTORY_PATH`] advertises.
pub const ADMIN_MATTER_DIRECTORY_SORT: &[&str] = webapp::matter_directory::MATTER_DIRECTORY_SORT;

/// Build the gated Dioxus router for the admin person show/edit page (#641
/// Phase 3): the same [`csrf_page_router`] as the other CRUD forms, mounted at
/// both [`ADMIN_PERSON_PATH`] and its `/edit` alias so the surface's two
/// URLs keep resolving. `POST /admin/person/{id}` (update), `.../welcome`, and
/// `.../impersonate` stay on the admin router; axum merges the same-path
/// methods.
///
/// `bootstrap_owner_email` (the configured `NAVIGATOR_BOOTSTRAP_OWNER_EMAIL`) is
/// injected as a request extension so the page's `#[server]` function can
/// resolve the immutable super-admin row — the same injection pattern as the
/// CSRF token, keeping the value off the environment so tests set it on state.
pub fn admin_person_show_router(
    bootstrap_owner_email: Option<String>,
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    csrf_page_router(
        ADMIN_PERSON_PATH,
        webapp::person_show::AdminPersonShow,
        surreal.clone(),
        sessions.clone(),
        policy.clone(),
        auth.clone(),
    )
    .merge(csrf_page_router(
        ADMIN_PERSON_EDIT_PATH,
        webapp::person_show::AdminPersonShow,
        surreal,
        sessions,
        policy,
        auth,
    ))
    .layer(axum::Extension(webapp::person_show::BootstrapOwnerEmail(
        bootstrap_owner_email,
    )))
}

/// The lawyer entities list path (#641 Phase 3) — a sortable list with per-row
/// edit/delete actions. `POST /lawyer/entities` (create) stays on the admin
/// router; axum merges the two same-path method routes.
pub const LAWYER_ENTITIES_PATH: &str = "/lawyer/entities";

/// Reject a `?sort=` the entities list does not advertise (it advertises `name`,
/// `entity_type`, `jurisdiction`), returning `400` before the render — the same
/// `SortSpec::validated` contract the other sortable pages apply. Layered onto
/// the list by [`entity_list_router`].
async fn reject_unadvertised_entity_list_sort(request: Request, next: Next) -> Response {
    use std::collections::{HashMap, HashSet};

    let Ok(params) = axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|query| query.0)
    else {
        return (StatusCode::BAD_REQUEST, "malformed query string").into_response();
    };
    let allowed: HashSet<&str> = ["name", "entity_type", "jurisdiction"]
        .into_iter()
        .collect();
    match views::components::SortSpec::parse(params.get("sort").map(String::as_str))
        .validated(&allowed)
    {
        Ok(_) => next.run(request).await,
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// The gated Dioxus lawyer entities list (#641 Phase 3). A sortable table with
/// per-row edit links and delete `POST` forms, so it carries both the sort
/// pre-handler (the `400` contract) and the `inject_csrf_token` layer (for the
/// delete forms), on top of the usual auth + embedded Rego policy gate and nonce CSP.
pub fn entity_list_router(
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            LAWYER_ENTITIES_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(reject_unadvertised_entity_list_sort)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::entity_list::LawyerEntityList,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The admin console people list path (#641 Phase 3) — the sortable directory
/// with a per-row action column. `POST /admin/people` (create) stays on the
/// admin router; axum merges the same-path methods.
pub const ADMIN_PEOPLE_PATH: &str = "/admin/people";

/// The gated Dioxus admin console people list (#641 Phase 3). The admin sibling
/// of the lawyer [`people_router`], adding the per-row Edit/Delete/Impersonate
/// action column — so it carries the `inject_csrf_token` layer (for the Delete /
/// Impersonate forms) and the injected bootstrap-Owner email (to resolve which
/// client rows are deletable), on top of the usual sort pre-handler, auth + embedded Rego policy
/// gate, and nonce CSP. It reuses [`reject_unadvertised_sort`] (name / email).
pub fn admin_people_router(
    bootstrap_owner_email: Option<String>,
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            ADMIN_PEOPLE_PATH,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(inject_csrf_token))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(reject_unadvertised_sort)),
        )
        .with_state(FullstackState::new(cfg, webapp::people::AdminPeople))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
        .layer(axum::Extension(webapp::person_show::BootstrapOwnerEmail(
            bootstrap_owner_email,
        )))
}

/// Path constants for the generic read-only admin listings (#641 Phase 3). Each
/// mounts through [`admin_listing_router`].
pub const LAWYER_JURISDICTIONS_PATH: &str = "/lawyer/jurisdictions";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_GIT_REPOSITORIES_PATH: &str = "/lawyer/git-repositories";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_PERSON_ENTITY_ROLES_PATH: &str = "/lawyer/person-entity-roles";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_NOTATIONS_PATH: &str = "/lawyer/notations";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_ANSWERS_PATH: &str = "/lawyer/answers";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_ADDRESSES_PATH: &str = "/lawyer/addresses";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_ASSETS_PATH: &str = "/lawyer/assets";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_PERSON_PROJECT_ROLES_PATH: &str = "/lawyer/person-project-roles";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_DISCLOSURES_PATH: &str = "/lawyer/disclosures";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_RELATIONSHIP_LOGS_PATH: &str = "/lawyer/relationship-logs";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_MAILROOMS_PATH: &str = "/lawyer/mailrooms";
/// See [`LAWYER_JURISDICTIONS_PATH`].
pub const LAWYER_LETTERS_PATH: &str = "/lawyer/letters";
/// See [`LAWYER_JURISDICTIONS_PATH`]. Paginated via `?page=`.
pub const LAWYER_EMAIL_LOG_PATH: &str = "/lawyer/email-log";
/// The letter-detail page — a single record keyed by the `{id}` path param, not
/// a listing, but mounted through the same gated-component factory.
pub const LAWYER_LETTER_DETAIL_PATH: &str = "/lawyer/letters/{id}";
/// The `/admin` console hub — a link table, not a listing, but it needs exactly
/// the same auth + policy + viewer-role stack, so it mounts through the same
/// gated-component factory. Its component re-checks for admin.
pub const ADMIN_LANDING_PATH: &str = "/app/admin";

/// Build a gated Dioxus router for one generic read-only admin listing (#641
/// Phase 3, admin cluster). `component` is the page's Dioxus component (e.g.
/// `webapp::admin_listings::LawyerJurisdictions`); it renders through the shared
/// `webapp::admin_listing` scaffold. Mirrors [`entity_types_router`] but carries
/// no sort pre-handler — these tables are non-sortable, exactly as the
/// `render_listing` pages they replace were. Mounted unconditionally so it
/// server-side renders even without a client bundle, with the same
/// authentication + embedded Rego policy gate; the database handle is injected for the page's
/// `#[server]` function and the nonce CSP allows hydration.
pub fn admin_listing_router<C, M>(
    path: &'static str,
    component: C,
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router
where
    C: dioxus_core::ComponentFunction<(), M> + Send + Sync + 'static,
    M: 'static,
{
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                // A matter-content listing scopes its rows to the caller's
                // participation ledger (ENG-303), which needs the signed-in
                // `persons.id`. Without this layer the extraction yields `None`
                // and every such listing renders empty for a Lawyer — the
                // fail-closed direction, but the wrong page. Pinned by
                // `matter_content_listings_are_scoped_to_participation`.
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(dioxus_document_head)),
        )
        .with_state(FullstackState::new(cfg, component))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// Reject a `?sort=` the listing does not advertise, returning `400` before the
/// render — the `SortSpec::validated` contract every sortable page holds.
///
/// The allowed set arrives as layer state so one function serves every sortable
/// listing, instead of a near-identical pre-handler per page.
async fn reject_unadvertised_listing_sort(
    axum::extract::State(allowed): axum::extract::State<&'static [&'static str]>,
    request: Request,
    next: Next,
) -> Response {
    use std::collections::{HashMap, HashSet};

    let params = axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|query| query.0)
        .unwrap_or_default();
    let allowed: HashSet<&str> = allowed.iter().copied().collect();
    match views::components::SortSpec::parse(params.get("sort").map(String::as_str))
        .validated(&allowed)
    {
        Ok(_) => next.run(request).await,
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// The lawyer playbooks listing path (#956 Phase 4) — a Company's negotiating
/// positions, sortable by company then playbook name. The `POST` on this same
/// path (the create) stays on `admin_playbooks`; axum merges the two.
pub const LAWYER_PLAYBOOKS_PATH: &str = "/lawyer/playbooks";
/// The `?sort=` keys [`LAWYER_PLAYBOOKS_PATH`] advertises — the two sortable
/// headers. Anything else is a `400` before the render.
pub const LAWYER_PLAYBOOKS_SORT: &[&str] = &["entity", "name"];
/// The "add playbook" form path (#956 Phase 4). A refused create redirects back
/// here with `?error=` and the rejected submission.
pub const LAWYER_PLAYBOOK_NEW_PATH: &str = "/lawyer/playbooks/new";
/// The "edit playbook positions" form path (#956 Phase 4). A refused update
/// redirects back here with `?error=` and the rejected positions text; `POST
/// /lawyer/playbooks/{id}` (the update) stays on `admin_playbooks`.
pub const LAWYER_PLAYBOOK_EDIT_PATH: &str = "/lawyer/playbooks/{id}/edit";

/// The lawyer templates catalog path (#956 Phase 4) — read-only and sortable.
pub const LAWYER_TEMPLATES_PATH: &str = "/lawyer/templates";
/// The lawyer questions directory path (#956 Phase 4) — read-only and sortable.
pub const LAWYER_QUESTIONS_PATH: &str = "/lawyer/questions";

/// A read-only admin listing whose headers sort: [`admin_listing_router`] plus
/// the `400`-on-unadvertised-sort pre-handler, so a header can never link to a
/// query the route refuses.
#[allow(clippy::too_many_arguments)] // + the Surreal handle (#1093; ENG-19)
pub fn sortable_admin_listing_router<C, M>(
    path: &'static str,
    component: C,
    allowed_sort: &'static [&'static str],
    surreal: store::surreal::SurrealDb,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router
where
    C: dioxus_core::ComponentFunction<(), M> + Send + Sync + 'static,
    M: 'static,
{
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![
        // A server fn can only reach what this list provides — a route
        // that renders a person and forgets it 500s at `consume_context`,
        // not at build.
        Box::new(move || Box::new(surreal.clone()) as Box<dyn std::any::Any>)
            as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>,
    ]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(inject_viewer_role))
                // Same injection the fixed-listing factory carries, so a
                // matter-content listing that later becomes sortable moves
                // between the two factories without silently losing its
                // participation scope and rendering empty.
                .layer(from_fn(inject_person_id))
                .layer(from_fn(inject_app_brand_mark))
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn_with_state(
                    allowed_sort,
                    reject_unadvertised_listing_sort,
                )),
        )
        .with_state(FullstackState::new(cfg, component))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The Dioxus design gallery. Mounted unconditionally so it server-side renders
/// even without a client bundle — the gallery is a contributor reference and its
/// content (theme swatches, icons, cards, toasts, the demo data table) is
/// readable pre-hydration. The per-response nonce CSP allows Dioxus's inline
/// hydration scripts, exactly as the lawyer and people routes do. `bootstrap`
/// mounts this router OUTSIDE the session boundary, so the gallery is a public
/// reference surface an anonymous reader reaches without signing in. The demo
/// table exercises the URL contract, so the route carries the same
/// `400`-on-unadvertised-`?sort=` pre-handler the people route does, scoped to
/// the table's advertised keys.
pub fn design_router() -> Router {
    Router::<FullstackState>::new()
        .route(
            DESIGN_PATH,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(reject_unadvertised_design_sort)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::design::DesignGallery,
        ))
}

/// Reject a `?sort=` on `/design` that targets a field the demo table does not
/// advertise, returning `400` before the render runs — the JSON:API
/// `SortSpec::validated` contract, the same guard the people route applies. The
/// advertised keys come from `webapp::design::DEMO_SORT_KEYS` so the gallery and
/// the guard can never disagree.
async fn reject_unadvertised_design_sort(request: Request, next: Next) -> Response {
    use std::collections::{HashMap, HashSet};

    // A malformed query (e.g. `?sort=%ZZ`) fails to parse entirely. Reject it
    // with a 400 rather than defaulting to "no sort" and letting the render
    // re-extract the same query and fail with a 200 "Failed to load" card; the
    // URL contract answers a bad `?sort=` with a 400.
    let Ok(params) = axum::extract::Query::<HashMap<String, String>>::try_from_uri(request.uri())
        .map(|query| query.0)
    else {
        return (StatusCode::BAD_REQUEST, "malformed query string").into_response();
    };
    let allowed: HashSet<&str> = webapp::design::DEMO_SORT_KEYS.iter().copied().collect();
    match views::components::SortSpec::parse(params.get("sort").map(String::as_str))
        .validated(&allowed)
    {
        Ok(_) => next.run(request).await,
        Err(error) => (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    }
}

/// The firm host's `/blog` index (#641 / #730 PR6 — the first content-backed
/// page port). Unlike the brand-only team pages, this one reads request state:
/// the caller builds the wasm-safe post list from the host's `BlogIndex` and it
/// is injected into the render context through `ServeConfig::context_providers`,
/// the same seam the lawyer pages use for the database, so
/// `webapp::blog_index::blog_index_view` reads it back. Public and firm-scoped
/// like the team routers.
pub fn blog_index_router(posts: webapp::blog_index::BlogPosts) -> Router {
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(posts.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            BLOG_PATH,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(cfg, webapp::blog_index::BlogIndexPage))
}

/// The firm `/blog/{slug}` post route (#641 / #730 PR6). A post is chosen by the
/// path parameter, not fixed per route, so — unlike the doc-only service pages —
/// its content is injected *per request*: [`inject_blog_post`] resolves the
/// matched post from `posts` and inserts it as an `axum::Extension`, which
/// `webapp::blog_post::blog_post_view` extracts back. That pre-layer also owns
/// the legacy underscore→hyphen redirect and the unknown-slug 404, matching the
/// handler. Public and firm-scoped.
pub fn blog_post_router(posts: webapp::blog_post::BlogPostSet) -> Router {
    Router::<FullstackState>::new()
        .route(
            BLOG_POST_PATH,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                // Outermost: redirect / 404 / inject before any rendering work.
                .layer(from_fn_with_state(posts, inject_blog_post)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::blog_post::BlogPostEntry,
        ))
}

/// The `/blog/{slug}` pre-layer: redirect a legacy underscore/upper slug to its
/// canonical kebab-case URL, 404 an unknown slug, or inject the matched post for
/// the render. This reproduces the `blog_post` handler's control flow.
async fn inject_blog_post(
    axum::extract::State(posts): axum::extract::State<webapp::blog_post::BlogPostSet>,
    mut req: Request,
    next: Next,
) -> Response {
    // Resolve the `{slug}` through axum's `Path` extractor so it arrives
    // percent-decoded, exactly as the removed handler's `Path<String>` did.
    // Reading `uri().path()` and splitting on `/` yields the raw, still-encoded
    // segment, so a percent-encoded spelling of a valid URL (e.g.
    // `/blog/thanks%2Dapple`) would miss the decoded post keys and 404 instead
    // of resolving or redirecting.
    let slug = match req.extract_parts::<axum::extract::Path<String>>().await {
        Ok(axum::extract::Path(slug)) => slug,
        Err(rejection) => return rejection.into_response(),
    };
    if views::slug::needs_redirect(&slug) {
        let canonical = format!("/blog/{}", views::slug::to_url(&slug));
        return axum::response::Redirect::permanent(&canonical).into_response();
    }
    match posts.get(&slug) {
        Some(content) => {
            req.extensions_mut()
                .insert(webapp::blog_post::InjectedBlogPost(content.clone()));
            next.run(req).await
        }
        None => (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response(),
    }
}

/// The template gallery index (#956 Phase 4). The curated allow-list is fixed at
/// compile time, so the caller resolves the cards once and they are injected at
/// construction. Firm-branded, and it joins the protected composition as the
/// route it replaces did.
pub fn template_gallery_router(content: webapp::template_gallery::GalleryContent) -> Router {
    let injected = webapp::template_gallery::InjectedGallery(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            TEMPLATES_PATH,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::template_gallery::GalleryEntry,
        ))
}

/// One template's detail page (#956 Phase 4). [`inject_template_entry`] owns
/// every non-render outcome on this path — the legacy alias and kebab-case
/// redirects, the `/download` raw markdown response, and the 404 for a template
/// that is not on the curated allow-list — because axum cannot register a second
/// `GET` handler where the render sits.
pub fn template_entry_router() -> Router {
    Router::<FullstackState>::new()
        .route(
            TEMPLATE_ENTRY_PATH,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                // Outermost: redirect / download / 404 / inject first.
                .layer(from_fn(inject_template_entry)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::template_gallery::TemplateDetailEntry,
        ))
}

/// The `/templates/{*path}` pre-layer, reproducing the handler's control
/// flow: canonicalize the path, serve the raw `.md` as an attachment for a
/// `/download` suffix, 404 anything off the curated allow-list (so an
/// uncurated template can never leak), or inject the matched detail.
async fn inject_template_entry(mut req: Request, next: Next) -> Response {
    let path = match req.extract_parts::<axum::extract::Path<String>>().await {
        Ok(axum::extract::Path(path)) => path,
        Err(rejection) => return rejection.into_response(),
    };

    let (path, is_download) = path
        .strip_suffix("/download")
        .map_or((path.as_str(), false), |base| (base, true));
    let path = crate::template_gallery::legacy_alias(path).unwrap_or(path);
    let redirect_segments = if is_download {
        ["templates", path, "download"].join("/")
    } else {
        ["templates", path].join("/")
    };
    let redirect_parts: Vec<&str> = redirect_segments.split('/').collect();
    if let Some(to) = crate::kebab_redirect_path(&redirect_parts) {
        return axum::response::Redirect::permanent(&to).into_response();
    }

    let Some(template) = crate::template_gallery::find_path(path) else {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    };

    if is_download {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/markdown; charset=utf-8"),
        );
        if let Ok(disposition) = HeaderValue::try_from(format!(
            "attachment; filename=\"{}\"",
            template.download_filename()
        )) {
            headers.insert(header::CONTENT_DISPOSITION, disposition);
        }
        return (StatusCode::OK, headers, template.raw).into_response();
    }

    let content = webapp::template_gallery::TemplateDetailContent {
        card: template_card(template),
        frontmatter: template.frontmatter().to_string(),
        download_href: template.download_path(),
        // A serious prospect routes into the firm's contact path.
        start_matter_href: "/contact".to_string(),
    };
    req.extensions_mut()
        .insert(webapp::template_gallery::InjectedTemplateDetail(content));
    next.run(req).await
}

/// Project a curated gallery entry onto the wasm-safe card the components
/// render.
#[must_use]
pub fn template_card(
    template: &'static crate::template_gallery::GalleryTemplate,
) -> webapp::template_gallery::TemplateCard {
    webapp::template_gallery::TemplateCard {
        href: template.detail_path(),
        name: template.name.to_string(),
        title: template.title.clone(),
        blurb: template.blurb.to_string(),
        jurisdiction_label: template.jurisdiction.label().to_string(),
        badge_class: template.jurisdiction.badge_class().to_string(),
    }
}

/// One marketing page built from the shared band vocabulary — `/navigator`,
/// `/fractional-cto`, and `/fractional-gc`.
///
/// One router serves them all: they differ only in the copy the caller
/// resolves, which is what keeps a further page a data change.
pub fn marketing_page_router(path: &str, content: webapp::marketing_page::PageContent) -> Router {
    let injected = webapp::marketing_page::InjectedMarketingPage(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::marketing_page::MarketingPageEntry,
        ))
}

/// The firm's platform page: what Neon Law Navigator is, why the firm builds
/// it, and the invitation to co-counsel a pro bono case.
pub const FIRM_NAVIGATOR_PATH: &str = "/navigator";

/// The firm's lead offering: it runs the technology function for a law firm —
/// AI enablement, the privacy and compliance work under it, and complex counsel
/// beside it. A law-related service, which is why its page carries an RPC 5.7
/// disclosure the other marketing pages do not need.
pub const FIRM_FRACTIONAL_CTO_PATH: &str = "/fractional-cto";

/// The firm's Legal Services page: the published flat-fee catalog of one-time
/// consumer legal work — a will, a trust, a name change, a formation — each
/// with the fee the firm charges for it printed on the page. A single page,
/// not a `/services/*` catalog.
pub const FIRM_SERVICES_PATH: &str = "/services";

/// One category index — [`WORKSHOP_INDEX_PATH`] or
/// [`PRESENTATION_INDEX_PATH`].
///
/// Both mounts are public.
///
/// The catalog is fixed at construction, but the index is still built by a
/// pre-layer rather than baked in, because the two mounts differ only by the
/// content injected and resolving it on the request task is what keeps them
/// one router.
pub fn catalog_index_router(
    path: &str,
    content: webapp::catalog_index::CatalogIndexContent,
) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                .layer(from_fn_with_state(content, inject_catalog_index)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::catalog_index::CatalogIndexEntry,
        ))
}

/// Hand this category's resolved content to the render task.
async fn inject_catalog_index(
    axum::extract::State(content): axum::extract::State<webapp::catalog_index::CatalogIndexContent>,
    mut req: Request,
    next: Next,
) -> Response {
    req.extensions_mut()
        .insert(webapp::catalog_index::InjectedCatalogIndex(content));
    next.run(req).await
}

/// A Catalog material's hub — `/workshops/{slug}` or `/presentations/{slug}`
/// (#956 Phase 4).
///
/// This path carries two behaviors and the pre-layer owns both, short-circuiting
/// before the render rather than letting a failed resolve fall through to a
/// `200` with an empty page:
///
/// * `…/{slug}.md` is the material's raw-Markdown twin. matchit captures the
///   whole `readme.md` segment into `{slug}`, so the suffix is a branch here
///   rather than a second route, exactly as the handler had it.
/// * An unknown material 404s.
pub fn catalog_material_router(path: &str, workshops: crate::WorkshopIndex) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                .layer(from_fn_with_state(workshops, inject_catalog_material)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::catalog_material::CatalogMaterialEntry,
        ))
}

async fn inject_catalog_material(
    axum::extract::State(workshops): axum::extract::State<crate::WorkshopIndex>,
    mut req: Request,
    next: Next,
) -> Response {
    let mut segments = req.uri().path().rsplit('/');
    let slug = segments.next().unwrap_or_default().to_string();
    let category = segments.next().unwrap_or_default().to_string();
    // The raw-Markdown twin never renders a page.
    if let Some(stem) = slug.strip_suffix(".md") {
        return match workshops.find_in_category(&category, stem) {
            Some(m) => crate::markdown_response_for(&m.raw_markdown),
            None => (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response(),
        };
    }
    let Some(material) = workshops.find_in_category(&category, &slug) else {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    };
    req.extensions_mut()
        .insert(webapp::catalog_material::InjectedMaterial(
            crate::material_content(material),
        ));
    next.run(req).await
}

/// A material's light table (`…/{slug}/slides`, #956 Phase 4).
///
/// Mints the double-submit CSRF token for the certificate form embedded on the
/// page, so the content is per request even though the slides are not.
pub fn catalog_slides_router(
    path: &str,
    workshops: crate::WorkshopIndex,
    sessions: crate::SessionStore,
    secure_cookies: bool,
) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                .layer(from_fn_with_state(
                    (workshops, sessions, secure_cookies),
                    inject_catalog_slides,
                )),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::catalog_slides::CatalogSlidesEntry,
        ))
}

async fn inject_catalog_slides(
    axum::extract::State((workshops, sessions, secure_cookies)): axum::extract::State<(
        crate::WorkshopIndex,
        crate::SessionStore,
        bool,
    )>,
    mut req: Request,
    next: Next,
) -> Response {
    let mut segments = req.uri().path().rsplit('/').skip(1);
    let slug = segments.next().unwrap_or_default().to_string();
    let category = segments.next().unwrap_or_default().to_string();
    let Some(material) = workshops.find_in_category(&category, &slug) else {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    };
    let Some(cookies) = req.extensions().get::<tower_cookies::Cookies>().cloned() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            webapp::error_pages::server_error(),
        )
            .into_response();
    };
    let csrf = crate::password_reset::mint_csrf_with(
        &sessions,
        secure_cookies,
        &cookies,
        crate::WORKSHOP_CERT_CSRF_COOKIE_NAME,
    );
    req.extensions_mut()
        .insert(webapp::catalog_slides::InjectedLightTable(
            crate::light_table_content(material, csrf),
        ));
    next.run(req).await
}

/// Split `…/{category}/{slug}/{face}/{n}` into its material and 1-based slide
/// number. Both the classroom step face and the projector face address a slide
/// this way, so they share one parse.
fn material_slide_path(path: &str) -> Option<(String, String, usize)> {
    let mut segments = path.rsplit('/');
    let step: usize = segments.next()?.parse().ok()?;
    let _face = segments.next()?;
    let slug = segments.next()?.to_string();
    let category = segments.next()?.to_string();
    Some((category, slug, step))
}

/// One classroom step (`…/{slug}/step/{n}`, #956 Phase 4).
///
/// Steps are 1-based, so index `0` and anything past the last section are out
/// of range. The pre-layer resolves the slide and 404s there rather than
/// letting an unresolved render fall through to a `200` with an empty page.
pub fn catalog_step_router(path: &str, workshops: crate::WorkshopIndex) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                .layer(from_fn_with_state(workshops, inject_catalog_step)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::catalog_step::CatalogStepEntry,
        ))
}

async fn inject_catalog_step(
    axum::extract::State(workshops): axum::extract::State<crate::WorkshopIndex>,
    mut req: Request,
    next: Next,
) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    let Some((category, slug, step)) = material_slide_path(req.uri().path()) else {
        return not_found();
    };
    let Some(content) = workshops
        .find_in_category(&category, &slug)
        .and_then(|material| crate::step_content(material, step))
    else {
        return not_found();
    };
    req.extensions_mut()
        .insert(webapp::catalog_step::InjectedStep(content));
    next.run(req).await
}

/// The slide-only projector face (`…/{slug}/display/{n}`, #956 Phase 4).
///
/// Same addressing and 404 rules as the classroom step, but the page wears no
/// site chrome at all, so no chrome pre-layer runs here — a projector shows the
/// slide and nothing else.
pub fn catalog_display_router(path: &str, workshops: crate::WorkshopIndex) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn_with_state(workshops, inject_catalog_display)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::catalog_display::CatalogDisplayEntry,
        ))
}

async fn inject_catalog_display(
    axum::extract::State(workshops): axum::extract::State<crate::WorkshopIndex>,
    mut req: Request,
    next: Next,
) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    let Some((category, slug, step)) = material_slide_path(req.uri().path()) else {
        return not_found();
    };
    let Some(content) = workshops
        .find_in_category(&category, &slug)
        .and_then(|material| crate::display_content(material, step))
    else {
        return not_found();
    };
    req.extensions_mut()
        .insert(webapp::catalog_display::InjectedDisplay(content));
    next.run(req).await
}

/// The certificate confirmation (`…/{slug}/certificate/sent`, #956 Phase 4) —
/// where the certificate POST redirects.
///
/// A GET route rather than the POST's own response body, so a reload
/// re-renders the confirmation instead of dispatching a second certificate.
pub fn certificate_sent_router(path: &str, workshops: crate::WorkshopIndex) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                .layer(from_fn_with_state(workshops, inject_certificate_sent)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::catalog_certificate_sent::CertificateSentEntry,
        ))
}

async fn inject_certificate_sent(
    axum::extract::State(workshops): axum::extract::State<crate::WorkshopIndex>,
    mut req: Request,
    next: Next,
) -> Response {
    let mut segments = req.uri().path().rsplit('/').skip(2);
    let slug = segments.next().unwrap_or_default().to_string();
    let category = segments.next().unwrap_or_default().to_string();
    let Some(material) = workshops.find_in_category(&category, &slug) else {
        return (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response();
    };
    req.extensions_mut()
        .insert(webapp::catalog_certificate_sent::InjectedCertificateSent(
            crate::certificate_sent_content(material),
        ));
    next.run(req).await
}

/// The five read routes one material category publishes.
///
/// A category owns a root path, and every material under it is addressed
/// relative to that root. Naming the set once is what lets the `workshops`
/// and `presentations` mounts be the same five routers behind different
/// route sets rather than two hand-maintained copies.
pub struct MaterialPaths {
    /// A material's hub, and — through the same pre-layer — its `.md` twin.
    pub material: &'static str,
    /// A material's light table.
    pub slides: &'static str,
    /// One classroom step.
    pub step: &'static str,
    /// The projector face a presenter opens on a second screen.
    pub display: &'static str,
    /// Where the certificate POST redirects.
    pub certificate_sent: &'static str,
}

/// The public `workshops` category.
pub const WORKSHOP_PATHS: MaterialPaths = MaterialPaths {
    material: WORKSHOP_MATERIAL_PATH,
    slides: "/workshops/{slug}/slides",
    step: "/workshops/{slug}/step/{step}",
    display: "/workshops/{slug}/display/{step}",
    certificate_sent: "/workshops/{slug}/certificate/sent",
};

/// The public `presentations` category.
pub const PRESENTATION_PATHS: MaterialPaths = MaterialPaths {
    material: PRESENTATION_MATERIAL_PATH,
    slides: "/presentations/{slug}/slides",
    step: "/presentations/{slug}/step/{step}",
    display: "/presentations/{slug}/display/{step}",
    certificate_sent: "/presentations/{slug}/certificate/sent",
};

/// The public workshop index.
pub const WORKSHOP_INDEX_PATH: &str = "/workshops";
/// The public index of the talks.
pub const PRESENTATION_INDEX_PATH: &str = "/presentations";
/// A workshop's hub.
pub const WORKSHOP_MATERIAL_PATH: &str = "/workshops/{slug}";
/// A presentation's hub.
pub const PRESENTATION_MATERIAL_PATH: &str = "/presentations/{slug}";
/// The certificate request itself — a POST, so it is owned by the
/// `AppState` table rather than a Dioxus router.
pub const WORKSHOP_CERTIFICATE_PATH: &str = "/workshops/{slug}/certificate";
/// The presentations twin of [`WORKSHOP_CERTIFICATE_PATH`].
pub const PRESENTATION_CERTIFICATE_PATH: &str = "/presentations/{slug}/certificate";

/// One category's five read routers, ungated.
///
/// The pre-layers recover the category and slug by counting segments back
/// from the end of the request path, so the same five constructors serve
/// either category without knowing which root they were mounted under.
#[must_use]
pub fn catalog_material_routers(
    paths: &MaterialPaths,
    workshops: crate::WorkshopIndex,
    sessions: &crate::SessionStore,
    secure_cookies: bool,
) -> Vec<Router> {
    vec![
        catalog_material_router(paths.material, workshops.clone()),
        catalog_slides_router(
            paths.slides,
            workshops.clone(),
            sessions.clone(),
            secure_cookies,
        ),
        catalog_step_router(paths.step, workshops.clone()),
        // The projector face wears no site chrome at all, so it takes none.
        catalog_display_router(paths.display, workshops.clone()),
        certificate_sent_router(paths.certificate_sent, workshops),
    ]
}

/// The workspace-documentation routes (#956 Phase 4): `/docs` renders the
/// `index` doc and `/docs/{slug}` renders one doc, both from the compiled-in
/// [`DocsIndex`]. `slug` is `None` for the index route, which has no path
/// parameter to read.
///
/// [`inject_doc`] resolves the doc and owns every non-render outcome on the
/// path — the kebab-case redirect, the `/docs/index` → `/docs` redirect, and the
/// unknown-slug 404 — because axum cannot register a second `GET` handler where
/// the render sits.
///
/// **Anonymous.** These routes mount outside the session boundary, beside
/// `/design`, and carry `inject_optional_session` so a signed-in reader still
/// gets the authenticated nav. The documentation is the manual for software
/// anyone can clone, so a login door in front of it guarded nothing.
/// [`app_docs_router`] is the second, role-scoped door to the same index; it
/// stays gated because it is part of the authenticated application, not because
/// these documents are restricted.
///
/// **One chrome on every host.** These routes live in the shared composition
/// [`crate::bootstrap`] mounts, so one mount serves `neon` and a white-label
/// `tenant` alike, and [`inject_public_utility`] resolves the same public
/// chrome here as everywhere else.
pub fn docs_router(
    path: &'static str,
    slug: Option<&'static str>,
    docs: crate::DocsIndex,
) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility))
                // Outermost: redirect / 404 / inject before any rendering work.
                .layer(from_fn_with_state((docs, slug), inject_doc)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::docs_page::DocsPageEntry,
        ))
}

/// `/app/docs` and `/app/docs/{slug}` — the same workspace documentation,
/// inside the authenticated application.
///
/// The `/docs` mount is anonymous — the source is public, so its manual is too.
/// This is a second door to the same [`crate::DocsIndex`], for the people who
/// operate Navigator: it wears the application chrome and is scoped to the tiers
/// that run the product. It restricts a *surface*, not the documents, which
/// anyone can read at `/docs`. It differs from [`docs_router`] in two ways,
/// both deliberate:
///
/// * **It wears the application chrome, not the public one.** A signed-in
///   reader keeps the app navbar and their viewer role, so the docs sit inside
///   the product instead of bouncing them out to a marketing shell.
/// * **It is gated.** `require_auth` then `require_policy`, in that order, so
///   an anonymous request is a redirect to sign-in rather than a policy denial.
///   The Rego rule admits Lawyer and Clerk explicitly, and Owner/Admin
///   through the policy's route bypass — `client` is the one authenticated tier
///   denied, because these documents describe firm-side operation. That role
///   restriction is what `/docs` does not have.
pub fn app_docs_router(
    path: &'static str,
    slug: Option<&'static str>,
    docs: crate::DocsIndex,
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark))
                // Outermost of the render layers: redirect / 404 / inject
                // before any rendering work, exactly as the public mount does.
                .layer(from_fn_with_state((docs, slug), inject_doc)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::docs_page::DocsPageEntry,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The authenticated documentation hub.
pub const APP_DOCS_PATH: &str = "/app/docs";
/// One document inside the authenticated hub.
pub const APP_DOC_PATH: &str = "/app/docs/{slug}";

/// The firm team home — the post-login landing for every firm tier.
pub const APP_TEAM_PATH: &str = "/app/team";

/// `/app/team` — the firm team home.
///
/// Gated exactly like [`app_docs_router`]: `require_auth` then `require_policy`,
/// so an anonymous request is a redirect to sign-in rather than a policy denial.
/// The Rego rule admits Lawyer and Clerk explicitly and Owner/Admin
/// through the route bypass — `client` is the one authenticated tier denied. That
/// is what makes this a safe post-login landing for the firm tiers: a client
/// never reaches it, so `complete_sign_in` sends a client to `/app/projects`
/// instead.
///
pub fn app_team_router(
    sessions: crate::session::SessionStore,
    policy: crate::policy::PolicyClient,
    auth: crate::auth::AuthConfig,
) -> Router {
    Router::<FullstackState>::new()
        .route(
            APP_TEAM_PATH,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_viewer_role))
                .layer(from_fn(inject_app_brand_mark)),
        )
        .with_state(FullstackState::new(
            ServeConfig::new(),
            webapp::team_home::TeamHome,
        ))
        .route_layer(from_fn_with_state(
            (sessions, policy),
            crate::policy::require_policy,
        ))
        .route_layer(from_fn_with_state(auth, crate::auth::require_auth))
}

/// The `/docs` and `/docs/{slug}` pre-layer: canonicalize the slug, 404 an
/// unknown one, or inject the matched doc for the render. This reproduces the
/// `docs_page` / `render_doc_page` control flow.
async fn inject_doc(
    axum::extract::State((docs, fixed_slug)): axum::extract::State<(
        crate::DocsIndex,
        Option<&'static str>,
    )>,
    mut req: Request,
    next: Next,
) -> Response {
    // The index route carries a fixed slug and no path parameter; the slug route
    // reads its own through axum's `Path` extractor, so it arrives
    // percent-decoded exactly as the handler's `Path<String>` did.
    let slug = match fixed_slug {
        Some(slug) => slug.to_string(),
        None => match req.extract_parts::<axum::extract::Path<String>>().await {
            Ok(axum::extract::Path(slug)) => slug,
            Err(rejection) => return rejection.into_response(),
        },
    };

    if fixed_slug.is_none() {
        if let Some(to) = crate::kebab_redirect_path(&["docs", &slug]) {
            return axum::response::Redirect::permanent(&to).into_response();
        }
        // `/docs/index` is the index route's content, so it has one canonical
        // URL rather than two.
        if slug == "index" {
            return axum::response::Redirect::permanent("/docs").into_response();
        }
    }

    match docs.find(&slug) {
        Some(doc) => {
            let mut catalog: Vec<_> = if slug == DOCS_INDEX_SLUG {
                docs.docs()
                    .iter()
                    .filter(|entry| entry.slug != DOCS_INDEX_SLUG)
                    .map(|entry| webapp::docs_page::DocCatalogEntry {
                        title: entry.title.clone(),
                        href: format!("/docs/{}", entry.slug),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            catalog.sort_by_cached_key(|entry| entry.title.to_lowercase());
            req.extensions_mut().insert(webapp::docs_page::InjectedDoc(
                webapp::docs_page::DocContent {
                    title: doc.title.clone(),
                    body_html: doc.body_html.clone(),
                    is_index: slug == DOCS_INDEX_SLUG,
                    catalog,
                },
            ));
            next.run(req).await
        }
        None => (StatusCode::NOT_FOUND, webapp::error_pages::not_found()).into_response(),
    }
}

/// The firm home page (`/`) — the Dioxus SSR port (#641 / #730 PR6). The static
/// copy (`content`) is resolved brand-safely by the caller and injected via
/// `ServeConfig::context_providers`. The page is a plain statement of the
/// practice, so it resolves no per-request data.
pub fn home_router(path: &str, content: webapp::home::HomeContent) -> Router {
    let injected = webapp::home::InjectedHome(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(cfg, webapp::home::HomePageEntry))
}

/// The firm `/litigation` page — the disputes practice. Static like
/// [`home_router`]: the caller resolves the copy brand-safely at router-build
/// time and injects it through `ServeConfig::context_providers`, and the page
/// resolves no per-request data.
pub fn litigation_router(
    path: &str,
    content: webapp::litigation_page::LitigationContent,
) -> Router {
    let injected = webapp::litigation_page::InjectedLitigation(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::litigation_page::LitigationPageEntry,
        ))
}

/// The firm `/fractional-gc` page — the flat-monthly-fee company-counsel
/// practice. Static and injected exactly like [`litigation_router`].
pub fn transactional_router(
    path: &str,
    content: webapp::transactional_page::TransactionalContent,
) -> Router {
    let injected = webapp::transactional_page::InjectedTransactional(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::transactional_page::TransactionalPageEntry,
        ))
}

/// The firm `/contact` page (#641 / #730 PR6), served through the Dioxus SSR
/// port. Content-backed like the service pages: the caller
/// (the firm's public Dioxus pages) resolves the [`ContactContent`] from the
/// mounted branding and injects it through `ServeConfig::context_providers`, and
/// `webapp::contact_page::contact_page_view` reads it back. `path` is the route
/// the page mounts at. Public and firm-scoped.
///
/// [`ContactContent`]: webapp::contact_page::ContactContent
pub fn contact_router(path: &str, content: webapp::contact_page::ContactContent) -> Router {
    let injected = webapp::contact_page::InjectedContact(content);
    let cfg = ServeConfig::new().context_providers(std::sync::Arc::new(vec![Box::new(move || {
        Box::new(injected.clone()) as Box<dyn std::any::Any>
    })
        as Box<dyn Fn() -> Box<dyn std::any::Any> + Send + Sync>]));

    Router::<FullstackState>::new()
        .route(
            path,
            get(render_handler)
                .layer(from_fn(dioxus_document_head))
                .layer(from_fn(inject_public_utility)),
        )
        .with_state(FullstackState::new(
            cfg,
            webapp::contact_page::ContactPageEntry,
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use tower::ServiceExt as _;

    /// Every matter route names its matter the same way: `{project_code}`.
    ///
    /// This is a router-construction invariant, not a style rule. `matchit`
    /// refuses two routes whose parameter at the same position carries
    /// different names, so one spelling that disagrees panics the whole router
    /// at boot — every surface, not just the route that disagreed. Nothing
    /// catches that at compile time and no constant-inspecting test sees it
    /// either; only something that builds the assembled router does.
    ///
    /// Asserting the constants is cheap and names the rule; the assembled
    /// routers in `portal/tests` are what prove they actually resolve.
    #[test]
    fn every_matter_route_names_its_matter_project_code() {
        for path in [
            PROJECT_DETAIL_PATH,
            LAWYER_PROJECT_EDIT_PATH,
            LAWYER_PARTICIPATION_NEW_PATH,
            LAWYER_PARTICIPATION_EDIT_PATH,
            PROJECT_DOCUMENT_PATH,
            CONVERSATION_PATH,
            PORTAL_INTAKE_PATH,
            REVIEW_PATH,
            crate::project_portal::PROJECT_PORTAL_PATH,
        ] {
            assert!(
                path.starts_with("/app/projects/{project_code}"),
                "`{path}` does not name its matter `{{project_code}}`, so registering it \
                 alongside the others panics the router"
            );
            assert!(
                !path.contains("{id}"),
                "`{path}` still routes on a row id; the code is the matter's public name"
            );
        }
    }

    /// The matter show page is keyed by the Project **code**, always.
    ///
    /// The code is a matter's whole public identity: `/app/projects/{code}` is
    /// its show page and `/app/projects/{code}/portal/` is its client portal,
    /// and the same string names its shared-drive folder and its object-storage
    /// prefix. So the internal row id must never surface in a URL — a link a
    /// client is sent, bookmarks, and quotes back over email should name the
    /// matter, not a row.
    ///
    /// What actually guarantees it is the *lookup*, not the shape of the
    /// segment: a lowercase UUID is a perfectly well-formed code, so nothing
    /// could refuse one on sight. Both directions go through the `code` column
    /// and neither consults the id, which is the claim asserted below —
    /// `project_show_path` writes a code into every link Navigator renders, and
    /// `project_id_from_path` reads one back. A row id in that segment names no
    /// Project.
    #[tokio::test]
    async fn the_matter_show_page_is_keyed_by_code_and_never_by_row_id() {
        assert_eq!(PROJECT_DETAIL_PATH, "/app/projects/{project_code}");
        let segments: Vec<&str> = PROJECT_DETAIL_PATH.split('/').skip(1).collect();
        assert_eq!(
            segments,
            ["app", "projects", "{project_code}"],
            "the show page is two literal segments and the code; a deeper path \
             or a differently named parameter is a different page"
        );

        let surreal = store::test_support::mem_surreal().await;
        let project = store::projects::create(
            &surreal,
            &store::projects::NewProject {
                code: "libra-formation".into(),
                name: "Libra formation".into(),
                status: "open".into(),
                entity_id: store::test_support::seed_entity(&surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("a seeded Project");

        // Out: every link Navigator renders spells the code.
        assert_eq!(
            project_show_path(&surreal, project.id).await,
            "/app/projects/libra-formation",
        );

        // Back in: the code resolves and the row id does not.
        assert_eq!(
            project_id_from_path(&surreal, "/app/projects/libra-formation").await,
            Some(project.id),
        );
        assert_eq!(
            project_id_from_path(&surreal, &format!("/app/projects/{}", project.id)).await,
            None,
            "a row id in the show-page segment must name no Project"
        );
    }

    fn guarded_router() -> Router {
        Router::new()
            .route("/admin/people", get(|| async { "ok" }))
            .layer(from_fn(reject_unadvertised_sort))
    }

    #[tokio::test]
    async fn rejects_an_unadvertised_sort_field_with_400() {
        let response = guarded_router()
            .oneshot(
                Request::builder()
                    .uri("/admin/people?sort=ssn")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn allows_an_advertised_sort_field() {
        for uri in [
            "/admin/people?sort=name",
            "/admin/people?sort=-email",
            "/admin/people",
        ] {
            let response = guarded_router()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "sort should be allowed: {uri}"
            );
        }
    }

    /// The head fragment declares both licensed faces and preloads the reading
    /// one, following the deployment asset origin — the repository ships the
    /// declaration, not the WOFF2 bytes.
    #[test]
    fn the_gorp_head_declares_both_faces_against_the_asset_origin() {
        let fragment = gorp_head_fragment(
            "https://storage.example.test/assets/fonts/gorp-serif/GORPSerif-Regular.woff2",
            "https://storage.example.test/assets/fonts/gorp-serif/GORPSerif-Bold.woff2",
        );
        assert!(fragment.contains("font-weight:400"), "{fragment}");
        assert!(fragment.contains("font-weight:700"), "{fragment}");
        assert!(fragment.contains("font-family:'GORP Serif'"), "{fragment}");
        assert!(
            fragment.contains(
                "<link rel=\"preload\" as=\"font\" type=\"font/woff2\" crossorigin \
                 href=\"https://storage.example.test/assets/fonts/gorp-serif/\
                 GORPSerif-Regular.woff2\">"
            ),
            "the reading face must be preloaded: {fragment}",
        );
    }

    /// A hostile asset origin cannot break out of the `<style>` element it is
    /// interpolated into. `views::layout` escapes the CSS string and the
    /// direct builder escapes the attribute; this pins that both still apply.
    #[test]
    fn a_hostile_asset_origin_cannot_escape_the_head_fragment() {
        let fragment = gorp_head_fragment(
            "https://evil.test/x');}</style><script>alert(1)</script>",
            "https://evil.test/bold.woff2",
        );
        assert!(
            !fragment.contains("</style><script>"),
            "the style element must not be closed early: {fragment}",
        );
        assert!(!fragment.contains("<script>"), "{fragment}");
    }

    /// One chrome serves the whole site — header included.
    ///
    /// Asserted against the brand constants rather than literals so a
    /// white-label bundle renaming the firm cannot fail it.
    #[test]
    fn one_public_chrome_serves_the_whole_site() {
        let chrome = webapp::public_chrome::firm_public_chrome(Vec::new());

        assert_eq!(chrome.brand_name, views::brand::FIRM_BRAND.site_name);
        assert_eq!(chrome.home_href, views::brand::FIRM_BRAND.home_href);
        assert_eq!(chrome.logo_href, views::brand::FIRM_BRAND.logo_href);

        // No header destination reaches a retired surface.
        assert!(
            chrome
                .destinations
                .iter()
                .all(|link| !link.href.starts_with("/foundation")),
            "the header links no retired page: {:?}",
            chrome
                .destinations
                .iter()
                .map(|link| &link.href)
                .collect::<Vec<_>>()
        );

        // And the footer names the firm's own regulated detail.
        assert!(!chrome.legal_entity.is_empty(), "the firm names its own");
        assert!(!chrome.offices.is_empty(), "the firm publishes its offices");
        assert!(
            !chrome.firm_email.is_empty(),
            "the band has something to show"
        );
    }

    /// The public-page utility links reproduce the navbar's auth block:
    /// anonymous → Sign in; signed-in → Projects and Sign out, whatever the
    /// tier.
    #[test]
    fn public_utility_links_match_the_navigation_auth_block() {
        use crate::session::SessionData;
        use store::persons::Role;

        let hrefs = |links: Vec<webapp::public_chrome::ChromeNavLink>| {
            links.into_iter().map(|link| link.href).collect::<Vec<_>>()
        };

        // Anonymous: Sign in, on every property — a white-label tenant signs
        // into the same portal the firm does.
        assert_eq!(hrefs(public_utility_links(None)), ["/auth/login"]);

        // Every signed-in tier gets the same two links. The nav no longer
        // names the viewer — that was the same mistake the URL prefixes made,
        // and a per-role desk link is how a "which surface am I allowed on"
        // question leaks back into the chrome.
        let session = |role| SessionData::fresh("viewer", role);
        for role in [
            Role::Client,
            Role::Clerk,
            Role::Lawyer,
            Role::Admin,
            Role::Owner,
        ] {
            assert_eq!(
                hrefs(public_utility_links(Some(&session(role)))),
                ["/app/projects", "/auth/logout"],
                "{role:?} gets the one matter surface"
            );
        }

        // The visible label comes from the catalog, not a hard-coded string.
        let anon = public_utility_links(None);
        assert_eq!(anon[0].label, "Sign in");
    }

    /// The stamp rewrites the opening `<html>` tag whatever the SSR shell was:
    /// dioxus-server's bare `<html>` default (unit + firm tests) *and* the
    /// bundled `index.html`'s attributed `<html lang="en">` (production, `web`
    /// SSR tests) both come out carrying the document language. The attributed
    /// case is the production regression this fixes — a literal `<html>` search
    /// matched only the bare shell.
    #[test]
    fn html_lang_is_stamped_over_any_opening_tag() {
        assert_eq!(
            stamp_html_lang(
                "<!DOCTYPE html><html><head></head><body></body></html>",
                "es"
            ),
            "<!DOCTYPE html><html lang=\"es\"><head></head><body></body></html>"
        );
        assert_eq!(
            stamp_html_lang(
                "<!DOCTYPE html><html lang=\"en\"><head></head><body></body></html>",
                "es"
            ),
            "<!DOCTYPE html><html lang=\"es\"><head></head><body></body></html>"
        );
        // The English canonical stays English.
        assert_eq!(
            stamp_html_lang("<html lang=\"en\"><head></head></html>", "en"),
            "<html lang=\"en\"><head></head></html>"
        );
    }
    /// The banner lands as the body's first child, whatever attributes the
    /// opening tag carries, and a document with no body is returned untouched.
    ///
    /// First child is the load-bearing part twice over. It is an advisory about
    /// everything below it, so a reader must meet it before the page; and it
    /// must land *outside* `<div id="main">`, because that node is what
    /// `dioxus-web` hydrates. Injecting inside it would put markup in the DOM
    /// that the client's virtual tree does not know about.
    #[test]
    fn the_banner_opens_the_body_and_leaves_a_bodyless_document_alone() {
        let banner = "<div id=\"b\">B</div>";

        assert_eq!(
            open_with_banner(
                "<html><head></head><body><div id=\"main\">P</div></body></html>",
                banner
            ),
            "<html><head></head><body><div id=\"b\">B</div><div id=\"main\">P</div></body></html>"
        );
        // The bundled template's body may carry attributes; the whole opening
        // tag is skipped rather than a literal `<body>` matched.
        assert_eq!(
            open_with_banner("<body class=\"x\" data-y><p>P</p></body>", banner),
            "<body class=\"x\" data-y><div id=\"b\">B</div><p>P</p></body>"
        );
        // A fragment response has no body to open. Dropping the banner beats
        // failing the response.
        assert_eq!(
            open_with_banner("<p>fragment</p>", banner),
            "<p>fragment</p>"
        );
        assert_eq!(open_with_banner("<body", banner), "<body");
    }

    /// The rendered banner says the matters are invented, and carries the id
    /// the browser walkthrough looks for. Asserted here as well as in `webapp`
    /// because this is the string that actually reaches a response.
    #[test]
    fn the_rendered_banner_is_the_one_the_walkthrough_finds() {
        let banner = &*SAMPLE_MATTERS_BANNER;
        assert!(
            banner.contains(webapp::components::SAMPLE_MATTERS_BANNER_ID),
            "{banner}"
        );
        assert!(banner.contains("Sample matters"), "{banner}");
    }

    /// Only a request path under `/app/` gets the footer — the public
    /// marketing, blog, docs, and template routes this middleware also layers
    /// onto must not.
    #[test]
    fn only_app_paths_render_the_footer() {
        assert!(renders_app_footer("/app/projects"));
        assert!(renders_app_footer("/app/projects/libra-formation"));
        assert!(renders_app_footer("/app/team"));
        assert!(!renders_app_footer("/blog"));
        assert!(!renders_app_footer("/docs"));
        assert!(!renders_app_footer("/lawyer/entity-types"));
        assert!(!renders_app_footer("/app"));
    }

    /// The rendered footer is the one that actually reaches a response.
    #[test]
    fn the_rendered_footer_names_the_firm_of_record() {
        let footer = &*APP_FOOTER;
        assert!(footer.contains("Shook Law PLLC"), "{footer}");
        assert!(footer.contains('©'), "{footer}");
    }

    /// The route-scoped policy widens `img-src`/`font-src` to the deployment
    /// asset origin — the licensed faces live there, and a `font-src 'self'`
    /// would block them and drop the page back to a fallback serif — while
    /// keeping scripts on `'self'` plus the per-response nonce.
    #[test]
    fn the_csp_admits_the_asset_origin_for_fonts_only_when_configured() {
        // A real generated nonce, not a literal: the policy has to carry
        // whatever `generate_nonce` produced for this response.
        let nonce = generate_nonce();

        let same_origin = csp_with_nonce(&nonce, None, None);
        assert!(same_origin.contains("font-src 'self';"), "{same_origin}");
        assert!(same_origin.contains("media-src 'self';"), "{same_origin}");
        assert!(
            !same_origin.contains("https://"),
            "the same-origin default admits no host: {same_origin}",
        );

        let cdn = csp_with_nonce(
            &nonce,
            Some("https://storage.example.test".to_string()),
            None,
        );
        assert!(
            cdn.contains("font-src 'self' https://storage.example.test;"),
            "{cdn}",
        );
        assert!(
            cdn.contains("img-src 'self' data: https://storage.example.test;"),
            "{cdn}",
        );
        // Catalog slides render through this route, so a slide's video is
        // governed here. Left to fall back to `default-src 'self'`, a clip
        // would play locally and be blocked from the bucket in production.
        assert!(
            cdn.contains("media-src 'self' https://storage.example.test;"),
            "{cdn}",
        );
        assert!(
            cdn.contains(&format!(
                "script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval'"
            )),
            "scripts stay same-origin plus the nonce: {cdn}",
        );
    }

    /// A deployment with no widget emits the policy it emitted before the
    /// widget existed — not two extra directives restating `default-src`, and
    /// above all no third-party origin. This is the assertion that keeps the
    /// off switch meaningful.
    #[test]
    fn without_a_widget_the_policy_names_no_third_party_origin() {
        let nonce = generate_nonce();
        let csp = csp_with_nonce(&nonce, None, None);
        assert!(!csp.contains("chatwoot"), "{csp}");
        assert!(!csp.contains("connect-src"), "{csp}");
        assert!(!csp.contains("frame-src"), "{csp}");
        assert!(csp.ends_with("form-action 'self'"), "{csp}");
    }

    /// With a widget the policy names its origin in all four directives the
    /// widget actually uses. `connect-src` is the one worth asserting
    /// explicitly: it is absent otherwise, so it inherits `default-src 'self'`,
    /// and getting it wrong yields a bubble that opens and never receives a
    /// reply — a failure no SSR-content test can see.
    #[test]
    fn a_widget_admits_its_origin_on_script_frame_img_and_socket() {
        let nonce = generate_nonce();
        let widget = chatwoot_widget();
        let csp = csp_with_nonce(&nonce, None, Some(&widget));

        assert!(
            csp.contains(&format!(
                "script-src 'self' 'nonce-{nonce}' 'wasm-unsafe-eval' https://app.chatwoot.com;"
            )),
            "the vendor SDK is admitted alongside the nonce: {csp}"
        );
        assert!(
            csp.contains("img-src 'self' data: https://app.chatwoot.com;"),
            "avatars and attachment thumbnails load: {csp}"
        );
        assert!(
            csp.contains("frame-src 'self' https://app.chatwoot.com"),
            "the conversation iframe is admitted: {csp}"
        );
        assert!(
            csp.contains("connect-src 'self' https://app.chatwoot.com wss://app.chatwoot.com;"),
            "the socket that delivers replies is admitted: {csp}"
        );
        // The widening is additive. Nothing the page already relied on is
        // dropped by taking the widget branch.
        assert!(csp.contains("default-src 'self';"), "{csp}");
        assert!(csp.contains("object-src 'none';"), "{csp}");
        assert!(csp.contains("frame-ancestors 'none';"), "{csp}");
        assert!(csp.contains("form-action 'self'"), "{csp}");
    }

    /// The widget rides the asset-origin widening rather than replacing it: a
    /// production deployment has both, and an `img-src` that admitted only one
    /// of them would break either every hero or every agent avatar.
    #[test]
    fn a_widget_and_an_asset_origin_are_both_admitted() {
        let nonce = generate_nonce();
        let widget = chatwoot_widget();
        let csp = csp_with_nonce(
            &nonce,
            Some("https://storage.example.test".to_string()),
            Some(&widget),
        );
        assert!(
            csp.contains(
                "img-src 'self' data: https://storage.example.test https://app.chatwoot.com;"
            ),
            "{csp}"
        );
        // The asset origin carries passive presentation bytes and must not
        // become a script source just because a widget widened `script-src`.
        assert!(
            !csp.contains("'wasm-unsafe-eval' https://storage.example.test"),
            "the asset origin stays out of script-src: {csp}"
        );
    }

    /// The widget's own test fixture — Chatwoot Cloud, as production resolves
    /// it, built through the public constructor so the test cannot drift from
    /// what a deployment actually produces.
    fn chatwoot_widget() -> crate::chatwoot::ChatwootWidget {
        crate::chatwoot::ChatwootWidget::from_lookup(|key| {
            (key == crate::chatwoot::NAVIGATOR_CHATWOOT_WEBSITE_TOKEN).then(|| "tok3n".to_string())
        })
        .expect("a token resolves a widget")
    }

    /// The middleware's widget branch, end to end: a configured deployment
    /// serving a public page gets the loader in its body and the widened policy
    /// on its header, and an authenticated page from the same process gets
    /// neither.
    ///
    /// The env var is set inside the test because `CHATWOOT` is resolved once
    /// per process and nextest runs each test in its own — so this observes a
    /// freshly configured deployment without leaking into any other test. It is
    /// the only place the static, the marker check, the injection, and the CSP
    /// are exercised together, which is what a unit test of each piece cannot
    /// tell you: that the middleware wires them to the same decision.
    #[tokio::test]
    async fn a_configured_deployment_boots_the_widget_on_public_pages_only() {
        std::env::set_var(
            crate::chatwoot::NAVIGATOR_CHATWOOT_WEBSITE_TOKEN,
            "tok3n-from-config",
        );

        let public_body = format!(
            "<html><head></head><body><div class=\"{}\">firm page</div></body></html>",
            webapp::components::PUBLIC_SHELL_MARKER
        );
        let router = Router::new()
            .route(
                "/",
                get(move || {
                    let body = public_body.clone();
                    async move { axum::response::Html(body) }
                }),
            )
            .route(
                "/app/projects",
                get(|| async {
                    axum::response::Html(
                        "<html><head></head><body><div class=\"navigator-shell nav-theme\">\
                         portal page</div></body></html>",
                    )
                }),
            )
            .layer(from_fn(dioxus_document_head));

        let fetch = |uri: &'static str| {
            let router = router.clone();
            async move {
                let response = router
                    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let csp = response
                    .headers()
                    .get(header::CONTENT_SECURITY_POLICY)
                    .expect("the render carries a policy")
                    .to_str()
                    .unwrap()
                    .to_string();
                let bytes = axum::body::to_bytes(response.into_body(), MAX_RENDER_BYTES)
                    .await
                    .unwrap();
                (csp, String::from_utf8(bytes.to_vec()).unwrap())
            }
        };

        let (public_csp, public_html) = fetch("/").await;
        assert!(
            public_html.contains(crate::chatwoot::CHATWOOT_LOADER_HREF),
            "the public page boots the widget: {public_html}"
        );
        assert!(
            public_html.contains("data-website-token=\"tok3n-from-config\""),
            "the configured inbox reaches the page: {public_html}"
        );
        // Injected before the close, so the widget follows the page's content.
        let loader_at = public_html
            .find(crate::chatwoot::CHATWOOT_LOADER_HREF)
            .unwrap();
        assert!(
            loader_at < public_html.find("</body>").unwrap()
                && public_html.find("firm page").unwrap() < loader_at,
            "the loader closes the body: {public_html}"
        );
        assert!(
            public_csp.contains("https://app.chatwoot.com"),
            "the policy admits the installation: {public_csp}"
        );
        assert!(
            public_csp.contains("wss://app.chatwoot.com"),
            "and the socket that delivers replies: {public_csp}"
        );

        // Same process, same configuration, authenticated shell: no widget and
        // no third-party origin. This is the assertion that would fail if the
        // page test were satisfied by a looser marker match.
        let (portal_csp, portal_html) = fetch("/app/projects").await;
        assert!(
            !portal_html.contains(crate::chatwoot::CHATWOOT_LOADER_HREF),
            "the authenticated page boots no widget: {portal_html}"
        );
        assert!(
            !portal_csp.contains("chatwoot"),
            "and keeps the strict policy: {portal_csp}"
        );
    }

    /// The public shell selects a page; the authenticated shell does not.
    ///
    /// Both roots carry `nav-theme`, in opposite order, which is exactly the
    /// substring match this must not be: a looser test would put a support-chat
    /// bubble on every `/app` and `/lawyer` page and widen those pages' CSP to
    /// a third-party origin. Both literals are the rendered roots asserted by
    /// the shells' own component tests.
    #[test]
    fn only_the_public_shell_marks_a_page_public() {
        assert!(is_public_page(r#"<div class="nav-theme public-shell">"#));
        assert!(!is_public_page(
            r#"<div class="navigator-shell nav-theme">"#
        ));
        // The nested region class shares the prefix and must not qualify a
        // document on its own.
        assert!(!is_public_page(r#"<main class="public-shell__main">"#));
        assert!(!is_public_page("<div>no shell at all</div>"));
    }

    /// The loader is injected as the last thing before `</body>` — after the
    /// page's own content, because the widget is chrome over a page that has to
    /// be readable without it.
    #[test]
    fn the_loader_closes_the_body() {
        let out = close_with_script(
            "<html><body><main>PAGE</main></body></html>",
            "<script src=\"/x.js\"></script>",
        );
        assert_eq!(
            out,
            "<html><body><main>PAGE</main><script src=\"/x.js\"></script></body></html>"
        );
    }

    /// A fragment with no `</body>` keeps its bytes rather than 500-ing, the
    /// same call [`open_with_banner`] makes for a document with no `<body>`.
    #[test]
    fn a_fragment_with_no_body_close_is_returned_untouched() {
        assert_eq!(
            close_with_script("<p>fragment</p>", "<script></script>"),
            "<p>fragment</p>"
        );
    }

    /// Only the HTML render is rewritten. A wasm/glue asset response passes
    /// through with no font `<style>` spliced into its bytes and no CSP header
    /// bolted on.
    #[tokio::test]
    async fn a_non_html_response_passes_through_untouched() {
        let router = Router::new()
            .route(
                "/assets/app.wasm",
                get(|| async {
                    ([(header::CONTENT_TYPE, "application/wasm")], "\0asm-bytes").into_response()
                }),
            )
            .layer(from_fn(dioxus_document_head));

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/assets/app.wasm")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert!(response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_none());
        let bytes = axum::body::to_bytes(response.into_body(), MAX_RENDER_BYTES)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"\0asm-bytes");
    }

    /// The HTML render gets the GORP faces spliced into its `<head>` exactly
    /// once, and after `<meta charset>` so that declaration stays inside the
    /// document's first 1024 bytes.
    #[tokio::test]
    async fn the_html_render_carries_the_gorp_faces_once() {
        let router = Router::new()
            .route(
                "/page",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        "<!DOCTYPE html><html><head><meta charset=\"UTF-8\" />\
                         <title>x</title></head><body></body></html>",
                    )
                        .into_response()
                }),
            )
            .layer(from_fn(dioxus_document_head));

        let response = router
            .oneshot(Request::builder().uri("/page").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .is_some());

        let bytes = axum::body::to_bytes(response.into_body(), MAX_RENDER_BYTES)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert_eq!(
            html.matches("font-family:'GORP Serif'").count(),
            2,
            "exactly the two faces, spliced once: {html}",
        );
        assert!(
            html.find("charset").unwrap() < html.find("@font-face").unwrap(),
            "the charset declaration must still lead the head: {html}",
        );
        assert!(
            html.find("@font-face").unwrap() < html.find("</head>").unwrap(),
            "the faces belong inside the head: {html}",
        );
    }
}
