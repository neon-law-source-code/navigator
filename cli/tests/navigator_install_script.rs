//! ENG-671: `.github/actions/navigator-install/install.sh` is the plain
//! Linux install path for a machine that is not a GitHub Actions runner.
//! These tests drive the real script — its version resolution and refusal
//! logic directly, and its download/checksum/install happy path against a
//! wiremock server via `NAVIGATOR_INSTALL_BASE_URL`, the seam that lets a
//! test stand in for github.com.

#[cfg(target_os = "linux")]
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;
#[cfg(target_os = "linux")]
use wiremock::matchers::{method, path};
#[cfg(target_os = "linux")]
use wiremock::{Mock, MockServer, ResponseTemplate};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root exists")
}

fn install_script() -> PathBuf {
    workspace_root().join(".github/actions/navigator-install/install.sh")
}

fn run_install(dir: &Path, args: &[&str], extra_env: &[(&str, &str)]) -> Output {
    let mut command = Command::new("sh");
    command.arg(install_script()).args(args).current_dir(dir);
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("run install.sh")
}

#[test]
fn refuses_latest_with_the_existing_message() {
    let dir = TempDir::new().unwrap();
    let output = run_install(dir.path(), &["--version", "latest"], &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("version must be an exact release tag, not 'latest'."),
        "{stderr}"
    );
}

#[test]
fn refuses_an_empty_version_when_no_navigator_yaml_exists() {
    let dir = TempDir::new().unwrap();
    let output = run_install(dir.path(), &[], &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("version must be an exact release tag, not ''."),
        "{stderr}"
    );
}

// install.sh refuses to run at all on a non-Linux platform (the Homebrew tap
// covers macOS instead), so the tests from here down — which need to reach
// the download step — only run on Linux, where CI's `cargo nextest run
// --workspace` exercises them for real.
#[cfg(target_os = "linux")]
#[test]
fn resolves_the_version_from_navigator_yaml_when_none_is_given() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("navigator.yaml"), "version: \"26.9.13\"\n").unwrap();
    // No server is reachable at this made-up host, so the script fails at
    // the download — proving it resolved 26.9.13 and tried to fetch it,
    // rather than refusing for want of a version.
    let output = run_install(
        dir.path(),
        &[],
        &[("NAVIGATOR_INSTALL_BASE_URL", "http://127.0.0.1:1")],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("navigator-26.9.13-linux.tar.gz") || stderr.contains("26.9.13"),
        "{stderr}"
    );
    assert!(!stderr.contains("must be an exact release tag"), "{stderr}");
}

/// Builds `navigator-<version>-linux.tar.gz` holding one executable script
/// that records its own invocation, plus a matching sha256 sidecar — the
/// same two-file shape `deploy.yml`'s Linux archive job now publishes.
#[cfg(target_os = "linux")]
fn write_release_fixture(dir: &Path, version: &str) -> (PathBuf, PathBuf) {
    let staging = dir.join("staging");
    fs::create_dir_all(&staging).unwrap();
    fs::write(
        staging.join("navigator"),
        "#!/bin/sh\necho ran-the-stub-navigator\n",
    )
    .unwrap();
    let asset = dir.join(format!("navigator-{version}-linux.tar.gz"));
    let status = Command::new("tar")
        .args(["-czf"])
        .arg(&asset)
        .args(["-C"])
        .arg(&staging)
        .arg("navigator")
        .status()
        .expect("run tar");
    assert!(status.success());

    let checksum = dir.join(format!("navigator-{version}-linux.tar.gz.sha256"));
    let sha = Command::new("sha256sum")
        .arg(&asset)
        .output()
        .expect("run sha256sum");
    assert!(sha.status.success());
    let line = String::from_utf8_lossy(&sha.stdout);
    let hash = line.split_whitespace().next().expect("a hash column");
    fs::write(
        &checksum,
        format!("{hash}  navigator-{version}-linux.tar.gz\n"),
    )
    .unwrap();

    (asset, checksum)
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn downloads_verifies_and_installs_the_published_archive() {
    let version = "26.9.42";
    let fixtures = TempDir::new().unwrap();
    write_release_fixture(fixtures.path(), version);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/navigator-{version}-linux.tar.gz")))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(
                fs::read(
                    fixtures
                        .path()
                        .join(format!("navigator-{version}-linux.tar.gz")),
                )
                .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/navigator-{version}-linux.tar.gz.sha256")))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(
                fs::read(
                    fixtures
                        .path()
                        .join(format!("navigator-{version}-linux.tar.gz.sha256")),
                )
                .unwrap(),
            ),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let install_dir = dir.path().join("bin");
    let output = run_install(
        dir.path(),
        &["--version", version, "--dir", install_dir.to_str().unwrap()],
        &[("NAVIGATOR_INSTALL_BASE_URL", &server.uri())],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        !stderr.contains("skipping checksum verification"),
        "{stderr}"
    );

    let installed = install_dir.join("navigator");
    assert!(installed.is_file());
    let metadata = fs::metadata(&installed).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o755);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_mismatched_checksum_fails_the_install() {
    let version = "26.9.43";
    let fixtures = TempDir::new().unwrap();
    let (_, checksum) = write_release_fixture(fixtures.path(), version);
    // Corrupt the recorded hash so it no longer matches the archive bytes.
    fs::write(
        &checksum,
        format!("0000000000000000000000000000000000000000000000000000000000000000  navigator-{version}-linux.tar.gz\n"),
    )
    .unwrap();

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/navigator-{version}-linux.tar.gz")))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(
                fs::read(
                    fixtures
                        .path()
                        .join(format!("navigator-{version}-linux.tar.gz")),
                )
                .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/navigator-{version}-linux.tar.gz.sha256")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(fs::read(&checksum).unwrap()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let install_dir = dir.path().join("bin");
    let output = run_install(
        dir.path(),
        &["--version", version, "--dir", install_dir.to_str().unwrap()],
        &[("NAVIGATOR_INSTALL_BASE_URL", &server.uri())],
    );
    assert!(!output.status.success());
    assert!(!install_dir.join("navigator").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_missing_checksum_sidecar_warns_but_still_installs() {
    let version = "26.9.44";
    let fixtures = TempDir::new().unwrap();
    write_release_fixture(fixtures.path(), version);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/navigator-{version}-linux.tar.gz")))
        .respond_with(
            ResponseTemplate::new(200).set_body_bytes(
                fs::read(
                    fixtures
                        .path()
                        .join(format!("navigator-{version}-linux.tar.gz")),
                )
                .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/navigator-{version}-linux.tar.gz.sha256")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let install_dir = dir.path().join("bin");
    let output = run_install(
        dir.path(),
        &["--version", version, "--dir", install_dir.to_str().unwrap()],
        &[("NAVIGATOR_INSTALL_BASE_URL", &server.uri())],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        stderr.contains("skipping checksum verification"),
        "{stderr}"
    );
    assert!(install_dir.join("navigator").is_file());
}
