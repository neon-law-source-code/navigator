//! `navigator project setup` — converge the existing Project setup doors.
//!
//! This is deliberately an HTTP composition. The surface, Slack, and Notion
//! operations keep their own authenticated server-side authorization and
//! provider semantics; this command only joins their per-resource outcomes
//! into one operator report. A retry therefore reuses the recorded resources
//! through the same doors instead of inventing a second provisioning path.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::projects::manifest;
use crate::projects::surfaces::{self, VisibleProject};
use crate::remote::{self, IntegrationOutcome};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ResourceOutcome {
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ProjectSetupOutcome {
    project_code: String,
    resources: BTreeMap<String, ResourceOutcome>,
}

#[derive(Debug, Serialize)]
struct SetupReport {
    results: Vec<ProjectSetupOutcome>,
}

fn resource(outcome: impl Into<String>) -> ResourceOutcome {
    ResourceOutcome {
        outcome: outcome.into(),
        detail: None,
    }
}

fn resource_with_detail(outcome: impl Into<String>, detail: impl Into<String>) -> ResourceOutcome {
    ResourceOutcome {
        outcome: outcome.into(),
        detail: Some(detail.into()),
    }
}

fn integration_result(code: &str, result: Result<IntegrationOutcome>) -> ResourceOutcome {
    match result {
        Ok(outcome) if outcome.project_code == code => match outcome.detail {
            Some(detail) => resource_with_detail(outcome.outcome, detail),
            None => resource(outcome.outcome),
        },
        Ok(_) => resource("malformed_result"),
        Err(error) => resource_with_detail("request_failed", format!("{error:#}")),
    }
}

fn surface_outcome(status: store::project_surfaces::SurfaceStatus) -> ResourceOutcome {
    let outcome = match status {
        store::project_surfaces::SurfaceStatus::Created => "created",
        store::project_surfaces::SurfaceStatus::Present => "present",
        store::project_surfaces::SurfaceStatus::Skipped => "skipped",
    };
    resource(outcome)
}

fn surface_results(
    result: Result<store::project_surfaces::ProjectSurfaces>,
) -> [(String, ResourceOutcome); 2] {
    match result {
        Ok(surfaces) => [
            ("drive".to_string(), surface_outcome(surfaces.drive_status)),
            (
                "repository".to_string(),
                surface_outcome(surfaces.repository_status),
            ),
        ],
        Err(error) => {
            let detail = format!("{error:#}");
            [
                (
                    "drive".to_string(),
                    resource_with_detail("request_failed", detail.clone()),
                ),
                (
                    "repository".to_string(),
                    resource_with_detail("request_failed", detail),
                ),
            ]
        }
    }
}

fn succeeded(outcome: &ResourceOutcome) -> bool {
    matches!(
        outcome.outcome.as_str(),
        "created" | "present" | "adopted" | "unchanged" | "repaired" | "notified"
    )
}

fn print_report(report: &SetupReport, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string(report).expect("setup report is serializable")
        );
        return;
    }
    for project in &report.results {
        for (name, outcome) in &project.resources {
            match &outcome.detail {
                Some(detail) => println!(
                    "{} {name}: {} ({detail})",
                    project.project_code, outcome.outcome
                ),
                None => println!("{} {name}: {}", project.project_code, outcome.outcome),
            }
        }
    }
}

async fn setup_one(base: &str, token: &str, project: &VisibleProject) -> ProjectSetupOutcome {
    let mut resources = BTreeMap::new();
    resources.extend(surface_results(
        surfaces::post_reconcile(base, token, project.id).await,
    ));
    resources.insert(
        "slack".to_string(),
        integration_result(
            &project.code,
            remote::post_integration_door(
                base,
                token,
                "/app/api/integrations/slack/ensure",
                serde_json::json!({ "project_code": project.code }),
            )
            .await
            .and_then(|report| one_integration_result(&report.results)),
        ),
    );
    resources.insert(
        "notion".to_string(),
        integration_result(
            &project.code,
            remote::post_integration_door(
                base,
                token,
                "/app/api/integrations/notion/ensure",
                serde_json::json!({ "project_code": project.code }),
            )
            .await
            .and_then(|report| one_integration_result(&report.results)),
        ),
    );
    ProjectSetupOutcome {
        project_code: project.code.clone(),
        resources,
    }
}

fn one_integration_result(results: &[IntegrationOutcome]) -> Result<IntegrationOutcome> {
    match results {
        [result] => Ok(result.clone()),
        _ => Err(anyhow!(
            "integration door returned an unexpected result count"
        )),
    }
}

/// Resolve `--host` and the Project code against `navigator.yaml` in `dir`
/// before touching the network. A repository root already names both, the
/// same way `project gate` reads them, so a bare `navigator project setup`
/// run there needs neither flag. An explicit `--host` that disagrees with the
/// manifest is refused rather than guessed at — the two can only name
/// different deployments by mistake.
fn resolve_from_manifest(
    dir: &Path,
    host: Option<&str>,
    project_code: Option<&str>,
    all: bool,
) -> Result<(Option<String>, Option<String>)> {
    let found = manifest::read(dir);
    let manifest_host = found
        .as_ref()
        .and_then(|manifest| manifest::non_empty(manifest.host.as_deref()));
    let manifest_project = found
        .as_ref()
        .and_then(|manifest| manifest::non_empty(manifest.project.as_deref()));

    if let (Some(flag), Some(declared)) = (host, manifest_host.as_deref()) {
        if flag != declared {
            return Err(anyhow!(
                "--host {flag} disagrees with navigator.yaml's project.host {declared}; refusing to guess which deployment you mean"
            ));
        }
    }

    let resolved_host = host.map(str::to_string).or(manifest_host);
    let resolved_project = if all {
        project_code.map(str::to_string)
    } else {
        project_code.map(str::to_string).or(manifest_project)
    };
    Ok((resolved_host, resolved_project))
}

/// Run setup for one visible Project or every visible Project admitted by the
/// authenticated list. Each resource is attempted even if an earlier one
/// fails, so a later invocation can retry only the incomplete provider work.
pub async fn run(
    host: Option<&str>,
    project_code: Option<&str>,
    all: bool,
    json: bool,
) -> ExitCode {
    let (host, project_code) = match resolve_from_manifest(Path::new("."), host, project_code, all)
    {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };
    let host = host.as_deref();
    let project_code = project_code.as_deref();
    let (base, token) = match remote::resolve(host) {
        Ok(pair) => pair,
        Err(error) => {
            eprintln!("navigator: {error:#}");
            return ExitCode::from(2);
        }
    };
    let projects = if all {
        match surfaces::list_visible_projects(&base, &token).await {
            Ok(projects) => projects,
            Err(error) => {
                eprintln!("navigator: {error:#}");
                return ExitCode::from(2);
            }
        }
    } else {
        let Some(code) = project_code else {
            eprintln!("navigator: provide a Project code or --all");
            return ExitCode::from(2);
        };
        if !store::projects::is_valid_code(code) {
            eprintln!("navigator: invalid project code");
            return ExitCode::from(2);
        }
        match surfaces::resolve_project_id(&base, &token, code).await {
            Ok(id) => vec![VisibleProject {
                id,
                code: code.to_string(),
            }],
            Err(error) => {
                eprintln!("navigator: {error:#}");
                return ExitCode::from(2);
            }
        }
    };
    let mut report = SetupReport {
        results: Vec::new(),
    };
    for project in &projects {
        report.results.push(setup_one(&base, &token, project).await);
    }
    let success = report
        .results
        .iter()
        .flat_map(|project| project.resources.values())
        .all(succeeded);
    print_report(&report, json);
    if success {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_from_manifest, resource, succeeded, surface_results};
    use store::project_surfaces::{ProjectSurfaces, SurfaceStatus};

    fn manifest_dir(contents: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("navigator.yaml"), contents).unwrap();
        dir
    }

    #[test]
    fn no_flags_resolves_host_and_code_from_the_manifest() {
        let dir = manifest_dir("project:\n  host: www.neonlaw.com\n  name: acme\n");
        let (host, code) = resolve_from_manifest(dir.path(), None, None, false).unwrap();
        assert_eq!(host.as_deref(), Some("www.neonlaw.com"));
        assert_eq!(code.as_deref(), Some("acme"));
    }

    #[test]
    fn an_explicit_flag_overrides_a_manifest_with_no_conflict() {
        let dir = manifest_dir("project:\n  host: www.neonlaw.com\n  name: acme\n");
        let (host, code) =
            resolve_from_manifest(dir.path(), None, Some("other-code"), false).unwrap();
        assert_eq!(host.as_deref(), Some("www.neonlaw.com"));
        assert_eq!(code.as_deref(), Some("other-code"));
    }

    #[test]
    fn a_conflicting_host_flag_is_refused_rather_than_guessed() {
        let dir = manifest_dir("project:\n  host: www.neonlaw.com\n  name: acme\n");
        let error = resolve_from_manifest(dir.path(), Some("staging.neonlaw.com"), None, false)
            .unwrap_err();
        assert!(error.to_string().contains("disagrees"));
    }

    #[test]
    fn all_ignores_the_manifest_project_code() {
        let dir = manifest_dir("project:\n  host: www.neonlaw.com\n  name: acme\n");
        let (host, code) = resolve_from_manifest(dir.path(), None, None, true).unwrap();
        assert_eq!(host.as_deref(), Some("www.neonlaw.com"));
        assert_eq!(code, None);
    }

    #[test]
    fn no_manifest_resolves_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (host, code) = resolve_from_manifest(dir.path(), None, None, false).unwrap();
        assert_eq!(host, None);
        assert_eq!(code, None);
    }

    #[test]
    fn skipped_existing_surfaces_are_required_setup_failures() {
        let outcomes = surface_results(Ok(ProjectSurfaces {
            code: "acme".to_string(),
            documents_prefix: "projects/acme/documents".to_string(),
            drive_folder_id: None,
            drive_status: SurfaceStatus::Skipped,
            repository_url: Some("https://forge.example/acme".to_string()),
            repository_status: SurfaceStatus::Present,
        }));
        assert_eq!(outcomes[0].1.outcome, "skipped");
        assert_eq!(outcomes[1].1.outcome, "present");
        assert!(!succeeded(&outcomes[0].1));
        assert!(succeeded(&outcomes[1].1));
    }

    #[test]
    fn unknown_provider_outcomes_fail_closed() {
        assert!(!succeeded(&resource("provider_unknown")));
    }
}
