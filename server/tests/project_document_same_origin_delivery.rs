//! `GET /app/projects/:code/documents/:doc_id/download` serves the bytes
//! same-origin, and decides `Content-Disposition` itself.
//!
//! LAW-22 / ENG-651: the route used to answer `307` with a `Location` on
//! the storage origin, which closed both ways a Project portal could render
//! a filed document — `pdf.js` follows the redirect cross-origin into a
//! bucket with no CORS policy, and an `<iframe>` re-evaluates CSP on the
//! redirect hop against a `frame-src` that falls back to `default-src
//! 'self'`. A top-level Download link still worked, so the failure showed up
//! only as a blank viewer.
//!
//! `docs/signed-url-delivery-audit.md` Finding 7 chose proxying the bytes
//! over admitting the storage origin to the portal CSP. These tests pin the
//! three halves of that: no redirect, an inline allowlist the caller cannot
//! talk its way past, and the two headers backstopping it.
//!
//! The storage double here *can* sign. That is the point — the fixture the
//! sibling ACL suite uses is `FsStorage`, whose `signed_url` is
//! `Unsupported`, so it would take the streaming path even before this
//! change and prove nothing. A backend that returns a URL and is still not
//! redirected through is the actual evidence.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::documents::{source, IngestArgs};
use store::persons::Role;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: &str = "test-session-key-not-for-production";

/// The URL [`SigningStore`] hands back. If the route ever redirects again,
/// this string turns up in a `Location` header and the assertions name it.
const SIGNED: &str = "https://storage.googleapis.com/bucket/object?X-Goog-Signature=deadbeef";

/// An [`FsStorage`] that also signs, standing in for GCS in production.
///
/// Every method delegates; only `signed_url` differs, answering `Ok` where
/// `FsStorage` answers `Unsupported`.
struct SigningStore(Arc<dyn cloud::StorageService>);

#[async_trait::async_trait]
impl cloud::StorageService for SigningStore {
    async fn put(
        &self,
        key: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<(), cloud::StorageError> {
        self.0.put(key, bytes, content_type).await
    }

    async fn get(&self, key: &str) -> Result<cloud::StoredObject, cloud::StorageError> {
        self.0.get(key).await
    }

    async fn delete(&self, key: &str) -> Result<(), cloud::StorageError> {
        self.0.delete(key).await
    }

    async fn signed_url(&self, _: &str, _: Duration) -> Result<String, cloud::StorageError> {
        Ok(SIGNED.to_string())
    }
}

struct Fixture {
    app: axum::Router,
    project_code: String,
    cookie: String,
    pdf: Uuid,
    html: Uuid,
}

/// One matter, one lawyer on it, and two client-visible documents that
/// differ only in content type: the PDF the allowlist admits, and the HTML
/// it must refuse however the caller asks.
async fn build_fixture() -> Fixture {
    let surreal = mem_surreal().await;
    let backing: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(
            std::env::temp_dir().join(format!("nav-docproxy-{}", Uuid::now_v7())),
        )
        .await
        .unwrap(),
    );
    let storage: Arc<dyn cloud::StorageService> = Arc::new(SigningStore(backing));

    let lawyer = store::persons::create(
        &surreal,
        &store::persons::NewPerson::with_role("Vega", "vega@neonlaw.com", Role::Lawyer),
    )
    .await
    .unwrap();
    let proj = store::test_support::seed_project(&surreal, "Widget Works outside counsel").await;
    store::projects::add_participation(&surreal, proj.id, lawyer.id, "lawyer")
        .await
        .unwrap();

    for (filename, content_type, bytes) in [
        ("order.pdf", "application/pdf", b"%PDF-1.7 filed".as_slice()),
        (
            "exhibit.html",
            "text/html",
            b"<script>alert(1)</script>".as_slice(),
        ),
    ] {
        let args = IngestArgs {
            project_id: proj.id,
            source: source::UPLOAD,
            filename,
            kind: "unclassified",
            content_type,
            description: None,
            secondary_storage_key: None,
            visibility: store::documents::visibility::CLIENT,
        };
        portal::matter_documents::record_document(
            &surreal,
            &storage,
            repos::Author {
                name: "Lawyer",
                email: "lawyer@example.com",
            },
            &args,
            bytes,
        )
        .await
        .unwrap();
    }

    let assets = store::assets::for_project(&surreal, proj.id).await.unwrap();
    let id_of = |name: &str| {
        assets
            .iter()
            .find(|a| a.filename.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} asset row"))
            .id
    };

    let sessions = SessionStore::new(KEY);
    let mut session = SessionData::fresh("vega-sub", Role::Lawyer);
    session.person_id = Some(lawyer.id);
    let cookie = format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&session));

    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let runtime = Arc::new(workflows::InMemoryRuntime::new());
    let state = AppState {
        sessions: SessionStore::new(KEY),
        storage: storage.clone(),
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };

    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        project_code: proj.code.clone(),
        cookie,
        pdf: id_of("order.pdf"),
        html: id_of("exhibit.html"),
    }
}

impl Fixture {
    async fn download(&self, doc: Uuid, query: &str) -> axum::http::Response<Body> {
        self.app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/app/projects/{}/documents/{doc}/download{query}",
                        self.project_code
                    ))
                    .header("cookie", &self.cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
}

fn header(response: &axum::http::Response<Body>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// The headline: a backend that can sign is still not redirected through.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_streams_the_bytes_rather_than_redirecting_to_a_signed_url() {
    let f = build_fixture().await;
    let response = f.download(f.pdf, "").await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the route must answer with the bytes, not a redirect"
    );
    assert!(
        response
            .headers()
            .get(axum::http::header::LOCATION)
            .is_none(),
        "no Location header may survive: a redirect hop is what CSP and CORS both refuse"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"%PDF-1.7 filed");
    assert!(
        !String::from_utf8_lossy(&body).contains("X-Goog-Signature"),
        "the signed URL must never reach the browser"
    );
}

/// Every streamed response carries both backstops, whatever the disposition.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streamed_document_carries_nosniff_and_a_sandbox_policy() {
    let f = build_fixture().await;
    for query in ["", "?inline=1"] {
        let response = f.download(f.pdf, query).await;
        assert_eq!(
            header(&response, "x-content-type-options"),
            "nosniff",
            "a caller-typed body must not be sniffed ({query:?})"
        );
        assert_eq!(
            header(&response, "content-security-policy"),
            "sandbox",
            "the sandbox is what holds if the allowlist is ever wrong ({query:?})"
        );
    }
}

/// Without the flag the bytes are saved, not rendered — the matter page's
/// Download link keeps its old behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_without_the_inline_flag_is_an_attachment() {
    let f = build_fixture().await;
    let response = f.download(f.pdf, "").await;
    assert_eq!(
        header(&response, "content-disposition"),
        "attachment; filename=\"order.pdf\""
    );
}

/// A PDF is on the allowlist, so the portal's embedded viewer gets its
/// `inline` — this is the whole point of the change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inline_request_for_a_pdf_is_granted() {
    let f = build_fixture().await;
    let response = f.download(f.pdf, "?inline=1").await;
    assert_eq!(
        header(&response, "content-disposition"),
        "inline; filename=\"order.pdf\"",
        "application/pdf is passive and renders in the viewer"
    );
    assert_eq!(header(&response, "content-type"), "application/pdf");
}

/// The allowlist is the decision, not the query string. Served `inline`
/// same-origin this body would execute as Navigator.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inline_request_for_html_is_refused_and_sent_as_an_attachment() {
    let f = build_fixture().await;
    let response = f.download(f.html, "?inline=1").await;
    assert_eq!(
        header(&response, "content-disposition"),
        "attachment; filename=\"exhibit.html\"",
        "a caller must not be able to ask an executable type into the origin"
    );
}

/// `?inline=1` is what a hand-written portal link spells; a `bool`-typed
/// extractor would 400 the request instead of serving the document.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_inline_flag_accepts_the_spellings_a_portal_link_uses() {
    let f = build_fixture().await;
    for query in ["?inline=1", "?inline=true", "?inline="] {
        let response = f.download(f.pdf, query).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{query} must not be a bad request"
        );
        assert_eq!(
            header(&response, "content-disposition"),
            "inline; filename=\"order.pdf\"",
            "{query} asks for inline"
        );
    }
    // Anything else is not an ask.
    let response = f.download(f.pdf, "?inline=0").await;
    assert_eq!(
        header(&response, "content-disposition"),
        "attachment; filename=\"order.pdf\""
    );
}
