#![allow(clippy::doc_markdown)]
//! End-to-end: form a Nevada LLC entirely through the `navigator` CLI
//! binary, driven against an in-process `web` app on a loopback port.
//!
//! This proves the formation flow through the **CLI surface** the prompt
//! specifies — `notation create` → `intake answer` (the seven `nv__llc_formation`
//! questions, including a `people_list` row) → `notation status` →
//! `notation approve` → `notation document` — and asserts the downloaded
//! bytes came through the sha-pin-verified AcroForm fill, flattened past
//! lawyer review: no interactive fields survive, yet every founder answer
//! still reads back as static page text, the same guarantee
//! `features/tests/nest_formation.rs` makes, now proven through the binary.
//!
//! Both the interactive walk (scripted stdin) and the non-interactive
//! flag walk (`--answer` / `--person`) are exercised. CI-safe: the
//! `StubSignatureProvider` records the send, so nothing reaches DocuSign,
//! and no cloud account is touched (FsStorage, in-memory runtime).

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

const SESSION_KEY: &str = "cli-llc-e2e-key-not-for-production";

// Both cases drive the same instrumented `navigator` child binary. macOS can
// return `EBADF` from concurrent `posix_spawn` calls while LLVM coverage has
// that binary open; serializing only these two end-to-end cases keeps the
// coverage gate deterministic without weakening either walk.
static CLI_PROCESS_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Build the seeded app with the same wiring `features::journey` uses —
/// canonical templates, FsStorage, a `DispatchingRuntime` that renders +
/// dispatches in-process, and a `StubSignatureProvider`. Auth is ENFORCED
/// (HS256) so the CLI's `Authorization: Bearer <SessionData>` is exercised
/// for real and the document download's required session is populated.
async fn build_app(tag: &str) -> (axum::Router, store::surreal::SurrealDb) {
    let repo_root =
        std::env::temp_dir().join(format!("navigator-cli-llc-repos-{tag}-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&repo_root).unwrap();
    std::env::set_var("NAVIGATOR_GIT_REPO_ROOT", &repo_root);

    let surreal = mem_surreal().await;
    let storage: Arc<dyn cloud::StorageService> = Arc::new(
        cloud::FsStorage::new(std::env::temp_dir().join(format!("navigator-cli-llc-e2e-{tag}")))
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
        // The blank NV packet is pulled from the assets lane and
        // verified against its pin at fill time; stage synthetic blanks
        // with matching pins on this test's storage root.
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

/// Open the formation matter directly in the DB and hang a prior retainer
/// notation on it, so the CLI flow exercises a matter that already has an
/// engagement. Returns the matter code the CLI's `--project` refers to, and
/// puts the acting admin on the matter: since ENG-81 no tier bypasses project
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

    // The matter's entity is named, because the walkthrough asserts on the
    // name the CLI prints back.
    let entity_id = store::entities::create(
        surreal,
        &store::entities::NewEntity {
            name: "Bright Star Ventures LLC".into(),
            entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
            jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
            phone: None,
            url: None,
            firm_anchor_key: None,
        },
    )
    .await
    .unwrap()
    .id;
    let dri = store::test_support::dri_person(surreal).await;
    let project = store::projects::create(
        surreal,
        &store::projects::NewProject {
            code: code.into(),
            name: "Bright Star Ventures LLC".into(),
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

/// Spawn the app on a loopback port and return its base URL + the seeded
/// DB (so the test can open the matter the CLI then works against).
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
/// the admin bearer with a far-future expiry, and return its path. The CLI
/// reads it via `NAVIGATOR_CREDENTIALS_FILE`.
fn write_creds(dir: &Path, base: &str, admin_id: uuid::Uuid) -> std::path::PathBuf {
    let path = dir.join("navigator.json");
    let body = serde_json::json!({
        "hosts": { base: { "token": admin_token(admin_id), "expires_at": 9_999_999_999i64 } }
    });
    std::fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
    path
}

/// Run the `navigator` binary with the credential file wired in; return
/// (success, stdout). stderr is surfaced into stdout on failure for
/// debugging.
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

/// Pull the notation UUID out of `notation create`'s stdout (color is
/// stripped for a pipe, so tokens are plain).
fn notation_id_from(stdout: &str) -> Uuid {
    stdout
        .split_whitespace()
        .find_map(|tok| Uuid::parse_str(tok.trim()).ok())
        .unwrap_or_else(|| panic!("no notation id in notation-create output:\n{stdout}"))
}

/// Assert the downloaded packet came through the government-form fill
/// path, flattened past lawyer review: no interactive fields survive
/// (nothing can re-edit an approved value on the way to the government
/// office), yet the founder's answers still read back as static page
/// content. The blank is the staged sha-pinned stand-in — production
/// pulls the official bytes from the assets bucket through the same
/// verify-then-fill seam.
fn assert_filled_packet(bytes: &[u8]) {
    assert!(bytes.starts_with(b"%PDF"), "the download is a PDF");
    assert!(
        pdf::field_names(bytes)
            .expect("field names readable")
            .is_empty(),
        "the filed packet is flattened — no interactive fields survive lawyer review",
    );
    assert_eq!(
        pdf::widget_annotation_count(bytes).expect("widget count readable"),
        0,
        "no widget annotation survives for a viewer to rebuild an editable field from",
    );
    let text = pdf::page_text(bytes).expect("extract flattened page text");
    assert!(
        text.contains("Bright Star Ventures"),
        "entity name lands on the Initial List as static content:\n{text}",
    );
    assert!(
        text.contains("Libra"),
        "the managing member fills slot 1 of the Articles:\n{text}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forms_an_llc_through_the_cli_with_answer_flags() {
    let _process_guard = CLI_PROCESS_LOCK.lock().await;
    let (base, surreal) = spawn("flags").await;
    let (project_code, admin_id) = seed_formation_matter(&surreal, "bright-star-flags").await;
    let tmp = tempfile::tempdir().unwrap();
    let creds = write_creds(tmp.path(), &base, admin_id);

    // 1. Open the formation notation on the pre-existing matter.
    let (ok, out) = run_cli(
        &creds,
        &[
            "site",
            "notation",
            "create",
            "--host",
            &base,
            "nv__llc_formation",
            "--client-email",
            "libra@example.com",
            "--project",
            &project_code,
        ],
    )
    .await;
    assert!(ok, "notation create failed:\n{out}");
    let id = notation_id_from(&out).to_string();

    // 2. Answer all six questions non-interactively: three picker
    //    selections, one scalar, one people_list row, then one scalar.
    let (ok, out) = run_cli(
        &creds,
        &[
            "site",
            "intake",
            "answer",
            &id,
            "--host",
            &base,
            "--select",
            "person__client=2",
            "--select",
            "entity__company=1",
            "--select",
            "person__registered_agent=1",
            "--answer",
            "members",
            "--person",
            "name=Libra,street=1 Main St,city=Las Vegas,state=NV,zip=89101,country=USA",
            "--answer",
            "2026-07-01",
        ],
    )
    .await;
    assert!(ok, "intake answer failed:\n{out}");
    assert!(
        out.contains("questionnaire complete"),
        "walk completes:\n{out}"
    );

    // 3. Status: the client/lawyer intake parks at attorney review. The
    //    packet is rendered only after explicit approval.
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
        "packet must not be ready before approval:\n{out}"
    );

    // 4. Approve: render + park the packet at the generate_pdf step.
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
        out.contains("\"state\": \"generate_pdf__articles_pdf\""),
        "state:\n{out}"
    );
    assert!(
        out.contains("\"document_ready\": true"),
        "packet ready after approve:\n{out}"
    );

    // 5. Download the filled packet and assert its AcroForm fields.
    let pdf_path = tmp.path().join("llc.pdf");
    let pdf_str = pdf_path.to_str().unwrap();
    let (ok, out) = run_cli(
        &creds,
        &[
            "site", "notation", "document", &id, "--out", pdf_str, "--host", &base,
        ],
    )
    .await;
    assert!(ok, "notation document failed:\n{out}");
    assert_filled_packet(&std::fs::read(&pdf_path).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forms_an_llc_through_the_interactive_cli_walk() {
    let _process_guard = CLI_PROCESS_LOCK.lock().await;
    let (base, surreal) = spawn("interactive").await;
    let (project_code, admin_id) = seed_formation_matter(&surreal, "bright-star-interactive").await;
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
            "nv__llc_formation",
            "--client-email",
            "libra@example.com",
            "--project",
            &project_code,
        ],
    )
    .await;
    assert!(ok, "notation create failed:\n{out}");
    let id = notation_id_from(&out).to_string();

    // Scripted stdin: picker selections for the client and matter entity,
    // then the registered-agent picker, then a scalar, then the
    // people_list row (name, then title/street/city/state/zip/country,
    // then a blank name to end), then the final scalar. A blank line is an
    // empty answer for that prompt.
    let stdin = concat!(
        "2\n", // person__client: the notation client row
        "1\n", // entity__company: the matter entity
        "1\n", // person__registered_agent: an in-scope matter person
        "members\n",
        // managing_members people_list, row 1:
        "Libra\n", // name
        "\n",      // title (blank)
        "1 Main St\n",
        "Las Vegas\n",
        "NV\n",
        "89101\n",
        "USA\n",
        "\n", // blank name ends the rows
        // formation_date:
        "2026-07-01\n",
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

    // The interactive walk parks at review. Approve to render the same
    // packet, then download via the CLI and assert the founder's answers
    // landed on the official form.
    let (ok, out) = run_cli(
        &creds,
        &["site", "notation", "approve", &id, "--host", &base],
    )
    .await;
    assert!(ok, "notation approve failed:\n{out}");

    let pdf_path = tmp.path().join("llc.pdf");
    let pdf_str = pdf_path.to_str().unwrap();
    let (ok, out) = run_cli(
        &creds,
        &[
            "site", "notation", "document", &id, "--out", pdf_str, "--host", &base,
        ],
    )
    .await;
    assert!(ok, "notation document failed:\n{out}");
    assert_filled_packet(&std::fs::read(&pdf_path).unwrap());
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
