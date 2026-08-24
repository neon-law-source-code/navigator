//! Shared scaffolding for the end-to-end *journey* runners — the specs
//! that follow one client and one lawyer across the full arc of a
//! representation (intake → portal → work product → signature → filing /
//! close) rather than pinning one surface.
//!
//! Each `tests/<journey>.rs` still owns its own `cucumber::World` and
//! step set; this module carries only the mechanics more than one
//! journey would otherwise duplicate: standing up the seeded app,
//! creating the client Person, driving the admin walker over real HTTP,
//! and a worker-shaped runtime for the lawyer-side workflow signals the
//! web surfaces don't expose.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{policy::PolicyClient, SessionStore};
use store::seed;
use tower::ServiceExt;
use workflows::{DispatchingRuntime, InMemoryRuntime};

use crate::{app_state, body_string, fs_storage, shared_surreal};

/// The CSRF token every minted journey session carries; the client POST
/// surfaces echo it back in the form body.
pub const JOURNEY_CSRF: &str = "journey-csrf";

/// The durable wiring one journey drives the firm and the client through.
pub struct Journey {
    pub app: axum::Router,
    /// The store — the same handle the router it drives was built with.
    pub surreal: store::surreal::SurrealDb,
    /// The shared in-memory journal that both the web app's dispatching
    /// runtime (inside `AppState`) and [`Journey::worker`] read and
    /// write, so a walker drive and a manual worker signal see one state.
    pub runtime: Arc<InMemoryRuntime>,
    pub storage: Arc<dyn cloud::StorageService>,
    /// The concrete billing stub wired into the app's `billing_provider`,
    /// held so a journey can assert the accounting seam stayed untouched —
    /// Navigator records legal work and never raises money.
    pub billing: Arc<portal::billing::StubBillingProvider>,
    /// The concrete signature stub wired into the app's
    /// `signature_provider`, held so a journey can assert the envelope's
    /// recipient routing and the bytes that were sent.
    pub signature: Arc<portal::signature::StubSignatureProvider>,
    /// The session store wired into the app, so a journey can mint a
    /// cookie session for a client Person and drive the client-facing
    /// portal surfaces (intake, review) as that human.
    pub sessions: SessionStore,
}

/// One captured HTTP response: status, `Location` (the walker redirects
/// after every answer), and the body as a string.
pub struct Captured {
    pub status: StatusCode,
    pub location: Option<String>,
    pub body: String,
}

impl Journey {
    /// Seed the canonical catalog and build the real site router over an
    /// in-memory app state, sharing one [`InMemoryRuntime`] journal.
    ///
    /// One door. The site's faces were separate hosts once and this had an
    /// `open_firm` twin to pick between them; one binary serves everything now,
    /// so every journey opens the same router and walks whichever pages it is
    /// about.
    pub async fn open(suite: &str) -> Self {
        let surreal = shared_surreal().await;
        let storage = fs_storage(suite).await;
        seed::seed_canonical(&surreal, &storage)
            .await
            .expect("seed canonical");
        let runtime = Arc::new(InMemoryRuntime::new());
        let sessions = SessionStore::new("test-session-key-not-for-production");
        let mut state = app_state(
            runtime.clone(),
            storage.clone(),
            PolicyClient::passthrough(),
            None,
            sessions.clone(),
        )
        .await;
        // Blank government forms live only in the assets bucket, sha-
        // pinned; stage synthetic blanks (with matching pins) on the
        // journey's storage root so formation fills run against the same
        // pull-and-verify seam production uses.
        state.forms_registry = portal::test_support::stage_blank_forms(storage.as_ref()).await;
        // Override the app's billing + signature providers with ones we
        // keep concrete handles to, so a journey can assert the accounting
        // seam stayed untouched and inspect the e-signature envelope.
        let billing = Arc::new(portal::billing::StubBillingProvider::new());
        state.billing_provider = billing.clone();
        let signature = Arc::new(portal::signature::StubSignatureProvider::new());
        state.signature_provider = signature.clone();
        let public_dir = std::path::Path::new(portal::DEFAULT_PUBLIC_DIR);
        // Composed through the `neon` crate rather than restated here, so the
        // BDD suite exercises the router the binary actually serves.
        let app = crate::neon_router(state, public_dir);
        Self {
            app,
            surreal,
            runtime,
            storage,
            billing,
            signature,
            sessions,
        }
    }

    /// The `Cookie` header value for a session as `person` (their role +
    /// id), carrying [`JOURNEY_CSRF`]. The basis for the client-facing
    /// portal drives.
    fn cookie_for(&self, person: &store::persons::Person) -> String {
        let session = SessionData {
            sub: format!("rauthy-{}-subject", person.email),
            email: Some(person.email.clone()),
            person_id: Some(person.id),
            exp: portal::session::now_unix_secs() + 600,
            role: person.role,
            csrf_token: JOURNEY_CSRF.into(),
            source: portal::session::SessionSource::Browser,
            provider: None,
            impersonation: None,
        };
        format!("{SESSION_COOKIE_NAME}={}", self.sessions.encode(&session))
    }

    /// `GET path` as `person` over a real cookie session — the client
    /// portal surfaces (intake, review) gate on the session + project ACL.
    pub async fn client_get(&self, person: &store::persons::Person, path: &str) -> Captured {
        self.send(
            Request::builder()
                .uri(path)
                .header("cookie", self.cookie_for(person))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    /// `POST path` (form-encoded) as `person`. The CSRF token is appended
    /// automatically so the middleware accepts the write.
    pub async fn client_post(
        &self,
        person: &store::persons::Person,
        path: &str,
        body: &str,
    ) -> Captured {
        let body = if body.is_empty() {
            format!("_csrf={JOURNEY_CSRF}")
        } else {
            format!("{body}&_csrf={JOURNEY_CSRF}")
        };
        self.send(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("cookie", self.cookie_for(person))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
    }

    /// A worker-shaped runtime (in-process dispatch + matter close +
    /// compliance `filings`) over the SAME journal the web app uses, so a
    /// runner can drive the lawyer-side workflow signals the web surfaces
    /// don't expose — e.g. recording the Secretary-of-State filing once
    /// the client has signed.
    #[must_use]
    pub fn worker(&self) -> DispatchingRuntime {
        DispatchingRuntime::new(
            self.runtime.clone(),
            Arc::new(portal::email::CapturingEmail::new()),
            self.storage.clone(),
        )
        .with_store(self.surreal.clone())
    }

    /// `GET path` as an anonymous client (no auth) — for the public,
    /// client-facing surfaces (marketing, the `/es` funnel).
    pub async fn visit(&self, path: &str) -> Captured {
        self.send(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
    }

    /// `GET path` as the firm (admin passthrough auth).
    pub async fn lawyer_get(&self, path: &str) -> Captured {
        self.send(
            Request::builder()
                .uri(path)
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    /// `POST path` (form-encoded) as the firm.
    pub async fn lawyer_post(&self, path: &str, body: String) -> Captured {
        self.send(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(
                    "authorization",
                    portal::test_support::lawyer_bearer_header(),
                )
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
    }

    async fn send(&self, req: Request<Body>) -> Captured {
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let body = body_string(resp).await;
        Captured {
            status,
            location,
            body,
        }
    }
}

/// Create a client Person with a display name and email, in the firm's
/// `client` role, so the portal and the signature manifest read the
/// human's real name rather than their email.
pub async fn client(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
) -> store::persons::Person {
    store::test_support::ensure_person(
        surreal,
        &store::persons::NewPerson::with_role(name, email, store::persons::Role::Client),
    )
    .await
}

/// Open a matter (Project) for `person_id` in the firm's `client`
/// participation, returning the project id. The demand-side mirror of the
/// admin retainer-walk's project bootstrap, for journeys that drive the
/// notation directly through the worker rather than the web walker.
pub async fn matter(
    surreal: &store::surreal::SurrealDb,
    person_id: uuid::Uuid,
    name: &str,
) -> uuid::Uuid {
    let project_id = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("journey-matter-{}", uuid::Uuid::now_v7()),
            name: name.into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(surreal).await,
            ..Default::default()
        },
    )
    .await
    .expect("insert project")
    .id;
    store::projects::add_participation(surreal, project_id, person_id, "client")
        .await
        .expect("insert person_project_role");
    project_id
}

/// Encode one walker answer as the `value=` body the step form expects.
#[must_use]
pub fn answer_body(value: &str) -> String {
    format!("value={}", crate::form_encode(value))
}
