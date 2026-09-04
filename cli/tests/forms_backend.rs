//! The `forms` subcommands resolve their assets storage through the shared,
//! backend-agnostic `cloud` seam (#478). Two behaviors are load-bearing and
//! only observable through the real binary, since `with_assets_storage`
//! builds a Tokio runtime and a live storage handle:
//!
//! - a `--bucket` override against the `fs` backend is refused loudly,
//!   rather than silently writing to the local `fs` root;
//! - with no backend selected at all the command fails naming
//!   `NAVIGATOR_STORAGE_BACKEND` (#618);
//! - with an explicit `fs` backend the command reaches the storage handle
//!   and surfaces a real read miss (proving the lane is wired end to end).
//!
//! Each run uses a temp CWD so the binary's `.devx/env` / `.env` auto-load
//! finds nothing, and sets the backend explicitly, so the process
//! environment of the test runner cannot leak in.

use assert_cmd::Command;
use predicates::str::contains;

/// The `fs` backend ignores bucket names — so honoring `--bucket` against it
/// would silently write to `./var/storage`. It must fail loudly instead.
#[test]
fn a_bucket_override_against_the_fs_backend_is_refused() {
    let cwd = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    Command::cargo_bin("navigator")
        .unwrap()
        .current_dir(cwd.path())
        .env("NAVIGATOR_STORAGE_BACKEND", "fs")
        .env("NAVIGATOR_STORAGE_FS_ROOT", root.path())
        .env_remove("NAVIGATOR_ASSETS_BUCKET")
        .args(["forms", "sync", "--bucket", "some-object-store-bucket"])
        .assert()
        .failure()
        .code(2)
        .stderr(contains("--bucket names an object store"));
}

/// With no backend selected the command must not fall back to the local
/// filesystem — `cloud`'s selector refuses by name, so the operator learns
/// storage is unconfigured instead of finding the vendored forms on the
/// wrong disk (#618).
#[test]
fn an_unset_backend_fails_naming_the_storage_selector() {
    let cwd = tempfile::tempdir().unwrap();
    Command::cargo_bin("navigator")
        .unwrap()
        .current_dir(cwd.path())
        .env_remove("NAVIGATOR_STORAGE_BACKEND")
        .env_remove("NAVIGATOR_ASSETS_BUCKET")
        .args(["forms", "fields", "us__naturalization"])
        .assert()
        .failure()
        .code(2)
        .stderr(contains("NAVIGATOR_STORAGE_BACKEND"));
}

/// With an explicit `fs` backend the forms lane builds its storage handle
/// and reads through it; an empty root is a real "not found", proving the
/// command reaches storage rather than erroring on construction.
#[test]
fn the_fs_backend_reaches_storage_and_surfaces_a_read_miss() {
    let cwd = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    Command::cargo_bin("navigator")
        .unwrap()
        .current_dir(cwd.path())
        .env("NAVIGATOR_STORAGE_BACKEND", "fs")
        .env("NAVIGATOR_STORAGE_FS_ROOT", root.path())
        .env_remove("NAVIGATOR_ASSETS_BUCKET")
        .args(["forms", "fields", "us__naturalization"])
        .assert()
        .failure()
        .code(2)
        .stderr(contains("us__naturalization.pdf"));
}
