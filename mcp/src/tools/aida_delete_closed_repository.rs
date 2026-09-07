//! `aida_delete_closed_repository` MCP tool.
//!
//! Deletes a closed Project's forge repository once its
//! [`rules::kind::Kind::ClosedRepository`] archive is filed and verified
//! current. This is the deliberate, separate step that follows a matter's
//! close and its archive: it refuses unless the Project is closed, a
//! `closed_repository` document exists for it, that document's recorded
//! commit matches the forge's live HEAD, and an operator has confirmed the
//! provider handoff. Deleting the forge's copy is the one part of this
//! sequence that cannot be undone, so nothing here happens on an assumption.

use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

use super::ToolError;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_delete_closed_repository",
        "description": "Delete a closed Project's forge repository. Refuses unless the Project is closed, a closed_repository document is filed for it, that document's recorded commit matches the forge's live HEAD, and the operator confirms the provider handoff.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "Uuid of the closed Project whose forge repository is being decommissioned."
                },
                "confirm_handoff": {
                    "type": "boolean",
                    "description": "The operator's explicit confirmation that the forge repository is no longer needed and may be permanently deleted. Must be true."
                }
            },
            "required": ["project_id", "confirm_handoff"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    project_id: Uuid,
    #[serde(default)]
    confirm_handoff: Option<bool>,
}

/// `forge` is `None` on a deployment that has not configured forge
/// credentials — the same optional-dependency shape as
/// [`crate::server::McpState::storage`] and
/// [`crate::server::McpState::email`]. The tool refuses cleanly rather than
/// panicking when it is absent.
pub async fn call(
    surreal: &store::surreal::SurrealDb,
    forge: Option<&Arc<dyn cloud::ForgeService>>,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;
    if !args.confirm_handoff.unwrap_or(false) {
        return Err(ToolError::InvalidArguments(
            "deleting a repository requires confirm_handoff=true to affirm the forge copy is no \
             longer needed"
                .into(),
        ));
    }
    let Some(forge) = forge else {
        return Err(ToolError::Internal(
            "no forge is configured on this deployment".into(),
        ));
    };

    let project = store::projects::find_by_id(surreal, args.project_id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?
        .ok_or_else(|| ToolError::NotFound(format!("project_id={}", args.project_id)))?;

    if project.status != "closed" {
        return Err(ToolError::InvalidArguments(format!(
            "project_id={} is {}, not closed — close the matter before deleting its repository",
            args.project_id, project.status
        )));
    }

    let asset = store::assets::current(
        surreal,
        args.project_id,
        rules::kind::Kind::ClosedRepository.as_str(),
        store::assets::Lens::Lawyer,
    )
    .await
    .map_err(|error| ToolError::Database(error.to_string()))?
    .ok_or_else(|| {
        ToolError::InvalidArguments(format!(
            "project_id={} has no filed closed_repository document — archive it first",
            args.project_id
        ))
    })?;

    let recorded_sha = asset
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("commit_sha"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ToolError::Internal("the filed closed_repository document carries no commit_sha".into())
        })?;

    let live_sha = forge
        .head_commit_sha(&project.code)
        .await
        .map_err(|error| ToolError::Internal(error.to_string()))?
        .ok_or_else(|| {
            ToolError::NotFound(format!(
                "no live repository found for project_id={}",
                args.project_id
            ))
        })?;

    if recorded_sha != live_sha {
        return Err(ToolError::InvalidArguments(format!(
            "the filed archive records commit {recorded_sha}, but the live repository's HEAD is \
             {live_sha} — the repository moved since the archive was filed; file a fresh \
             closed_repository document before deleting"
        )));
    }

    forge
        .delete_repository(&project.code)
        .await
        .map_err(|error| ToolError::Internal(error.to_string()))?;

    store::projects::set_repository_url(surreal, args.project_id, None)
        .await
        .map_err(|error| ToolError::Internal(error.to_string()))?;

    let summary = format!(
        "Deleted the forge repository for project_id={} ({}) at verified commit {live_sha}.",
        project.id, project.code
    );
    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "id": project.id,
            "code": project.code,
            "deleted_commit_sha": live_sha,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::ToolError;
    use cloud::forge::FakeForge;
    use cloud::{ForgeService, StorageService};
    use serde_json::json;
    use std::sync::Arc;
    use store::documents::{DocumentIdentity, IngestArgs};
    use store::projects::{create, transition_project, NewProject, Transition};
    use store::test_support::{mem_surreal, seed_entity};
    use uuid::Uuid;

    async fn storage() -> Arc<dyn StorageService> {
        let dir = std::env::temp_dir().join(format!(
            "navigator-mcp-delete-closed-repo-{}",
            Uuid::now_v7()
        ));
        Arc::new(cloud::FsStorage::new(dir).await.unwrap())
    }

    async fn closed_project(surreal: &store::surreal::SurrealDb) -> (Uuid, String) {
        let code = format!("closed-repo-{}", Uuid::now_v7().simple());
        let created = create(
            surreal,
            &NewProject {
                code: code.clone(),
                name: "Decommissionable matter".into(),
                status: "open".into(),
                entity_id: seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("create project");
        transition_project(surreal, created.id, Transition::Close, None)
            .await
            .expect("close project");
        (created.id, code)
    }

    async fn file_archive(
        surreal: &store::surreal::SurrealDb,
        storage: &Arc<dyn StorageService>,
        project_id: Uuid,
        commit_sha: &str,
    ) {
        let kind = rules::kind::Kind::ClosedRepository.as_str();
        store::documents::ingest_bytes_as(
            surreal,
            storage,
            &IngestArgs {
                project_id,
                source: "generated",
                filename: "repository.zip",
                kind,
                content_type: "application/zip",
                description: None,
                secondary_storage_key: None,
                visibility: store::documents::visibility::INTERNAL,
            },
            &DocumentIdentity {
                slug: Some(kind),
                published_at: None,
                metadata: Some(json!({ "commit_sha": commit_sha })),
            },
            b"a zip archive's bytes",
        )
        .await
        .expect("file closed_repository asset");
    }

    #[test]
    fn descriptor_requires_project_and_confirmation() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_delete_closed_repository");
        assert_eq!(
            d["inputSchema"]["required"],
            json!(["project_id", "confirm_handoff"])
        );
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
    }

    #[tokio::test]
    async fn deletes_when_the_archive_matches_live_head() {
        let surreal = mem_surreal().await;
        let (project_id, code) = closed_project(&surreal).await;
        file_archive(&surreal, &storage().await, project_id, "deadbeefcafe").await;

        let forge = FakeForge::new();
        forge.ensure_repository(&code).await.unwrap();
        forge.set_head_commit_sha(&code, "deadbeefcafe");
        let forge: Arc<dyn ForgeService> = Arc::new(forge);

        let result = call(
            &surreal,
            Some(&forge),
            &json!({ "project_id": project_id, "confirm_handoff": true }),
        )
        .await
        .expect("delete repository");

        assert_eq!(result["structuredContent"]["code"], code);
        assert_eq!(forge.find_repository(&code).await.unwrap(), None);
        let project = store::projects::find_by_id(&surreal, project_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(project.repository_url, None);
    }

    #[tokio::test]
    async fn refuses_when_the_recorded_commit_does_not_match_live_head() {
        let surreal = mem_surreal().await;
        let (project_id, code) = closed_project(&surreal).await;
        file_archive(&surreal, &storage().await, project_id, "deadbeefcafe").await;

        let forge = FakeForge::new();
        forge.ensure_repository(&code).await.unwrap();
        forge.set_head_commit_sha(&code, "0000000000000");
        let forge: Arc<dyn ForgeService> = Arc::new(forge);

        let error = call(
            &surreal,
            Some(&forge),
            &json!({ "project_id": project_id, "confirm_handoff": true }),
        )
        .await
        .expect_err("mismatched commit must be refused");
        assert!(matches!(error, ToolError::InvalidArguments(_)));
        assert!(forge.find_repository(&code).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn refuses_an_open_project() {
        let surreal = mem_surreal().await;
        let code = format!("open-repo-{}", Uuid::now_v7().simple());
        let created = create(
            &surreal,
            &NewProject {
                code: code.clone(),
                name: "Still-open matter".into(),
                status: "open".into(),
                entity_id: seed_entity(&surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("create project");

        let forge: Arc<dyn ForgeService> = Arc::new(FakeForge::new());
        let error = call(
            &surreal,
            Some(&forge),
            &json!({ "project_id": created.id, "confirm_handoff": true }),
        )
        .await
        .expect_err("open project must be refused");
        assert!(matches!(error, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn refuses_without_a_filed_archive() {
        let surreal = mem_surreal().await;
        let (project_id, _code) = closed_project(&surreal).await;

        let forge: Arc<dyn ForgeService> = Arc::new(FakeForge::new());
        let error = call(
            &surreal,
            Some(&forge),
            &json!({ "project_id": project_id, "confirm_handoff": true }),
        )
        .await
        .expect_err("missing archive must be refused");
        assert!(matches!(error, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn requires_confirmation_and_a_configured_forge() {
        let surreal = mem_surreal().await;
        let (project_id, _code) = closed_project(&surreal).await;

        let error = call(&surreal, None, &json!({"project_id": project_id}))
            .await
            .expect_err("missing confirm_handoff must be rejected");
        assert!(matches!(error, ToolError::InvalidArguments(_)));

        let error = call(
            &surreal,
            None,
            &json!({"project_id": project_id, "confirm_handoff": true}),
        )
        .await
        .expect_err("an unconfigured forge must be rejected");
        assert!(matches!(error, ToolError::Internal(_)));
    }
}
