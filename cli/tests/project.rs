//! End-to-end tests for `cli project create`. The subcommand runs the
//! canonical seed against the target store so the `shook.family` entity
//! is in place by the time we look it up. The
//! canonical seed only seeds Nick as ADMIN, so each test that needs a
//! client DRI seeds a `role = client` person explicitly first — the
//! row survives the (idempotent) seed the subcommand runs.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use assert_cmd::cargo::cargo_bin;
use store::test_support::ServerSurreal;
use tempfile::TempDir;

/// A throwaway object-storage root shared by every subprocess this binary
/// spawns. `project create` opens object storage before it touches the
/// database, and `cloud`'s selector fails closed on an unset
/// `NAVIGATOR_STORAGE_BACKEND` (#618) — so each invocation names a backend.
/// These tests assert on database rows and never on stored objects, so one
/// scratch root is enough, and pointing at it keeps a `./var/storage` out of
/// the source tree.
fn storage_root() -> &'static Path {
    static ROOT: OnceLock<TempDir> = OnceLock::new();
    ROOT.get_or_init(|| TempDir::new().expect("storage root tempdir"))
        .path()
}

/// The `navigator` binary with object storage configured.
fn navigator() -> Command {
    let mut cmd = Command::new(cargo_bin("navigator"));
    cmd.env("NAVIGATOR_STORAGE_BACKEND", "fs")
        .env("NAVIGATOR_STORAGE_FS_ROOT", storage_root());
    cmd
}

/// The `navigator` binary with object storage configured **and** pointed
/// at this test's person store, so the subprocess resolves the client the
/// test just seeded.
fn navigator_with(store: &ServerSurreal) -> Command {
    let mut cmd = navigator();
    cmd.env(store::surreal::ENDPOINT_ENV, &store.config.endpoint)
        .env(store::surreal::NAMESPACE_ENV, &store.config.namespace)
        .env(store::surreal::DATABASE_ENV, &store.config.database);
    if let store::surreal::SurrealAuth::Password {
        scope,
        username,
        password,
    } = &store.config.auth
    {
        cmd.env(store::surreal::USER_ENV, username)
            .env(store::surreal::PASSWORD_ENV, password)
            .env(store::surreal::AUTH_SCOPE_ENV, scope.as_str());
    }
    cmd
}

/// Insert a `role = client` person with `email` into this test's person
/// store so `project create --client-email <email>` can resolve it.
async fn seed_client(surreal: &store::surreal::SurrealDb, name: &str, email: &str) {
    store::persons::create(
        surreal,
        &store::persons::NewPerson::with_role(name, email, store::persons::Role::Client),
    )
    .await
    .expect("seed client person");
}

#[tokio::test]
async fn create_project_inserts_row_linked_to_seeded_entity() {
    let Some(store) = store::test_support::server_surreal(
        "test_cli_create_project_inserts_row_linked_to_seeded_entity",
    )
    .await
    else {
        return;
    };
    seed_client(&store.db, "Estate Client", "estate.client@example.com").await;
    let repo_root = tempfile::tempdir().expect("repo root tempdir");
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Shook Estate",
            "--code",
            "shook-estate",
            "--entity-name",
            "shook.family",
            "--client-email",
            "estate.client@example.com",
            "--attest",
        ])
        .env("NAVIGATOR_GIT_REPO_ROOT", repo_root.path())
        .output()
        .expect("run navigator project create");
    assert!(
        out.status.success(),
        "project create failed: stdout=\n{}\nstderr=\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Shook Estate"),
        "expected name in stdout: {stdout}"
    );

    store::projects::find_by_name(&store.db, "Shook Estate")
        .await
        .expect("query project")
        .expect("project row exists");
}

#[tokio::test]
async fn create_project_needs_no_repository_configuration() {
    let Some(store) = store::test_support::server_surreal(
        "test_cli_create_project_needs_no_repository_configuration",
    )
    .await
    else {
        return;
    };
    seed_client(&store.db, "Repo Client", "repo.client@example.com").await;
    // A matter has no repository, so opening one provisions nothing and
    // reaches no forge: `create` succeeds with nothing configured and no
    // network available.
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Repo Matter",
            "--code",
            "repo-matter",
            "--entity-name",
            "shook.family",
            "--client-email",
            "repo.client@example.com",
            "--attest",
        ])
        .output()
        .expect("run navigator project create");
    assert!(
        out.status.success(),
        "project create failed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr),
    );

    store::projects::find_by_name(&store.db, "Repo Matter")
        .await
        .expect("query project")
        .expect("project row exists");
}

#[tokio::test]
async fn create_project_seeds_participation_for_both_dris() {
    let Some(store) = store::test_support::server_surreal(
        "test_cli_create_project_seeds_participation_for_both_dris",
    )
    .await
    else {
        return;
    };
    seed_client(&store.db, "Seen Client", "seen.client@example.com").await;
    let repo_root = tempfile::tempdir().expect("repo root tempdir");
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Participation Matter",
            "--code",
            "participation-matter",
            "--entity-name",
            "shook.family",
            "--client-email",
            "seen.client@example.com",
            "--attest",
        ])
        .env("NAVIGATOR_GIT_REPO_ROOT", repo_root.path())
        .output()
        .expect("run navigator project create");
    assert!(
        out.status.success(),
        "project create failed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    // The output surfaces the matter code so the next step can refer to it.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("code="),
        "expected a code in stdout: {stdout}"
    );

    let row = store::projects::find_by_name(&store.db, "Participation Matter")
        .await
        .expect("query project")
        .expect("project row exists");
    // Both DRIs participate — `can_see_project` gates on these rows, so a
    // later `notation create --project` is authorized without a portal step.
    let roles = store::projects::participations_for_project(&store.db, row.id)
        .await
        .expect("query participation");
    let mut parts: Vec<String> = roles.into_iter().map(|r| r.participation).collect();
    parts.sort();
    assert!(
        parts.contains(&"client".to_string()),
        "client must participate: {parts:?}",
    );
    assert!(
        parts.iter().any(|p| p == "attorney"),
        "lawyer DRI must participate: {parts:?}",
    );
}

#[tokio::test]
async fn create_project_without_entity_link_is_rejected() {
    // A matter always opens against a pre-existing entity, so `project
    // create` requires `--entity-name`. The entity is resolved before
    // the client, so this fails on the missing entity even though a
    // valid `--client-email` is supplied.
    let Some(store) = store::test_support::server_surreal(
        "test_cli_create_project_without_entity_link_is_rejected",
    )
    .await
    else {
        return;
    };
    seed_client(&store.db, "Orphan Client", "orphan.client@example.com").await;
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Orphan Matter",
            "--code",
            "orphan-matter",
            "--client-email",
            "orphan.client@example.com",
            "--attest",
        ])
        .output()
        .expect("run navigator project create");
    assert!(
        !out.status.success(),
        "create without --entity-name should be rejected",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("entity"),
        "error should name the missing entity: {stderr}"
    );
}

#[tokio::test]
async fn create_project_with_skip_seed_uses_the_existing_schema() {
    let Some(store) = store::test_support::server_surreal("test_cli_skip_migrate_and_seed").await
    else {
        return;
    };
    // First pass: prime the store by seeding via the default mode.
    seed_client(&store.db, "Prime Client", "prime.client@example.com").await;
    let repo_root = tempfile::tempdir().expect("repo root tempdir");
    let prime = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Prime Project",
            "--code",
            "prime-project",
            "--entity-name",
            "shook.family",
            "--client-email",
            "prime.client@example.com",
            "--attest",
        ])
        .env("NAVIGATOR_GIT_REPO_ROOT", repo_root.path())
        .output()
        .expect("prime run");
    assert!(
        prime.status.success(),
        "prime failed: stderr=\n{}",
        String::from_utf8_lossy(&prime.stderr)
    );

    // Second pass: --skip-seed against the same store. No seed — must
    // still succeed because the `shook.family` row and the seeded client
    // already exist from the first pass.
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Shook Estate Production",
            "--code",
            "shook-estate-production",
            "--entity-name",
            "shook.family",
            "--client-email",
            "prime.client@example.com",
            "--attest",
            "--skip-seed",
        ])
        .env("NAVIGATOR_GIT_REPO_ROOT", repo_root.path())
        .output()
        .expect("run navigator project create --skip-seed");
    assert!(
        out.status.success(),
        "--skip-seed create failed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Shook Estate Production"));
}

#[tokio::test]
async fn create_project_without_attest_is_refused() {
    // Every matter open requires the attorney's conflict attestation. Without
    // `--attest` the shared command refuses the open and writes nothing, even
    // when every other argument resolves. This is the CLI door onto the same
    // attestation gate the web form and `POST /app/api/projects` enforce (#355).
    let Some(store) =
        store::test_support::server_surreal("test_cli_create_project_without_attest_is_refused")
            .await
    else {
        return;
    };
    seed_client(
        &store.db,
        "Unattested Client",
        "unattested.client@example.com",
    )
    .await;
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Unattested Matter",
            "--code",
            "unattested-matter",
            "--entity-name",
            "shook.family",
            "--client-email",
            "unattested.client@example.com",
        ])
        .output()
        .expect("run navigator project create");
    assert!(
        !out.status.success(),
        "create without --attest should be refused",
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("requires attestation"),
        "error should require attestation: {stderr}"
    );
    let row = store::projects::find_by_name(&store.db, "Unattested Matter")
        .await
        .expect("query project");
    assert!(row.is_none(), "a refused open writes no project row");
}

#[tokio::test]
async fn create_project_rejects_unknown_entity_name() {
    // The entity is resolved before the client, so an unknown
    // `--entity-name` fails even with a valid `--client-email`.
    let Some(store) =
        store::test_support::server_surreal("test_cli_create_project_rejects_unknown_entity_name")
            .await
    else {
        return;
    };
    seed_client(&store.db, "Bad Link Client", "badlink.client@example.com").await;
    let out = navigator_with(&store)
        .args([
            "project",
            "create",
            "--name",
            "Bad Link",
            "--code",
            "bad-link",
            "--entity-name",
            "definitely.not.a.real.entity",
            "--client-email",
            "badlink.client@example.com",
            "--attest",
        ])
        .output()
        .expect("run navigator project create");
    assert!(
        !out.status.success(),
        "expected nonzero exit for unknown entity"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no entity named"),
        "expected explanatory error on stderr: {stderr}"
    );
}
