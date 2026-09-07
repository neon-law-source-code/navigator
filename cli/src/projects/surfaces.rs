//! `navigator site projects surfaces` — create or adopt a Project's three handles.
//!
//! Opening a Project records its identity. This command then creates or
//! adopts the documents-bucket prefix, the Drive ingest folder, and the
//! source repository that identity names. Matter-open already runs the same
//! pass best-effort; this is the operator retry when Drive or the forge was
//! down, or when a legacy row never received one.

use std::process::ExitCode;

use crate::palette;
use store::project_surfaces::SurfaceStatus;

/// The label `navigator site projects surfaces reconcile` prints for a
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

/// `navigator site projects surfaces reconcile --project <code>`.
pub async fn reconcile(project_code: &str) -> ExitCode {
    if !store::projects::is_valid_code(project_code) {
        eprintln!("navigator: invalid project code");
        return ExitCode::from(2);
    }
    let surreal = match store::surreal::connect_from_env().await {
        Ok(db) => db,
        Err(error) => {
            eprintln!("navigator: surreal: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = store::schema::apply(&surreal).await {
        eprintln!("navigator: schema: {error}");
        return ExitCode::from(2);
    }
    let project = match store::projects::find_by_code(&surreal, project_code).await {
        Ok(Some(project)) => project,
        Ok(None) => {
            eprintln!("navigator: no matter with that code");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("navigator: {error}");
            return ExitCode::from(2);
        }
    };
    match store::project_surfaces::reconcile_from_env(&surreal, project.id).await {
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
            eprintln!("navigator: surfaces: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn invalid_code_is_refused_without_connecting() {
        assert_eq!(reconcile("NOT A CODE").await, ExitCode::from(2));
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
