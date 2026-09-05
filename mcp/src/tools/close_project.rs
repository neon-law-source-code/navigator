//! `aida_close_project` MCP tool.
//!
//! Closes an existing Project through the shared lifecycle command. The
//! attestation is explicit because closing is a firm-policy decision, and the
//! store derives the coupled `closed_at` from the transition and optional
//! effective time.

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::ToolError;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_close_project",
        "description": "Close an existing Project (matter) in Neon Law Navigator. This is a lifecycle transition: it sets status to closed and stamps closed_at for retention. The closing attorney must explicitly attest that the matter is ready to close.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "Uuid of the existing Project to close."
                },
                "attestation": {
                    "type": "boolean",
                    "description": "The closing attorney's explicit attestation that the matter is ready to close. Must be true."
                },
                "effective_at": {
                    "type": "string",
                    "format": "date-time",
                    "description": "Optional RFC 3339 time when the matter actually closed. Must not be in the future or precede matter-open."
                }
            },
            "required": ["project_id", "attestation"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    project_id: Uuid,
    #[serde(default)]
    attestation: Option<bool>,
    effective_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn call(
    surreal: &store::surreal::SurrealDb,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;
    if !args.attestation.unwrap_or(false) {
        return Err(ToolError::InvalidArguments(
            "this matter close requires attestation — pass attestation=true to affirm the matter is ready to close".into(),
        ));
    }

    let closed = store::projects::transition_project(
        surreal,
        args.project_id,
        store::projects::Transition::Close,
        args.effective_at,
    )
    .await
    .map_err(|error| match error {
        store::projects::ProjectCommandError::NotFound => {
            ToolError::NotFound(format!("project_id={}", args.project_id))
        }
        store::projects::ProjectCommandError::Invalid(message) => {
            ToolError::InvalidArguments(message.into())
        }
        store::projects::ProjectCommandError::Db(message) => ToolError::Database(message),
        store::projects::ProjectCommandError::Referenced(detail) => ToolError::Internal(detail),
    })?;

    let summary = format!(
        "Closed project id={} ({}, status={}, closed_at={}).",
        closed.id,
        closed.name,
        closed.status,
        closed.closed_at.as_deref().unwrap_or("unknown")
    );
    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "id": closed.id,
            "name": closed.name,
            "status": closed.status,
            "closed_at": closed.closed_at,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::ToolError;
    use serde_json::json;
    use store::projects::{create, NewProject};
    use store::test_support::{mem_surreal, seed_entity};
    use uuid::Uuid;

    async fn project(surreal: &store::surreal::SurrealDb) -> Uuid {
        create(
            surreal,
            &NewProject {
                code: format!("close-{}", Uuid::now_v7().simple()),
                name: "Closeable matter".into(),
                status: "open".into(),
                entity_id: seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .expect("create project")
        .id
    }

    #[test]
    fn descriptor_requires_project_and_attestation() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_close_project");
        assert_eq!(
            d["inputSchema"]["required"],
            json!(["project_id", "attestation"])
        );
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            d["inputSchema"]["properties"]["effective_at"]["format"],
            "date-time"
        );
    }

    #[tokio::test]
    async fn closes_project_and_stamps_date() {
        let surreal = mem_surreal().await;
        let id = project(&surreal).await;
        let effective_at = chrono::Utc::now();

        let result = call(
            &surreal,
            &json!({
                "project_id": id,
                "attestation": true,
                "effective_at": effective_at,
            }),
        )
        .await
        .expect("close project");

        assert_eq!(result["structuredContent"]["status"], "closed");
        let closed_at: chrono::DateTime<chrono::Utc> = result["structuredContent"]["closed_at"]
            .as_str()
            .expect("close time")
            .parse()
            .expect("RFC 3339 close time");
        assert_eq!(closed_at, effective_at);
    }

    #[tokio::test]
    async fn requires_attestation_and_rejects_unknown_fields() {
        let surreal = mem_surreal().await;
        let id = project(&surreal).await;

        let error = call(&surreal, &json!({"project_id": id}))
            .await
            .expect_err("missing attestation must be rejected");
        assert!(matches!(error, ToolError::InvalidArguments(_)));

        let error = call(
            &surreal,
            &json!({"project_id": id, "attestation": true, "closed_at": "now"}),
        )
        .await
        .expect_err("unknown fields must be rejected");
        assert!(matches!(error, ToolError::InvalidArguments(_)));
    }
}
