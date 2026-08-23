//! Repository-to-Project correlation for GitHub webhook consumers.
//!
//! This module owns the only database read involved in webhook Project
//! correlation. It deliberately does not consult persons, participation, embedded Rego policy,
//! or forge collaborators: repository identity is routing metadata, not an
//! authorization grant.

use async_trait::async_trait;
use github_webhooks::worker::{RepositoryResolutionError, RepositoryResolver, ResolvedRepository};

/// Database-backed correlation configuration shared with the webhook receiver.
#[derive(Clone)]
pub struct ProjectRepositoryResolver {
    surreal: store::surreal::SurrealDb,
    canonical_repository: String,
    project_owner: String,
}

impl ProjectRepositoryResolver {
    #[must_use]
    pub fn new(
        surreal: store::surreal::SurrealDb,
        canonical_repository: impl Into<String>,
        project_owner: impl Into<String>,
    ) -> Self {
        Self {
            surreal,
            canonical_repository: canonical_repository.into(),
            project_owner: project_owner.into(),
        }
    }
}

#[async_trait]
impl RepositoryResolver for ProjectRepositoryResolver {
    async fn resolve(
        &self,
        repository: &str,
    ) -> Result<ResolvedRepository, RepositoryResolutionError> {
        if repository == self.canonical_repository {
            return Ok(ResolvedRepository::Code {
                repository: repository.into(),
            });
        }

        let (owner, code) = repository
            .split_once('/')
            .filter(|(_, code)| !code.is_empty() && !code.contains('/'))
            .ok_or(RepositoryResolutionError::Malformed)?;
        if owner != self.project_owner {
            return Err(RepositoryResolutionError::WrongOwner);
        }

        let projects = store::projects::find_by_code(&self.surreal, code)
            .await
            .map_err(|_| RepositoryResolutionError::Database)?;
        match projects.as_slice() {
            [project] => Ok(ResolvedRepository::Project {
                repository: repository.into(),
                project_id: project.id,
            }),
            [] => Err(RepositoryResolutionError::MissingProject),
            _ => Err(RepositoryResolutionError::AmbiguousProject),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectRepositoryResolver;
    use github_webhooks::worker::{RepositoryResolver, ResolvedRepository};

    use store::test_support::mem_surreal;
    const CODE_REPOSITORY: &str = "neon-law-source-code/navigator";
    const PROJECT_OWNER: &str = "neon-law-firm";

    fn resolver(surreal: store::surreal::SurrealDb) -> ProjectRepositoryResolver {
        ProjectRepositoryResolver::new(surreal, CODE_REPOSITORY, PROJECT_OWNER)
    }

    async fn insert_project(surreal: &store::surreal::SurrealDb, code: &str) -> uuid::Uuid {
        store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: code.into(),
                name: format!("Test Project {code}"),
                status: "open".into(),
                entity_id: store::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("insert Project")
        .id
    }

    #[tokio::test]
    async fn code_repository_resolves_without_a_project_lookup() {
        let surreal = store::test_support::mem_surreal().await;

        assert_eq!(
            resolver(surreal)
                .resolve(CODE_REPOSITORY)
                .await
                .expect("code repository resolves"),
            ResolvedRepository::Code {
                repository: CODE_REPOSITORY.into(),
            }
        );
    }

    #[tokio::test]
    async fn known_project_repository_resolves_by_exact_code() {
        let surreal = mem_surreal().await;
        let project_id = insert_project(&surreal, "arthur").await;

        let resolved = resolver(surreal)
            .resolve("neon-law-firm/arthur")
            .await
            .expect("known Project repository resolves");
        assert_eq!(
            resolved,
            ResolvedRepository::Project {
                repository: "neon-law-firm/arthur".into(),
                project_id,
            }
        );
        assert!(resolved.workflow_key().contains("neon-law-firm__arthur"));
        assert!(resolved.workflow_key().contains(&project_id.to_string()));
    }

    #[tokio::test]
    async fn unknown_wrong_owner_and_malformed_project_repositories_fail_closed() {
        let surreal = store::test_support::mem_surreal().await;
        let resolver = resolver(surreal);

        for repository in [
            "neon-law-firm/unknown",
            "neon-law-source-code/arthur",
            "neon-law-firm/",
            "neon-law-firm/arthur/extra",
            "not-a-repository",
        ] {
            assert!(
                resolver.resolve(repository).await.is_err(),
                "{repository} must not resolve"
            );
        }
    }

    #[tokio::test]
    async fn redelivery_reuses_the_same_project_workflow_key_without_writes() {
        let surreal = mem_surreal().await;
        insert_project(&surreal, "arthur").await;
        let resolver = resolver(surreal.clone());
        let projects_before = store::projects::all(&surreal).await.unwrap().len();
        let people_before = store::persons::list_directory(&surreal, "", "", &[])
            .await
            .unwrap()
            .len();
        let participation_before = store::projects::all_participations(&surreal)
            .await
            .unwrap()
            .len();

        let first = resolver
            .resolve("neon-law-firm/arthur")
            .await
            .expect("first delivery resolves");
        let second = resolver
            .resolve("neon-law-firm/arthur")
            .await
            .expect("redelivery resolves");
        assert_eq!(first, second);
        assert_eq!(first.workflow_key(), second.workflow_key());
        assert_eq!(
            store::projects::all(&surreal).await.unwrap().len(),
            projects_before
        );
        assert_eq!(
            store::persons::list_directory(&surreal, "", "", &[])
                .await
                .unwrap()
                .len(),
            people_before
        );
        assert_eq!(
            store::projects::all_participations(&surreal)
                .await
                .unwrap()
                .len(),
            participation_before
        );
    }
}
