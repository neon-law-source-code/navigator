//! `navigator projects surfaces` — create or adopt a Project's three handles.
//!
//! Opening a Project records its identity. This command then creates or
//! adopts the documents-bucket prefix, the Drive ingest folder, and the
//! source repository that identity names. Matter-open already runs the same
//! pass best-effort; this is the operator retry when Drive or the forge was
//! down, or when a legacy row never received one.

use std::process::ExitCode;

use crate::palette;

/// `navigator projects surfaces reconcile --project <code>`.
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
            println!(
                "{}  {:<20}  {}",
                palette::highlight("ok  "),
                "code",
                surfaces.code
            );
            println!(
                "{}  {:<20}  {}",
                palette::highlight("ok  "),
                "documents prefix",
                surfaces.documents_prefix
            );
            println!(
                "{}  {:<20}  {}",
                palette::highlight("ok  "),
                "drive folder",
                surfaces.drive_folder_id.as_deref().unwrap_or("—")
            );
            println!(
                "{}  {:<20}  {}",
                palette::highlight("ok  "),
                "repository",
                surfaces.repository_url.as_deref().unwrap_or("—")
            );
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
}
