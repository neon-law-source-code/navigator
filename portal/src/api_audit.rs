//! Who asked for which API endpoint.
//!
//! One `target: "audit"` event per request to the private `/app/api` surface,
//! naming the acting person, their tier, the endpoint, and how it was
//! answered. The stream lands in the same Iceberg archive lane as the other
//! audit events (agent authorization, rate-limit refusals, OAuth outcomes), so
//! "who read the people directory last Tuesday" is a query rather than an
//! archaeology exercise.
//!
//! # What is deliberately not in the event
//!
//! **No query string and no request body.** The event carries `uri().path()`,
//! never `path_and_query()`, and never reads the body. That is not tidiness —
//! a `?email=` filter or a `POST` body on this surface is client content, and
//! client content does not belong in telemetry. The path itself does carry
//! record identifiers (`/app/api/people/{id}`), and that is the point: an
//! access log that cannot say *which* record was read answers nothing.
//!
//! # Where it sits in the stack
//!
//! Between [`crate::auth::require_session`] and [`crate::policy::require_policy`]
//! (see [`crate::bootstrap`]). That position is load-bearing in both
//! directions:
//!
//! - *After* the session boundary, so the session is already resolved into
//!   request extensions and the event can name a person rather than a cookie.
//! - *Outside* the policy gate, so a **refused** request is logged too. A gate
//!   that only records successful reads is the wrong half of an audit trail —
//!   the denied attempt is usually the interesting one.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::session::SessionData;

/// Emit one audit event per `/app/api` request, then pass it along unchanged.
///
/// Reads the session from request extensions rather than decoding a cookie:
/// [`crate::auth::require_session`] has already done that work, and repeating
/// it would let this layer disagree with the gate about who is calling.
pub async fn audit_api_request(req: Request, next: Next) -> Response {
    // `path()`, never `path_and_query()` — see the module docs.
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let session = req.extensions().get::<SessionData>().cloned();

    let response = next.run(req).await;

    let (actor, role) = session
        .as_ref()
        .map_or(("anonymous".to_string(), "none"), |s| {
            (actor_id(s), s.role.as_str())
        });

    tracing::info!(
        target: "audit",
        event = "api.request",
        user_id = %actor,
        role = role,
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        "User {actor} with role {role} requested \"{path}\""
    );

    response
}

/// The acting person's identifier, preferring our own `persons.id` over the
/// IdP's subject.
///
/// `person_id` is the id every other table joins on, so it is the one that
/// makes this stream correlatable with the rest of the store. It is `None` on
/// the enforced-bearer path, where the claims carry a subject but no local row
/// has been resolved — the `sub` is then the only identifier there is, and an
/// event naming it beats an event naming nobody.
fn actor_id(session: &SessionData) -> String {
    session
        .person_id
        .map_or_else(|| session.sub.clone(), |id| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;
    use uuid::Uuid;

    fn session_with(person_id: Option<Uuid>, role: store::persons::Role) -> SessionData {
        SessionData {
            sub: "oidc-subject-1".to_string(),
            email: Some("lawyer@neonlaw.com".to_string()),
            person_id,
            exp: i64::MAX,
            role,
            csrf_token: String::new(),
            source: crate::session::SessionSource::Browser,
            provider: None,
            impersonation: None,
        }
    }

    /// The layer is transparent: it observes and passes the response through
    /// untouched, so adding it can never change what a caller receives.
    #[tokio::test]
    async fn the_audited_response_is_unchanged() {
        let app = Router::new()
            .route("/app/api/people", get(|| async { "the directory" }))
            .layer(axum::middleware::from_fn(audit_api_request));

        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/app/api/people?email=someone@example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"the directory");
    }

    /// The event itself: the sentence a reader wants, the fields a query wants,
    /// and — the load-bearing half — no query string.
    #[tokio::test]
    async fn the_event_names_the_person_the_role_and_the_endpoint_but_no_params() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Buf {
                self.clone()
            }
        }

        crate::test_tracing::ensure_callsite_interest();
        let person = Uuid::new_v4();
        let session = session_with(Some(person), store::persons::Role::Lawyer);
        let app = Router::new()
            .route("/app/api/people", get(|| async { "the directory" }))
            .layer(axum::middleware::from_fn(audit_api_request))
            // Stands in for `require_session`, which is what puts the resolved
            // session in extensions in the real stack.
            .layer(axum::middleware::from_fn(
                move |mut req: Request, next: Next| {
                    let session = session.clone();
                    async move {
                        req.extensions_mut().insert(session);
                        next.run(req).await
                    }
                },
            ));

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::INFO)
            .with_writer(Buf(buf.clone()))
            .finish();

        // `#[tokio::test]` is a current-thread runtime, so the event fires on
        // this thread and the thread-local default captures it.
        {
            let _guard = tracing::subscriber::set_default(subscriber);
            let resp = app
                .oneshot(
                    axum::http::Request::builder()
                        // A query string a caller might filter by. It must not
                        // reach the log: on this surface it is client content.
                        .uri("/app/api/people?email=someone@example.com")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let line = logged
            .lines()
            .find(|l| l.contains("api.request"))
            .unwrap_or_else(|| panic!("expected the api.request event in: {logged}"));

        assert!(
            line.contains(&format!(
                "User {person} with role lawyer requested \\\"/app/api/people\\\""
            )),
            "the message must name the person, the role, and the endpoint: {line}"
        );
        for field in [
            "\"role\":\"lawyer\"",
            "\"method\":\"GET\"",
            "\"path\":\"/app/api/people\"",
            "\"status\":200",
        ] {
            assert!(
                line.contains(field),
                "{field} must be a queryable field: {line}"
            );
        }
        assert!(
            line.contains(&format!("\"user_id\":\"{person}\"")),
            "the acting person must be queryable: {line}"
        );
        // The whole point of reading `path()` rather than `path_and_query()`.
        assert!(
            !line.contains("someone@example.com") && !line.contains("email="),
            "no query parameter may reach the audit stream: {line}"
        );
    }

    /// A refused request is audited too — the denied attempt is usually the
    /// interesting one, which is why the layer sits outside the policy gate.
    #[tokio::test]
    async fn a_refused_request_is_still_audited() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for Buf {
            type Writer = Buf;
            fn make_writer(&'a self) -> Buf {
                self.clone()
            }
        }

        crate::test_tracing::ensure_callsite_interest();
        let session = session_with(Some(Uuid::new_v4()), store::persons::Role::Client);
        let app = Router::new()
            .route("/app/api/people", get(|| async { StatusCode::FORBIDDEN }))
            .layer(axum::middleware::from_fn(audit_api_request))
            .layer(axum::middleware::from_fn(
                move |mut req: Request, next: Next| {
                    let session = session.clone();
                    async move {
                        req.extensions_mut().insert(session);
                        next.run(req).await
                    }
                },
            ));

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::INFO)
            .with_writer(Buf(buf.clone()))
            .finish();

        {
            let _guard = tracing::subscriber::set_default(subscriber);
            let _ = app
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/app/api/people")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
        }

        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        let line = logged
            .lines()
            .find(|l| l.contains("api.request"))
            .unwrap_or_else(|| panic!("expected the api.request event in: {logged}"));
        assert!(
            line.contains("\"status\":403") && line.contains("\"role\":\"client\""),
            "a refusal must be recorded with its status and the tier refused: {line}"
        );
    }

    /// `person_id` is preferred, and the `sub` is the fallback rather than a
    /// blank — an event that names nobody is not an audit record.
    #[test]
    fn actor_prefers_the_local_person_row_and_falls_back_to_the_subject() {
        let id = Uuid::new_v4();
        assert_eq!(
            actor_id(&session_with(Some(id), store::persons::Role::Lawyer)),
            id.to_string()
        );
        assert_eq!(
            actor_id(&session_with(None, store::persons::Role::Lawyer)),
            "oidc-subject-1"
        );
    }
}
