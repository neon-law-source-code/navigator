//! `navigator site asset upload` — file one local public-safe asset to one
//! or more logged-in deployments' public assets buckets, through
//! `POST /app/api/assets` (`portal::assets_api`). This is the OAuth-backed
//! sibling of the ADC-backed `navigator ops assets upload` batch command
//! (`cli::assets::run_upload`): that command targets real GCS directly with
//! an operator's own bucket credentials for bulk gallery publication; this
//! one goes out with nothing but a `navigator site login` bearer, so the
//! caller never needs a bucket name or a GCP credential.
//!
//! `ASSET_NAME` is the local path the CLI reads bytes from. When it lives
//! below `server/public/`, its path relative to that root becomes the
//! bucket key — matching `docs/assets.md`'s convention for a hand-placed
//! asset (a brand SVG, a hero image, a font file). Pass `--key` when the
//! file lives elsewhere (a generated asset not yet staged under
//! `server/public/`) or the deployed key should differ from the local
//! path. The content type is always derived from the key's extension, the
//! same derivation the server itself performs and cross-checks — there is
//! no `--content-type` override to drift out of sync with it.
//!
//! The CLI reads no GCP environment and writes nothing locally: the server
//! resolves the bearer, checks the Owner/Admin tier, validates the key and
//! content type, checks the declared `sha256` against what it decodes, and
//! writes through `cloud::StorageService`. After a host's write succeeds,
//! this reads the asset straight back through that deployment's own public
//! `/assets/{key}` origin and compares it byte-for-byte against the local
//! file — proof the object a browser will actually fetch matches what was
//! sent, not merely that the authenticated write returned success.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::palette;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// The first non-blank line of a response body — an HTML error page or a
/// stack trace is one line of context, not a wall of text.
fn first_line(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("(empty response body)")
        .to_string()
}

/// `POST /app/api/assets` response — `portal::assets_api::UploadAssetResponse`
/// carries no `Deserialize`, so this is the CLI's own read-side mirror of the
/// same wire shape.
#[derive(Debug, Deserialize)]
struct UploadedAsset {
    key: String,
    bytes: usize,
    content_type: String,
    sha256: String,
    unchanged: bool,
}

/// The content type `portal::assets_api::expected_content_type` derives for
/// `key` — kept in sync with that function by hand, since the two crates
/// share no code. An unrecognized extension is `None`, which the caller
/// reports rather than sending a request the server will reject anyway.
fn expected_content_type(key: &str) -> Option<&'static str> {
    let path = Path::new(key);
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("webp") => Some("image/webp"),
        Some("avif") => Some("image/avif"),
        Some("svg") => Some("image/svg+xml"),
        Some("ico") => Some("image/x-icon"),
        Some("mp4") => Some("video/mp4"),
        Some("woff2") => Some("font/woff2"),
        Some("woff") => Some("font/woff"),
        Some("txt")
            if key.starts_with("fonts/")
                && path.file_name().and_then(|name| name.to_str()) == Some("OFL.txt") =>
        {
            Some("text/plain")
        }
        _ => None,
    }
}

/// The bucket key for `asset_name`: `explicit_key` verbatim (backslashes
/// normalized) when given, otherwise `asset_name`'s path relative to
/// `server/public/` under `root`. Errors when neither applies — an asset
/// outside `server/public/` needs an explicit `--key`.
fn resolve_key(root: &Path, asset_name: &Path, explicit_key: Option<&str>) -> Result<String> {
    if let Some(key) = explicit_key.map(str::trim).filter(|key| !key.is_empty()) {
        return Ok(key.replace('\\', "/"));
    }
    let public_root = root.join("server/public");
    let relative = asset_name.strip_prefix(&public_root).unwrap_or(asset_name);
    if relative == asset_name && asset_name.is_absolute() {
        return Err(anyhow!(
            "{} is outside server/public — pass --key brand/... or img/...",
            asset_name.display()
        ));
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

async fn upload_to_host(
    host: &str,
    key: &str,
    bytes: &[u8],
    content_type: &str,
    sha256: &str,
) -> Result<UploadedAsset> {
    let (base, token) = crate::remote::resolve(Some(host))?;
    let body = serde_json::json!({
        "key": key,
        "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
        "content_type": content_type,
        "sha256": sha256,
    });
    let url = format!("{base}/app/api/assets");
    let response = client()
        .post(&url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "asset upload to {base} failed: {status}: {}",
            first_line(&text)
        ));
    }
    let uploaded: UploadedAsset =
        serde_json::from_str(&text).context("parse asset upload response")?;

    // Read the object back through the deployment's own public origin — the
    // same route a browser uses — and compare it byte-for-byte, so success
    // here proves the write is live, not merely that the server accepted it.
    let asset_url = format!("{base}/assets/{key}");
    let served = client()
        .get(&asset_url)
        .send()
        .await
        .with_context(|| format!("GET {asset_url}"))?;
    if !served.status().is_success() {
        return Err(anyhow!(
            "{base} accepted the upload but {asset_url} answered {}",
            served.status()
        ));
    }
    let served_bytes = served
        .bytes()
        .await
        .with_context(|| format!("read {asset_url}"))?;
    if served_bytes.as_ref() != bytes {
        return Err(anyhow!(
            "{base} accepted the upload but {asset_url} serves different bytes — read-back verification failed"
        ));
    }

    Ok(uploaded)
}

/// `navigator site asset upload --host <h1> [--host <h2> ...] ASSET_NAME`
/// — publish one local asset to every named host, reporting each
/// separately. Exits non-zero if any requested host fails; hosts that
/// succeeded before a later failure have still been published — there is no
/// cross-host transaction, since each deployment is an independent write.
pub async fn upload(hosts: &[String], asset_name: &Path, key: Option<&str>) -> ExitCode {
    let root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(error) => {
            eprintln!("navigator: asset upload: current directory: {error}");
            return ExitCode::from(2);
        }
    };
    run_upload(&root, hosts, asset_name, key).await
}

/// [`upload`] with the repository root passed explicitly rather than read
/// from the process's current directory — the seam the test module drives,
/// so exercising this command never mutates process-global state that a
/// concurrently running test could observe.
async fn run_upload(
    root: &Path,
    hosts: &[String],
    asset_name: &Path,
    key: Option<&str>,
) -> ExitCode {
    let local_path: PathBuf = if asset_name.is_absolute() {
        asset_name.to_path_buf()
    } else {
        root.join(if key.is_some() {
            asset_name.to_path_buf()
        } else {
            Path::new("server/public").join(asset_name)
        })
    };
    let bytes = match std::fs::read(&local_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!(
                "navigator: asset upload: read {}: {error}",
                local_path.display()
            );
            return ExitCode::from(2);
        }
    };
    let key = match resolve_key(root, asset_name, key) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("navigator: asset upload: {error:#}");
            return ExitCode::from(2);
        }
    };
    let Some(content_type) = expected_content_type(&key) else {
        eprintln!(
            "navigator: asset upload: `{key}` has an unsupported extension for a public asset"
        );
        return ExitCode::from(2);
    };
    let sha256 = hex_digest(&bytes);

    let mut failures = Vec::new();
    for host in hosts {
        match upload_to_host(host, &key, &bytes, content_type, &sha256).await {
            Ok(uploaded) => {
                println!(
                    "{host}: {} {} ({} bytes, {}, sha256:{})",
                    if uploaded.unchanged {
                        "unchanged"
                    } else {
                        "uploaded"
                    },
                    uploaded.key,
                    uploaded.bytes,
                    uploaded.content_type,
                    uploaded.sha256
                );
            }
            Err(error) => {
                eprintln!("{host}: {error:#}");
                failures.push(host.clone());
            }
        }
    }

    if failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        eprintln!(
            "{}",
            palette::dim(format!("failed host(s): {}", failures.join(", ")))
        );
        ExitCode::from(1)
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::run_upload;
    use crate::credentials::{self, Credentials, HostCredential};
    use base64::Engine as _;
    use std::process::ExitCode;
    use std::sync::LazyLock;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // `NAVIGATOR_CREDENTIALS_FILE` is process-global env, so tests that set
    // it must not run concurrently with each other — the same discipline
    // `cli::remote`'s and `cli::authorities`'s own tests use.
    static CREDENTIALS_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    struct CredentialsEnv {
        _dir: tempfile::TempDir,
    }

    impl CredentialsEnv {
        fn logged_in(hosts: &[&str]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("navigator.json");
            let mut creds = Credentials::default();
            for host in hosts {
                creds.set(
                    *host,
                    HostCredential {
                        token: format!("test-token-{host}"),
                        person_email: Some("admin@neonlaw.com".into()),
                        role: Some("admin".into()),
                        expires_at: i64::MAX,
                    },
                );
            }
            credentials::save(&path, &creds).unwrap();
            std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", path);
            Self { _dir: dir }
        }

        fn logged_out() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("navigator.json");
            credentials::save(&path, &Credentials::default()).unwrap();
            std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", path);
            Self { _dir: dir }
        }
    }

    fn write_asset(dir: &std::path::Path, relative: &str, bytes: &[u8]) -> std::path::PathBuf {
        let full = dir.join("server/public").join(relative);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, bytes).unwrap();
        std::path::PathBuf::from(relative)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_refuses_without_a_stored_login() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let _env = CredentialsEnv::logged_out();
        let workdir = tempfile::tempdir().unwrap();
        let asset = write_asset(workdir.path(), "brand/rabbit.svg", b"<svg></svg>");

        let exit_code = run_upload(workdir.path(), &[server.uri()], &asset, None).await;

        assert_eq!(exit_code, ExitCode::from(1));
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "an unauthenticated caller must never reach the network"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_sends_the_bearer_the_digest_and_verifies_the_public_read_back() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::logged_in(&[server_uri.as_str()]);
        let workdir = tempfile::tempdir().unwrap();
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_vec();
        let asset = write_asset(workdir.path(), "brand/rabbit.svg", &bytes);
        let sha256 = super::hex_digest(&bytes);

        Mock::given(method("POST"))
            .and(path("/app/api/assets"))
            .and(body_json(serde_json::json!({
                "key": "brand/rabbit.svg",
                "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "content_type": "image/svg+xml",
                "sha256": sha256,
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "key": "brand/rabbit.svg",
                "bytes": bytes.len(),
                "content_type": "image/svg+xml",
                "sha256": sha256,
                "unchanged": false,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/assets/brand/rabbit.svg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(bytes.clone())
                    .insert_header("content-type", "image/svg+xml"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let exit_code = run_upload(workdir.path(), &[server_uri], &asset, None).await;

        assert_eq!(exit_code, ExitCode::SUCCESS);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_reports_each_host_and_fails_non_zero_when_one_host_errors() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let good = MockServer::start().await;
        let bad = MockServer::start().await;
        let good_uri = good.uri();
        let bad_uri = bad.uri();
        let _env = CredentialsEnv::logged_in(&[good_uri.as_str(), bad_uri.as_str()]);
        let workdir = tempfile::tempdir().unwrap();
        let bytes = b"\x89PNG logo bytes".to_vec();
        let asset = write_asset(workdir.path(), "brand/logo.png", &bytes);
        let sha256 = super::hex_digest(&bytes);

        Mock::given(method("POST"))
            .and(path("/app/api/assets"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "key": "brand/logo.png",
                "bytes": bytes.len(),
                "content_type": "image/png",
                "sha256": sha256,
                "unchanged": false,
            })))
            .mount(&good)
            .await;
        Mock::given(method("GET"))
            .and(path("/assets/brand/logo.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(bytes.clone())
                    .insert_header("content-type", "image/png"),
            )
            .mount(&good)
            .await;
        Mock::given(method("POST"))
            .and(path("/app/api/assets"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": "forbidden",
                "message": "not an admin",
            })))
            .mount(&bad)
            .await;

        let exit_code = run_upload(workdir.path(), &[good_uri, bad_uri], &asset, None).await;

        assert_eq!(exit_code, ExitCode::from(1));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_refuses_an_unsupported_extension() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::logged_in(&[server_uri.as_str()]);
        let workdir = tempfile::tempdir().unwrap();
        let asset = write_asset(workdir.path(), "brand/mystery.bin", b"bytes");

        let exit_code = run_upload(workdir.path(), &[server_uri], &asset, None).await;

        assert_eq!(exit_code, ExitCode::from(2));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_requires_an_explicit_key_outside_server_public() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::logged_in(&[server_uri.as_str()]);
        let workdir = tempfile::tempdir().unwrap();
        let outside = workdir.path().join("generated").join("art.png");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, b"art bytes").unwrap();

        let exit_code = run_upload(workdir.path(), &[server_uri], &outside, None).await;

        assert_eq!(exit_code, ExitCode::from(2));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn upload_accepts_an_explicit_key_for_an_asset_outside_server_public() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::logged_in(&[server_uri.as_str()]);
        let workdir = tempfile::tempdir().unwrap();
        let outside = workdir.path().join("generated").join("art.png");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        let bytes = b"\x89PNG art bytes".to_vec();
        std::fs::write(&outside, &bytes).unwrap();
        let sha256 = super::hex_digest(&bytes);

        Mock::given(method("POST"))
            .and(path("/app/api/assets"))
            .and(body_json(serde_json::json!({
                "key": "img/death-and-divorce/video-art.png",
                "content_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
                "content_type": "image/png",
                "sha256": sha256,
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "key": "img/death-and-divorce/video-art.png",
                "bytes": bytes.len(),
                "content_type": "image/png",
                "sha256": sha256,
                "unchanged": false,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/assets/img/death-and-divorce/video-art.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(bytes.clone())
                    .insert_header("content-type", "image/png"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let exit_code = run_upload(
            workdir.path(),
            &[server_uri],
            &outside,
            Some("img/death-and-divorce/video-art.png"),
        )
        .await;

        assert_eq!(exit_code, ExitCode::SUCCESS);
    }
}
