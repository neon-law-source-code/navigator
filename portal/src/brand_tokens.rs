//! Generated per-brand tokens stylesheets.
//!
//! `GET /public/css/brand-{key}-tokens.css` is rendered from the brand row's
//! typeface and palette ids (falling back to the compiled registry) so a
//! static `brand-*-tokens.css` file is not the source of truth. Axum cannot
//! bind `{key}` inside a mixed path segment, so this intercepts the full path
//! and leaves every other `/public` file to the static mount.

use axum::extract::{Request, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;

/// Intercept a tokens stylesheet request; otherwise continue to `/public`.
pub async fn intercept(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(key) = key_from_tokens_path(req.uri().path()) else {
        return next.run(req).await;
    };
    tokens_css(&state, key).await
}

fn key_from_tokens_path(path: &str) -> Option<&str> {
    let name = path.strip_prefix("/public/css/")?;
    let key = name.strip_prefix("brand-")?.strip_suffix("-tokens.css")?;
    store::projects::is_valid_code(key).then_some(key)
}

async fn tokens_css(state: &AppState, key: &str) -> Response {
    let row = match store::brands::find_by_key(&state.surreal, key).await {
        Ok(row) => row,
        Err(error) => {
            tracing::error!(error = %error, brand_key = %key, "brand tokens row read failed");
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };
    let compiled = views::brand::BrandKey::parse(key);
    // Owned, not borrowed: `row` is moved into `brand` below, and these two
    // are read again after that move.
    let typeface_id = row.as_ref().and_then(|brand| brand.typeface.clone());
    let primary_color = row.as_ref().and_then(|brand| brand.primary_color.clone());

    // The closed-catalog path, unchanged: every compiled house brand
    // resolves here regardless of what hex its row stores, because
    // `resolve_presentation` falls back to the compiled key's own palette
    // when the stored value is not a catalog id (ENG-586 stores each
    // compiled brand's real hex there for the free-hex path below, not a
    // palette id — the fallback is what keeps the three house brands
    // rendering unchanged).
    if let Some((face, palette)) = views::brand::resolve_presentation(
        typeface_id.as_deref(),
        primary_color.as_deref(),
        compiled,
    ) {
        return css_response(views::brand::tokens_stylesheet(face, palette));
    }

    // A runtime brand wearing a free hex primary and/or an uploaded font
    // (ENG-586): font and colour are resolved independently, since neither
    // is guaranteed to be a catalog id.
    let Some(brand) = row else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(font_stack) = views::brand::font_stack_for(
        typeface_id.as_deref(),
        brand.font_family.as_deref(),
        compiled,
    ) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let font_object_url = brand
        .font_object_key
        .as_deref()
        .map(|k| format!("/assets/{k}"));
    let font_face = views::brand::font_face_for(
        typeface_id.as_deref(),
        brand.font_family.as_deref().zip(font_object_url.as_deref()),
    );
    let Some(hex) = brand.primary_color.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(scheme) = views::brand::derive_scheme(hex) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    css_response(views::brand::tokens_stylesheet_from_hex(
        &font_stack,
        font_face.as_deref(),
        &scheme,
    ))
}

fn css_response(css: String) -> Response {
    let mut response = css.into_response();
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

#[cfg(test)]
mod tests {
    use super::{key_from_tokens_path, tokens_css};
    use store::test_support::mem_surreal;

    /// ENG-586: a runtime brand wearing a free hex primary and an uploaded
    /// font (no compiled `BrandKey` fallback, since its key is custom)
    /// renders the derived colour tokens and the uploaded `@font-face`,
    /// rather than 404ing the way the closed-catalog-only path used to for
    /// any key `resolve_presentation` couldn't match.
    #[tokio::test]
    async fn a_custom_brand_with_a_free_hex_and_uploaded_font_renders_derived_tokens() {
        let surreal = mem_surreal().await;
        let brand = store::brands::create(
            &surreal,
            store::persons::Role::Owner,
            None,
            &store::brands::NewBrand {
                name: "Custom Brand".to_string(),
                key: "custom-brand".to_string(),
                primary_color: Some("#007c91".to_string()),
                typeface: Some("uploaded".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::brands::set_font(
            &surreal,
            store::persons::Role::Owner,
            None,
            brand.id,
            "Custom Sans",
            "fonts/brands/custom-brand/abc123.woff2",
            "OFL-1.1",
        )
        .await
        .unwrap();

        let state = crate::test_support::app_state(surreal).await;
        let response = tokens_css(&state, "custom-brand").await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let css = String::from_utf8(body.to_vec()).unwrap();
        assert!(css.contains("--nav-color-primary: #007c91"), "{css}");
        assert!(css.contains("font-family:'Custom Sans'"), "{css}");
        assert!(
            css.contains("/assets/fonts/brands/custom-brand/abc123.woff2"),
            "{css}"
        );
    }

    /// A brand with no row at all still 404s.
    #[tokio::test]
    async fn an_unknown_brand_key_404s() {
        let surreal = mem_surreal().await;
        let state = crate::test_support::app_state(surreal).await;
        let response = tokens_css(&state, "never-created").await;
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    }

    #[test]
    fn tokens_paths_yield_the_brand_key() {
        assert_eq!(
            key_from_tokens_path("/public/css/brand-neon-tokens.css"),
            Some("neon")
        );
        assert_eq!(
            key_from_tokens_path("/public/css/brand-delete-your-data-tokens.css"),
            Some("delete-your-data")
        );
        assert_eq!(key_from_tokens_path("/public/css/theme.css"), None);
        assert_eq!(key_from_tokens_path("/public/css/brand-tokens.css"), None);
        assert_eq!(
            key_from_tokens_path("/public/css/brand-Not_a_code-tokens.css"),
            None
        );
    }
}
