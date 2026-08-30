//! CSRF protection keyed on the **credential**, not the content type.
//!
//! The credential a browser auto-attaches cross-site is the session
//! cookie, so that — not the request body's shape — is what a CSRF
//! defense has to guard. This middleware runs on every state-changing
//! request under `/app/*` and the mutating `/app/api/*`
//! routes:
//!
//!   - **No session cookie** → pass through. This is the bearer
//!     exemption: a `navigator` CLI / MCP / A2A caller presents its
//!     credential as `Authorization: Bearer …` and carries no cookie
//!     (the browser never auto-attaches a bearer token cross-site, so it
//!     is not CSRF-exposed — see [`crate::auth::inject_bearer_session`]).
//!     Anonymous requests also land here and fail at the auth layer
//!     instead, which keeps the dev / tests path working without
//!     per-test CSRF token plumbing.
//!   - **Cookie-authenticated** (a valid session cookie is present) →
//!     require a valid CSRF token. Every authenticated session carries a
//!     32-byte url-safe random `csrf_token` in its signed cookie; the
//!     request must echo it back, either in the `X-CSRF-Token` header
//!     (JSON / HTMX) or the hidden `_csrf` form field (classic form
//!     POST). Constant-time compare; missing or mismatched → 403.
//!
//! Cookie-authenticated state changes additionally get an
//! Origin/Referer check as defense-in-depth: when the browser sends an
//! `Origin` (or `Referer`) whose host does not match the request's
//! `Host`, the request is rejected outright, independent of the token.
//! An absent Origin/Referer is not treated as failure — the token stays
//! the primary control, the origin check the belt-and-suspenders layer
//! OWASP recommends alongside it.

use axum::body::{to_bytes, Body};
use axum::extract::{Multipart, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tower_cookies::Cookies;

use crate::session::{SessionData, SessionStore, SESSION_COOKIE_NAME};

/// Maximum form body we'll buffer before refusing — 1 MiB. Admin
/// forms are tiny in practice; this just bounds the middleware's
/// allocations.
pub const MAX_FORM_BODY_BYTES: usize = 1024 * 1024;

/// Form field name carrying the CSRF token.
pub const CSRF_FIELD: &str = "_csrf";

/// Header name carrying the CSRF token on JSON / HTMX requests, where
/// there is no form field to read it from. Case-insensitive on the
/// wire; this is the canonical spelling.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// How strictly a cookie-authenticated state change is checked when it
/// carries neither the `X-CSRF-Token` header nor a form `_csrf` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsrfMode {
    /// The JSON `/app/api/*` surface. A cookie-authenticated write must
    /// prove intent: it needs a valid token (header or form field) and a
    /// same-origin `Origin`/`Referer`, or it is rejected. This is what
    /// closes the cookie-authenticated-JSON hole — a JSON body with no
    /// token no longer slips through on content type.
    Strict,
    /// The classic `/app` form surfaces. Enforce the
    /// token on the `X-CSRF-Token` header and the `_csrf` form field, and
    /// pass a bodyless HTMX `DELETE` (which carries neither) through
    /// unchanged. A `multipart/form-data` upload also passes through here
    /// because reading its `_csrf` field would mean buffering the whole
    /// (potentially large) upload; those handlers instead call
    /// [`require_multipart_csrf`], which reads the token from the form's
    /// first field without buffering the file.
    Form,
}

pub async fn require_csrf(
    State((sessions, mode)): State<(SessionStore, CsrfMode)>,
    cookies: Cookies,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Decode the session cookie up front (if present + valid) so
    // GET handlers can render the per-session CSRF token into their
    // forms via the request's `Extension<SessionData>`. Never clobber a
    // principal an upstream layer already resolved: on the API router
    // `inject_bearer_session` and `require_policy` run *before* this
    // layer, so a request carrying both a bearer credential and a cookie
    // is already authorized as the bearer principal. Overwriting it here
    // would let policy check one principal and the handler run as
    // another — the same no-clobber rule `auth::inject_bearer_session`
    // follows.
    let session = cookies
        .get(SESSION_COOKIE_NAME)
        .and_then(|c| sessions.decode(c.value()));
    if req.extensions().get::<SessionData>().is_none() {
        if let Some(s) = session.clone() {
            req.extensions_mut().insert(s);
        }
    }

    // Only state-changing methods are CSRF-checked.
    if !matches!(
        req.method(),
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    ) {
        return Ok(next.run(req).await);
    }

    // No session cookie → not cookie-authenticated, so nothing a browser
    // auto-attaches cross-site and nothing to forge against. Pass
    // through. This is the bearer exemption: a `navigator` CLI / MCP /
    // A2A caller presents its credential as `Authorization: Bearer …`
    // and carries no cookie (see `auth::inject_bearer_session`), so it
    // lands here. Anonymous requests land here too and fail at the auth
    // layer instead.
    let Some(session) = session else {
        return Ok(next.run(req).await);
    };

    // Cookie-authenticated from here down. Defense-in-depth on the API
    // surface: a cross-site Origin/Referer is rejected outright,
    // independent of the token check below.
    if mode == CsrfMode::Strict && origin_is_cross_site(req.headers()) {
        return Err(StatusCode::FORBIDDEN);
    }

    // The token may arrive in the `X-CSRF-Token` header (JSON / HTMX) or
    // the `_csrf` form field (classic form POST). Prefer the header so a
    // JSON body is never buffered here.
    if let Some(header_token) = req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) {
        if constant_time_eq(header_token.as_bytes(), session.csrf_token.as_bytes()) {
            return Ok(next.run(req).await);
        }
        return Err(StatusCode::FORBIDDEN);
    }

    // No header — fall back to the `_csrf` form field, which only a
    // form-encoded body carries.
    let ct = content_type(req.headers());
    if ct != "application/x-www-form-urlencoded" {
        // No token anywhere. On the API surface that is the hole we are
        // closing, so reject. On the form surfaces a tokenless non-form
        // body (multipart upload, bodyless HTMX) keeps its long-standing
        // passthrough — the browser can't forge those cross-site without
        // hitting `SameSite=Lax` and same-origin policy first.
        return match mode {
            CsrfMode::Strict => Err(StatusCode::FORBIDDEN),
            CsrfMode::Form => Ok(next.run(req).await),
        };
    }

    // Read the body so we can extract `_csrf`. We then rebuild the
    // request so the downstream `Form<T>` extractor still parses.
    let (parts, body) = req.into_parts();
    let bytes = to_bytes(body, MAX_FORM_BODY_BYTES)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    let body_str = std::str::from_utf8(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    let submitted = extract_csrf_field(body_str).ok_or(StatusCode::FORBIDDEN)?;

    if !constant_time_eq(submitted.as_bytes(), session.csrf_token.as_bytes()) {
        return Err(StatusCode::FORBIDDEN);
    }

    let req = Request::from_parts(parts, Body::from(bytes));
    Ok(next.run(req).await)
}

/// Enforce CSRF on a cookie-authenticated `multipart/form-data` upload by
/// reading the token from the form's **first** field.
///
/// [`require_csrf`] can't check `_csrf` on a multipart body without
/// buffering the whole upload — a file can be many megabytes — so it lets
/// multipart through and each upload handler calls this instead. Every
/// upload form is a `FormCard`, which renders the hidden `_csrf` input as
/// the first element inside the `<form>`, so the
/// token is the first part on the wire: this advances `multipart` by
/// exactly one field, constant-time-compares it to the session token, and
/// leaves the remaining fields (the file included) for the handler to
/// stream. Rejecting before the file field is read keeps a forged upload
/// from spending memory on the payload.
///
/// The exemption is the same credential-keyed rule as [`require_csrf`]:
/// with no session cookie the caller is a bearer (`navigator` CLI / MCP /
/// A2A) or anonymous — nothing a browser auto-attaches cross-site — so
/// there is nothing to forge and the check is skipped. `session` is the
/// principal the handler acts as; for a cookie request it is decoded from
/// that cookie, so its `csrf_token` is the value the form echoes back.
///
/// # Errors
/// Returns `FORBIDDEN` when the request is cookie-authenticated but the
/// first field is absent, is not `_csrf`, or does not match the session
/// token; `BAD_REQUEST` when the multipart stream is malformed.
pub async fn require_multipart_csrf(
    cookies: &Cookies,
    session: &SessionData,
    multipart: &mut Multipart,
) -> Result<(), StatusCode> {
    // Bearer / anonymous: no session cookie, so not cookie-authenticated
    // and not CSRF-exposed. Matches the passthrough in `require_csrf`.
    if cookies.get(SESSION_COOKIE_NAME).is_none() {
        return Ok(());
    }
    let field = multipart
        .next_field()
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .ok_or(StatusCode::FORBIDDEN)?;
    if field.name() != Some(CSRF_FIELD) {
        return Err(StatusCode::FORBIDDEN);
    }
    let submitted = field.text().await.map_err(|_| StatusCode::BAD_REQUEST)?;
    if !constant_time_eq(submitted.as_bytes(), session.csrf_token.as_bytes()) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// The request's `Content-Type` with any `; charset=…` parameter and
/// surrounding whitespace stripped, lowercased for comparison.
fn content_type(headers: &HeaderMap) -> String {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| {
            s.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

/// Defense-in-depth Origin/Referer check for a cookie-authenticated,
/// state-changing request. Returns `true` only when the browser sent an
/// `Origin` (or, failing that, a `Referer`) AND a `Host` AND the source
/// header's host does not match `Host`. A missing `Origin`/`Referer` or
/// `Host` yields `false` — the token check remains the primary control,
/// so an absent header is never treated as an attack.
fn origin_is_cross_site(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(source) = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    !origin_host_matches(source, host)
}

/// Compare the host authority of an `Origin`/`Referer` value against the
/// request's `Host`. `source` is a URL such as `https://example.com`
/// (Origin) or `https://example.com/some/path` (Referer); we strip the
/// scheme and take the authority up to the first `/`. The comparison is
/// ASCII-case-insensitive because DNS host names are (a same-origin
/// `Host: App.Example` / `Origin: https://app.example` pair must match).
/// Port is part of the authority, so `example.com` and `example.com:8443`
/// differ, as they should.
#[must_use]
fn origin_host_matches(source: &str, host: &str) -> bool {
    let after_scheme = source.split_once("://").map_or(source, |(_, rest)| rest);
    let source_authority = after_scheme.split('/').next().unwrap_or("");
    source_authority.eq_ignore_ascii_case(host)
}

/// Pull the first `_csrf=<value>` field out of a form-encoded body.
/// Returns the raw url-safe-base64 value (no `+`, `/`, `=` to
/// percent-decode).
#[must_use]
pub fn extract_csrf_field(body: &str) -> Option<String> {
    body.split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{CSRF_FIELD}=")))
        .map(ToString::to_string)
}

/// Constant-time `==` for byte slices of any length. Returns false
/// for length mismatch without examining contents.
#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::{constant_time_eq, extract_csrf_field, origin_host_matches};

    #[test]
    fn origin_host_matches_bare_origin() {
        assert!(origin_host_matches("https://app.example", "app.example"));
        assert!(origin_host_matches("http://app.example", "app.example"));
    }

    #[test]
    fn origin_host_matches_referer_with_path() {
        assert!(origin_host_matches(
            "https://app.example/app/admin/people",
            "app.example"
        ));
    }

    #[test]
    fn origin_host_matches_rejects_cross_site() {
        assert!(!origin_host_matches("https://evil.example", "app.example"));
        assert!(!origin_host_matches(
            "https://evil.example/steal",
            "app.example"
        ));
    }

    #[test]
    fn origin_host_matches_is_case_insensitive() {
        // DNS host names are case-insensitive, so a same-origin write
        // must match regardless of casing on either side.
        assert!(origin_host_matches("https://App.Example", "app.example"));
        assert!(origin_host_matches("https://app.example", "APP.EXAMPLE"));
        assert!(origin_host_matches(
            "https://App.Example:8443/x",
            "app.example:8443"
        ));
    }

    #[test]
    fn origin_host_matches_is_port_sensitive() {
        assert!(!origin_host_matches(
            "https://app.example:8443",
            "app.example"
        ));
        assert!(origin_host_matches(
            "https://app.example:8443",
            "app.example:8443"
        ));
    }

    #[test]
    fn extracts_csrf_field_from_form_body() {
        assert_eq!(
            extract_csrf_field("name=Libra&_csrf=ABC123&email=a%40b").as_deref(),
            Some("ABC123"),
        );
    }

    #[test]
    fn extracts_csrf_field_when_first() {
        assert_eq!(
            extract_csrf_field("_csrf=XYZ&name=Libra").as_deref(),
            Some("XYZ"),
        );
    }

    #[test]
    fn returns_none_when_csrf_field_missing() {
        assert!(extract_csrf_field("name=Libra&email=a%40b").is_none());
        assert!(extract_csrf_field("").is_none());
    }

    #[test]
    fn constant_time_eq_matches_equal_slices() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn constant_time_eq_rejects_unequal_slices() {
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
    }
}
