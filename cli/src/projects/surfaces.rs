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
            eprintln!("navigator: no matter with code {project_code}");
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
