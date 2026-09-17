//! `navigator project surfaces` — create or adopt a Project's three handles.
//!
//! Opening a Project records its identity. This command then creates or
//! adopts the documents-bucket prefix, the Drive ingest folder, and the
//! source repository that identity names. Matter-open already runs the same
//! pass best-effort; this is the operator retry when Drive or the forge is
//! down, or when a legacy row never received one.
//!
//! This is an HTTP client, not a database client: it authenticates like every
//! other `navigator site` command, through `crate::remote::resolve`, and does
//! its work through `GET /app/api/projects` (to resolve the given code to a
//! matter id, the same way `navigator project close` does) and
//! `POST /app/api/project-surfaces/{id}` (the door `store::project_surfaces`
//! sits behind). It never opens a `SurrealDb` connection of its own — even
//! against a local deployment, the site's own admin-tier check and the
//! server's Drive/forge credentials are what run the reconcile, not this
//! process.

use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use uuid::Uuid;

use crate::palette;
use store::project_surfaces::{ProjectSurfaces, SurfaceStatus};

/// The label `navigator project surfaces reconcile` prints for a
/// surface this pass never attempts — the documents-bucket prefix is a key
/// convention derived from the code, and the code itself is the input that
/// named which matter reconcile ran against. Neither is created, adopted, or
/// skipped for lack of configuration; both simply are, so both print this.
const NOT_APPLICABLE: &str = "not applicable";

fn status_label(status: SurfaceStatus) -> &'static str {
    match status {
        SurfaceStatus::Created => "created",
        SurfaceStatus::Present => "present",
        SurfaceStatus::Skipped => "skipped",
    }
}

fn print_surface_row(status_text: &str, name: &str, value: Option<&str>) {
    println!(
        "{}  {:<20}  {}",
        palette::highlight(format!("{status_text:<15}")),
        name,
        value.unwrap_or("—")
    );
}

/// Print one provisioned surface's row and report whether it is an
/// inconsistent "expected but not produced" state — attempted (`created` or
/// `present`) yet carrying no value, which reconcile's own contract never
/// leaves behind but a caller reading this report must be able to catch
/// rather than trust.
fn print_provisioned_surface(status: SurfaceStatus, name: &str, value: Option<&str>) -> bool {
    print_surface_row(status_label(status), name, value);
    let expected = matches!(status, SurfaceStatus::Created | SurfaceStatus::Present);
    expected && value.is_none()
}

/// One entry from `GET /app/api/projects` — only the two fields this command
/// needs to turn a human-typed code into the id the reconcile door takes.
#[derive(Debug, serde::Deserialize)]
struct VisibleProject {
    id: Uuid,
    code: String,
}

/// The first line of a response body, for a one-line error.
fn first_line(body: &str) -> &str {
    body.lines().next().unwrap_or_default().trim()
}

/// Resolve a Project code to its id through `GET /app/api/projects`, the same
/// door `navigator project close` reads. The reconcile door itself
/// takes an id, not a code, because a Project code is not guaranteed unique
/// across the id space at the route layer the way it is at the store layer.
async fn resolve_project_id(base: &str, token: &str, project_code: &str) -> Result<Uuid> {
    let response = reqwest::Client::new()
        .get(format!("{base}/app/api/projects"))
        .bearer_auth(token)
        .send()
        .await
        .context("GET /app/api/projects")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "list projects failed: {status}: {}",
            first_line(&body)
        ));
    }
    let projects: Vec<VisibleProject> =
        serde_json::from_str(&body).context("parse GET /app/api/projects")?;
    // Never interpolate the CLI's own project-code argument into a message:
    // it is read out of the same clap `Commands` enum a `Secrets` variant
    // lives on, which CodeQL's cleartext-logging query treats as tainted —
    // see `stderr_does_not_echo_the_cli_project_argument` below.
    projects
        .into_iter()
        .find(|project| project.code == project_code)
        .map(|project| project.id)
        .ok_or_else(|| anyhow!("no visible matter with that code"))
}

/// Create or adopt the Project's three handles through the admin-tier
/// reconcile door, `POST /app/api/project-surfaces/{id}`.
async fn post_reconcile(base: &str, token: &str, project_id: Uuid) -> Result<ProjectSurfaces> {
    let url = format!("{base}/app/api/project-surfaces/{project_id}");
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "surfaces reconcile failed: {status}: {}",
            first_line(&body)
        ));
    }
    serde_json::from_str(&body).context("parse POST /app/api/project-surfaces/{id}")
}

/// `navigator project surfaces reconcile --project <code> [--host h]`.
pub async fn reconcile(host: Option<&str>, project_code: &str) -> ExitCode {
    if !store::projects::is_valid_code(project_code) {
        eprintln!("navigator: invalid project code");
        return ExitCode::from(2);
    }
    let (base, token) = match crate::remote::resolve(host) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };
    let project_id = match resolve_project_id(&base, &token, project_code).await {
        Ok(id) => id,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };
    match post_reconcile(&base, &token, project_id).await {
        Ok(surfaces) => {
            print_surface_row(NOT_APPLICABLE, "code", Some(&surfaces.code));
            print_surface_row(
                NOT_APPLICABLE,
                "documents prefix",
                Some(&surfaces.documents_prefix),
            );
            let drive_incomplete = print_provisioned_surface(
                surfaces.drive_status,
                "drive folder",
                surfaces.drive_folder_id.as_deref(),
            );
            let repository_incomplete = print_provisioned_surface(
                surfaces.repository_status,
                "repository",
                surfaces.repository_url.as_deref(),
            );
            if drive_incomplete || repository_incomplete {
                eprintln!(
                    "navigator: a surface reconcile attempted produced no value; retry \
                     or check the deployment's Drive/forge configuration"
                );
                return ExitCode::from(2);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("navigator: {error:#}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn stderr_does_not_echo_the_cli_project_argument() {
        let src = include_str!("surfaces.rs");
        let production = src
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes the test module");
        assert!(
            !production.contains("{project_code}"),
            "echoing the CLI project argument trips CodeQL cleartext-logging because Command also carries Secrets"
        );
    }

    #[tokio::test]
    async fn invalid_code_is_refused_without_a_request() {
        assert_eq!(reconcile(None, "NOT A CODE").await, ExitCode::from(2));
    }

    #[tokio::test]
    async fn resolve_project_id_matches_the_visible_project_by_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .and(header("authorization", "Bearer a-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"id": "11111111-1111-1111-1111-111111111111", "code": "beta"},
                {"id": "22222222-2222-2222-2222-222222222222", "code": "acme"},
            ])))
            .mount(&server)
            .await;

        let id = resolve_project_id(&server.uri(), "a-token", "acme")
            .await
            .expect("acme is in the visible list");

        assert_eq!(id.to_string(), "22222222-2222-2222-2222-222222222222");
    }

    /// The one place a caller might expect the CLI's own argument to be quoted
    /// back — and deliberately isn't, per `stderr_does_not_echo_the_cli_project_argument`.
    #[tokio::test]
    async fn resolve_project_id_fails_closed_on_an_unlisted_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/app/api/projects"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let error = resolve_project_id(&server.uri(), "a-token", "acme")
            .await
            .expect_err("an absent code is an error");

        assert!(format!("{error:#}").contains("no visible matter"));
    }

    #[tokio::test]
    async fn post_reconcile_parses_the_recorded_surfaces() {
        let server = MockServer::start().await;
        let id = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path(format!("/app/api/project-surfaces/{id}")))
            .and(header("authorization", "Bearer a-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": "acme",
                "documents_prefix": "projects/acme/documents",
                "drive_folder_id": "folder-1",
                "drive_status": "created",
                "repository_url": "https://forge.example/acme",
                "repository_status": "present",
            })))
            .mount(&server)
            .await;

        let surfaces = post_reconcile(&server.uri(), "a-token", id)
            .await
            .expect("the door answers");

        assert_eq!(surfaces.code, "acme");
        assert_eq!(surfaces.drive_status, SurfaceStatus::Created);
        assert_eq!(surfaces.repository_status, SurfaceStatus::Present);
    }

    /// The reason the body is quoted rather than discarded: this door is
    /// admin-tier, so the overwhelmingly likely refusal is a caller who is
    /// merely lawyer-tier, and a bare `403 Forbidden` leaves them guessing.
    #[tokio::test]
    async fn post_reconcile_quotes_the_hosts_own_refusal() {
        let server = MockServer::start().await;
        let id = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path(format!("/app/api/project-surfaces/{id}")))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": "forbidden",
                "message": "This endpoint requires the admin tier."
            })))
            .mount(&server)
            .await;

        let error = post_reconcile(&server.uri(), "a-token", id)
            .await
            .expect_err("a refusal is an error");
        let rendered = format!("{error:#}");

        assert!(rendered.contains("403"), "{rendered}");
        assert!(rendered.contains("admin tier"), "{rendered}");
    }

    #[test]
    fn every_status_prints_a_distinct_label() {
        assert_eq!(status_label(SurfaceStatus::Created), "created");
        assert_eq!(status_label(SurfaceStatus::Present), "present");
        assert_eq!(status_label(SurfaceStatus::Skipped), "skipped");
    }

    #[test]
    fn a_produced_value_is_never_reported_incomplete() {
        assert!(!print_provisioned_surface(
            SurfaceStatus::Created,
            "drive folder",
            Some("folder-1")
        ));
        assert!(!print_provisioned_surface(
            SurfaceStatus::Present,
            "repository",
            Some("https://forge.example/acme")
        ));
    }

    #[test]
    fn skipping_for_lack_of_configuration_is_not_a_failure() {
        assert!(!print_provisioned_surface(
            SurfaceStatus::Skipped,
            "drive folder",
            None
        ));
    }

    #[test]
    fn an_attempted_surface_with_no_value_is_reported_incomplete() {
        assert!(print_provisioned_surface(
            SurfaceStatus::Created,
            "drive folder",
            None
        ));
        assert!(print_provisioned_surface(
            SurfaceStatus::Present,
            "repository",
            None
        ));
    }
}
