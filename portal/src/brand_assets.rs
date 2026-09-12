//! Native multipart uploads for a brand's logo and font (ENG-586).
//!
//! Two upload doors on the `/app/brands/{key}/edit` page, each: CSRF-guarded
//! multipart (the same `require_multipart_csrf` shape the avatar uploads
//! use), validated by size and content type, scanned by the shared
//! [`crate::attachment_scanner::AttachmentScanner`], written to the public
//! assets bucket, and recorded on the `brand` row through
//! `store::brands::set_logo`/`set_font`. The target brand's Firm capability is
//! resolved before the public bucket write, while the store methods retain
//! their own authorization guard as defence in depth. Every refusal redirects
//! back to the edit page with `?error=` naming the rule; nothing is written to
//! any bucket on refusal.

use axum::extract::{Multipart, Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Extension;

use crate::admin::AdminState;
use crate::attachment_scanner::ScanVerdict;
use crate::session::SessionData;

/// A brand logo is a small, static image — 512 KB comfortably covers a
/// crisp SVG wordmark or a PNG at retina size.
pub(crate) const MAX_LOGO_BYTES: usize = 512 * 1024;
/// A single `.woff2` web-font file rarely exceeds a couple hundred KB; 2 MB
/// leaves headroom for a heavier glyph set without admitting a whole family.
pub(crate) const MAX_FONT_BYTES: usize = 2 * 1024 * 1024;

const ALLOWED_LOGO_CONTENT_TYPES: [&str; 2] = ["image/png", "image/svg+xml"];
const ALLOWED_FONT_CONTENT_TYPES: [&str; 2] = ["font/woff2", "application/octet-stream"];

fn back_to_edit(key: &str, message: &str) -> Response {
    let mut query = String::new();
    crate::admin::push_query(&mut query, "error", message);
    Redirect::to(&format!("/app/brands/{key}/edit?{query}")).into_response()
}

/// Whether `bytes` — already known to be `image/svg+xml` — is free of the
/// constructs a brand logo must never carry: an inline `<script>`, an
/// event-handler attribute (`onload=`, `onclick=`, …), a `<foreignObject>`,
/// or a reference to an external origin. A conservative substring scan
/// rather than a full XML parse: good enough to refuse the shapes that
/// matter without a parser dependency, and it fails closed on anything it
/// cannot decode as UTF-8.
fn svg_is_safe(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let lower = text.to_lowercase();
    if lower.contains("<script") || lower.contains("<foreignobject") {
        return false;
    }
    if contains_event_handler_attribute(&lower) {
        return false;
    }
    for needle in [
        "href=\"http",
        "href='http",
        "xlink:href=\"http",
        "xlink:href='http",
    ] {
        if lower.contains(needle) {
            return false;
        }
    }
    true
}

/// Whether `lower` (already lowercased) contains an `on<word>=` attribute —
/// `onload=`, `onclick=`, with or without whitespace before the `=`. Only
/// matches where `on` starts a word (preceded by whitespace, a quote, or a
/// tag-open `<`), so it does not fire on ordinary text containing "on".
fn contains_event_handler_attribute(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut search_from = 0;
    while let Some(offset) = lower[search_from..].find("on") {
        let start = search_from + offset;
        let preceded_ok = start == 0
            || matches!(
                bytes[start - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'"' | b'\'' | b'<'
            );
        if preceded_ok {
            let mut cursor = start + 2;
            while cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
                cursor += 1;
            }
            if cursor > start + 2 {
                let mut after_word = cursor;
                while after_word < bytes.len() && matches!(bytes[after_word], b' ' | b'\t') {
                    after_word += 1;
                }
                if after_word < bytes.len() && bytes[after_word] == b'=' {
                    return true;
                }
            }
        }
        search_from = start + 2;
        if search_from >= lower.len() {
            break;
        }
    }
    false
}

/// `POST /app/brands/{key}/logo`.
pub async fn upload_logo(
    State(state): State<AdminState>,
    Path(key): Path<String>,
    cookies: tower_cookies::Cookies,
    session: Option<Extension<SessionData>>,
    mut multipart: Multipart,
) -> Response {
    let Some(Extension(session)) = session else {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    };
    if crate::csrf::require_multipart_csrf(&cookies, &session, &mut multipart)
        .await
        .is_err()
    {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }
    let field = match multipart.next_field().await {
        Ok(Some(field)) if field.name() == Some("file") => field,
        _ => return back_to_edit(&key, "Choose a logo file."),
    };
    let content_type = field.content_type().map(str::to_string).unwrap_or_default();
    if !ALLOWED_LOGO_CONTENT_TYPES.contains(&content_type.as_str()) {
        return back_to_edit(&key, "Logo must be a PNG or an SVG.");
    }
    let Ok(bytes) = field.bytes().await else {
        return back_to_edit(&key, "Could not read the uploaded file.");
    };
    if bytes.is_empty() {
        return back_to_edit(&key, "Choose a logo file.");
    }
    if bytes.len() > MAX_LOGO_BYTES {
        return back_to_edit(&key, "Logo must be at most 512 KB.");
    }
    if content_type == "image/svg+xml" && !svg_is_safe(&bytes) {
        return back_to_edit(
            &key,
            "That SVG contains a script, an event handler, a foreignObject, or an external reference.",
        );
    }
    match state.attachment_scanner.scan(&bytes).await {
        Ok(ScanVerdict::Clean) => {}
        Ok(ScanVerdict::Found { .. }) => {
            return back_to_edit(&key, "That file failed a malware scan.");
        }
        Err(_) => return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }

    let brand = match authorized_brand_for_key(&state, &session, &key).await {
        Ok(brand) => brand,
        Err(response) => return response,
    };
    let ext = if content_type == "image/svg+xml" {
        "svg"
    } else {
        "png"
    };
    let object_key = format!("brands/{key}/logo.{ext}");
    if let Err(error) = state
        .assets_storage
        .put(&object_key, &bytes, &content_type)
        .await
    {
        tracing::error!(error = %error, brand_key = %key, "brand logo upload: storage write failed");
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match store::brands::set_logo(
        &state.surreal,
        session.role,
        session.person_id,
        brand.id,
        &object_key,
        &content_type,
    )
    .await
    {
        Ok(_) => Redirect::to(&format!("/app/brands/{key}/edit")).into_response(),
        Err(store::brands::BrandError::NotAuthorized) => {
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => back_to_edit(&key, &error.user_message()),
    }
}

/// `POST /app/brands/{key}/font`. Field order matters: the multipart form
/// posts CSRF, then `family`, then `licence`, then `file`, matching the
/// order `webapp::brands_edit`'s font form renders them.
pub async fn upload_font(
    State(state): State<AdminState>,
    Path(key): Path<String>,
    cookies: tower_cookies::Cookies,
    session: Option<Extension<SessionData>>,
    mut multipart: Multipart,
) -> Response {
    let Some(Extension(session)) = session else {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    };
    if crate::csrf::require_multipart_csrf(&cookies, &session, &mut multipart)
        .await
        .is_err()
    {
        return axum::http::StatusCode::FORBIDDEN.into_response();
    }

    let Some(family) = read_text_field(&mut multipart, "family").await else {
        return back_to_edit(&key, "Name the font family.");
    };
    let Some(licence) = read_text_field(&mut multipart, "licence").await else {
        return back_to_edit(&key, "Pick a font licence.");
    };
    if family.trim().is_empty() {
        return back_to_edit(&key, "Name the font family.");
    }
    if !store::brands::FONT_LICENCES.contains(&licence.as_str()) {
        return back_to_edit(
            &key,
            &format!(
                "Pick a font licence: {}.",
                store::brands::FONT_LICENCES.join(", ")
            ),
        );
    }

    let field = match multipart.next_field().await {
        Ok(Some(field)) if field.name() == Some("file") => field,
        _ => return back_to_edit(&key, "Choose a .woff2 font file."),
    };
    let content_type = field.content_type().map(str::to_string).unwrap_or_default();
    let file_name = field.file_name().unwrap_or_default().to_string();
    if !file_name.to_lowercase().ends_with(".woff2")
        && !ALLOWED_FONT_CONTENT_TYPES.contains(&content_type.as_str())
    {
        return back_to_edit(&key, "Font must be a .woff2 file.");
    }
    let Ok(bytes) = field.bytes().await else {
        return back_to_edit(&key, "Could not read the uploaded file.");
    };
    if bytes.is_empty() {
        return back_to_edit(&key, "Choose a .woff2 font file.");
    }
    if bytes.len() > MAX_FONT_BYTES {
        return back_to_edit(&key, "Font must be at most 2 MB.");
    }
    match state.attachment_scanner.scan(&bytes).await {
        Ok(ScanVerdict::Clean) => {}
        Ok(ScanVerdict::Found { .. }) => {
            return back_to_edit(&key, "That file failed a malware scan.");
        }
        Err(_) => return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }

    let brand = match authorized_brand_for_key(&state, &session, &key).await {
        Ok(brand) => brand,
        Err(response) => return response,
    };
    let sha = store::assets::sha256_hex(&bytes);
    let object_key = format!("fonts/brands/{key}/{sha}.woff2");
    if let Err(error) = state
        .assets_storage
        .put(&object_key, &bytes, "font/woff2")
        .await
    {
        tracing::error!(error = %error, brand_key = %key, "brand font upload: storage write failed");
        return axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match store::brands::set_font(
        &state.surreal,
        session.role,
        session.person_id,
        brand.id,
        family.trim(),
        &object_key,
        &licence,
    )
    .await
    {
        Ok(_) => Redirect::to(&format!("/app/brands/{key}/edit")).into_response(),
        Err(store::brands::BrandError::NotAuthorized) => {
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => back_to_edit(&key, &error.user_message()),
    }
}

async fn read_text_field(multipart: &mut Multipart, expected_name: &str) -> Option<String> {
    let field = multipart.next_field().await.ok()??;
    if field.name() != Some(expected_name) {
        return None;
    }
    field.text().await.ok()
}

async fn authorized_brand_for_key(
    state: &AdminState,
    session: &SessionData,
    key: &str,
) -> Result<store::brands::Brand, Response> {
    match store::brands::find_by_key_for_actor(&state.surreal, session.role, session.person_id, key)
        .await
    {
        Ok(brand) => Ok(brand),
        Err(
            store::brands::BrandError::NotAuthorized
            | store::brands::BrandError::NoSuchBrand(_)
            | store::brands::BrandError::NoSuchFirm(_),
        ) => Err(axum::http::StatusCode::NOT_FOUND.into_response()),
        Err(error) => {
            tracing::error!(error = %error, brand_key = %key, "brand authorization lookup failed");
            Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{contains_event_handler_attribute, svg_is_safe};

    #[test]
    fn refuses_a_script_tag() {
        assert!(!svg_is_safe(br"<svg><script>alert(1)</script></svg>"));
    }

    #[test]
    fn refuses_an_event_handler_attribute() {
        assert!(!svg_is_safe(br#"<svg onload="alert(1)"></svg>"#));
        assert!(contains_event_handler_attribute("<svg onload=\"x\">"));
    }

    #[test]
    fn refuses_a_foreign_object() {
        assert!(!svg_is_safe(
            br"<svg><foreignObject><body>x</body></foreignObject></svg>"
        ));
    }

    #[test]
    fn refuses_an_external_reference() {
        assert!(!svg_is_safe(
            br#"<svg><image href="http://evil.example/x.png"/></svg>"#
        ));
    }

    #[test]
    fn accepts_an_ordinary_svg() {
        assert!(svg_is_safe(
            br##"<svg viewBox="0 0 10 10"><rect width="10" height="10" fill="#007c91"/></svg>"##
        ));
    }

    #[test]
    fn does_not_false_positive_on_the_word_on_in_text() {
        assert!(!contains_event_handler_attribute("stone on stone"));
    }
}
