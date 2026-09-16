//! `navigator site authorities create` — file a global Authority (the citation
//! apparatus' shared reference data, #890) with an archived artifact,
//! through the logged-in deployment's `POST /app/api/authorities` door
//! (`portal::authorities_api`).
//!
//! The CLI reads no `SurrealDB` environment and writes nothing locally: the
//! server resolves the bearer back into the caller's session, checks the
//! lawyer tier, ingests the archive through the Asset service, and calls
//! `store::authorities::record`. This module only forms that one
//! authenticated HTTP request — [`crate::remote::resolve`] is the same
//! stored-login seam every other CLI verb reaches through, reused here
//! rather than duplicated.
//!
//! This is a Project repository's supported destination for an archived
//! legal authority — see the `legal-authority` skill's note that, before
//! this command existed, there was none.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use base64::Engine as _;
use serde::Deserialize;
use uuid::Uuid;

/// [`crate::remote::exit_code_for`] distinguishes a CI mint refusal from
/// every other failure — not reachable from this command today (it always
/// uses the stored login, never a CI-minted token), but reused so a future
/// caller of this function inherits that behavior for free.
async fn run<F>(fut: F) -> ExitCode
where
    F: std::future::Future<Output = Result<()>>,
{
    match fut.await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            crate::remote::exit_code_for(&error)
        }
    }
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

/// The `POST /app/api/authorities` response — `store::authorities::Authority`
/// carries no `Deserialize`, so this is the CLI's own read-side mirror of the
/// same wire shape `portal::authorities_api::create_authority_door` returns.
#[derive(Debug, Deserialize, serde::Serialize)]
struct AuthorityResponse {
    id: Uuid,
    class: String,
    citation: String,
    short_cite: Option<String>,
    title: String,
    publisher: Option<String>,
    issued_on: Option<String>,
    canonical_url: Option<String>,
    checked_on: Option<String>,
    archived_asset_id: Option<Uuid>,
    inserted_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// What a new Authority needs — mirrors `store::authorities::NewAuthority`
/// plus the archive this command reads locally, none of which a bare
/// datastore write (forbidden by the `legal-authority` skill) could carry
/// authenticated.
#[allow(clippy::too_many_arguments)]
struct NewAuthorityArgs<'a> {
    class: &'a str,
    citation: &'a str,
    title: &'a str,
    short_cite: Option<&'a str>,
    publisher: Option<&'a str>,
    issued_on: Option<&'a str>,
    canonical_url: Option<&'a str>,
    checked_on: Option<&'a str>,
    file: &'a Path,
    content_type: Option<&'a str>,
}

async fn create_authority(
    host: Option<&str>,
    args: &NewAuthorityArgs<'_>,
) -> Result<AuthorityResponse> {
    let bytes =
        std::fs::read(args.file).with_context(|| format!("read {}", args.file.display()))?;
    let (base, token) = crate::remote::resolve(host)?;
    let body = serde_json::json!({
        "class": args.class,
        "citation": args.citation,
        "title": args.title,
        "short_cite": args.short_cite,
        "publisher": args.publisher,
        "issued_on": args.issued_on,
        "canonical_url": args.canonical_url,
        "checked_on": args.checked_on,
        "archive_base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "content_type": args.content_type,
    });
    let url = format!("{base}/app/api/authorities");
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
            "authority create failed: {status}: {}",
            first_line(&text)
        ));
    }
    serde_json::from_str(&text).context("parse authority response")
}

/// `navigator site authorities create --class … --citation … --title … --file …`
/// — file one Authority with its archived artifact through the live site's
/// `POST /app/api/authorities` door, using the stored login credential.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    host: Option<&str>,
    class: &str,
    citation: &str,
    title: &str,
    short_cite: Option<&str>,
    publisher: Option<&str>,
    issued_on: Option<&str>,
    canonical_url: Option<&str>,
    checked_on: Option<&str>,
    file: &Path,
    content_type: Option<&str>,
) -> ExitCode {
    run(async {
        let authority = create_authority(
            host,
            &NewAuthorityArgs {
                class,
                citation,
                title,
                short_cite,
                publisher,
                issued_on,
                canonical_url,
                checked_on,
                file,
                content_type,
            },
        )
        .await?;
        print!(
            "{}",
            serde_yaml::to_string(&authority).context("render authority as yaml")?
        );
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::create;
    use crate::credentials::{self, Credentials, HostCredential};
    use std::process::ExitCode;
    use std::sync::LazyLock;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // `NAVIGATOR_CREDENTIALS_FILE` is process-global env, so tests that set
    // it must not run concurrently with each other — the same discipline
    // `cli::remote`'s own tests use.
    static CREDENTIALS_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
        LazyLock::new(|| tokio::sync::Mutex::new(()));

    struct CredentialsEnv {
        _dir: tempfile::TempDir,
    }

    impl CredentialsEnv {
        fn logged_in(base: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("navigator.json");
            let mut creds = Credentials::default();
            creds.set(
                base,
                HostCredential {
                    token: "test-token".into(),
                    person_email: Some("lawyer@neonlaw.com".into()),
                    role: Some("lawyer".into()),
                    expires_at: i64::MAX,
                },
            );
            credentials::save(&path, &creds).unwrap();
            std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", path);
            Self { _dir: dir }
        }

        /// A credentials file that exists but holds no login for any host.
        fn logged_out() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("navigator.json");
            credentials::save(&path, &Credentials::default()).unwrap();
            std::env::set_var("NAVIGATOR_CREDENTIALS_FILE", path);
            Self { _dir: dir }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn create_refuses_without_a_stored_login() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let _env = CredentialsEnv::logged_out();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("opinion.pdf");
        std::fs::write(&file, b"synthetic opinion bytes").unwrap();

        let exit_code = create(
            Some(server.uri().as_str()),
            "case_law",
            "410 U.S. 113 (1973)",
            "Roe v. Wade",
            None,
            None,
            None,
            None,
            None,
            &file,
            None,
        )
        .await;

        assert_eq!(exit_code, ExitCode::from(2));
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "an unauthenticated caller must never reach the network"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn create_forms_the_request_correctly_against_a_mock() {
        let _lock = CREDENTIALS_ENV_LOCK.lock().await;
        let server = MockServer::start().await;
        let server_uri = server.uri();
        let _env = CredentialsEnv::logged_in(&server_uri);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("opinion.pdf");
        std::fs::write(&file, b"synthetic opinion bytes").unwrap();
        let asset_id = uuid::Uuid::now_v7();
        let authority_id = uuid::Uuid::now_v7();

        Mock::given(method("POST"))
            .and(path("/app/api/authorities"))
            .and(body_json(serde_json::json!({
                "class": "case_law",
                "citation": "410 U.S. 113 (1973)",
                "title": "Roe v. Wade",
                "short_cite": "Roe",
                "publisher": null,
                "issued_on": null,
                "canonical_url": "https://example.com/roe",
                "checked_on": null,
                "archive_base64": "c3ludGhldGljIG9waW5pb24gYnl0ZXM=",
                "content_type": "application/pdf",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": authority_id,
                "class": "case_law",
                "citation": "410 U.S. 113 (1973)",
                "short_cite": "Roe",
                "title": "Roe v. Wade",
                "publisher": null,
                "issued_on": null,
                "canonical_url": "https://example.com/roe",
                "checked_on": null,
                "archived_asset_id": asset_id,
                "inserted_at": "2026-09-16T00:00:00Z",
                "updated_at": "2026-09-16T00:00:00Z",
            })))
            .expect(1)
            .mount(&server)
            .await;

        let exit_code = create(
            Some(server_uri.as_str()),
            "case_law",
            "410 U.S. 113 (1973)",
            "Roe v. Wade",
            Some("Roe"),
            None,
            None,
            Some("https://example.com/roe"),
            None,
            &file,
            Some("application/pdf"),
        )
        .await;

        assert_eq!(exit_code, ExitCode::SUCCESS);
    }
}
