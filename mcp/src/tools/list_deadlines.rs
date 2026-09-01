//! `aida_list_deadlines` MCP tool.

use serde_json::{json, Value};

use super::{ReadScope, ToolError};

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_list_deadlines",
        "description": "List the deadlines on Projects (matters) the signed-in caller \
                        participates in. Each deadline includes its due date, kind, status, \
                        statute, and source so the date can be traced to its authority and \
                        producing workflow. Takes no arguments.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }
    })
}

pub async fn call(
    surreal: &store::surreal::SurrealDb,
    scope: &ReadScope,
    _arguments: &Value,
) -> Result<Value, ToolError> {
    let projects = match scope {
        ReadScope::Membership { person_id, role } => {
            store::access::visible_projects(surreal, Some(*person_id), *role)
                .await
                .map_err(ToolError::Database)?
        }
        // A directory is deliberately metadata-only, and no identity cannot
        // establish a participation boundary. Neither scope may read matter
        // contents through this tool.
        ReadScope::Deployment | ReadScope::Directory { .. } | ReadScope::Unlinked => Vec::new(),
    };

    let mut deadlines = Vec::new();
    for project in projects {
        deadlines.extend(
            store::statutory_deadlines::by_project(surreal, project.id)
                .await
                .map_err(|error| ToolError::Database(error.to_string()))?,
        );
    }

    let rows: Vec<Value> = deadlines
        .iter()
        .map(|deadline| {
            json!({
                "id": deadline.id,
                "project_id": deadline.project_id,
                "kind": deadline.kind,
                "trigger_on": deadline.trigger_on,
                "due_on": deadline.due_on,
                "statute": deadline.statute,
                "source": deadline.source,
                "status": deadline.status,
                "inserted_at": deadline.inserted_at,
                "updated_at": deadline.updated_at,
            })
        })
        .collect();

    let summary = if deadlines.is_empty() {
        "No deadlines you participate in.".to_string()
    } else {
        let listed = deadlines
            .iter()
            .map(|deadline| {
                format!(
                    "project {}: {} due {} (statute: {}; source: {}; status: {})",
                    deadline.project_id,
                    deadline.kind,
                    deadline.due_on,
                    deadline.statute,
                    deadline.source,
                    deadline.status,
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} deadlines: {listed}.", deadlines.len())
    };

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "count": rows.len(),
            "deadlines": rows,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::ReadScope;
    use chrono::NaiveDate;
    use serde_json::json;
    use uuid::Uuid;

    use store::test_support::mem_surreal;

    async fn project(surreal: &store::surreal::SurrealDb, name: &str) -> Uuid {
        store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: format!("deadline-{}", Uuid::now_v7()),
                name: name.into(),
                status: "open".into(),
                entity_id: store::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    async fn lawyer(
        surreal: &store::surreal::SurrealDb,
        project_id: Option<Uuid>,
    ) -> store::persons::Person {
        let person = store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                format!("Lawyer {project_id:?}"),
                format!("lawyer-{}@example.com", Uuid::now_v7()),
                store::persons::Role::Lawyer,
            ),
        )
        .await
        .unwrap();
        if let Some(project_id) = project_id {
            store::participation::add_participant(
                surreal,
                &store::participation::AddParticipantCommand {
                    project_id,
                    person_id: person.id,
                    dri: store::participation::DriRequest::Unchanged,
                    actor: store::participation::DriActor::System,
                },
            )
            .await
            .unwrap();
        }
        person
    }

    async fn deadline(surreal: &store::surreal::SurrealDb, project_id: Uuid, source: &str) {
        store::statutory_deadlines::record(
            surreal,
            &store::statutory_deadlines::NewStatutoryDeadline {
                project_id,
                kind: "filing",
                trigger_on: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                due_on: NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
                statute: "15 U.S.C. § 1681i(a)(1)",
                source,
            },
        )
        .await
        .unwrap();
    }

    #[test]
    fn descriptor_names_the_tool_and_takes_no_arguments() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_list_deadlines");
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        assert!(d["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn participating_lawyer_receives_only_their_deadlines_with_authority_and_source() {
        let surreal = mem_surreal().await;
        let permitted = project(&surreal, "Permitted Matter").await;
        let denied = project(&surreal, "Denied Matter").await;
        deadline(&surreal, permitted, "triage:permitted").await;
        deadline(&surreal, denied, "triage:denied").await;
        let person = lawyer(&surreal, Some(permitted)).await;

        let result = call(
            &surreal,
            &ReadScope::Membership {
                person_id: person.id,
                role: person.role,
            },
            &json!({}),
        )
        .await
        .unwrap();

        assert_eq!(result["structuredContent"]["count"], 1);
        let row = &result["structuredContent"]["deadlines"][0];
        assert_eq!(row["project_id"], permitted.to_string());
        assert_eq!(row["statute"], "15 U.S.C. § 1681i(a)(1)");
        assert_eq!(row["source"], "triage:permitted");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("15 U.S.C. § 1681i(a)(1)"));
        assert!(text.contains("triage:permitted"));
        assert!(!text.contains("triage:denied"));
    }

    #[tokio::test]
    async fn lawyer_without_participation_receives_no_deadlines() {
        let surreal = mem_surreal().await;
        let project_id = project(&surreal, "Private Matter").await;
        deadline(&surreal, project_id, "triage:private").await;
        let person = lawyer(&surreal, None).await;

        let result = call(
            &surreal,
            &ReadScope::Membership {
                person_id: person.id,
                role: person.role,
            },
            &json!({}),
        )
        .await
        .unwrap();

        assert_eq!(result["structuredContent"]["count"], 0);
        assert!(result["structuredContent"]["deadlines"]
            .as_array()
            .unwrap()
            .is_empty());
        assert_eq!(
            result["content"][0]["text"],
            "No deadlines you participate in."
        );
    }
}
