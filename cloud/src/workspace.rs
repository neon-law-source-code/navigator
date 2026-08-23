//! Deployment-owned Project workspace coordinates.
//!
//! A Project's Drive folder and its one source repository are determined by the
//! deployment serving the Project. The Drive root is a property of the
//! deployment; the forge organization and host are configuration this module
//! reads. This module contains no provider client: it is a pure, fail-closed
//! resolver that later provisioning and diagnostic commands can share.
//!
//! # One `(host, organization)` pair per deployment
//!
//! [`NAVIGATOR_GIT_HOST`] and [`NAVIGATOR_GITHUB_ORG`] are one coordinate in
//! two keys, resolved together on [`WorkspaceConfig`]. That pair is where a
//! deployment's Project repositories are created, and it is the boundary `ops
//! github setup` refuses a governance write outside. Only the host carries a
//! default, and only because it has a right answer:
//! [`DEFAULT_GIT_HOST`].
//!
//! # One repository per Project code
//!
//! A Project has exactly one repository, named for its Project code, holding
//! that Project's notation templates under `templates/` and its client portal
//! under `portal/`. The repository name *is* the code, so there is nothing to
//! compose, nothing to parse, and nothing to reconcile by equality.

use thiserror::Error;

/// Environment key naming the GCP project that owns the active deployment.
pub const NAVIGATOR_GCP_PROJECT_ID: &str = "NAVIGATOR_GCP_PROJECT_ID";
/// Optional path where a workstation has mounted this deployment's Drive.
pub const NAVIGATOR_PROJECTS_DRIVE_MOUNT: &str = "NAVIGATOR_PROJECTS_DRIVE_MOUNT";
/// Environment key naming the organization that holds this deployment's
/// Project repositories.
///
/// One organization per deployment, read from configuration. The organization
/// identifies nothing in this codebase — it only says which forge namespace a
/// deployment's Project repositories live in — so it belongs in configuration
/// and nowhere in source.
pub const NAVIGATOR_GITHUB_ORG: &str = "NAVIGATOR_GITHUB_ORG";
/// Environment key naming the forge host this deployment's repositories live
/// on.
///
/// This is the other half of a **coordinate**, paired with
/// [`NAVIGATOR_GITHUB_ORG`]: one deployment is coupled to one `(host,
/// organization)` pair, and that pair is where its Project repositories are
/// created. `ops github setup` also reads it as the authorization boundary on
/// governance writes, but the boundary is now the whole pair rather than the
/// host alone — on a public forge, *every repository on this host* and *every
/// repository the Firm owns* stopped being the same set.
///
/// A Project's source repository stays a whole URL stored on the Project
/// (`store::projects::Project::repository_url`) and may live on any forge, so
/// nothing composes this host into a clone URL for an already-recorded Project.
/// The pair supplies the default *target at creation time*.
///
/// Unset resolves to [`DEFAULT_GIT_HOST`], which is where Navigator itself and
/// every deployment the Firm operates live. Present-but-blank does **not**: a
/// configuration that was templated and never filled in fails closed naming
/// this key, exactly as [`NAVIGATOR_GITHUB_ORG`] does. The asymmetry is the
/// point — a host has one right answer for almost every deployment, and an
/// organization has none.
pub const NAVIGATOR_GIT_HOST: &str = "NAVIGATOR_GIT_HOST";

/// The host [`NAVIGATOR_GIT_HOST`] resolves to when a deployment names none.
///
/// Navigator's own public URL is not configuration: it is always
/// `github.com/neon-law-source-code/navigator`. A fresh clone, a laptop, or a CI
/// job that sourced no deployment config therefore has a host without being
/// told one.
pub const DEFAULT_GIT_HOST: &str = "github.com";

/// The customer whose Projects this deployment serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCustomer {
    NeonLaw,
    NeonLawFoundation,
}

/// The Google Workspace that owns the selected Shared Drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleWorkspace {
    NeonLaw,
}

/// The three persistent Navigator deployments that own Project workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentWorkspace {
    NeonLawProduction,
    NeonLawStaging,
    NeonLawFoundation,
}

/// The maximum length of a Project code.
///
/// `store::projects::PROJECT_CODE_MAX_LEN` is this constant: the two shapes
/// are one shape, defined once. A repository name is the code alone, so the
/// forge's own 100-character cap cannot be reached by a valid code.
pub const SLUG_MAX_LEN: usize = 80;

/// The one route segment a Project's portal is mounted under.
///
/// A literal, not a name: every Project has exactly one client portal, served
/// at `/app/projects/{code}/portal`. The extra segment is what keeps
/// `/app/projects/{id}` rendering Navigator's own matter page.
pub const PORTAL_MOUNT_SEGMENT: &str = "portal";

/// Project codes Navigator routes on its own and may therefore not accept.
///
/// [`is_valid_slug`] would happily accept `new` — it is well-formed
/// kebab-case. The refusal is about *where the code is mounted*, not about its
/// shape: `/app/projects/new` is the matter-open form, so a Project whose code
/// is `new` would collide with it.
///
/// This guards the segment a Project code actually occupies. It replaces a
/// reserved *application-name* list that guarded the wrong depth entirely —
/// those eight names were first- and second-segment routes (`/app/admin`,
/// `/app/api`, `/auth/*`), which a fourth-segment name could never shadow.
/// There is no application name left to reserve, because the mount segment is
/// the literal [`PORTAL_MOUNT_SEGMENT`].
pub const RESERVED_PROJECT_CODES: &[&str] = &["new"];

/// Whether a value is safe as a URL segment, a repository name, and a folder
/// name in the firm's shared drive.
///
/// Lowercase letters, digits, and single hyphens; alphanumeric at both ends.
/// This is the single definition of that shape. `store::projects::is_valid_code`
/// calls it and additionally refuses [`RESERVED_PROJECT_CODES`], so a Project
/// code and its repository name are enforced identical rather than documented
/// as identical.
///
/// Two restrictions are deliberate rather than incidental, because a matter's
/// code *is* its shared-drive folder name (#938) and the mapping is an equality
/// check, not a normalization:
///
/// - **Lowercase only.** Google Drive and macOS are case-insensitive, so
///   allowing uppercase would let one folder answer to two distinct codes.
/// - **Hyphens, not underscores.** One separator. A second would force a
///   normalization step in both directions and the mapping would stop being an
///   equality check.
#[must_use]
pub fn is_valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= SLUG_MAX_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && value
            .bytes()
            .last()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && !value.contains("--")
}

/// The Drive coordinates selected from a deployment-owned workspace map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveCoordinates {
    pub google_workspace: GoogleWorkspace,
    pub shared_drive_id: String,
    pub projects_root_folder_id: String,
    pub expected_projects_root_name: &'static str,
    pub local_mount: Option<String>,
}

impl DriveCoordinates {
    /// The human-readable Drive path expected for one Project code.
    #[must_use]
    pub fn project_path(&self, project_code: &str) -> String {
        format!("{}/{}", self.expected_projects_root_name, project_code)
    }

    /// The optional workstation path expected for one Project code.
    #[must_use]
    pub fn local_project_path(&self, project_code: &str) -> Option<String> {
        self.local_mount.as_ref().map(|mount| {
            format!(
                "{}/{}",
                mount.trim_end_matches('/'),
                self.project_path(project_code)
            )
        })
    }
}

/// All non-secret Project workspace coordinates for one deployment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub deployment: DeploymentWorkspace,
    pub customer: WorkspaceCustomer,
    pub google_workspace: GoogleWorkspace,
    pub expected_projects_root_name: &'static str,
    /// The one organization this deployment's own automation lives in, read
    /// from [`NAVIGATOR_GITHUB_ORG`].
    ///
    /// Not a Project's source coordinate: a Project stores its repository as a
    /// whole URL on any forge (`store::projects::Project::repository_url`).
    pub organization: String,
    /// The forge host that organization lives on, read from
    /// [`NAVIGATOR_GIT_HOST`] and defaulting to [`DEFAULT_GIT_HOST`].
    ///
    /// Paired with [`Self::organization`]: together they are the one `(host,
    /// organization)` pair this deployment creates Project repositories in, and
    /// the boundary `ops github setup` refuses a governance write outside.
    pub host: String,
    shared_drive_id_key: &'static str,
    projects_root_folder_id_key: &'static str,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkspaceConfigError {
    #[error("{NAVIGATOR_GCP_PROJECT_ID} must select a known Project workspace deployment")]
    MissingDeployment,
    #[error("{NAVIGATOR_GCP_PROJECT_ID} {project_id:?} has no Project workspace configuration")]
    UnknownDeployment { project_id: String },
    #[error("missing Project workspace coordinate: {0}")]
    MissingCoordinate(&'static str),
}

/// The per-deployment facts that are properties of the deployment rather than
/// configuration: which customer it serves, which Google Workspace owns its
/// Drive, and which environment keys carry that Drive's identifiers.
struct DeploymentFacts {
    customer: WorkspaceCustomer,
    google_workspace: GoogleWorkspace,
    expected_projects_root_name: &'static str,
    shared_drive_id_key: &'static str,
    projects_root_folder_id_key: &'static str,
}

impl DeploymentWorkspace {
    /// Resolve which deployment is active without reading provider credentials
    /// or making a network call. Unknown deployments deliberately have no
    /// fallback: borrowing another deployment's Drive or repository
    /// organization would cross the workspace boundary.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, WorkspaceConfigError> {
        let project_id = get(NAVIGATOR_GCP_PROJECT_ID)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or(WorkspaceConfigError::MissingDeployment)?;
        match project_id.as_str() {
            "neon-law" => Ok(Self::NeonLawProduction),
            "neon-law-stg" => Ok(Self::NeonLawStaging),
            "neon-law-org" => Ok(Self::NeonLawFoundation),
            _ => Err(WorkspaceConfigError::UnknownDeployment { project_id }),
        }
    }

    pub fn from_env() -> Result<Self, WorkspaceConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    const fn facts(self) -> DeploymentFacts {
        match self {
            Self::NeonLawProduction => DeploymentFacts {
                customer: WorkspaceCustomer::NeonLaw,
                google_workspace: GoogleWorkspace::NeonLaw,
                expected_projects_root_name: "Projects",
                shared_drive_id_key: "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID",
                projects_root_folder_id_key:
                    "NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID",
            },
            Self::NeonLawStaging => DeploymentFacts {
                customer: WorkspaceCustomer::NeonLaw,
                google_workspace: GoogleWorkspace::NeonLaw,
                expected_projects_root_name: "Staging Projects",
                shared_drive_id_key: "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID",
                projects_root_folder_id_key:
                    "NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID",
            },
            Self::NeonLawFoundation => DeploymentFacts {
                customer: WorkspaceCustomer::NeonLawFoundation,
                google_workspace: GoogleWorkspace::NeonLaw,
                expected_projects_root_name: "NLF Projects",
                shared_drive_id_key: "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID",
                projects_root_folder_id_key: "NAVIGATOR_DRIVE_NEON_LAW_NLF_PROJECTS_ROOT_FOLDER_ID",
            },
        }
    }
}

/// Read one required, non-empty configured value.
fn required<F: Fn(&str) -> Option<String>>(
    get: &F,
    key: &'static str,
) -> Result<String, WorkspaceConfigError> {
    get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or(WorkspaceConfigError::MissingCoordinate(key))
}

/// Read one configured value that has a right answer when nobody names it, and
/// fail closed when somebody names it blank.
///
/// Absent and blank are the same thing for [`required`] and deliberately
/// different here. Absent means "this deployment did not have to say", and the
/// default is correct. Blank means the key was written down and left empty —
/// a templated configuration nobody filled in — and resolving that to the
/// default would hide the one case where the operator believed they had
/// configured something.
fn defaulted<F: Fn(&str) -> Option<String>>(
    get: &F,
    key: &'static str,
    default: &str,
) -> Result<String, WorkspaceConfigError> {
    match get(key) {
        None => Ok(default.to_string()),
        Some(value) if value.trim().is_empty() => Err(WorkspaceConfigError::MissingCoordinate(key)),
        Some(value) => Ok(value.trim().to_string()),
    }
}

impl WorkspaceConfig {
    /// Resolve the active deployment and its forge configuration.
    ///
    /// The two failures are deliberately different questions. **No deployment
    /// named** means this process is not operating a deployment at all — the
    /// local loop, the test suite — and every derived coordinate is legitimately
    /// absent. **A deployment named with no organization** is a misconfigured
    /// deployment, and it fails closed with no fallback.
    ///
    /// The two halves of the forge pair are read to different rules, and the
    /// difference is what each key means. [`NAVIGATOR_GITHUB_ORG`] has no right
    /// answer, so a named deployment must state it. [`NAVIGATOR_GIT_HOST`] has
    /// one — [`DEFAULT_GIT_HOST`] — so absence resolves rather than failing,
    /// while a blank value still fails closed naming the key.
    ///
    /// # Errors
    ///
    /// [`WorkspaceConfigError::MissingDeployment`] or
    /// [`WorkspaceConfigError::UnknownDeployment`] when
    /// [`NAVIGATOR_GCP_PROJECT_ID`] names no deployment, and
    /// [`WorkspaceConfigError::MissingCoordinate`] when it names one but
    /// [`NAVIGATOR_GITHUB_ORG`] is unset or [`NAVIGATOR_GIT_HOST`] is blank.
    pub fn from_lookup<F: Fn(&str) -> Option<String>>(
        get: F,
    ) -> Result<Self, WorkspaceConfigError> {
        let deployment = DeploymentWorkspace::from_lookup(&get)?;
        let facts = deployment.facts();
        Ok(Self {
            deployment,
            customer: facts.customer,
            google_workspace: facts.google_workspace,
            expected_projects_root_name: facts.expected_projects_root_name,
            organization: required(&get, NAVIGATOR_GITHUB_ORG)?,
            host: defaulted(&get, NAVIGATOR_GIT_HOST, DEFAULT_GIT_HOST)?,
            shared_drive_id_key: facts.shared_drive_id_key,
            projects_root_folder_id_key: facts.projects_root_folder_id_key,
        })
    }

    pub fn from_env() -> Result<Self, WorkspaceConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// The path Navigator serves one Project's client portal at.
    #[must_use]
    pub fn portal_mount(project_code: &str) -> String {
        format!("/app/projects/{project_code}/{PORTAL_MOUNT_SEGMENT}/")
    }

    pub fn drive_coordinates<F: Fn(&str) -> Option<String>>(
        &self,
        get: F,
    ) -> Result<DriveCoordinates, WorkspaceConfigError> {
        Ok(DriveCoordinates {
            google_workspace: self.google_workspace,
            shared_drive_id: required(&get, self.shared_drive_id_key)?,
            projects_root_folder_id: required(&get, self.projects_root_folder_id_key)?,
            expected_projects_root_name: self.expected_projects_root_name,
            local_mount: get(NAVIGATOR_PROJECTS_DRIVE_MOUNT)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    pub fn drive_coordinates_from_env(&self) -> Result<DriveCoordinates, WorkspaceConfigError> {
        self.drive_coordinates(|key| std::env::var(key).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_valid_slug, DeploymentWorkspace, GoogleWorkspace, WorkspaceConfig, WorkspaceConfigError,
        WorkspaceCustomer, DEFAULT_GIT_HOST, NAVIGATOR_GCP_PROJECT_ID, NAVIGATOR_GITHUB_ORG,
        NAVIGATOR_GIT_HOST, NAVIGATOR_PROJECTS_DRIVE_MOUNT, RESERVED_PROJECT_CODES, SLUG_MAX_LEN,
    };
    use std::collections::HashMap;

    /// A synthetic organization. Which organization a deployment's Project
    /// repositories live in is configuration, so no real organization name is
    /// a constant or a fixture value in this workspace.
    const AN_ORGANIZATION: &str = "an-organization";

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    /// The configured organization every deployment fixture supplies.
    fn forge(project_id: &'static str) -> Vec<(&'static str, &'static str)> {
        vec![
            (NAVIGATOR_GCP_PROJECT_ID, project_id),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
        ]
    }

    #[test]
    fn deployment_maps_drive_paths_without_network_io() {
        let cases = [
            (
                "neon-law",
                "drive-neon",
                "root-production",
                "Projects",
                WorkspaceCustomer::NeonLaw,
            ),
            (
                "neon-law-stg",
                "drive-neon",
                "root-staging",
                "Staging Projects",
                WorkspaceCustomer::NeonLaw,
            ),
            (
                "neon-law-org",
                "drive-neon",
                "root-nlf",
                "NLF Projects",
                WorkspaceCustomer::NeonLawFoundation,
            ),
        ];

        for (project_id, shared_drive_id, root_folder_id, root_name, customer) in cases {
            let workspace = WorkspaceConfig::from_lookup(lookup(&forge(project_id)))
                .expect("known deployment resolves");
            assert_eq!(workspace.customer, customer, "{project_id}");
            assert_eq!(
                workspace.google_workspace,
                GoogleWorkspace::NeonLaw,
                "{project_id}"
            );

            // The organization this deployment's own automation lives in.
            // Nothing here composes a Project's source coordinate: that is a
            // whole URL stored on the Project, on whatever forge hosts it.
            assert_eq!(workspace.organization, AN_ORGANIZATION, "{project_id}");

            let root_key = match project_id {
                "neon-law" => "NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID",
                "neon-law-stg" => "NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID",
                "neon-law-org" => "NAVIGATOR_DRIVE_NEON_LAW_NLF_PROJECTS_ROOT_FOLDER_ID",
                _ => unreachable!(),
            };
            let drive = workspace
                .drive_coordinates(lookup(&[
                    (
                        "NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID",
                        shared_drive_id,
                    ),
                    (root_key, root_folder_id),
                    (NAVIGATOR_PROJECTS_DRIVE_MOUNT, "/Volumes/Projects"),
                ]))
                .expect("selected deployment reads only its coordinates");
            assert_eq!(drive.shared_drive_id, shared_drive_id, "{project_id}");
            assert_eq!(
                drive.projects_root_folder_id, root_folder_id,
                "{project_id}"
            );
            assert_eq!(
                drive.project_path("matter-42"),
                format!("{root_name}/matter-42")
            );
            assert_eq!(
                drive.local_project_path("matter-42"),
                Some(format!("/Volumes/Projects/{root_name}/matter-42"))
            );
        }
    }

    #[test]
    fn deployment_workspace_does_not_fall_back_to_another_deployments_coordinates() {
        let staging = WorkspaceConfig::from_lookup(lookup(&forge("neon-law-stg"))).unwrap();
        let error = staging
            .drive_coordinates(lookup(&[
                ("NAVIGATOR_DRIVE_NEON_LAW_PROJECTS_DRIVE_ID", "drive-neon"),
                (
                    "NAVIGATOR_DRIVE_NEON_LAW_PRODUCTION_PROJECTS_ROOT_FOLDER_ID",
                    "wrong-root",
                ),
            ]))
            .expect_err("staging must not borrow production's root");
        assert_eq!(
            error,
            WorkspaceConfigError::MissingCoordinate(
                "NAVIGATOR_DRIVE_NEON_LAW_STAGING_PROJECTS_ROOT_FOLDER_ID"
            )
        );

        let error = WorkspaceConfig::from_lookup(lookup(&forge("other")))
            .expect_err("unknown deployment must not borrow a repository organization");
        assert_eq!(
            error,
            WorkspaceConfigError::UnknownDeployment {
                project_id: "other".into()
            }
        );
    }

    #[test]
    fn a_missing_deployment_is_not_assumed_to_be_production() {
        assert_eq!(
            DeploymentWorkspace::from_lookup(|_| None),
            Err(WorkspaceConfigError::MissingDeployment)
        );
    }

    /// The two absences are different questions, and the answers differ.
    ///
    /// No deployment named is not an error: the local loop and the test suite
    /// operate no deployment, so every derived coordinate is legitimately
    /// absent. A deployment named with no organization is a misconfigured
    /// deployment, and there is no permissive default to hide it.
    #[test]
    fn a_named_deployment_missing_its_organization_fails_closed() {
        assert_eq!(
            WorkspaceConfig::from_lookup(lookup(&[(NAVIGATOR_GCP_PROJECT_ID, "neon-law")]))
                .expect_err("a named deployment must carry an organization"),
            WorkspaceConfigError::MissingCoordinate(NAVIGATOR_GITHUB_ORG)
        );

        // Present-but-blank is the same as unset: a deployment whose
        // configuration was templated and never filled in must not resolve.
        assert_eq!(
            WorkspaceConfig::from_lookup(lookup(&[
                (NAVIGATOR_GCP_PROJECT_ID, "neon-law"),
                (NAVIGATOR_GITHUB_ORG, "   "),
            ]))
            .expect_err("a blank organization is not an organization"),
            WorkspaceConfigError::MissingCoordinate(NAVIGATOR_GITHUB_ORG)
        );

        // And no deployment at all stays the benign absence it is today.
        assert_eq!(
            WorkspaceConfig::from_lookup(|_| None).expect_err("no deployment named"),
            WorkspaceConfigError::MissingDeployment
        );
    }

    /// A named deployment whose host was templated and never filled in fails
    /// closed naming that key.
    ///
    /// The host carries a default, so this is the one way it can fail — and it
    /// has to be able to fail, or the pair is only half validated. Absence
    /// means "this deployment did not have to say"; blank means somebody
    /// believed they had said something.
    #[test]
    fn a_named_deployment_whose_host_is_blank_fails_closed_naming_the_key() {
        assert_eq!(
            WorkspaceConfig::from_lookup(lookup(&[
                (NAVIGATOR_GCP_PROJECT_ID, "neon-law"),
                (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
                (NAVIGATOR_GIT_HOST, "   "),
            ]))
            .expect_err("a blank host is not a host"),
            WorkspaceConfigError::MissingCoordinate(NAVIGATOR_GIT_HOST)
        );
    }

    /// The pair resolves together, and the host has a right answer when nobody
    /// names one.
    ///
    /// Every fixture in this module omits [`NAVIGATOR_GIT_HOST`] on purpose:
    /// that is the shape a fresh clone, a laptop, and a CI job that sourced no
    /// deployment config all have, and it must resolve rather than fail.
    #[test]
    fn the_forge_pair_resolves_together_and_the_host_defaults() {
        let configured = WorkspaceConfig::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law"),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            (NAVIGATOR_GIT_HOST, "forge.example"),
        ]))
        .expect("a fully configured pair resolves");
        assert_eq!(configured.organization, AN_ORGANIZATION);
        assert_eq!(configured.host, "forge.example");

        let defaulted = WorkspaceConfig::from_lookup(lookup(&forge("neon-law")))
            .expect("an unnamed host resolves to the default");
        assert_eq!(defaulted.organization, AN_ORGANIZATION);
        assert_eq!(defaulted.host, DEFAULT_GIT_HOST);
    }

    /// The mount is the code plus one literal segment.
    ///
    /// Nothing composes a name into it, so the Vite base a Project's portal is
    /// built for is derivable from the repository name alone.
    #[test]
    fn the_portal_mount_is_the_code_and_one_literal_segment() {
        assert_eq!(
            WorkspaceConfig::portal_mount("per-diem"),
            "/app/projects/per-diem/portal/"
        );
        // The trailing slash is load-bearing twice: Vite joins asset URLs
        // directly onto the base, and Navigator redirects the bare mount here.
        assert!(WorkspaceConfig::portal_mount("kizuna").ends_with('/'));
    }

    #[test]
    fn a_project_code_is_a_slug() {
        for accepted in [
            "call-prep",
            "per-diem",
            "estate-planning-call-prep",
            "a",
            "app2",
            "2fa",
        ] {
            assert!(is_valid_slug(accepted), "{accepted:?} must be a valid slug");
        }

        for refused in [
            "",
            "Litigation", // uppercase: Drive and macOS are case-insensitive
            "call_prep",  // underscore: hyphens, one separator
            "-call-prep", // must start alphanumeric
            "call-prep-", // must end alphanumeric
            "call--prep", // no doubled separator
            "call prep",
            "call/prep",
            "call.prep",
        ] {
            assert!(!is_valid_slug(refused), "{refused:?} must not be a slug");
        }

        assert!(!is_valid_slug(&"a".repeat(SLUG_MAX_LEN + 1)));
        assert!(is_valid_slug(&"a".repeat(SLUG_MAX_LEN)));
    }

    /// The reservation guards the segment a Project code actually occupies.
    ///
    /// `new` is well-formed, so the shape check is not what rejects it —
    /// `/app/projects/new` is the matter-open form, and a Project code that
    /// collides with it is refused by `store::projects::is_valid_code`.
    #[test]
    fn a_reserved_project_code_is_well_formed_and_still_refused() {
        for reserved in RESERVED_PROJECT_CODES {
            assert!(
                is_valid_slug(reserved),
                "{reserved} is well-formed — the shape check is not what rejects it"
            );
        }
        assert!(
            RESERVED_PROJECT_CODES.contains(&"new"),
            "`/app/projects/new` is a literal route, so `new` cannot be a Project code"
        );
    }
}
