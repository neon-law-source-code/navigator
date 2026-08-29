//! Cucumber runner for `features/esignature_webhook.feature`.
//!
//! Drives the inbound e-signature completion webhook
//! (`portal::esignature_webhook`) against an in-memory runtime: a retainer
//! is parked at `sent_for_signature__pending` with a known envelope id,
//! then the provider's completion callback is POSTed — once validly
//! signed (advances to END), once with a forged signature (rejected,
//! stays pending).

#![allow(clippy::unused_async)]
#![allow(clippy::doc_markdown)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use cucumber::{given, then, when, World};
use features::{app_state, body_string, fs_storage};
use portal::webhook_auth::sign_hmac_sha256_b64;
use portal::{policy::PolicyClient, SessionStore};
use store::seed;
use tower::ServiceExt;
use uuid::Uuid;
use workflows::{InMemoryRuntime, MachineKind, StateMachineRuntime};

const TEMPLATE_CODE: &str = "onboarding__engagement_letter";
const HMAC_KEY: &str = "test-docusign-hmac-key";
const PARKED: &str = "sent_for_signature__pending";

/// The DocuSign Connect completion payload for one envelope.
fn completion_body(envelope_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "event": "envelope-completed",
        "data": {
            "envelopeId": envelope_id,
            "envelopeSummary": { "status": "completed" },
        },
    }))
    .unwrap()
}

#[derive(Default, World)]
#[world(init = Self::default)]
struct WebhookWorld {
    app: Option<axum::Router>,
    runtime: Option<Arc<InMemoryRuntime>>,
    notation_id: Option<Uuid>,
    /// The envelope id actually recorded in `signatures`, unique per
    /// scenario: `signatures` lives in the process-shared
    /// [`features::shared_surreal`] engine, so two scenarios both parking on
    /// the feature file's literal `"env-abc"` would collide on the
    /// `(provider, provider_id)`
    /// unique index and resolve to each other's notation. Mangled here with
    /// the scenario's own notation id so each scenario's envelope is
    /// distinct while the feature file keeps its readable literal text.
    envelope_id: Option<String>,
    last_status: Option<StatusCode>,
    last_body: String,
}

impl std::fmt::Debug for WebhookWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookWorld")
            .field("notation_id", &self.notation_id)
            .field("last_status", &self.last_status)
            .finish_non_exhaustive()
    }
}

impl WebhookWorld {
    fn app(&self) -> axum::Router {
        self.app.as_ref().expect("app not built").clone()
    }
    fn runtime(&self) -> &Arc<InMemoryRuntime> {
        self.runtime.as_ref().expect("runtime not built")
    }
    fn notation_id(&self) -> Uuid {
        self.notation_id.expect("notation not built")
    }
    fn envelope_id(&self) -> &str {
        self.envelope_id.as_deref().expect("envelope not built")
    }
}

#[given("a Neon Law Navigator app with an HMAC-secured e-signature webhook")]
async fn build_app(world: &mut WebhookWorld) {
    let surreal = features::shared_surreal().await;
    let storage = fs_storage("esignature-webhook").await;
    seed::seed_canonical(&surreal, &storage)
        .await
        .expect("seed canonical");
    let runtime = Arc::new(InMemoryRuntime::new());
    let mut state = app_state(
        runtime.clone(),
        storage,
        PolicyClient::passthrough(),
        None,
        SessionStore::new("test-session-key-not-for-production"),
    )
    .await;
    // Arm the HMAC gate so the webhook actually verifies signatures —
    // without this the dev posture would accept the forged callback.
    state.esignature_hmac_key = Some(HMAC_KEY.to_string());
    let router = features::neon_router(state, std::path::Path::new(portal::DEFAULT_PUBLIC_DIR));
    world.app = Some(router);
    world.runtime = Some(runtime);
}

#[given(regex = r#"^a retainer parked at sent_for_signature__pending with envelope id "([^"]+)"$"#)]
async fn park_retainer(world: &mut WebhookWorld, envelope_id: String) {
    let surreal = features::shared_surreal().await;
    let tmpl = store::templates::resolve(&features::shared_surreal().await, None, TEMPLATE_CODE)
        .await
        .unwrap()
        .expect("seed_canonical inserts onboarding__engagement_letter");
    let person = store::test_support::ensure_person(
        &surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await;
    let proj = store::test_support::seed_project(&surreal, "retainer matter").await;
    let notation_id = store::notations::create(
        &surreal,
        &store::notations::NewNotation::new(tmpl.id, person.id, proj.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id;

    // Drive the workflow timeline to the parked state through the same
    // in-memory runtime the webhook will later signal. The retainer
    // engagement is attorney-reviewed at lawyer_review before reaching
    // the signature wait — mirror that path exactly.
    let rt = world.runtime().as_ref();
    let spec = workflows::retainer_intake_spec();
    StateMachineRuntime::start(rt, MachineKind::Workflow, notation_id, &spec)
        .await
        .unwrap();
    for condition in [
        "intake_submitted",
        "retainer_rendered",
        "approved",
        "pdf_persisted",
    ] {
        StateMachineRuntime::signal(rt, MachineKind::Workflow, notation_id, condition, None)
            .await
            .unwrap();
    }
    assert_eq!(
        StateMachineRuntime::current_state(rt, MachineKind::Workflow, notation_id)
            .await
            .unwrap()
            .as_str(),
        PARKED,
        "workflow should be parked before the callback"
    );

    // Persist the parked state + record the provider's envelope id in
    // `signatures`, exactly as the retainer walk does at send time. Mangled
    // unique to this scenario — see the `envelope_id` field doc.
    let envelope_id = format!("{envelope_id}-{notation_id}");
    store::notations::update_state(&surreal, notation_id, PARKED)
        .await
        .unwrap();
    store::signatures::record_request(
        &surreal,
        notation_id,
        store::signatures::SignatureProvider::DocuSign,
        &envelope_id,
    )
    .await
    .unwrap();

    world.notation_id = Some(notation_id);
    world.envelope_id = Some(envelope_id);
}

async fn post_callback(world: &mut WebhookWorld, envelope_id: &str, signature: &str) {
    let body = completion_body(envelope_id);
    let resp = world
        .app()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/esignature/any-path-token")
                .header("content-type", "application/json")
                .header("x-docusign-signature-1", signature)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    world.last_status = Some(resp.status());
    world.last_body = body_string(resp).await;
}

#[when(
    regex = r#"^the provider posts a validly-signed completion callback for envelope "([^"]+)"$"#
)]
async fn post_valid(world: &mut WebhookWorld, feature_envelope_id: String) {
    // `feature_envelope_id` is the feature file's literal, kept only as the
    // scenario-mangled envelope's un-mangled prefix; the real envelope id
    // recorded in `signatures` is `world.envelope_id()` — see the field doc.
    let envelope_id = world.envelope_id().to_string();
    assert!(envelope_id.starts_with(&feature_envelope_id));
    let signature = sign_hmac_sha256_b64(HMAC_KEY.as_bytes(), &completion_body(&envelope_id));
    post_callback(world, &envelope_id, &signature).await;
}

#[when(
    regex = r#"^an attacker posts a completion callback with a forged signature for envelope "([^"]+)"$"#
)]
async fn post_forged(world: &mut WebhookWorld, feature_envelope_id: String) {
    let envelope_id = world.envelope_id().to_string();
    assert!(envelope_id.starts_with(&feature_envelope_id));
    // A plausible-looking but wrong base64 digest.
    post_callback(world, &envelope_id, "Zm9yZ2VkLXNpZ25hdHVyZS1ub3QtdmFsaWQ=").await;
}

#[then(regex = r"^the response status is (\d+)$")]
async fn assert_status(world: &mut WebhookWorld, code: u16) {
    assert_eq!(
        world.last_status.expect("no status captured").as_u16(),
        code,
        "body: {}",
        world.last_body
    );
}

#[then(regex = r#"^the retainer workflow has advanced to "([^"]+)"$"#)]
async fn assert_advanced(world: &mut WebhookWorld, state: String) {
    let events = StateMachineRuntime::events(
        world.runtime().as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await;
    let last = events.last().expect("at least one transition");
    assert_eq!(last.to.as_str(), state, "events: {events:?}");
}

#[then(regex = r#"^the retainer workflow is still at "([^"]+)"$"#)]
async fn assert_still_at(world: &mut WebhookWorld, state: String) {
    let current = StateMachineRuntime::current_state(
        world.runtime().as_ref(),
        MachineKind::Workflow,
        world.notation_id(),
    )
    .await
    .expect("workflow exists");
    assert_eq!(current.as_str(), state);
}

#[then(regex = r#"^the notation row state is "([^"]+)"$"#)]
async fn assert_row_state(world: &mut WebhookWorld, state: String) {
    let surreal = features::shared_surreal().await;
    let row = store::notations::find_by_id(&surreal, world.notation_id())
        .await
        .unwrap()
        .expect("notation row");
    assert_eq!(row.state, state);
}

#[tokio::main]
async fn main() {
    WebhookWorld::cucumber()
        .run_and_exit("tests/features/esignature_webhook.feature")
        .await;
}
