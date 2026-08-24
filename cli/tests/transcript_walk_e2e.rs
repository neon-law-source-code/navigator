#![allow(clippy::doc_markdown)]
//! End-to-end: drive the questionnaire walk's **transcript input mode** through
//! the `navigator` CLI binary against an in-process `web` app on a loopback
//! port — PR 3 of #349.
//!
//! `navigator site intake answer <id> --transcript <file>` runs batch coverage
//! server-side (seeding proposed `source = extracted` answers), then walks the
//! questionnaire interactively: the covered questions surface their proposal as
//! an Enter-to-accept default (confirming writes a normal `source = lawyer`
//! answer), and the uncovered question still prompts. CI-safe: FsStorage +
//! in-memory runtime, no cloud account touched.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use portal::session::SessionData;
use portal::{AppState, AuthConfig, SessionStore};
use store::persons::Role;
use store::seed;
use store::test_support::mem_surreal;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use uuid::Uuid;
use workflows::InMemoryRuntime;

const SESSION_KEY: &str = "cli-transcript-e2e-key-not-for-production";

/// A questionnaire the coverage engine partially covers: recording consent
/// (covered on "consent"), testator name (covered via the `testator` label),
/// and a free-text note the transcript never touches (left to the walk).
const QUESTIONNAIRE: &[u8] = br"---
questionnaire:
  BEGIN:
    _: custom_yes_no__recording_consent
  custom_yes_no__recording_consent:
    _: custom_text__testator_name
  custom_text__testator_name:
    _: custom_text__note
  custom_text__note:
    _: END
  END: {}
---

# Transcript walk
";

const TRANSCRIPT: &str =
    "The client gave their consent to record this sitting. The testator is Jane Doe.\n";

async fn build_app(tag: &str) -> (axum::Router, store::surreal::SurrealDb) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-cli-transcript-repos-{tag}-{}",
        Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-cli-transcript-{tag}")))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let runtime = Arc::new(InMemoryRuntime::new());
    let state = AppState {
        auth: AuthConfig::new(false, Some("unused-hs256-secret")),
        sessions: SessionStore::new(SESSION_KEY),
        storage,
        workflow_runtime: runtime.clone(),
        questionnaire_runtime: runtime,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (neon_router(state), surreal)
}

/// Seed a matter + notation whose questionnaire is [`QUESTIONNAIRE`]; return
/// the notation id. The template is a **project-scoped** version of an
/// `onboarding__retainer_*` code: the project-scoped blob's `questionnaire:`
/// wins for the walk (so the coverable questions drive it), while completion
/// resolves the bundled retainer workflow from the catalog by the code prefix —
/// so the walk reaches END and parks at `lawyer_review` cleanly, rather than
/// 500ing on a synthetic code the workflow catalog doesn't know. The admin
/// bearer bypasses project scoping.
async fn seed_transcript_notation(
    surreal: &store::surreal::SurrealDb,
    storage: &Arc<dyn cloud::StorageService>,
) -> Uuid {
    let client = store::persons::create(
        surreal,
        &store::persons::NewPerson::new("Libra", "libra@example.com"),
    )
    .await
    .unwrap();
    let entity_id = store::test_support::seed_entity(surreal).await;
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: format!("transcript-matter-{}", Uuid::now_v7()),
            name: "Transcript matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let asset = store::assets::ingest_content(surreal, storage, QUESTIONNAIRE, "text/markdown")
        .await
        .unwrap();
    let template = store::templates::save_version(
        surreal,
        Some(project.id),
        "onboarding__retainer_transcript",
        store::templates::Version {
            title: "Transcript walk".into(),
            respondent_type: "person".into(),
            asset_id: Some(asset),
            form_code: None,
            kind: None,
            source_commit_sha: None,
        },
    )
    .await
    .unwrap()
    .into_model();

    store::notations::create(
        surreal,
        &store::notations::NewNotation::new(template.id, client.id, project.id, "BEGIN"),
    )
    .await
    .unwrap()
    .id
}

fn admin_token() -> String {
    let mut session = SessionData::fresh("cli-admin", Role::Admin);
    session.email = Some("nick@neonlaw.com".into());
    SessionStore::new(SESSION_KEY).encode(&session)
}

async fn spawn(
    tag: &str,
) -> (
    String,
    store::surreal::SurrealDb,
    Arc<dyn cloud::StorageService>,
) {
    let (app, surreal) = build_app(tag).await;
    // Recover the storage handle for seeding the template blob.
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-cli-transcript-{tag}")))
            .await
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        format!("http://127.0.0.1:{}", addr.port()),
        surreal,
        storage,
    )
}

fn write_creds(dir: &Path, base: &str) -> std::path::PathBuf {
    let path = dir.join("navigator.json");
    let body = serde_json::json!({
        "hosts": { base: { "token": admin_token(), "expires_at": 9_999_999_999i64 } }
    });
    std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
    path
}

async fn run_cli_stdin(creds: &Path, args: &[&str], stdin: &str) -> (bool, String) {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_navigator"))
        .env("NAVIGATOR_CREDENTIALS_FILE", creds)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn navigator");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .await
        .unwrap();
    let out = child.wait_with_output().await.unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status.success(), format!("{stdout}\n{stderr}"))
}

async fn latest(
    surreal: &store::surreal::SurrealDb,
    nid: Uuid,
    state_name: &str,
) -> store::answers::Answer {
    // Append-only: the last row for this state is the latest answer.
    store::answers::for_notation(surreal, nid)
        .await
        .unwrap()
        .into_iter()
        .rfind(|a| a.state_name.as_deref() == Some(state_name))
        .unwrap_or_else(|| panic!("no answer for {state_name}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcript_prefills_the_walk_then_confirms_the_proposals() {
    let (base, surreal, storage) = spawn("walk").await;
    let nid = seed_transcript_notation(&surreal, &storage).await;
    let id = nid.to_string();

    let tmp = tempfile::tempdir().unwrap();
    let creds = write_creds(tmp.path(), &base);
    let transcript_path = tmp.path().join("sitting.txt");
    std::fs::write(&transcript_path, TRANSCRIPT).unwrap();

    // Accept the consent proposal (Enter), accept the testator proposal
    // (Enter), then type the uncovered note.
    let (ok, out) = run_cli_stdin(
        &creds,
        &[
            "site",
            "intake",
            "answer",
            &id,
            "--host",
            &base,
            "--transcript",
            transcript_path.to_str().unwrap(),
        ],
        "\n\nGuardianship instructions for the minor children\n",
    )
    .await;
    assert!(ok, "transcript walk failed:\n{out}");

    // The coverage summary reports two covered inquiries and offers them as
    // proposals in the walk.
    assert!(
        out.contains("2 covered"),
        "coverage summary printed:\n{out}"
    );
    assert!(
        out.contains("proposed from transcript"),
        "the walk labels the transcript proposals:\n{out}"
    );

    // Confirming a proposal writes a normal lawyer answer that supersedes the
    // extracted one.
    let consent = latest(&surreal, nid, "custom_yes_no__recording_consent").await;
    assert_eq!(consent.source, "lawyer");
    assert_eq!(store::answers::display_value(&consent.value), "Yes");

    let testator = latest(&surreal, nid, "custom_text__testator_name").await;
    assert_eq!(testator.source, "lawyer");
    assert_eq!(store::answers::display_value(&testator.value), "Jane Doe");

    // The uncovered question is answered by the typed value.
    let note = latest(&surreal, nid, "custom_text__note").await;
    assert_eq!(
        store::answers::display_value(&note.value),
        "Guardianship instructions for the minor children"
    );
}

/// The router the `neon` binary serves. These walks drive the application
/// behind a brand, and this composes that brand through `neon`'s own entry
/// points rather than restating a route table the binary would not match.
fn neon_router(state: portal::AppState) -> axum::Router {
    let dioxus = neon::public_dioxus_routers(&state);
    portal::bootstrap(
        state,
        std::path::Path::new(portal::DEFAULT_PUBLIC_DIR),
        neon::public_routes(),
        neon::PUBLIC_PATHS,
        dioxus,
    )
    .expect("the public host must not claim Navigator-owned routes")
}
