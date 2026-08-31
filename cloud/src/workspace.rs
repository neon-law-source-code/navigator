//! Deployment-owned Project workspace coordinates.
//!
//! A Project's Drive ingest folder and its one source repository are determined by the
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

/// Navigator's own repository, which is never a Project's.
///
/// Not configuration and not derived from any deployment's pair: this one
/// repository holds Navigator itself, on one host, under one organization,
/// forever. Every deployment the Firm operates is built from it, so it is the
/// one repository URL that is the same everywhere while a Project's is the one
/// that differs everywhere.
///
/// It is a constant here so the distinction is checkable rather than
/// remembered. Two rules read it, and both exist because the confusion is easy
/// to make and silent to live with: [`RESERVED_PROJECT_CODES`] refuses
/// `navigator` as a Project code, and `store::project_reconcile` fails a
/// Project row that records this URL. A matter whose repository is Navigator
/// would mount the product's own source as a client portal.
pub const NAVIGATOR_REPOSITORY_URL: &str = "https://github.com/neon-law-source-code/navigator";

/// Whether a URL names Navigator's own repository rather than a Project's.
///
/// Compared on the normalized form a person is likely to paste — a trailing
/// slash, a `.git` suffix, surrounding whitespace, and case in the scheme and
/// host — because the point is to catch the paste, not to reward a tidy one.
/// The path is compared case-sensitively: forge paths are case-sensitive, and
/// a differently-cased path is a different repository.
#[must_use]
pub fn is_navigator_repository(url: &str) -> bool {
    fn normalize(url: &str) -> String {
        let trimmed = url.trim().trim_end_matches('/');
        let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
        match trimmed.split_once("://") {
            Some((scheme, rest)) => match rest.split_once('/') {
                Some((authority, path)) => format!(
                    "{}://{}/{path}",
                    scheme.to_ascii_lowercase(),
                    authority.to_ascii_lowercase()
                ),
                None => format!(
                    "{}://{}",
                    scheme.to_ascii_lowercase(),
                    rest.to_ascii_lowercase()
                ),
            },
            None => trimmed.to_string(),
        }
    }
    normalize(url) == NAVIGATOR_REPOSITORY_URL
}

/// The customer whose Projects this deployment serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCustomer {
    NeonLaw,
}

/// The Google Workspace that owns the selected Shared Drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleWorkspace {
    NeonLaw,
}

/// The persistent Navigator deployments that own Project workspaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentWorkspace {
    NeonLawProduction,
    NeonLawStaging,
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
///
/// `navigator` is reserved for a different reason: not a route collision but a
/// repository one. A Project code *is* its repository name, so a matter coded
/// `navigator` in the Firm's own organization would name
/// [`NAVIGATOR_REPOSITORY_URL`] — the product's own source — and every rule
/// that treats a Project repository as client-adjacent would then be pointed at
/// Navigator itself. A Project is never Navigator, and this is where that stops
/// being a convention.
pub const RESERVED_PROJECT_CODES: &[&str] = &["navigator", "new"];

/// Whether a value is safe as a URL segment and a repository name.
///
/// Lowercase letters, digits, and single hyphens; alphanumeric at both ends.
/// This is the single definition of that shape. `store::projects::is_valid_code`
/// calls it and additionally refuses [`RESERVED_PROJECT_CODES`], so a Project
/// code and its repository name are enforced identical rather than documented
/// as identical.
///
/// Two restrictions are deliberate rather than incidental, because a matter's
/// code *is* its repository's directory name and the mapping is an equality
/// check, not a normalization:
///
/// - **Lowercase only.** A checkout and macOS are case-insensitive, so
///   allowing uppercase would let one directory answer to two distinct codes.
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

/// The documents-bucket key prefix for one Project.
///
/// The prefix *is* the Project code. Navigator does not create a bucket per
/// Project; working-file keys live under this prefix in the deployment's
/// private documents bucket.
#[must_use]
pub fn documents_prefix(project_code: &str) -> String {
    format!("projects/{project_code}")
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

    /// The repository URL this deployment would create for `project_code`.
    ///
    /// Composable without asking anyone: [`Self::host`] and
    /// [`Self::organization`] are the deployment's one creation target, and a
    /// Project code *is* its repository name by construction
    /// ([`is_valid_slug`]). So the coordinate a new Project's repository will
    /// occupy is known before the repository exists.
    ///
    /// # This is a default and an expectation, never a fallback
    ///
    /// Nothing may compose this into a link for a matter whose
    /// `repository_url` is unset. Composing on *read* is what
    /// `store::projects::Project::repository_url` replaced, and the reason is
    /// still true: a composed URL always resolves, so it renders a confident
    /// link for a matter that has no repository at all. The stored URL remains
    /// the only truth about where a Project's source actually is.
    ///
    /// The two legitimate uses are the two this method exists for: seeding the
    /// target at creation time, and comparing against what a row recorded so a
    /// reconciler can say *this row is not where this deployment would have put
    /// it*. A Project whose source genuinely lives on another forge is a
    /// difference worth surfacing, not an error — which is why the comparison
    /// belongs to a caller that can grade it, rather than to this method.
    #[must_use]
    pub fn expected_repository_url(&self, project_code: &str) -> String {
        format!(
            "https://{}/{}/{}",
            self.host, self.organization, project_code
        )
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
        documents_prefix, is_navigator_repository, is_valid_slug, DeploymentWorkspace,
        GoogleWorkspace, WorkspaceConfig, WorkspaceConfigError, WorkspaceCustomer,
        DEFAULT_GIT_HOST, NAVIGATOR_GCP_PROJECT_ID, NAVIGATOR_GITHUB_ORG, NAVIGATOR_GIT_HOST,
        NAVIGATOR_PROJECTS_DRIVE_MOUNT, NAVIGATOR_REPOSITORY_URL, RESERVED_PROJECT_CODES,
        SLUG_MAX_LEN,
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

    /// The creation target is the pair plus the code, in that order.
    ///
    /// Composed rather than stored because every part is already known: the
    /// pair is this deployment's one creation target, and a Project code is its
    /// repository name by construction.
    #[test]
    fn the_expected_repository_is_the_forge_pair_plus_the_code() {
        let workspace = WorkspaceConfig::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, AN_ORGANIZATION),
            (NAVIGATOR_GIT_HOST, "forge.example"),
        ]))
        .expect("a fully configured pair resolves");

        assert_eq!(
            workspace.expected_repository_url("sample-litigation"),
            "https://forge.example/an-organization/sample-litigation"
        );
        assert_eq!(
            documents_prefix("sample-litigation"),
            "projects/sample-litigation"
        );
        assert_eq!(documents_prefix("acme"), "projects/acme");
    }

    /// The default host is composed in exactly as a configured one is, so a
    /// deployment that names no host still has a creation target.
    #[test]
    fn the_expected_repository_uses_the_default_host_when_none_is_named() {
        let workspace = WorkspaceConfig::from_lookup(lookup(&forge("neon-law-stg")))
            .expect("an unnamed host resolves to the default");

        assert_eq!(
            workspace.expected_repository_url("sample-estate"),
            format!("https://{DEFAULT_GIT_HOST}/{AN_ORGANIZATION}/sample-estate")
        );
    }

    /// The last path segment of the expected URL is the code itself.
    ///
    /// This is the property a reconciler reads back: a Project code is its
    /// repository name, so a recorded URL naming a different last segment is
    /// drift no matter which forge or organization holds it. Asserted here
    /// rather than only where it is consumed, because it is this method that
    /// has to keep it true.
    #[test]
    fn the_expected_repository_ends_in_the_code() {
        let workspace =
            WorkspaceConfig::from_lookup(lookup(&forge("neon-law-stg"))).expect("resolves");

        for code in ["sample-litigation", "sample-transactional", "a1"] {
            let url = workspace.expected_repository_url(code);
            assert_eq!(
                url.rsplit('/').next(),
                Some(code),
                "the expected URL for {code} must end in the code: {url}"
            );
        }
    }

    /// A Project's expected repository is never Navigator's own, on any
    /// deployment. The two enforcements meet here: the code `navigator` is
    /// reserved, so nothing composable reaches Navigator's URL even when the
    /// deployment's pair is Navigator's own host and organization.
    #[test]
    fn no_deployment_composes_navigators_own_repository_for_a_project() {
        let navigators_own_pair = WorkspaceConfig::from_lookup(lookup(&[
            (NAVIGATOR_GCP_PROJECT_ID, "neon-law-stg"),
            (NAVIGATOR_GITHUB_ORG, "neon-law-source-code"),
        ]))
        .expect("resolves");

        assert!(
            RESERVED_PROJECT_CODES.contains(&"navigator"),
            "the refusal that makes the composition below unreachable"
        );
        // The one code that would have composed it is refused upstream, so this
        // asserts the shape rather than a reachable state.
        assert_eq!(
            navigators_own_pair.expected_repository_url("navigator"),
            NAVIGATOR_REPOSITORY_URL,
            "if this ever stops matching, the reserved code above stops protecting anything"
        );
        for code in ["sample-litigation", "acme", "a1"] {
            assert!(
                !is_navigator_repository(&navigators_own_pair.expected_repository_url(code)),
                "{code} must not compose Navigator's own repository"
            );
        }
    }

    /// The three parts of Navigator's own URL, so a fixture can vary one at a
    /// time without spelling a forge host — which the coordinate guard in
    /// `cli/tests/forge_coordinate_retired.rs` forbids in this file, and rightly:
    /// a bare host here is how a composed Project coordinate got reintroduced
    /// the first time.
    fn navigator_url_parts() -> (&'static str, &'static str, &'static str) {
        let (scheme, rest) = NAVIGATOR_REPOSITORY_URL
            .split_once("://")
            .expect("the constant carries a scheme");
        let (host, path) = rest.split_once('/').expect("the constant carries a path");
        (scheme, host, path)
    }

    /// The paste, in every spelling someone plausibly makes it.
    #[test]
    fn navigators_repository_is_recognized_however_it_is_written() {
        let (scheme, host, path) = navigator_url_parts();
        for spelling in [
            NAVIGATOR_REPOSITORY_URL.to_string(),
            format!("{NAVIGATOR_REPOSITORY_URL}/"),
            format!("{NAVIGATOR_REPOSITORY_URL}.git"),
            format!("  {NAVIGATOR_REPOSITORY_URL}  "),
            // Scheme and host shouted, path left alone: the two halves the
            // normalization lowercases.
            format!("{}://{}/{path}", scheme.to_uppercase(), host.to_uppercase()),
        ] {
            assert!(is_navigator_repository(&spelling), "{spelling}");
        }
    }

    /// The rule is about one repository, not a namespace. The Firm's own
    /// organization is an ordinary place for a matter to live, and a
    /// differently-cased path is a different repository on a case-sensitive
    /// forge.
    #[test]
    fn only_navigators_own_repository_is_navigator() {
        let (scheme, host, path) = navigator_url_parts();
        let (organization, repository) = path.split_once('/').expect("org and repository");
        for other in [
            // A different repository in Navigator's own organization.
            format!("{scheme}://{host}/{organization}/acme"),
            // Navigator's repository name in somebody else's organization.
            format!("{scheme}://{host}/an-organization/{repository}"),
            // The same path on a different forge.
            format!("{scheme}://forge.example/{path}"),
            // A case-sensitive forge path is a different repository.
            format!(
                "{scheme}://{host}/{organization}/{}",
                repository.to_uppercase()
            ),
        ] {
            assert!(!is_navigator_repository(&other), "{other}");
        }
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
