#![allow(clippy::doc_markdown)]
//! End-to-end: open a federal naturalization matter and walk the Form
//! N-400 intake entirely through the `navigator` CLI binary, driven against
//! an in-process `web` app on a loopback port.
//!
//! This is the CLI demo path for the immigration workflow — `notation create`
//! → `intake answer` (the ten `us__naturalization` questions) →
//! `notation status` → `notation approve` → `notation document` — proving
//! the applicant's answers render into the filled N-400 AcroForm, all
//! through the binary.
//!
//! Both the non-interactive flag walk (`--answer`) and the interactive
//! scripted-stdin walk are exercised. CI-safe: the `StubSignatureProvider`
//! records the send, so nothing reaches DocuSign, and no cloud account is
//! touched (FsStorage, in-memory runtime).

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
use workflows::{DispatchingRuntime, InMemoryRuntime, StateMachineRuntime};

const SESSION_KEY: &str = "cli-naturalization-e2e-key-not-for-production";

// Both cases drive the same instrumented `navigator` child binary. macOS can
// return `EBADF` from concurrent `posix_spawn` calls while LLVM coverage has
// that binary open; serializing only these two end-to-end cases keeps the
// coverage gate deterministic without weakening either walk.
static CLI_PROCESS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The scalar N-400 intake answers, in questionnaire order, for the
/// non-interactive `--answer` walk. Picker-backed client and country
/// questions are supplied separately through `--select`.
const SCALAR_ANSWERS: [&str; 7] = [
    "1990-04-12",
    "2019-03-01",
    "702-555-0100",
    "five_year",
    "married",
    "45",
    "no",
];

/// Build the seeded app with the same wiring `features::journey` uses —
/// canonical templates (including `us__naturalization`), FsStorage, a
/// `DispatchingRuntime` that renders + dispatches in-process, and a
/// `StubSignatureProvider`. Auth is ENFORCED (HS256) so the CLI's
/// `Authorization: Bearer <SessionData>` is exercised for real.
async fn build_app(tag: &str) -> (axum::Router, store::surreal::SurrealDb) {
    let repo_root = std::env::temp_dir().join(format!(
        "navigator-cli-naturalization-repos-{tag}-{}",
        Uuid::now_v7()
    ));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-cli-natz-e2e-{tag}")))
            .await
            .unwrap(),
    );
    seed::seed_canonical(&surreal, &storage).await.unwrap();

    let runtime = Arc::new(InMemoryRuntime::new());
    let email: Arc<dyn portal::email::EmailService> =
        Arc::new(portal::email::CapturingEmail::new());
    let workflow_runtime: Arc<dyn StateMachineRuntime> = Arc::new(DispatchingRuntime::new(
        runtime.clone(),
        email.clone(),
        storage.clone(),
    ));
    let state = AppState {
        auth: AuthConfig::new(false, Some("unused-hs256-secret")),
        sessions: SessionStore::new(SESSION_KEY),
        // The N-400 blank is pulled from the assets lane and verified
        // against its pin at fill time; stage synthetic blanks with
        // matching pins on this test's storage root.
        assets_storage: storage.clone(),
        forms_registry: portal::test_support::stage_blank_forms(storage.as_ref()).await,
        storage,
        workflow_runtime,
        questionnaire_runtime: runtime,
        signature_provider: Arc::new(portal::signature::StubSignatureProvider::new()),
        email,
        ..portal::test_support::app_state(surreal.clone()).await
    };
    (neon_router(state), surreal)
}

/// Open the naturalization matter directly in the DB with a prior retainer
/// notation, so the N-400 the test drives is not the matter's first
/// notation (that slot is the retainer's). Returns the matter code, and puts
/// the acting admin on the matter: since ENG-81 no tier bypasses project
/// scoping on the matter surface, so the CLI's bearer needs a participation row
/// like any other caller.
async fn seed_formation_matter(
    surreal: &store::surreal::SurrealDb,
    code: &str,
) -> (String, uuid::Uuid) {
    let acting_admin = store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role("CLI Admin", CLI_ADMIN_EMAIL, Role::Admin),
    )
    .await
    .expect("seed the acting admin");

    let entity_id = store::test_support::seed_entity(surreal).await;
    let dri = store::test_support::dri_person(surreal).await;
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: code.into(),
            name: "Naturalization Matter".into(),
            status: "open".into(),
            entity_id,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Give the matter its accountable lawyer DRI as a participation row — the
    // client is added later when `notation create` resolves `--client-email`.
    // Two people on the matter is what makes the `person__client` picker offer
    // more than one option, the shape the walk below selects against. Replaces
    // the dropped `projects.*_dri_person_id` columns this fixture used to set.
    store::projects::designate_dri_in_surreal(
        surreal,
        project.id,
        dri,
        store::projects::DriSide::Lawyer,
    )
    .await
    .unwrap();
    let retainer = store::templates::resolve(surreal, None, "onboarding__retainer")
        .await
        .unwrap()
        .expect("seeded retainer template");
    store::notations::create(
        surreal,
        &store::notations::NewNotation::new(retainer.id, dri, project.id, "BEGIN"),
    )
    .await
    .unwrap();
    store::projects::add_participation(surreal, project.id, acting_admin.id, "attorney")
        .await
        .expect("put the acting admin on the matter");
    (project.code, acting_admin.id)
}

/// The email the CLI's bearer identifies as.
const CLI_ADMIN_EMAIL: &str = "cli-admin@neonlaw.com";

/// A fresh admin session bearer, signed with the test session key — the
/// blob the CLI presents as `Authorization: Bearer …`.
fn admin_token(person_id: uuid::Uuid) -> String {
    let mut session = SessionData::fresh("cli-admin", Role::Admin);
    session.email = Some(CLI_ADMIN_EMAIL.into());
    session.person_id = Some(person_id);
    SessionStore::new(SESSION_KEY).encode(&session)
}

/// Spawn the app on a loopback port and return its base URL.
async fn spawn(tag: &str) -> (String, store::surreal::SurrealDb) {
    let (app, surreal) = build_app(tag).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), surreal)
}

/// Write a `~/.navigator.json`-shaped credential file for `base`, holding
/// the admin bearer with a far-future expiry, and return its path.
fn write_creds(dir: &Path, base: &str, admin_id: uuid::Uuid) -> std::path::PathBuf {
    let path = dir.join("navigator.json");
    let body = serde_json::json!({
        "hosts": { base: { "token": admin_token(admin_id), "expires_at": 9_999_999_999i64 } }
    });
    std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
    path
}

/// Run the `navigator` binary with the credential file wired in; return
/// (success, stdout+stderr).
async fn run_cli(creds: &Path, args: &[&str]) -> (bool, String) {
    let out = tokio::process::Command::new(env!("CARGO_BIN_EXE_navigator"))
        .env("NAVIGATOR_CREDENTIALS_FILE", creds)
        .args(args)
        .output()
        .await
        .expect("run navigator");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status.success(), format!("{stdout}\n{stderr}"))
}

/// Run the binary feeding `stdin`, for the interactive walk.
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

/// Pull the notation UUID out of `notation create`'s stdout.
fn notation_id_from(stdout: &str) -> Uuid {
    stdout
        .split_whitespace()
        .find_map(|tok| Uuid::parse_str(tok.trim()).ok())
        .unwrap_or_else(|| panic!("no notation id in notation-create output:\n{stdout}"))
}

/// Assert the downloaded artifact is the filled N-400 AcroForm, flattened
/// past lawyer review.
fn assert_rendered_pdf(bytes: &[u8]) {
    assert!(bytes.starts_with(b"%PDF"), "the download is a PDF");
    assert!(
        pdf::field_names(bytes)
            .expect("field names readable")
            .is_empty(),
        "the downloaded N-400 is flattened — no interactive field survives lawyer review",
    );
    let text = pdf::page_text(bytes).expect("extract flattened page text");
    assert!(
        text.contains("maria@example.com"),
        "the applicant email fills the N-400 from the Person row:\n{text}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naturalization_intake_through_the_cli_with_answer_flags() {
    let _process_guard = CLI_PROCESS_LOCK.lock().await;
    let (base, surreal) = spawn("flags").await;
    let (project_code, admin_id) = seed_formation_matter(&surreal, "naturalization-flags").await;
    let tmp = tempfile::tempdir().unwrap();
    let creds = write_creds(tmp.path(), &base, admin_id);

    // 1. Open the naturalization notation on the pre-existing matter.
    let (ok, out) = run_cli(
        &creds,
        &[
            "site",
            "notation",
            "create",
            "--host",
            &base,
            "us__naturalization",
            "--client-email",
            "maria@example.com",
            "--project",
            &project_code,
        ],
    )
    .await;
    assert!(ok, "notation create failed:\n{out}");
    let id = notation_id_from(&out).to_string();

    // 2. Answer all ten N-400 questions non-interactively: three picker
    //    selections by question code, then seven scalars in questionnaire
    //    order.
    let mut args: Vec<&str> = vec![
        "site",
        "intake",
        "answer",
        &id,
        "--host",
        &base,
        "--select",
        "person__client=2",
        "--select",
        "country__of_birth=114",
        "--select",
        "country__of_citizenship=114",
    ];
    for a in SCALAR_ANSWERS {
        args.push("--answer");
        args.push(a);
    }
    let (ok, out) = run_cli(&creds, &args).await;
    assert!(ok, "intake answer failed:\n{out}");
    assert!(
        out.contains("questionnaire complete"),
        "walk completes:\n{out}"
    );

    // 3. Status: the intake parks at attorney review. The N-400 is filled
    //    only after explicit approval.
    let (ok, out) = run_cli(
        &creds,
        &["site", "notation", "status", &id, "--host", &base, "--json"],
    )
    .await;
    assert!(ok, "notation status failed:\n{out}");
    assert!(
        out.contains("\"state\": \"lawyer_review\""),
        "state:\n{out}"
    );
    assert!(
        out.contains("\"document_ready\": false"),
        "N-400 must not be ready before approval:\n{out}"
    );

    // 4. Approve: fill + park the N-400 at the generate_pdf step.
    let (ok, out) = run_cli(
        &creds,
        &["site", "notation", "approve", &id, "--host", &base],
    )
    .await;
    assert!(ok, "notation approve failed:\n{out}");

    let (ok, out) = run_cli(
        &creds,
        &["site", "notation", "status", &id, "--host", &base, "--json"],
    )
    .await;
    assert!(ok, "notation status after approve failed:\n{out}");
    assert!(
        out.contains("\"state\": \"generate_pdf__n400_summary\""),
        "state:\n{out}"
    );
    assert!(
        out.contains("\"document_ready\": true"),
        "N-400 ready after approve:\n{out}"
    );

    // 5. Download the filled N-400.
    let pdf_path = tmp.path().join("n400.pdf");
    let pdf_str = pdf_path.to_str().unwrap();
    let (ok, out) = run_cli(
        &creds,
        &[
            "site", "notation", "document", &id, "--out", pdf_str, "--host", &base,
        ],
    )
    .await;
    assert!(ok, "notation document failed:\n{out}");
    assert_rendered_pdf(&std::fs::read(&pdf_path).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn naturalization_intake_through_the_interactive_cli_walk() {
    let _process_guard = CLI_PROCESS_LOCK.lock().await;
    let (base, surreal) = spawn("interactive").await;
    let (project_code, admin_id) =
        seed_formation_matter(&surreal, "naturalization-interactive").await;
    let tmp = tempfile::tempdir().unwrap();
    let creds = write_creds(tmp.path(), &base, admin_id);

    let (ok, out) = run_cli(
        &creds,
        &[
            "site",
            "notation",
            "create",
            "--host",
            &base,
            "us__naturalization",
            "--client-email",
            "maria@example.com",
            "--project",
            &project_code,
        ],
    )
    .await;
    assert!(ok, "notation create failed:\n{out}");
    let id = notation_id_from(&out).to_string();

    // Scripted stdin: picker selections for the client and two country
    // questions, plus scalar answers for the rest. The country picker is
    // sorted by seeded jurisdiction name; Mexico is candidate 114.
    let stdin = concat!(
        "2\n",
        "1990-04-12\n",
        "114\n",
        "114\n",
        "2019-03-01\n",
        "702-555-0100\n",
        "five_year\n",
        "married\n",
        "45\n",
        "no\n",
    );
    let (ok, out) = run_cli_stdin(
        &creds,
        &["site", "intake", "answer", &id, "--host", &base],
        stdin,
    )
    .await;
    assert!(ok, "interactive intake answer failed:\n{out}");
    assert!(
        out.contains("questionnaire complete"),
        "walk completes:\n{out}"
    );

    // The interactive walk parks at review. Approve to fill the same
    // N-400, then download it via the CLI.
    let (ok, out) = run_cli(
        &creds,
        &["site", "notation", "approve", &id, "--host", &base],
    )
    .await;
    assert!(ok, "notation approve failed:\n{out}");

    let pdf_path = tmp.path().join("n400.pdf");
    let pdf_str = pdf_path.to_str().unwrap();
    let (ok, out) = run_cli(
        &creds,
        &[
            "site", "notation", "document", &id, "--out", pdf_str, "--host", &base,
        ],
    )
    .await;
    assert!(ok, "notation document failed:\n{out}");
    assert_rendered_pdf(&std::fs::read(&pdf_path).unwrap());
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
