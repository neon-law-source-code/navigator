//! Anonymous-access contract for the mountable portal entry point.
//!
//! Every first-tranche path is classified exactly once in [`CONTRACT`], and
//! the table is the specification: a route may be a host-owned public page,
//! a protected human browser surface, a protected protocol/API surface, or
//! an explicit anonymous operational/protocol ingress — never two of those,
//! and never unclassified.
//!
//! The state these tests build carries `PolicyClient::passthrough`, so embedded Rego policy
//! allows every request. That is deliberate: it proves the boundary is a
//! property of router composition rather than of a policy bundle that has to
//! redeploy in lockstep with the binary.

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use store::test_support::mem_surreal;
use tower::ServiceExt;

/// How one path must answer an anonymous request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    /// A brand host owns this page. The mounted portal does not serve it, so
    /// an anonymous request falls through to the host — 404 in a bare mount.
    HostPublic,
    /// A shared human surface. An anonymous browser is sent through the
    /// existing login door with a `303` to `/auth/login?return_to=…`.
    ProtectedHuman,
    /// A protocol/API surface. An anonymous caller gets a parseable
    /// unauthenticated document, never an HTML login redirect.
    ProtectedProtocol,
    /// An explicit anonymous exception: operational probes, the credential
    /// handshakes, and verified webhook ingress. Protocol *discovery* is no
    /// longer in this class — the A2A agent card moved under the private
    /// `/app/api` prefix with the rest of the API surface.
    PublicIngress,
    /// A page the mounted portal serves to anyone. Distinct from
    /// [`Access::HostPublic`], which a brand host owns and the portal answers
    /// `404` for, and from [`Access::PublicIngress`], which is machine ingress
    /// rather than a page: this one must render its own `200`.
    PortalPublic,
}

/// Every first-tranche path and the single class it belongs to.
///
/// `/app/lawyer` and `/app/admin` are listed as bare roots on purpose. Their
/// descendants were already gated; the roots render their own dashboard
/// handlers, so protection there cannot be inferred from a child route. The
/// retired `/lawyer` and `/admin` roots are asserted gone further down.
///
const CONTRACT: &[(&str, Access)] = &[
    // Human-facing Navigator application surfaces.
    ("/app/projects", Access::ProtectedHuman),
    // One Project's client portal. An anonymous browser goes through the same
    // login door as every other `/app` page; what an *authenticated*
    // nonparticipant gets is a non-disclosing 404, asserted in
    // `portal/tests/project_portal_route.rs`.
    ("/app/projects/some-code/portal", Access::ProtectedHuman),
    ("/app/lawyer", Access::ProtectedHuman),
    ("/app/admin", Access::ProtectedHuman),
    // The Owner/Admin matter directory (ENG-221). Listed beside the desk root
    // rather than inferred from it: which authenticated tiers reach it is the
    // Rego suite's assertion, and this one is that an anonymous browser never
    // gets that far.
    ("/app/admin/projects", Access::ProtectedHuman),
    // The blank government-forms index, migrated off the retired `/portal`
    // subtree onto `/app/forms`. Any authenticated person may browse it; an
    // anonymous browser goes through the login door like every `/app` page.
    ("/app/forms", Access::ProtectedHuman),
    // The workspace documentation reads anonymously. The repository is
    // source-available, so these documents are the manual for software anyone can
    // clone — a login door in front of them guarded nothing and cost a reader
    // the one page that explains how to run it.
    ("/docs", Access::PortalPublic),
    ("/docs/glossary", Access::PortalPublic),
    // The same documentation inside the application. `/docs` above renders for
    // anyone; these carry the session boundary plus a policy rule that admits
    // only the tiers who operate Navigator. What that gates is the application
    // surface, not the documents.
    ("/app/docs", Access::ProtectedHuman),
    ("/app/docs/glossary", Access::ProtectedHuman),
    // The living design system reads anonymously: it is a contributor
    // reference, so it renders for a reader who has no account rather than
    // sending them through the login door.
    ("/design", Access::PortalPublic),
    ("/templates", Access::ProtectedHuman),
    (
        "/templates/forms/united-states/federal/irs/us--form-990",
        Access::ProtectedHuman,
    ),
    // The Swagger UI shell at the `/app/api` root is an HTML page a reader
    // lands on, so it takes the login door rather than a JSON refusal.
    ("/app/api", Access::ProtectedHuman),
    // Machine surfaces answer machines, even when refusing.
    ("/app/api/openapi.json", Access::ProtectedProtocol),
    ("/app/api/people", Access::ProtectedProtocol),
    (
        "/app/api/templates/neon-law/shared/retainer",
        Access::ProtectedProtocol,
    ),
    // The A2A agent card. It moved off the anonymous allowlist when the API
    // surface consolidated under the private `/app/api` prefix: a client
    // now needs a session to read the card, so A2A discovery is not
    // self-service. See `portal::a2a` for why that is the accepted trade.
    ("/app/api/aida.json", Access::ProtectedProtocol),
    // Host-owned public pages. Legal and crawler documents belong to the
    // brand host that publishes them, not to the shared application.
    ("/privacy", Access::HostPublic),
    ("/terms", Access::HostPublic),
    ("/robots.txt", Access::HostPublic),
    ("/sitemap.xml", Access::HostPublic),
    ("/llms.txt", Access::HostPublic),
    ("/blog", Access::HostPublic),
    ("/contact", Access::HostPublic),
    // The explicit anonymous allowlist.
    ("/health", Access::PublicIngress),
    ("/readyz", Access::PublicIngress),
    ("/version", Access::PublicIngress),
    ("/auth/login", Access::PublicIngress),
    ("/auth/callback", Access::PublicIngress),
    ("/auth/logout", Access::PublicIngress),
    ("/auth/cli/start", Access::PublicIngress),
    ("/docusign/consent-callback", Access::PublicIngress),
    ("/assets/img/router-contract.svg", Access::PublicIngress),
];

/// An [`AppState`](portal::AppState) with the bundled docs and a configured
/// OAuth door, so the login handshakes this contract pins actually mount.
async fn contract_state() -> portal::AppState {
    let mut state = portal::test_support::app_state(mem_surreal().await).await;
    state.docs = portal::docs::loader::bundled();
    state.oauth = Some(portal::OAuthConfig::new(
        "navigator",
        "secret",
        "http://localhost:3001/auth/callback",
        "https://rauthy.example/auth/v1/oidc/authorize",
        "https://rauthy.example/auth/v1/oidc/token",
    ));
    state
        .assets_storage
        .put(
            "img/router-contract.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            "image/svg+xml",
        )
        .await
        .unwrap();
    state
}

async fn anonymous_get(app: &Router, path: &str) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

/// The whole table, asserted against the real mountable router.
#[tokio::test]
async fn every_first_tranche_path_answers_its_declared_anonymous_contract() {
    let app = portal::router(contract_state().await);

    for (path, access) in CONTRACT {
        let response = anonymous_get(&app, path).await;
        let status = response.status();
        match access {
            Access::HostPublic => assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{path} is host-owned; the mounted portal must not serve it"
            ),
            Access::ProtectedHuman => {
                assert_eq!(
                    status,
                    StatusCode::SEE_OTHER,
                    "{path} must send an anonymous browser to the login door"
                );
                let location = response
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    location.starts_with("/auth/login?return_to="),
                    "{path} must redirect through /auth/login, got {location}"
                );
            }
            Access::ProtectedProtocol => {
                assert_eq!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{path} must refuse a machine caller with a status, not a redirect"
                );
                let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let document: serde_json::Value = serde_json::from_slice(&body)
                    .unwrap_or_else(|e| panic!("{path} must answer with JSON: {e}"));
                assert_eq!(
                    document.get("error").and_then(serde_json::Value::as_str),
                    Some("unauthenticated"),
                    "{path} must keep the structured unauthenticated shape"
                );
            }
            Access::PublicIngress => {
                assert_ne!(
                    status,
                    StatusCode::NOT_FOUND,
                    "{path} is an explicit anonymous exception and must stay mounted"
                );
                assert_ne!(
                    status,
                    StatusCode::UNAUTHORIZED,
                    "{path} must stay reachable without a session"
                );
                let location = response
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                assert!(
                    !location.starts_with("/auth/login"),
                    "{path} must not be bounced to the login door"
                );
            }
            Access::PortalPublic => {
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{path} is a public page and must render for an anonymous reader"
                );
                let location = response
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default();
                assert!(
                    !location.starts_with("/auth/login"),
                    "{path} must not be bounced to the login door"
                );
            }
        }
    }
}

/// `/github-stars` is not routed, and no host reserves a prefix for it.
///
/// The endpoint proxied a star count from
/// `api.github.com/repos/neon-law-source-code/navigator`. It was deleted while
/// that repository was private and the upstream answered `404`; the repository
/// is public again, so the lane could be made to work, and that is exactly why
/// the absence is pinned rather than assumed. Re-adding the route puts an
/// anonymous ingress back on the boundary this file specifies, which is a
/// decision to argue for and not a repair.
#[tokio::test]
async fn github_stars_is_not_routed() {
    let app = portal::router(contract_state().await);

    assert_eq!(
        anonymous_get(&app, "/github-stars").await.status(),
        StatusCode::NOT_FOUND,
        "/github-stars was deleted; the portal must not serve it"
    );
    assert!(
        !portal::RESERVED_PATH_PREFIXES.contains(&"/github-stars"),
        "a deleted route must not keep a reserved host prefix"
    );
}

#[tokio::test]
async fn private_bucket_assets_are_served_anonymously_with_bounded_caching() {
    let app = portal::router(contract_state().await);

    let response = anonymous_get(&app, "/assets/img/router-contract.svg").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(axum::http::header::CONTENT_TYPE),
        Some(&axum::http::HeaderValue::from_static("image/svg+xml"))
    );
    assert_eq!(
        response.headers().get(axum::http::header::CACHE_CONTROL),
        Some(&axum::http::HeaderValue::from_static(
            "public, max-age=3600"
        ))
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::X_CONTENT_TYPE_OPTIONS),
        Some(&axum::http::HeaderValue::from_static("nosniff"))
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body, &b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"[..]);
}

#[tokio::test]
async fn private_bucket_asset_proxy_hides_missing_and_unsafe_keys() {
    let app = portal::router(contract_state().await);

    for path in [
        "/assets/img/missing.svg",
        "/assets/%2e%2e/documents/private.pdf",
        "/assets/img%5cprivate.pdf",
    ] {
        assert_eq!(
            anonymous_get(&app, path).await.status(),
            StatusCode::NOT_FOUND,
            "{path} must not expose an object"
        );
    }
}

/// Signature- and secret-verified webhook ingress stays anonymous: the
/// provider posting to it holds a credential, not a session.
#[tokio::test]
async fn verified_webhook_ingress_stays_anonymous() {
    let app = portal::router(contract_state().await);

    for path in [
        "/webhook/sendgrid/inbound/dev-secret",
        "/webhook/email-events/dev-secret",
        "/webhook/esignature/dev-secret",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} must stay mounted for its verified sender"
        );
        assert_ne!(
            response.status(),
            StatusCode::SEE_OTHER,
            "{path} must never bounce a webhook sender to a login page"
        );
    }
}

/// The GitHub webhook receiver no longer lives in `web`. It moved to
/// `workflows-service` on the public `workflows` host so `www` can go behind the
/// tailnet, and GitHub — which cannot join the tailnet — reaches it there. `web`
/// must 404 the path rather than accept a signed delivery meant for the worker.
#[tokio::test]
async fn web_does_not_serve_the_github_webhook_receiver() {
    let app = portal::router(contract_state().await);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhooks/github/dev-secret")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "web must not mount the GitHub webhook receiver"
    );
}

/// A signed session crosses the boundary and reaches the handler — the
/// boundary refuses anonymity, it does not refuse everyone.
#[tokio::test]
async fn a_signed_session_passes_the_shared_boundary() {
    let app = portal::router(contract_state().await);
    let app_for_gallery = app.clone();
    let sessions = portal::SessionStore::new(portal::test_support::TEST_SESSION_KEY);
    let cookie = format!(
        "{}={}",
        portal::session::SESSION_COOKIE_NAME,
        sessions.encode(&portal::SessionData::fresh(
            "contract-lawyer",
            store::persons::Role::Lawyer,
        ))
    );
    let gallery_cookie = cookie.clone();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/docs/glossary")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "an authenticated reader still gets the shared docs"
    );
    // `/docs` is anonymous, so this no longer proves the boundary passes a
    // signed session — a gated surface does. `/templates` is behind the same
    // boundary and renders for any authenticated person.
    let gallery = app_for_gallery
        .oneshot(
            Request::builder()
                .uri("/templates")
                .header("cookie", gallery_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        gallery.status(),
        StatusCode::OK,
        "a signed session passes the boundary onto a gated shared surface"
    );
}

#[tokio::test]
async fn mount_rejects_a_host_route_that_would_shadow_the_app() {
    let state = portal::test_support::app_state(mem_surreal().await).await;
    let host = Router::new().route("/app", get(|| async { StatusCode::IM_A_TEAPOT }));

    let error = portal::mount(state, host, &["/app"]).unwrap_err();

    assert_eq!(
        error.to_string(),
        "host route /app is reserved for the Navigator portal"
    );
}

/// Every reserved prefix — not just `/app` — is refused, and both the
/// bare prefix and a descendant of it collide.
#[tokio::test]
async fn mount_rejects_every_portal_owned_prefix() {
    // One migrated schema for the whole table: `mount` rejects before it
    // touches the database, and building a state is cheap once the schema
    // exists.
    let surreal = mem_surreal().await;
    for prefix in portal::RESERVED_PATH_PREFIXES {
        for host_path in [(*prefix).to_string(), format!("{prefix}/anything")] {
            let state = portal::test_support::app_state(surreal.clone()).await;
            let host = Router::new().route(&host_path, get(|| async { StatusCode::IM_A_TEAPOT }));
            let error = portal::mount(state, host, &[&host_path])
                .expect_err("a host may not claim a portal-owned path");
            assert_eq!(
                error.to_string(),
                format!("host route {host_path} is reserved for the Navigator portal")
            );
        }
    }
}

#[tokio::test]
async fn mount_keeps_host_public_routes_and_protects_portal_routes() {
    let host = Router::new().route("/host-page", get(|| async { StatusCode::IM_A_TEAPOT }));
    let app = portal::mount(contract_state().await, host, &["/host-page"])
        .expect("host route is not reserved");

    let host_response = anonymous_get(&app, "/host-page").await;
    assert_eq!(
        host_response.status(),
        StatusCode::IM_A_TEAPOT,
        "the host keeps serving its own public page"
    );

    // `/templates`, not `/docs`: the documentation reads anonymously now, so it
    // can no longer stand for "the boundary still closes under a host mount".
    // The template gallery is the nearest shared surface that is still gated.
    let gallery_response = anonymous_get(&app, "/templates").await;
    assert_eq!(
        gallery_response.status(),
        StatusCode::SEE_OTHER,
        "a shared Navigator tool stays behind the session boundary under a host mount"
    );
}

/// The reusable host bootstrap mounts a host's public `Router<AppState>` while
/// the shared session boundary still closes every human and protocol surface.
///
/// This is #730's "hosts publish; portal authenticates" invariant expressed as
/// one composition seam rather than a `portal_only` boolean: a host binary hands
/// [`portal::bootstrap`] its own public pages, and the anonymous-access matrix
/// #732 pinned holds regardless of what the host publishes.
#[tokio::test]
async fn bootstrap_mounts_host_public_routes_behind_the_shared_boundary() {
    use axum::extract::State;
    // A host-supplied public page that reads `AppState`, exactly as the firm's
    // marketing handlers do — proving the seam shares the application state,
    // not just stateless routes the way `mount` does.
    async fn host_home(State(_state): State<portal::AppState>) -> &'static str {
        "host home"
    }
    let host_public = Router::new().route("/host-home", get(host_home));
    let app = portal::bootstrap(
        contract_state().await,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        host_public,
        &["/host-home"],
        Vec::new(),
    )
    .expect("the host route does not collide with Navigator");

    let home = anonymous_get(&app, "/host-home").await;
    assert_eq!(
        home.status(),
        StatusCode::OK,
        "the host's own public page is served to an anonymous visitor"
    );

    let lawyer = anonymous_get(&app, "/app/lawyer").await;
    assert_eq!(
        lawyer.status(),
        StatusCode::SEE_OTHER,
        "the shared boundary still sends an anonymous browser to the login door"
    );

    let api = anonymous_get(&app, "/app/api/people").await;
    assert_eq!(
        api.status(),
        StatusCode::UNAUTHORIZED,
        "the shared boundary still refuses an anonymous machine caller with a status"
    );
}

/// The production brand composition path enforces the same reserved-prefix
/// contract as the standalone [`portal::mount`] seam. A brand declaration is
/// rejected before Axum can merge it, so host order can never shadow a shared
/// application route or turn a collision into a boot-time panic.
#[tokio::test]
async fn bootstrap_rejects_every_navigator_owned_prefix() {
    let surreal = mem_surreal().await;
    for prefix in portal::RESERVED_PATH_PREFIXES {
        for host_path in [(*prefix).to_string(), format!("{prefix}/anything")] {
            let state = portal::test_support::app_state(surreal.clone()).await;
            let host = Router::new().route(&host_path, get(|| async { StatusCode::IM_A_TEAPOT }));
            let error = portal::bootstrap(
                state,
                std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
                host,
                &[&host_path],
                Vec::new(),
            )
            .expect_err("a brand host may not claim a Navigator-owned path");
            assert_eq!(
                error.to_string(),
                format!("host route {host_path} is reserved for the Navigator portal")
            );
        }
    }
}
