//! The single deployment authorized to consume GitHub engineering automation.
//!
//! Navigator ships one image to several persistent environments, while a
//! GitHub App supplies one webhook URL. Letting every deployment mount the
//! receiver or register the `DevX` Restate services would duplicate work and
//! defeat global budget accounting. The staging project is therefore the one
//! automation authority; all other deployments deliberately stay dark. It runs
//! there rather than in a production project because engineering automation
//! acts on this repository, not on anyone's matters.

/// The one GCP project allowed to run GitHub engineering automation.
pub use store::deployment::GITHUB_AUTOMATION_HOME_PROJECT as AUTOMATION_HOME_PROJECT;

/// Whether a deployment is the authoritative GitHub-automation home.
///
/// Missing or mismatched project identity is deliberately `false`: a copied
/// Secret must not turn a tenant deployment into a second webhook consumer.
#[must_use]
pub fn is_automation_home(project_id: Option<&str>) -> bool {
    project_id == Some(AUTOMATION_HOME_PROJECT)
}

#[cfg(test)]
mod tests {
    use super::{is_automation_home, AUTOMATION_HOME_PROJECT};

    #[test]
    fn only_the_staging_project_is_the_automation_authority() {
        assert!(is_automation_home(Some(AUTOMATION_HOME_PROJECT)));
        // Every entry must be a project that is NOT the automation home, so
        // none of them may be `AUTOMATION_HOME_PROJECT` — the assertion below is
        // vacuous the moment one is.
        for project in [
            None,
            Some("neon-law"),
            Some("another-deployment"),
            Some("ghcr"),
        ] {
            assert!(
                !is_automation_home(project),
                "{project:?} must not consume the singleton GitHub webhook stream"
            );
        }
    }
}
