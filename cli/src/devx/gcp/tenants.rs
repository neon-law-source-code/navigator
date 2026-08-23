//! The workspace's permanent Google Cloud project IDs, and the guard that
//! keeps one project from being provisioned as two different things.
//!
//! `docs/environments.md` records four projects: one **hub** that holds
//! container images, the CI pusher service account, and the GitHub Workload
//! Identity pool, plus three **environments** that actually run Navigator.
//! Project IDs are immutable in GCP, so the list here is permanent.
//!
//! The provisioners are separate commands with very different blast radii —
//! `ops gcp hub setup` creates a registry and an identity, `ops gcp setup`
//! creates buckets and a GKE cluster. Pointing either at the
//! other's project would provision resources into a project whose whole point
//! is not to hold them. [`validate_target`] refuses that before the first GCP
//! call, and [`validate_images_project`] refuses an environment that names
//! itself as its own image hub.
//!
//! An unrecorded project ID is always allowed: a fork provisions its own
//! projects, and the registry only knows this workspace's four.

use super::error::{SetupError, SetupResult};

/// The shared image hub. Not an environment — nothing runs there.
pub const HUB_PROJECT_ID: &str = "ghcr";

/// The runtime projects Neon Law Navigator runs in, in release order.
/// `neon-law-stg` proves a release before production takes it.
pub const ENVIRONMENT_PROJECT_IDS: &[&str] = &["neon-law-stg", "neon-law"];

/// What a project ID is being provisioned as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantRole {
    /// `ops gcp hub setup` — the shared registry project.
    Hub,
    /// `ops gcp setup` — one environment.
    Environment,
}

impl TenantRole {
    /// Human-readable form used in [`SetupError::TenantConflict`].
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hub => "the image hub",
            Self::Environment => "an environment",
        }
    }
}

/// The role `project_id` is recorded under, or `None` when the workspace does
/// not know it — a fork's own project, or a scratch project.
#[must_use]
pub fn recorded_role(project_id: &str) -> Option<TenantRole> {
    let project_id = project_id.trim();
    if project_id == HUB_PROJECT_ID {
        return Some(TenantRole::Hub);
    }
    ENVIRONMENT_PROJECT_IDS
        .contains(&project_id)
        .then_some(TenantRole::Environment)
}

/// Refuse `project_id` when it is recorded for a target other than `role`.
///
/// The successor to `staging::StagingSetupConfig::validate`'s
/// `staging != production` check, generalized from two targets to four.
pub fn validate_target(role: TenantRole, project_id: &str) -> SetupResult<()> {
    if project_id.trim().is_empty() {
        return Err(SetupError::MissingConfiguration("--project-id"));
    }
    match recorded_role(project_id) {
        Some(recorded) if recorded != role => Err(SetupError::TenantConflict {
            project_id: project_id.trim().to_string(),
            recorded: recorded.label(),
            requested: role.label(),
        }),
        _ => Ok(()),
    }
}

/// Refuse an `--images-project-id` that is not a usable hub for
/// `environment_project_id`: empty, the environment itself, or a project
/// recorded as an environment.
pub fn validate_images_project(
    environment_project_id: &str,
    images_project_id: &str,
) -> SetupResult<()> {
    if images_project_id.trim().is_empty() {
        return Err(SetupError::MissingConfiguration("--images-project-id"));
    }
    if images_project_id.trim() == environment_project_id.trim() {
        return Err(SetupError::TenantConflict {
            project_id: images_project_id.trim().to_string(),
            recorded: TenantRole::Environment.label(),
            requested: TenantRole::Hub.label(),
        });
    }
    validate_target(TenantRole::Hub, images_project_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_is_not_one_of_the_environments() {
        assert!(
            !ENVIRONMENT_PROJECT_IDS.contains(&HUB_PROJECT_ID),
            "the hub holds images and runs nothing; it is never an environment",
        );
        assert_eq!(
            ENVIRONMENT_PROJECT_IDS.len(),
            3,
            "docs/environments.md records three runtime projects",
        );
    }

    #[test]
    fn the_hub_command_refuses_an_environment_project() {
        for project_id in ENVIRONMENT_PROJECT_IDS {
            let err = validate_target(TenantRole::Hub, project_id)
                .expect_err("{project_id} is an environment, not the hub");
            let message = err.to_string();
            assert!(message.contains(project_id), "{message}");
            assert!(message.contains("an environment"), "{message}");
            assert!(message.contains("the image hub"), "{message}");
        }
    }

    #[test]
    fn the_environment_command_refuses_the_hub_project() {
        let err = validate_target(TenantRole::Environment, HUB_PROJECT_ID)
            .expect_err("the hub must never receive buckets or GKE");
        let message = err.to_string();
        assert!(message.contains(HUB_PROJECT_ID), "{message}");
        assert!(message.contains("the image hub"), "{message}");
    }

    #[test]
    fn each_recorded_project_is_accepted_for_its_own_role() {
        validate_target(TenantRole::Hub, HUB_PROJECT_ID).unwrap();
        for project_id in ENVIRONMENT_PROJECT_IDS {
            validate_target(TenantRole::Environment, project_id).unwrap();
        }
    }

    #[test]
    fn an_unrecorded_project_is_allowed_for_either_role() {
        validate_target(TenantRole::Hub, "a-fork-registry").unwrap();
        validate_target(TenantRole::Environment, "a-fork-prod").unwrap();
    }

    #[test]
    fn an_empty_project_id_is_refused_before_any_gcp_call() {
        let err = validate_target(TenantRole::Hub, "   ").unwrap_err();
        assert!(err.to_string().contains("--project-id"), "{err}");
    }

    #[test]
    fn an_environment_may_not_be_its_own_image_hub() {
        let err = validate_images_project("neon-law", "neon-law")
            .expect_err("an environment never hosts the shared registry");
        assert!(err.to_string().contains("neon-law"), "{err}");
    }

    #[test]
    fn images_project_must_not_be_another_environment() {
        let err = validate_images_project("neon-law", "neon-law-stg")
            .expect_err("images come from the hub, never from a peer environment");
        assert!(err.to_string().contains("neon-law-stg"), "{err}");
    }

    #[test]
    fn the_hub_is_a_valid_images_project_for_every_environment() {
        for project_id in ENVIRONMENT_PROJECT_IDS {
            validate_images_project(project_id, HUB_PROJECT_ID).unwrap();
        }
    }

    #[test]
    fn an_empty_images_project_names_its_own_flag() {
        let err = validate_images_project("neon-law", "").unwrap_err();
        assert!(err.to_string().contains("--images-project-id"), "{err}");
    }
}
