//! Anonymous lead capture for the firm's public pages.
//!
//! The route is deliberately narrow: it stores an inquiry, never creates a
//! [`person`](store::persons), and answers accepted and rejected submissions
//! with the same neutral redirect. The form's CSRF token is minted by the
//! public Dioxus page middleware, while the write is rate-limited here at the
//! route boundary.

use axum::{
    extract::{DefaultBodyLimit, Extension, Form, Request, State},
    http::StatusCode,
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Redirect, Response},
    routing::post,
    Router,
};
use chrono::Utc;
use serde::Deserialize;
use tower_cookies::Cookies;
use views::brand::BrandKey;

use crate::{password_reset, rate_limit::RateLimit, AppState, SessionStore};

/// The dedicated double-submit cookie used by every public lead form.
pub const LEAD_CSRF_COOKIE_NAME: &str = "navigator_lead_csrf";

/// The maximum encoded form body. The fields are all short, and a bounded
/// body keeps malformed submissions from becoming an allocation surface.
pub const MAX_BODY_BYTES: usize = 8 * 1024;

#[derive(Debug, Deserialize)]
struct LeadForm {
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    sms_consent: Option<String>,
    #[serde(default)]
    website: String,
    #[serde(default)]
    csrf_token: String,
    #[serde(default)]
    source_path: String,
    #[serde(default)]
    consent_version: String,
}

/// Build the anonymous lead write route with its own rate-limit boundary and
/// bounded request body.
pub fn routes(rate_limit: RateLimit) -> Router<AppState> {
    Router::new()
        .route("/leads", post(submit))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .route_layer(from_fn_with_state(rate_limit, crate::rate_limit::enforce))
}

/// Mint the page's double-submit token and carry the token plus source path
/// into the server function that renders the form.
pub async fn inject_page_context(
    State((sessions, secure_cookies)): State<(SessionStore, bool)>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(cookies) = req.extensions().get::<Cookies>().cloned() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            webapp::error_pages::server_error(),
        )
            .into_response();
    };
    let csrf_token =
        password_reset::mint_csrf_with(&sessions, secure_cookies, &cookies, LEAD_CSRF_COOKIE_NAME);
    let source_path = req.uri().path().to_string();
    req.extensions_mut()
        .insert(webapp::lead_capture::LeadCaptureContext {
            csrf_token,
            source_path,
        });
    next.run(req).await
}

async fn submit(
    State(state): State<AppState>,
    Extension(brand): Extension<BrandKey>,
    cookies: Cookies,
    form: Result<Form<LeadForm>, axum::extract::rejection::FormRejection>,
) -> Response {
    let Ok(Form(form)) = form else {
        audit(brand, "/leads", "none", "rejected_form");
        return neutral_redirect();
    };

    let source_path = safe_source_path(&form.source_path);
    if !password_reset::verify_csrf_with(
        &state.sessions,
        &cookies,
        &form.csrf_token,
        LEAD_CSRF_COOKIE_NAME,
    ) {
        audit(brand, &source_path, "none", "rejected_csrf");
        return neutral_redirect();
    }
    if !form.website.trim().is_empty() {
        audit(brand, &source_path, "none", "rejected_honeypot");
        return neutral_redirect();
    }

    cookies.add(password_reset::csrf_cookie_with_ttl(
        LEAD_CSRF_COOKIE_NAME,
        String::new(),
        secure_cookies(&state),
        0,
    ));

    let email = form.email.trim();
    let phone = form.phone.trim();
    if !valid_email(email)
        || !valid_phone(phone)
        || form.consent_version.trim().is_empty()
        || source_path == "/leads"
    {
        audit(brand, &source_path, "none", "rejected_validation");
        return neutral_redirect();
    }

    let phone = (!phone.is_empty()).then(|| phone.to_string());
    let sms_consented_at =
        (form.sms_consent.as_deref() == Some("on") && phone.is_some()).then(Utc::now);
    let new_lead = store::leads::NewLead {
        email: email.to_string(),
        phone,
        brand_key: brand.as_str().to_string(),
        source_path: source_path.clone(),
        consent_version: form.consent_version,
        consented_at: Utc::now(),
        sms_consented_at,
    };
    match store::leads::record(&state.surreal, &new_lead).await {
        Ok(lead) => {
            let lead_id = lead.id.to_string();
            audit(brand, &source_path, &lead_id, "accepted");
        }
        Err(error) => {
            tracing::error!(error = %error, brand = brand.as_str(), source_path = %source_path, "lead submission failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    neutral_redirect()
}

fn neutral_redirect() -> Response {
    Redirect::to("/contact/sent").into_response()
}

fn secure_cookies(state: &AppState) -> bool {
    crate::secure_cookies(state)
}

fn valid_email(email: &str) -> bool {
    email.len() <= 254 && email.contains('@')
}

fn valid_phone(phone: &str) -> bool {
    phone.chars().count() <= 32
        && phone
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | ' ' | '-' | '(' | ')'))
}

fn safe_source_path(path: &str) -> String {
    let path = path.trim();
    if path.starts_with('/')
        && path.len() <= 256
        && !path.chars().any(char::is_control)
        && !path.contains("//")
    {
        path.to_string()
    } else {
        "/leads".to_string()
    }
}

fn audit(brand: BrandKey, source_path: &str, lead_id: &str, outcome: &str) {
    tracing::info!(
        target: "audit",
        lead_id,
        brand = brand.as_str(),
        source_path,
        outcome,
        "lead submission"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::{body::Body, http::Request as HttpRequest, Router};
    use std::time::Duration;
    use tower::ServiceExt;
    use tower_cookies::CookieManagerLayer;

    const CONSENT: &str = "By sending this, you agree that Neon Law may email you about this inquiry. Sending it does not make you a client, and nothing on this page is legal advice. See our Privacy Policy.";

    fn encoded(fields: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.extend_pairs(fields.iter().copied());
        serializer.finish()
    }

    fn csrf_cookie(sessions: &SessionStore, token: &str) -> String {
        sessions.encode_signed_bytes(token.as_bytes())
    }

    async fn test_app(
        db: store::surreal::SurrealDb,
        rate_limit: RateLimit,
    ) -> (Router, SessionStore) {
        let state = crate::test_support::app_state(db).await;
        let sessions = state.sessions.clone();
        let router = Router::new()
            .merge(routes(rate_limit))
            .layer(axum::middleware::from_fn(
                |mut req: Request, next: Next| async move {
                    req.extensions_mut().insert(BrandKey::Neon);
                    next.run(req).await
                },
            ))
            .layer(CookieManagerLayer::new())
            .with_state(state);
        (router, sessions)
    }

    fn request(body: String, cookie: Option<String>) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder()
            .method("POST")
            .uri("/leads")
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = cookie {
            builder = builder.header("cookie", format!("{LEAD_CSRF_COOKIE_NAME}={cookie}"));
        }
        builder.body(Body::from(body)).unwrap()
    }

    fn body(token: &str, email: &str, phone: &str, sms: bool) -> String {
        let mut fields = vec![
            ("email", email),
            ("phone", phone),
            ("website", ""),
            ("csrf_token", token),
            ("source_path", "/services"),
            ("consent_version", CONSENT),
        ];
        if sms {
            fields.push(("sms_consent", "on"));
        }
        encoded(&fields)
    }

    #[tokio::test]
    async fn accepts_a_lead_without_creating_a_person() {
        let db = store::test_support::mem_surreal().await;
        let (app, sessions) = test_app(db.clone(), RateLimit::disabled()).await;
        let token = "lead-csrf";
        let response = app
            .oneshot(request(
                body(token, "visitor@example.com", "", false),
                Some(csrf_cookie(&sessions, token)),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(response.headers()["location"], "/contact/sent");
        let leads = store::leads::list(&db).await.unwrap();
        assert_eq!(leads.len(), 1);
        assert_eq!(leads[0].email_lower, "visitor@example.com");
        assert!(store::persons::find_by_email_ci(&db, "visitor@example.com")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn records_sms_consent_only_when_a_phone_is_present() {
        let db = store::test_support::mem_surreal().await;
        let (app, sessions) = test_app(db.clone(), RateLimit::disabled()).await;
        let token = "sms-csrf";
        let response = app
            .oneshot(request(
                body(token, "sms@example.com", "+ ()", true),
                Some(csrf_cookie(&sessions, token)),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let leads = store::leads::list(&db).await.unwrap();
        assert!(leads[0].sms_consented_at.is_some());

        let db = store::test_support::mem_surreal().await;
        let (app, sessions) = test_app(db.clone(), RateLimit::disabled()).await;
        let response = app
            .oneshot(request(
                body(token, "email@example.com", "+ ()", false),
                Some(csrf_cookie(&sessions, token)),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let leads = store::leads::list(&db).await.unwrap();
        assert!(leads[0].sms_consented_at.is_none());
    }

    #[tokio::test]
    async fn rejects_invalid_submissions_with_the_same_redirect_and_no_row() {
        for (email, phone, honeypot, token_ok) in [
            ("no-at.example.com", "", "", true),
            ("too-long@example.com", "!", "", true),
            ("honeypot@example.com", "", "filled", true),
            ("csrf@example.com", "", "", false),
        ] {
            let db = store::test_support::mem_surreal().await;
            let (app, sessions) = test_app(db.clone(), RateLimit::disabled()).await;
            let token = "invalid-csrf";
            let form = encoded(&[
                ("email", email),
                ("phone", phone),
                ("website", honeypot),
                ("csrf_token", token),
                ("source_path", "/contact"),
                ("consent_version", CONSENT),
            ]);
            let response = app
                .oneshot(request(
                    form,
                    token_ok.then(|| csrf_cookie(&sessions, token)),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(store::leads::list(&db).await.unwrap().is_empty());
            assert!(store::persons::find_by_email_ci(&db, email)
                .await
                .unwrap()
                .is_none());
        }

        let db = store::test_support::mem_surreal().await;
        let (app, sessions) = test_app(db.clone(), RateLimit::disabled()).await;
        let token = "oversized-csrf";
        let oversized_email = "x".repeat(MAX_BODY_BYTES);
        let form = encoded(&[
            ("email", oversized_email.as_str()),
            ("csrf_token", token),
            ("source_path", "/contact"),
            ("consent_version", CONSENT),
        ]);
        let response = app
            .oneshot(request(form, Some(csrf_cookie(&sessions, token))))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(store::leads::list(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rate_limits_a_burst() {
        let db = store::test_support::mem_surreal().await;
        let (app, sessions) = test_app(db, RateLimit::new(1, Duration::from_mins(1))).await;
        let token = "rate-csrf";
        let cookie = csrf_cookie(&sessions, token);
        let first = app
            .clone()
            .oneshot(request(
                body(token, "first@example.com", "", false),
                Some(cookie.clone()),
            ))
            .await
            .unwrap();
        let second = app
            .oneshot(request(
                body(token, "second@example.com", "", false),
                Some(cookie),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::SEE_OTHER);
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn audit_contains_the_lead_id_and_source_but_not_the_email() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);
        impl Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buffer {
            type Writer = Buffer;
            fn make_writer(&'a self) -> Buffer {
                self.clone()
            }
        }

        crate::test_tracing::ensure_callsite_interest();
        let output = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Buffer(output.clone()))
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let db = store::test_support::mem_surreal().await;
        let (app, sessions) = test_app(db, RateLimit::disabled()).await;
        let token = "audit-csrf";
        let email = "audit@example.com";
        let response = app
            .oneshot(request(
                body(token, email, "", false),
                Some(csrf_cookie(&sessions, token)),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let output = String::from_utf8(output.lock().unwrap().clone()).unwrap();
        assert!(
            output.contains("source_path=\"/services\""),
            "audit: {output}"
        );
        assert!(output.contains("lead_id="), "audit: {output}");
        assert!(!output.contains(email), "email leaked into audit: {output}");
    }
}
