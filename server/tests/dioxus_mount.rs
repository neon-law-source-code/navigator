//! Phase 0 Dioxus mount coverage (issue #641).
//!
//! Proves the two properties the mount must hold: the `/dioxus-demo` page is
//! server-side rendered — its content readable before any hydration runs — and
//! it is absent when no client bundle has been built, so every other route and
//! the global fallback stay exactly as they were.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

/// A minimal, CDN-free bundle `index.html`: enough for `dioxus-server` to parse
/// a template with the `main` mount point and render the component into it.
const INDEX_HTML: &str = "<!DOCTYPE html>\n\
<html lang=\"en\"><head><meta charset=\"UTF-8\" />\
<title>Neon Law Navigator</title></head>\
<body><div id=\"main\"></div></body></html>\n";

/// The mount reads its bundle directory from `DIOXUS_PUBLIC_PATH`, so this test
/// owns that process-global variable and drives both cases in one process (safe
/// under nextest's process-per-test isolation and correct under any runner).
#[tokio::test]
async fn dioxus_demo_is_server_rendered_and_absent_without_a_bundle() {
    // No bundle directory → no Dioxus route.
    std::env::remove_var("DIOXUS_PUBLIC_PATH");
    assert!(
        portal::dioxus_app::router().is_none(),
        "with no built bundle the Dioxus page must not mount",
    );

    // Point the mount at a minimal bundle directory (an index.html is all the
    // renderer needs). The component renders to HTML server-side — readable
    // before the wasm client hydrates it.
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("index.html"), INDEX_HTML).expect("write index.html");
    std::fs::create_dir(dir.path().join("assets")).expect("dx assets directory");
    std::fs::create_dir(dir.path().join("wasm")).expect("wasm directory");
    std::fs::write(
        dir.path().join("wasm/webapp.js"),
        "// browser bundle fixture",
    )
    .expect("browser bundle");
    std::env::set_var("DIOXUS_PUBLIC_PATH", dir.path());

    let router = portal::dioxus_app::router().expect("a built bundle mounts the Dioxus page");
    let response = router
        .oneshot(
            Request::builder()
                .uri(portal::dioxus_app::DIOXUS_DEMO_PATH)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);

    // The route scopes a nonce'd CSP so Dioxus's inline hydration scripts run
    // under `script-src 'self' 'nonce-…' 'wasm-unsafe-eval'` — never blanket
    // `'unsafe-inline'`, never a CDN host. (Dioxus 0.7 ships hydration data as
    // inline scripts; a strict `script-src 'self'` blocks them and the page
    // never hydrates — the failure this covers.)
    let csp = response
        .headers()
        .get("content-security-policy")
        .and_then(|value| value.to_str().ok())
        .expect("rendered Dioxus page must carry a CSP")
        .to_string();
    assert!(
        csp.contains("script-src 'self' 'nonce-") && csp.contains("'wasm-unsafe-eval'"),
        "CSP must nonce the hydration scripts and allow wasm: {csp}",
    );
    // The script-src directive must never fall back to blanket `'unsafe-inline'`
    // (style-src keeps it for inline styles, which is fine and unchanged).
    let script_src = csp.split("script-src").nth(1).unwrap_or_default();
    assert!(
        !script_src.contains("'unsafe-inline'"),
        "script-src must not allow blanket inline: {csp}"
    );
    assert!(
        !csp.contains("http://") && !csp.contains("https://"),
        "no CDN host: {csp}"
    );

    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let html = String::from_utf8(body.to_vec()).expect("utf-8 body");
    assert!(
        html.contains(r#"id="dioxus-demo""#),
        "server-rendered HTML must carry the component's mount point before hydration; got: {html}",
    );
    assert!(
        html.contains("<script nonce=\""),
        "Dioxus's inline hydration scripts must be nonce-tagged so they run under CSP",
    );

    // The firm's typeface. `theme.css` names `GORP Serif` as the family for
    // every Dioxus surface, but only the server knows the deployment asset
    // origin the licensed WOFF2 files live behind — so the render must carry
    // the `@font-face` declarations and the reading-face preload. Without them
    // the pages fall back to the browser's default serif while the pages
    // render GORP.
    assert!(
        html.contains("@font-face") && html.contains("'GORP Serif'"),
        "the rendered head must declare the GORP Serif faces; got: {html}",
    );
    assert!(
        html.contains(
            "<link rel=\"preload\" as=\"font\" type=\"font/woff2\" crossorigin \
             href=\"/public/fonts/gorp-serif/GORPSerif-Regular.woff2\">"
        ),
        "the reading face must be preloaded to avoid a fallback-serif flash; got: {html}",
    );

    // A real dx bundle has an assets directory. Mounting it must neither
    // collide with `/assets/{*key}` nor send anonymous JS requests to login.
    let state = portal::test_support::app_state(store::test_support::mem_surreal().await).await;
    let app = portal::bootstrap(state, dir.path(), axum::Router::new(), &[], Vec::new())
        .expect("bundle and public object assets coexist");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/wasm/webapp.js")
                .body(Body::empty())
                .expect("bundle request"),
        )
        .await
        .expect("bundle response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.into_body().collect().await.unwrap().to_bytes(),
        "// browser bundle fixture"
    );
    std::env::remove_var("DIOXUS_PUBLIC_PATH");
}
