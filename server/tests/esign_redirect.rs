//! Integration tests for the door to the signing ceremony:
//! `GET /app/notations/{id}/sign` (client lens) and
//! `GET /app/lawyer/notations/{id}/sign` (firm lens).
//!
//! Navigator does not host the ceremony: the route mints a single-use
//! recipient-view URL and redirects the signer to the provider's own site
//! (#1010). These tests drive the composed router so they cover the whole
//! handler — the two authorization gates, the notation and recipient lookups,
//! the "not sent yet" guard, and the redirect itself — rather than only the
//! pure URL check that `portal::esign_view`'s unit tests cover.
//!
//! **Participation says you may look at the matter; identity says you may sign
//! as this person.** Both gates answer `404`, never `403`, so a refusal
//! confirms nothing about whether the notation exists or who is on the matter.
//! Rego admits any authenticated session on `/app/notations/**` without
//! constraining depth (`portal/policy/navigator.rego`), so these handler gates
//! are the entire authorization boundary and this file is what holds them.
//!
//! Worth stating because it is the point of the design: **nothing here proves
//! the signature completes.** Completion arrives on
//! `POST /webhook/esignature/{secret}` (see `esignature_loop.rs`), which is
//! deliberate — the signer may finish on their phone and never come back to
//! this browser session at all.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use portal::session::{SessionData, SESSION_COOKIE_NAME};
use portal::{AppState, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tower::ServiceExt;
use uuid::Uuid;

const TEMPLATE_CODE: &str = "onboarding__letter";
const ENVELOPE_ID: &str = "env-refer-out-1";
const KEY: &str = "test-session-key-not-for-production";

/// A composed router over a seeded store carrying one matter with a signer,
/// a co-client, an outsider, and both a participating and a non-participating
/// lawyer.
///
/// The handle is returned rather than re-derived by the caller: each
/// `mem_surreal()` opens its own engine, so a second call would hand back an
/// empty store in which this notation does not exist. The stub
/// `SignatureProvider` is the default in `test_support::app_state`, so
/// `create_recipient_view` returns a deterministic
/// `https://stub.docusign.local/...` URL with no network call.
struct Fixture {
    app: axum::Router,
    surreal: store::surreal::SurrealDb,
    sessions: SessionStore,
    /// The same handle the router holds, so a test can read back the
    /// `RecipientView` the handler actually built.
    provider: Arc<portal::signature::StubSignatureProvider>,
    notation_id: Uuid,
    template_id: Uuid,
    project_id: Uuid,
    project_code: String,
    /// The notation's bound signer — a client participant.
    signer: Uuid,
    /// A second client participant on the same matter who is *not* the signer.
    co_client: Uuid,
    /// A client with no participation row on this matter.
    outsider: Uuid,
    /// A lawyer with a firm participation row on this matter.
    firm_participant: Uuid,
    /// A lawyer with no participation row on this matter.
    firm_outsider: Uuid,
}

async fn build() -> Fixture {
    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join("navigator-esign-redirect-storage"))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();
    let tmpl = store::templates::resolve(&surreal, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed inserts the retainer template");

    let signer = mk_person(&surreal, "Libra", "libra@example.com", Role::Client).await;
    let co_client = mk_person(&surreal, "Spouse", "spouse@example.com", Role::Client).await;
    let outsider = mk_person(&surreal, "Outsider", "outsider@example.com", Role::Client).await;
    let firm_participant =
        mk_person(&surreal, "Counsel", "counsel@example.com", Role::Lawyer).await;
    let firm_outsider = mk_person(&surreal, "Other", "other@example.com", Role::Lawyer).await;

    let project = store::projects::create(
        &surreal,
        &store::projects::NewProject {
            code: format!("libra-retainer-{}", Uuid::now_v7()),
            name: "Libra retainer".into(),
            status: "open".into(),
            entity_id: store::test_support::seed_entity(&surreal).await,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Both clients participate on the matter; only one of them is the
    // notation's signer, which is the whole point of the identity gate.
    // Participation follows the tier — there is no second vocabulary.
    for pid in [signer, co_client] {
        store::projects::add_participation(
            &surreal,
            project.id,
            pid,
            store::projects::participation_for_role(Role::Client),
        )
        .await
        .unwrap();
    }
    store::projects::add_participation(
        &surreal,
        project.id,
        firm_participant,
        store::projects::participation_for_role(Role::Lawyer),
    )
    .await
    .unwrap();

    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(
            tmpl.id,
            signer,
            project.id,
            "sent_for_signature__pending",
        ),
    )
    .await
    .unwrap()
    .id;

    let provider = Arc::new(portal::signature::StubSignatureProvider::new());
    let state = AppState {
        storage,
        sessions: SessionStore::new(KEY),
        signature_provider: provider.clone(),
        ..portal::test_support::app_state(surreal.clone()).await
    };
    Fixture {
        app: server::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR)),
        surreal,
        sessions: SessionStore::new(KEY),
        provider,
        notation_id,
        template_id: tmpl.id,
        project_id: project.id,
        project_code: project.code,
        signer,
        co_client,
        outsider,
        firm_participant,
        firm_outsider,
    }
}

async fn mk_person(
    surreal: &store::surreal::SurrealDb,
    name: &str,
    email: &str,
    role: Role,
) -> Uuid {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, role),
    )
    .await
    .unwrap()
    .id
}

fn cookie_for(sessions: &SessionStore, role: Role, person_id: Uuid) -> String {
    let mut s = SessionData::fresh("sub", role);
    s.person_id = Some(person_id);
    format!("{SESSION_COOKIE_NAME}={}", sessions.encode(&s))
}

async fn record_envelope(f: &Fixture) {
    store::signatures::record_request(
        &f.surreal,
        f.notation_id,
        store::signatures::SignatureProvider::DocuSign,
        ENVELOPE_ID,
    )
    .await
    .unwrap();
}

/// Drive the route at `uri` with a session cookie and a `Host` header. The
/// host is what `portal::openapi::base_url_for` resolves the absolute
/// `return_url` against, so every request carries one.
async fn get(f: &Fixture, uri: &str, cookie: &str) -> axum::http::Response<Body> {
    f.app
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header("host", "staging.neonlaw.com")
                .header("cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

fn client_sign_uri(notation_id: Uuid) -> String {
    format!("/app/notations/{notation_id}/sign")
}

fn lawyer_sign_uri(notation_id: Uuid) -> String {
    format!("/app/lawyer/notations/{notation_id}/sign")
}

/// The `Location` of a `303`, or a panic naming the status that came instead.
fn location_of(response: &axum::http::Response<Body>) -> String {
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "expected a redirect to the provider"
    );
    response
        .headers()
        .get(header::LOCATION)
        .expect("a redirect carries a Location")
        .to_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn the_signer_is_redirected_to_the_provider() {
    let f = build().await;
    record_envelope(&f).await;

    let response = get(
        &f,
        &client_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Client, f.signer),
    )
    .await;
    let location = location_of(&response);
    assert!(
        location.starts_with("https://stub.docusign.local/signing/"),
        "the signer is sent to the provider's own site, not shown a page here: {location}"
    );
    assert!(
        location.contains(ENVELOPE_ID),
        "the recipient view is for this envelope: {location}"
    );
    // No HTML at all — the whole point of #1010. A body here would mean
    // Navigator is still hosting some part of the ceremony.
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&body).contains("<iframe"),
        "no iframe survives the referral-out"
    );
}

#[tokio::test]
async fn a_participant_who_is_not_the_signer_is_not_found() {
    // A matter can carry several client participants. Participation alone
    // would let the co-client open the signer's ceremony and sign in their
    // name — the identity gate is the only thing preventing it, and the
    // answer is 404 so the refusal confirms nothing.
    let f = build().await;
    record_envelope(&f).await;

    let response = get(
        &f,
        &client_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Client, f.co_client),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response.headers().get(header::LOCATION).is_none(),
        "a refusal is not a redirect"
    );
}

#[tokio::test]
async fn a_non_participant_client_is_not_found_rather_than_forbidden() {
    let f = build().await;
    record_envelope(&f).await;

    let response = get(
        &f,
        &client_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Client, f.outsider),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "403 would confirm this notation exists",
    );
}

#[tokio::test]
async fn a_non_participant_lawyer_is_not_found_and_a_participating_one_succeeds() {
    // The firm lens keeps the in-office signing this route was built for, so
    // a participating lawyer still mints the view — but the participation
    // gate now applies to that lens too, which is a real behavior change from
    // the unguarded handler.
    let f = build().await;
    record_envelope(&f).await;

    let refused = get(
        &f,
        &lawyer_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Lawyer, f.firm_outsider),
    )
    .await;
    assert_eq!(
        refused.status(),
        StatusCode::NOT_FOUND,
        "a lawyer off the matter has no more reach here than an outsider",
    );

    let allowed = get(
        &f,
        &lawyer_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Lawyer, f.firm_participant),
    )
    .await;
    assert!(
        location_of(&allowed).starts_with("https://stub.docusign.local/signing/"),
        "the existing /app/lawyer behavior is preserved for a participant",
    );
}

#[tokio::test]
async fn the_return_url_handed_to_the_provider_is_absolute() {
    // ENG-557 defect 1. A relative `returnUrl` reaches the wire unmodified
    // and a provider redirecting a browser resolves it against *its own*
    // origin, stranding the signer on DocuSign. The stub ignores the field,
    // so only the manifest the handler builds can prove this — read it back
    // off the request the handler made.
    let f = build().await;
    record_envelope(&f).await;

    let firm = get(
        &f,
        &lawyer_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Lawyer, f.firm_participant),
    )
    .await;
    assert_eq!(firm.status(), StatusCode::SEE_OTHER);

    let client = get(
        &f,
        &client_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Client, f.signer),
    )
    .await;
    assert_eq!(client.status(), StatusCode::SEE_OTHER);

    let views = f.provider.recipient_views();
    assert_eq!(views.len(), 2, "one recipient view per request");
    for view in &views {
        assert!(
            view.return_url.starts_with("https://staging.neonlaw.com/"),
            "the return_url must be absolute against Navigator's own authority, resolved \
             from the request Host: {}",
            view.return_url
        );
    }
    assert_eq!(
        views[0].return_url,
        format!(
            "https://staging.neonlaw.com/app/lawyer/notations/{}/step",
            f.notation_id
        ),
        "the firm lens returns to the matter's step page",
    );
    assert_eq!(
        views[1].return_url,
        format!(
            "https://staging.neonlaw.com/app/projects/{}",
            f.project_code
        ),
        "a client returns to their matter page, not to /app/lawyer, which the policy refuses them",
    );
}

#[tokio::test]
async fn a_notation_not_yet_sent_for_signature_conflicts() {
    // No `signatures` row: there is no envelope, so there is nothing to sign
    // and no URL to mint. This must not become a redirect to an empty ceremony.
    let f = build().await;
    let response = get(
        &f,
        &client_sign_uri(f.notation_id),
        &cookie_for(&f.sessions, Role::Client, f.signer),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        response.headers().get(header::LOCATION).is_none(),
        "a conflict is not a redirect"
    );
}

#[tokio::test]
async fn an_emailed_envelope_has_no_embedded_session_to_open() {
    // `emailed` delivery sends a non-captive recipient, and DocuSign issues a
    // recipient view only for a recipient carrying a `clientUserId`. Resolving
    // the recipient through the send path's own function is what surfaces
    // this: the third leg of the triple is `None`, so there is nothing to
    // replay and the honest answer is a conflict rather than a minted URL the
    // provider would refuse.
    let f = build().await;
    let emailed = store::notations::create(
        &f.surreal,
        &store::notations::NewNotation {
            delivery: store::notations::DELIVERY_EMAILED.to_string(),
            ..store::notations::NewNotation::new(
                f.template_id,
                f.signer,
                f.project_id,
                "sent_for_signature__pending",
            )
        },
    )
    .await
    .unwrap()
    .id;
    store::signatures::record_request(
        &f.surreal,
        emailed,
        store::signatures::SignatureProvider::DocuSign,
        "env-emailed-1",
    )
    .await
    .unwrap();

    let response = get(
        &f,
        &client_sign_uri(emailed),
        &cookie_for(&f.sessions, Role::Client, f.signer),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(
        f.provider.recipient_views().is_empty(),
        "no recipient view is requested for a non-captive recipient",
    );
}

#[tokio::test]
async fn an_unknown_notation_is_not_found() {
    let f = build().await;
    let response = get(
        &f,
        &client_sign_uri(Uuid::now_v7()),
        &cookie_for(&f.sessions, Role::Client, f.signer),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(
        response.headers().get(header::LOCATION).is_none(),
        "a miss is not a redirect"
    );
}
