//! Generated per-brand tokens stylesheets.
//!
//! `GET /public/css/brand-{key}-tokens.css` is rendered from the brand row's
//! typeface and palette ids (falling back to the compiled registry) so a
//! static `brand-*-tokens.css` file is not the source of truth.

use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::AppState;

/// Render the tokens layer for one brand key.
pub async fn tokens_css(State(state): State<AppState>, Path(key): Path<String>) -> Response {
    if !store::projects::is_valid_code(&key) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let row = match store::brands::find_by_key(&state.surreal, &key).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(error = %error, brand_key = %key, "brand tokens row read failed");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let compiled = views::brand::BrandKey::parse(&key);
    let Some((face, palette)) = views::brand::resolve_presentation(
        row.as_ref().and_then(|brand| brand.typeface.as_deref()),
        row.as_ref()
            .and_then(|brand| brand.primary_color.as_deref()),
        compiled,
    ) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut response = views::brand::tokens_stylesheet(face, palette).into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, crate::STATIC_CACHE_CONTROL);
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, crate::NOSNIFF);
    response
}
