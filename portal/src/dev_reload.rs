//! Opt-in browser refresh for `navigator dev serve`; never compiled in release builds.

use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, HeaderValue},
    middleware::Next,
    response::{IntoResponse, Response},
};

const SCRIPT: &str = r"let revision;
async function check() {
  try {
    const response = await fetch('/__dev/revision', {cache: 'no-store'});
    if (response.ok) {
      const next = await response.text();
      if (revision !== undefined && next !== revision) location.reload();
      revision = next;
    }
  } catch (_) { /* Keep the page visible while the server restarts. */ }
  setTimeout(check, 1000);
}
check();";

pub(crate) fn enabled() -> bool {
    std::env::var("NAVIGATOR_DEV_RELOAD").as_deref() == Ok("1")
        && std::env::var("NAVIGATOR_ENVIRONMENT").as_deref() == Ok("dev")
}

pub(crate) async fn refresh(request: Request, next: Next) -> Response {
    let mut response = match request.uri().path() {
        "/__dev/revision" => tokio::fs::read_to_string(".devx/revision")
            .await
            .unwrap_or_default()
            .into_response(),
        "/__dev/reload.js" => (
            [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
            SCRIPT,
        )
            .into_response(),
        _ => next.run(request).await,
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"text/html"))
    {
        let (mut parts, body) = response.into_parts();
        let bytes = match to_bytes(body, 16 * 1024 * 1024).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(%error, "reading development HTML for browser refresh");
                return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };
        let html = String::from_utf8_lossy(&bytes).replace(
            "</body>",
            "<script src=\"/__dev/reload.js\" defer></script></body>",
        );
        parts.headers.remove(header::CONTENT_LENGTH);
        return Response::from_parts(parts, Body::from(html));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Router};
    use tower::ServiceExt;

    #[tokio::test]
    async fn refresh_injects_only_html_and_disables_asset_caching() {
        let app = Router::new()
            .route(
                "/",
                get(|| async { axum::response::Html("<body>Preview</body>") }),
            )
            .route("/asset.css", get(|| async { "body{}" }))
            .layer(axum::middleware::from_fn(refresh));
        for (path, expected) in [("/", true), ("/asset.css", false)] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body = to_bytes(response.into_body(), 1024).await.unwrap();
            assert_eq!(
                String::from_utf8_lossy(&body).contains("/__dev/reload.js"),
                expected
            );
        }
    }
}
