//! `navigator projects doctor` — read-only Project workspace diagnostics.
//!
//! Verify a machine and a Project workspace *before* Navigator is allowed to
//! create anything. Every check here reads: no folder is created, no file is
//! written, no repository is provisioned, and no network call is made. The
//! command exists so that provisioning (`projects sync --apply`) can assume a
//! verified machine instead of discovering a misconfiguration halfway through
//! a partially created Drive tree.
//!
//! The diagnosis is a pure function of four inputs — an environment lookup, a
//! filesystem-existence probe, the stored credentials, and a clock — so the
//! production and staging configurations are both testable without
//! mutating process-global environment variables (which races under parallel
//! tests) and without touching a real Drive.
//!
//! A Workspace, Drive, folder, or identity *mismatch* is a hard failure: the
//! command exits nonzero rather than reporting a warning an operator might
//! scroll past. Absent-but-optional configuration is a warning.

use std::path::Path;
use std::process::ExitCode;

use cloud::workspace::{
    DriveCoordinates, WorkspaceConfig, WorkspaceConfigError, NAVIGATOR_GITHUB_ORG,
    NAVIGATOR_GIT_HOST, NAVIGATOR_PROJECTS_DRIVE_MOUNT,
};

use crate::credentials::{self, Credentials};
use crate::palette;

/// The outcome of one check.
///
/// `Warn` is reserved for configuration that is genuinely optional — a Drive
/// mount an operator has not set up, or a site they have not logged into.
/// Anything that would make provisioning write to the wrong Workspace is
/// `Fail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

/// One named diagnostic and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Owned rather than `&'static str` because a check's name may carry the
    /// Project code it is about.
    pub name: String,
    pub status: Status,
    pub detail: String,
}

impl Check {
    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Ok,
            detail: detail.into(),
        }
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Warn,
            detail: detail.into(),
        }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: Status::Fail,
            detail: detail.into(),
        }
    }
}

/// Every check the doctor ran, in report order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub checks: Vec<Check>,
}

impl Diagnosis {
    /// True when nothing failed. Warnings do not make a machine unhealthy.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !self.checks.iter().any(|c| c.status == Status::Fail)
    }

    /// Look one check up by name — the accessor the tests assert through.
    /// The report itself prints every check in order and never looks one up.
    #[cfg(test)]
    #[must_use]
    pub fn check(&self, name: &str) -> Option<&Check> {
        self.checks.iter().find(|c| c.name == name)
    }
}

/// The read-only inputs a diagnosis is computed from.
///
/// Bundled into one struct so the pure [`diagnose`] entry point keeps a short
/// signature as checks are added, and so a caller cannot accidentally pass a
/// live environment to one input and a fixture to another.
pub struct Probe<'a> {
    /// Environment lookup — `NAVIGATOR_GCP_PROJECT_ID`, the Drive coordinates,
    /// and `NAVIGATOR_GITHUB_ORG`.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Whether a path exists on this machine. Never reads file contents.
    pub path_exists: &'a dyn Fn(&Path) -> bool,
    /// The stored `site login` bearer tokens.
    pub credentials: &'a Credentials,
    /// Unix epoch seconds, for token-expiry math.
    pub now: i64,
    /// The site whose login to check. `None` selects the sole stored host.
    pub host: Option<&'a str>,
    /// The Project code to resolve the folder path and portal mount for.
    /// `None` reports deployment-wide configuration only.
    pub project_code: Option<&'a str>,
}

/// Compute the diagnosis. Pure: reads nothing but the supplied [`Probe`].
#[must_use]
pub fn diagnose(probe: &Probe<'_>) -> Diagnosis {
    let mut checks = Vec::new();

    // Everything downstream is scoped by the deployment, so an unresolved
    // deployment stops the report rather than letting later checks silently
    // describe some other Workspace.
    let workspace = match WorkspaceConfig::from_lookup(|k| (probe.env)(k)) {
        Ok(workspace) => {
            checks.push(Check::ok(
                "deployment",
                format!(
                    "{:?} serving {:?} Projects",
                    workspace.deployment, workspace.customer
                ),
            ));
            workspace
        }
        Err(e) => {
            checks.push(Check::fail("deployment", deployment_error_detail(&e)));
            return Diagnosis { checks };
        }
    };

    checks.push(Check::ok(
        "google workspace",
        format!(
            "{:?}, expecting the Projects root to be named {:?}",
            workspace.google_workspace, workspace.expected_projects_root_name
        ),
    ));

    let drive = match workspace.drive_coordinates(|k| (probe.env)(k)) {
        Ok(drive) => {
            checks.push(Check::ok(
                "shared drive",
                format!("id {}", drive.shared_drive_id),
            ));
            checks.push(Check::ok(
                "projects root",
                format!(
                    "folder id {} ({})",
                    drive.projects_root_folder_id, drive.expected_projects_root_name
                ),
            ));
            Some(drive)
        }
        Err(WorkspaceConfigError::MissingCoordinate(key)) => {
            // Fail closed. Falling back to another deployment's root is how a
            // staging run writes into the production Drive.
            checks.push(Check::fail(
                "shared drive",
                format!("{key} is unset; this deployment has no Drive coordinates and must not borrow another's"),
            ));
            None
        }
        Err(e) => {
            checks.push(Check::fail("shared drive", e.to_string()));
            None
        }
    };

    checks.push(drive_mount_check(drive.as_ref(), probe));
    checks.push(site_login_check(probe));

    if let Some(code) = probe.project_code {
        checks.extend(project_checks(drive.as_ref(), code, probe));
    }

    Diagnosis { checks }
}

/// Spell the deployment-resolution failures as operator instructions rather
/// than as the bare `Display` of the error.
fn deployment_error_detail(error: &WorkspaceConfigError) -> String {
    match error {
        WorkspaceConfigError::MissingDeployment => format!(
            "{} is unset — set it to the GCP project of the deployment you are operating",
            cloud::workspace::NAVIGATOR_GCP_PROJECT_ID
        ),
        WorkspaceConfigError::UnknownDeployment { project_id } => format!(
            "{project_id:?} is not a Project workspace deployment; expected one of \
             neon-law, neon-law-stg"
        ),
        WorkspaceConfigError::MissingCoordinate(key) if *key == NAVIGATOR_GITHUB_ORG => {
            format!(
                "{key} is unset; a named deployment must configure the organization its own \
                 automation occupies, and there is no default"
            )
        }
        WorkspaceConfigError::MissingCoordinate(key) if *key == NAVIGATOR_GIT_HOST => {
            format!(
                "{key} is set to a blank value; unset it to take the default host, or name the \
                 host this deployment's organization lives on"
            )
        }
        other @ WorkspaceConfigError::MissingCoordinate(_) => other.to_string(),
    }
}

/// The optional workstation Drive mount. Unset is a warning — plenty of
/// operators never mount Drive. Set-but-absent is a failure, because a sync
/// pointed at a path that does not exist would create the tree in the wrong
/// place.
fn drive_mount_check(drive: Option<&DriveCoordinates>, probe: &Probe<'_>) -> Check {
    let Some(mount) = drive.and_then(|d| d.local_mount.clone()) else {
        return Check::warn(
            "drive mount",
            format!("{NAVIGATOR_PROJECTS_DRIVE_MOUNT} is unset; no local Drive mount to verify"),
        );
    };
    if (probe.path_exists)(Path::new(&mount)) {
        Check::ok("drive mount", mount)
    } else {
        Check::fail(
            "drive mount",
            format!("{NAVIGATOR_PROJECTS_DRIVE_MOUNT} points at {mount}, which does not exist"),
        )
    }
}

/// Whether this machine holds a usable bearer token for the site. A missing
/// login is a warning (the doctor still reports every coordinate); an expired
/// one is a failure, because a command that assumed it would work will not.
fn site_login_check(probe: &Probe<'_>) -> Check {
    let base = match probe.host {
        Some(host) => Some(credentials::base_url(host)),
        None => probe.credentials.sole_host().map(str::to_owned),
    };
    let Some(base) = base else {
        return Check::warn(
            "site login",
            "not logged in to any site — run `navigator site login --host …`",
        );
    };
    match probe.credentials.get(&base) {
        None => Check::warn(
            "site login",
            format!("no stored token for {base} — run `navigator site login --host {base}`"),
        ),
        Some(cred) if cred.is_expired(probe.now) => Check::fail(
            "site login",
            format!("the stored token for {base} has expired — run `navigator site login --host {base}`"),
        ),
        Some(cred) => Check::ok(
            "site login",
            format!(
                "{} as {}, {} left",
                base,
                cred.person_email.as_deref().unwrap_or("unknown identity"),
                credentials::humanize_remaining(cred.seconds_remaining(probe.now)),
            ),
        ),
    }
}

/// The per-Project coordinates: where the folder belongs and where Navigator
/// serves its portal.
fn project_checks(drive: Option<&DriveCoordinates>, code: &str, probe: &Probe<'_>) -> Vec<Check> {
    let mut checks = Vec::new();

    match drive {
        Some(drive) => {
            checks.push(Check::ok("project folder", drive.project_path(code)));
            // Only meaningful with a mount configured; `drive mount` already
            // reported an absent one.
            if let Some(local) = drive.local_project_path(code) {
                if (probe.path_exists)(Path::new(&local)) {
                    checks.push(Check::ok("project folder (local)", local));
                } else {
                    checks.push(Check::warn(
                        "project folder (local)",
                        format!("{local} does not exist yet — `projects sync` would create it"),
                    ));
                }
            }
        }
        None => checks.push(Check::fail(
            "project folder",
            format!("cannot place {code}: this deployment's Drive coordinates are unresolved"),
        )),
    }

    // A Project's source repository is a whole URL stored on the Project, not
    // a coordinate composed from deployment configuration, so there is nothing
    // for a configuration doctor to derive or check here. `projects
    // repository validate` inspects an actual checkout instead.
    checks.push(Check::ok(
        "portal mount",
        WorkspaceConfig::portal_mount(code),
    ));

    checks
}

/// Print one diagnosis and return the process exit code.
fn report(diagnosis: &Diagnosis) -> ExitCode {
    for check in &diagnosis.checks {
        let marker = match check.status {
            Status::Ok => palette::highlight("ok  "),
            Status::Warn => palette::dim("warn"),
            Status::Fail => palette::highlight("FAIL"),
        };
        println!("{marker}  {:<24}  {}", check.name, check.detail);
    }

    if diagnosis.is_healthy() {
        println!();
        println!(
            "{}",
            palette::dim("workspace verified; nothing was modified")
        );
        ExitCode::SUCCESS
    } else {
        println!();
        println!(
            "{}",
            palette::highlight(
                "workspace NOT verified — fix the failures above before provisioning"
            ),
        );
        ExitCode::FAILURE
    }
}

/// `navigator projects doctor [--host h] [--project code]`.
///
/// Reads the live environment, the real filesystem, and the stored
/// credentials. Writes nothing, and makes no network or database call: the
/// diagnosis is now a pure function of its [`Probe`] all the way out to the
/// command, because there is no registry left to read.
pub fn run(host: Option<&str>, project_code: Option<&str>) -> ExitCode {
    let creds = match credentials::load(&credentials::default_credentials_path()) {
        Ok(creds) => creds,
        Err(e) => {
            eprintln!("could not read the credential store: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    let diagnosis = diagnose(&Probe {
        env: &|key| std::env::var(key).ok(),
        path_exists: &|path: &Path| path.exists(),
        credentials: &creds,
        now: now_secs(),
        host,
        project_code,
    });
    report(&diagnosis)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::HostCredential;
    use std::collections::{HashMap, HashSet};

    const DRIVE_ID: &str = "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID";
    const GCP: &str = "NAVIGATOR_GCP_PROJECT_ID";

    /// The synthetic forge coordinate every fixture configures.
    ///
    /// The organization is *configuration*, so this module spells no real
    /// organization: a fixture that did would be a second vocabulary alongside
    /// the configured one.
    const AN_ORGANIZATION: &str = "an-organization";

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    /// A deployment's full fixture: its identity plus the organization a named
    /// deployment must carry.
    fn deployment(project_id: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![
            (GCP, project_id),
            (DRIVE_ID, "drive-neon"),
            (root_key(project_id), "root-folder"),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
        ]
    }

    fn existing(paths: &[&str]) -> impl Fn(&Path) -> bool {
        let set: HashSet<String> = paths.iter().map(|p| (*p).to_string()).collect();
        move |path: &Path| set.contains(path.to_string_lossy().as_ref())
    }

    fn nothing_exists(_: &Path) -> bool {
        false
    }

    fn logged_in(base: &str, expires_at: i64) -> Credentials {
        let mut creds = Credentials::default();
        creds.set(
            base,
            HostCredential {
                token: "tok".into(),
                person_email: Some("nick@neonlaw.com".into()),
                role: Some("admin".into()),
                expires_at,
            },
        );
        creds
    }

    /// The root-folder env key each deployment reads — deliberately spelled
    /// out here rather than derived, so a change to the mapping has to be
    /// made twice on purpose.
    fn root_key(project_id: &str) -> &'static str {
        match project_id {
            "neon-law" => "NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID",
            "neon-law-stg" => "NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID",
            other => unreachable!("unknown deployment {other}"),
        }
    }

    /// One repository coordinate per Project, in the configured organization.
    #[test]
    fn every_deployment_resolves_its_own_drive_and_one_repository_coordinate() {
        for (project_id, root_name) in [
            ("neon-law", "Projects"),
            ("neon-law-stg", "Staging Projects"),
        ] {
            let creds = logged_in("https://www.neonlaw.com", 10_000);
            let lookup = env(&deployment(project_id));
            let diagnosis = diagnose(&Probe {
                env: &lookup,
                path_exists: &nothing_exists,
                credentials: &creds,
                now: 0,
                host: None,
                project_code: Some("acme"),
            });

            assert!(
                diagnosis.is_healthy(),
                "{project_id} should be healthy: {:?}",
                diagnosis.checks
            );
            assert_eq!(
                diagnosis.check("project folder").unwrap().detail,
                format!("{root_name}/acme"),
                "{project_id}"
            );
            assert_eq!(
                diagnosis.check("portal mount").unwrap().detail,
                "/app/projects/acme/portal/",
                "{project_id}"
            );
            // A Project's repository is a URL stored on the matter, so a
            // configuration doctor has nothing to derive and must not report
            // one. It holds no database connection to read the real value.
            assert!(
                diagnosis.check("project repository").is_none(),
                "{project_id} must report no derived repository coordinate"
            );
        }
    }

    /// A named deployment missing its organization fails closed.
    ///
    /// The organization is this deployment's own, and there is no default: a
    /// value that silently appeared would let a staging run describe production.
    /// Its paired host is read here too, to the different rule the pair's two
    /// halves carry — see
    /// [`a_named_deployment_whose_host_is_blank_fails_and_names_the_key`].
    #[test]
    fn a_named_deployment_without_its_organization_fails_and_names_the_key() {
        let creds = Credentials::default();
        let missing = NAVIGATOR_GITHUB_ORG;
        let mut pairs = deployment("neon-law");
        pairs.retain(|(key, _)| *key != missing);
        let lookup = env(&pairs);
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: Some("acme"),
        });

        assert!(!diagnosis.is_healthy(), "{missing} must fail the report");
        let detail = &diagnosis.check("deployment").unwrap().detail;
        assert!(detail.contains(missing), "{detail}");
        assert!(
            detail.contains("no default"),
            "the report must say there is no fallback: {detail}"
        );
        // And nothing may be reported for a deployment that did not resolve.
        assert!(diagnosis.check("project folder").is_none());
    }

    /// A named deployment whose host is blank fails closed naming that key.
    ///
    /// The host is now half of this deployment's forge coordinate, so the
    /// diagnosis reads it. It carries a default, which is why *absence* stays
    /// healthy — every fixture in this module omits it — and why a
    /// templated-and-never-filled value is the failure worth reporting.
    #[test]
    fn a_named_deployment_whose_host_is_blank_fails_and_names_the_key() {
        let creds = Credentials::default();
        let mut pairs = deployment("neon-law");
        pairs.push((NAVIGATOR_GIT_HOST, "   "));
        let lookup = env(&pairs);
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: Some("acme"),
        });

        assert!(
            !diagnosis.is_healthy(),
            "{NAVIGATOR_GIT_HOST} must fail the report"
        );
        let detail = &diagnosis.check("deployment").unwrap().detail;
        assert!(detail.contains(NAVIGATOR_GIT_HOST), "{detail}");
        assert!(
            detail.contains("blank"),
            "the report must say what is wrong with the value: {detail}"
        );
        assert!(diagnosis.check("project folder").is_none());
    }

    #[test]
    fn a_missing_deployment_fails_and_stops_the_report() {
        let creds = Credentials::default();
        let diagnosis = diagnose(&Probe {
            env: &env(&[]),
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: Some("acme"),
        });

        assert!(!diagnosis.is_healthy());
        assert_eq!(diagnosis.checks.len(), 1, "{:?}", diagnosis.checks);
        assert_eq!(diagnosis.checks[0].name, "deployment");
        assert!(
            diagnosis.checks[0].detail.contains(GCP),
            "{}",
            diagnosis.checks[0].detail
        );
        // No Drive or repository coordinate may be reported for an
        // unresolved deployment.
        assert!(diagnosis.check("project repository").is_none());
        assert!(diagnosis.check("project folder").is_none());
    }

    #[test]
    fn an_unknown_deployment_names_the_known_ones_instead_of_guessing() {
        let creds = Credentials::default();
        let diagnosis = diagnose(&Probe {
            env: &env(&[(GCP, "some-other-project")]),
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: None,
        });

        assert!(!diagnosis.is_healthy());
        let detail = &diagnosis.check("deployment").unwrap().detail;
        assert!(detail.contains("some-other-project"), "{detail}");
        assert!(detail.contains("neon-law-stg"), "{detail}");
    }

    #[test]
    fn staging_does_not_borrow_productions_root_folder() {
        let creds = Credentials::default();
        // Production's root is present; staging's is not. Staging must fail
        // rather than silently write into the production Drive.
        let lookup = env(&[
            (GCP, "neon-law-stg"),
            (DRIVE_ID, "drive-neon"),
            (
                "NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID",
                "root-production",
            ),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
        ]);
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: Some("acme"),
        });

        assert!(!diagnosis.is_healthy());
        let drive = diagnosis.check("shared drive").unwrap();
        assert_eq!(drive.status, Status::Fail);
        assert!(
            drive
                .detail
                .contains("NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID"),
            "{}",
            drive.detail
        );
        // And the Project folder cannot be placed without them.
        assert_eq!(
            diagnosis.check("project folder").unwrap().status,
            Status::Fail
        );
    }

    #[test]
    fn a_configured_drive_mount_that_does_not_exist_is_a_hard_failure() {
        let creds = Credentials::default();
        let mut pairs = deployment("neon-law");
        pairs.push((NAVIGATOR_PROJECTS_DRIVE_MOUNT, "/Volumes/Gone"));
        let lookup = env(&pairs);
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: None,
        });

        assert!(!diagnosis.is_healthy());
        let mount = diagnosis.check("drive mount").unwrap();
        assert_eq!(mount.status, Status::Fail);
        assert!(mount.detail.contains("/Volumes/Gone"), "{}", mount.detail);
    }

    #[test]
    fn an_unset_drive_mount_is_only_a_warning() {
        let creds = Credentials::default();
        let lookup = env(&deployment("neon-law"));
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: None,
        });

        assert!(diagnosis.is_healthy(), "{:?}", diagnosis.checks);
        assert_eq!(diagnosis.check("drive mount").unwrap().status, Status::Warn);
        // Not logged in is likewise only a warning.
        assert_eq!(diagnosis.check("site login").unwrap().status, Status::Warn);
    }

    #[test]
    fn a_mounted_project_folder_is_reported_present_or_yet_to_be_created() {
        let mut pairs = deployment("neon-law");
        pairs.push((NAVIGATOR_PROJECTS_DRIVE_MOUNT, "/Volumes/Drive"));
        let lookup = env(&pairs);
        let creds = Credentials::default();

        // The mount exists but this Project's folder does not: a warning,
        // because `projects sync` is exactly what would create it.
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &existing(&["/Volumes/Drive"]),
            credentials: &creds,
            now: 0,
            host: None,
            project_code: Some("acme"),
        });
        assert!(diagnosis.is_healthy(), "{:?}", diagnosis.checks);
        assert_eq!(
            diagnosis.check("project folder (local)").unwrap().status,
            Status::Warn
        );

        // Both present: clean.
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &existing(&["/Volumes/Drive", "/Volumes/Drive/Projects/acme"]),
            credentials: &creds,
            now: 0,
            host: None,
            project_code: Some("acme"),
        });
        let local = diagnosis.check("project folder (local)").unwrap();
        assert_eq!(local.status, Status::Ok);
        assert_eq!(local.detail, "/Volumes/Drive/Projects/acme");
    }

    #[test]
    fn an_expired_token_fails_while_a_live_one_reports_its_identity() {
        let lookup = env(&deployment("neon-law"));

        let expired = logged_in("https://www.neonlaw.com", 100);
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &expired,
            now: 1_000,
            host: None,
            project_code: None,
        });
        assert!(!diagnosis.is_healthy());
        assert_eq!(diagnosis.check("site login").unwrap().status, Status::Fail);

        let live = logged_in("https://www.neonlaw.com", 10_000);
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &live,
            now: 1_000,
            host: None,
            project_code: None,
        });
        assert!(diagnosis.is_healthy(), "{:?}", diagnosis.checks);
        let login = diagnosis.check("site login").unwrap();
        assert!(
            login.detail.contains("nick@neonlaw.com"),
            "{}",
            login.detail
        );
    }

    #[test]
    fn an_explicit_host_selects_that_login_rather_than_the_sole_one() {
        let lookup = env(&deployment("neon-law"));
        let creds = logged_in("https://www.neonlaw.com", 10_000);

        // Asking about a host with no stored token warns, even though
        // exactly one other host is logged in.
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: Some("staging.example"),
            project_code: None,
        });
        let login = diagnosis.check("site login").unwrap();
        assert_eq!(login.status, Status::Warn);
        assert!(
            login.detail.contains("https://staging.example"),
            "{}",
            login.detail
        );
    }

    #[test]
    fn without_a_project_code_no_per_project_check_is_reported() {
        let creds = Credentials::default();
        let lookup = env(&deployment("neon-law"));
        let diagnosis = diagnose(&Probe {
            env: &lookup,
            path_exists: &nothing_exists,
            credentials: &creds,
            now: 0,
            host: None,
            project_code: None,
        });
        assert!(diagnosis.check("project folder").is_none());
        assert!(diagnosis.check("project repository").is_none());
        assert!(diagnosis.check("portal mount").is_none());
    }
}
