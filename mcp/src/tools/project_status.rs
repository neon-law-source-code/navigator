//! `aida_project_status` MCP tool.
//!
//! Answers "what is the current state of this matter?" in one call:
//! the deadline docket, the most recent notation-workflow events, and the
//! participation ledger. Scoped by the caller's own [`ReadScope`] exactly
//! like every other read — a caller with no participation row on the named
//! Project gets the same not-found response as one naming a Project that
//! does not exist, so neither the model nor a transcript can distinguish
//! "wrong id" from "not on this matter."

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{ReadScope, ToolError};

/// How many of a matter's most recent notation events to surface. A status
/// answer is a snapshot of *current* activity, not the full audit trail —
/// `aida_validate_notation` and the portal's own history views are where a
/// complete journal belongs.
const RECENT_EVENTS_LIMIT: usize = 10;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_project_status",
        "description": "The current state of one Project (matter) the signed-in caller \
                        participates in: its name/code/status, open and satisfied \
                        deadlines with their authority and source, the most recent \
                        notation-workflow events, and the participation ledger (each \
                        person's participation, whether they are the lawyer DRI, and \
                        whether they are the client DRI). A caller with no participation \
                        row on this Project receives the same not-found response as one \
                        naming a Project that does not exist.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "project_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "Uuid of the Project (matter) to report on."
                }
            },
            "required": ["project_id"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    project_id: Uuid,
}

pub async fn call(
    surreal: &store::surreal::SurrealDb,
    scope: &ReadScope,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;

    // The same predicate the matter surface itself gates on — checked
    // before the project row is ever read, so an unauthorized caller
    // never learns whether the id even exists.
    let permitted = match scope {
        ReadScope::Membership { person_id, role } => {
            store::access::can_see_project(surreal, Some(*person_id), *role, args.project_id)
                .await
                .map_err(ToolError::Database)?
        }
        // The directory lens is metadata-only, and neither an unlinked
        // caller nor an unauthenticated one has a participation boundary
        // to check against. No scope here may reach matter contents.
        ReadScope::Deployment | ReadScope::Directory { .. } | ReadScope::Unlinked => false,
    };
    if !permitted {
        return Err(ToolError::NotFound(format!(
            "project_id={}",
            args.project_id
        )));
    }

    let project = store::projects::find_by_id(surreal, args.project_id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?
        .ok_or_else(|| ToolError::NotFound(format!("project_id={}", args.project_id)))?;

    let deadlines = store::statutory_deadlines::by_project(surreal, args.project_id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?;

    let notations = store::notations::list_by_project(surreal, args.project_id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?;
    let mut events = Vec::new();
    for notation in &notations {
        events.extend(
            store::notation_events::for_notation(surreal, notation.id)
                .await
                .map_err(|error| ToolError::Database(error.to_string()))?,
        );
    }
    // Newest first — `id` is a UUIDv7, the same ordering convention
    // `notation_events::latest_for_kind` uses for "current state".
    events.sort_by_key(|event| std::cmp::Reverse(event.id));
    events.truncate(RECENT_EVENTS_LIMIT);

    let participations = store::projects::participations_for_project(surreal, args.project_id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?;
    let people: HashMap<Uuid, store::persons::Person> = store::persons::find_by_ids(
        surreal,
        &participations
            .iter()
            .map(|row| row.person_id)
            .collect::<Vec<_>>(),
    )
    .await
    .map_err(|error| ToolError::Database(error.to_string()))?
    .into_iter()
    .map(|person| (person.id, person))
    .collect();

    let deadline_rows = deadline_rows(&deadlines);
    let event_rows = event_rows(&events);
    let participation_rows = participation_rows(&participations, &people);

    let summary = [
        format!(
            "{} [{}] (status: {}).",
            project.name, project.code, project.status
        ),
        deadlines_line(&deadlines),
        events_line(&events),
        participation_line(&participations, &people),
    ]
    .join("\n");

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "project_id": project.id,
            "code": project.code,
            "name": project.name,
            "status": project.status,
            "deadlines": deadline_rows,
            "notation_events": event_rows,
            "participation": participation_rows,
        }
    }))
}

fn deadline_rows(deadlines: &[store::statutory_deadlines::StatutoryDeadline]) -> Vec<Value> {
    deadlines
        .iter()
        .map(|deadline| {
            json!({
                "id": deadline.id,
                "kind": deadline.kind,
                "trigger_on": deadline.trigger_on,
                "due_on": deadline.due_on,
                "statute": deadline.statute,
                "source": deadline.source,
                "status": deadline.status,
            })
        })
        .collect()
}

fn event_rows(events: &[store::notation_events::NotationEvent]) -> Vec<Value> {
    events
        .iter()
        .map(|event| {
            json!({
                "id": event.id,
                "notation_id": event.notation_id,
                "machine_kind": event.machine_kind,
                "from_state": event.from_state,
                "to_state": event.to_state,
                "condition": event.condition,
                "recorded_at": event.recorded_at,
            })
        })
        .collect()
}

fn participation_rows(
    participations: &[store::projects::PersonProjectRole],
    people: &HashMap<Uuid, store::persons::Person>,
) -> Vec<Value> {
    participations
        .iter()
        .map(|row| {
            let person = people.get(&row.person_id);
            json!({
                "person_id": row.person_id,
                "name": person.map(|p| p.name.as_str()),
                "email": person.map(|p| p.email.as_str()),
                "participation": row.participation,
                "is_lawyer_dri": row.is_lawyer_dri,
                "is_client_dri": row.is_client_dri,
            })
        })
        .collect()
}

fn deadlines_line(deadlines: &[store::statutory_deadlines::StatutoryDeadline]) -> String {
    if deadlines.is_empty() {
        return "Deadlines: none.".to_string();
    }
    let listed = deadlines
        .iter()
        .map(|d| {
            format!(
                "{} due {} (statute: {}; source: {}; status: {})",
                d.kind, d.due_on, d.statute, d.source, d.status
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Deadlines ({}): {listed}.", deadlines.len())
}

fn events_line(events: &[store::notation_events::NotationEvent]) -> String {
    if events.is_empty() {
        return "Recent notation events: none.".to_string();
    }
    let listed = events
        .iter()
        .map(|e| {
            format!(
                "{} {} -> {} at {}",
                e.machine_kind, e.from_state, e.to_state, e.recorded_at
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Recent notation events ({}): {listed}.", events.len())
}

fn participation_line(
    participations: &[store::projects::PersonProjectRole],
    people: &HashMap<Uuid, store::persons::Person>,
) -> String {
    if participations.is_empty() {
        return "Participation: none.".to_string();
    }
    let listed = participations
        .iter()
        .map(|row| {
            let who = people
                .get(&row.person_id)
                .map_or_else(|| row.person_id.to_string(), |p| p.name.clone());
            let marker = if row.is_lawyer_dri {
                " [lawyer DRI]"
            } else if row.is_client_dri {
                " [client DRI]"
            } else {
                ""
            };
            format!("{who} ({}){marker}", row.participation)
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Participation ({}): {listed}.", participations.len())
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
                code: format!("status-{}", Uuid::now_v7()),
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
                "Lawyer",
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
                    dri: store::participation::DriRequest::Designate(
                        store::projects::DriSide::Lawyer,
                    ),
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

    /// One notation on `project_id`, with one journaled event moving it
    /// `BEGIN -> lawyer_review`.
    async fn notation_with_event(
        surreal: &store::surreal::SurrealDb,
        project_id: Uuid,
        respondent_id: Uuid,
    ) {
        let template = store::templates::save_version(
            surreal,
            None,
            &format!("status-template-{}", Uuid::now_v7()),
            store::templates::Version {
                title: "Status Fixture".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: Some("onboarding".into()),
                source_commit_sha: None,
            },
        )
        .await
        .unwrap()
        .into_model();

        let notation = store::notations::create(
            surreal,
            &store::notations::NewNotation::new(
                template.id,
                respondent_id,
                project_id,
                "lawyer_review",
            ),
        )
        .await
        .unwrap();

        store::notation_events::append_event(
            surreal,
            store::notation_events::TransitionRecord {
                notation_id: notation.id,
                acting_person_id: Some(respondent_id),
                machine_kind: store::notation_events::MACHINE_WORKFLOW,
                from_state: "BEGIN",
                to_state: "lawyer_review",
                condition: "submitted",
                payload_json: None,
                recorded_at: "2026-01-15T00:00:00Z",
            },
        )
        .await
        .unwrap();
    }

    #[test]
    fn descriptor_requires_project_id() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_project_status");
        assert_eq!(d["inputSchema"]["required"], json!(["project_id"]));
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
    }

    /// The issue's first Done-when: a participating lawyer gets deadlines,
    /// recent notation events, and the participation ledger in one call.
    #[tokio::test]
    async fn a_participating_lawyer_receives_deadlines_events_and_participation() {
        let surreal = mem_surreal().await;
        let matter = project(&surreal, "Status Matter").await;
        let person = lawyer(&surreal, Some(matter)).await;
        deadline(&surreal, matter, "triage:status").await;
        notation_with_event(&surreal, matter, person.id).await;

        let result = call(
            &surreal,
            &ReadScope::Membership {
                person_id: person.id,
                role: person.role,
            },
            &json!({ "project_id": matter }),
        )
        .await
        .unwrap();

        assert!(result["structuredContent"]["code"].as_str().is_some());
        assert_eq!(result["structuredContent"]["name"], "Status Matter");
        assert_eq!(result["structuredContent"]["status"], "open");

        let deadlines = result["structuredContent"]["deadlines"].as_array().unwrap();
        assert_eq!(deadlines.len(), 1);
        assert_eq!(deadlines[0]["statute"], "15 U.S.C. § 1681i(a)(1)");

        let events = result["structuredContent"]["notation_events"]
            .as_array()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["from_state"], "BEGIN");
        assert_eq!(events[0]["to_state"], "lawyer_review");

        let participation = result["structuredContent"]["participation"]
            .as_array()
            .unwrap();
        assert_eq!(participation.len(), 1);
        assert_eq!(participation[0]["person_id"], person.id.to_string());
        assert_eq!(participation[0]["participation"], "lawyer");
        assert_eq!(participation[0]["is_lawyer_dri"], true);
        assert_eq!(participation[0]["is_client_dri"], false);

        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Status Matter"));
        assert!(text.contains("15 U.S.C. § 1681i(a)(1)"));
        assert!(text.contains("lawyer_review"));
        assert!(text.contains("lawyer DRI"));
    }

    /// The issue's second Done-when: a non-participant gets nothing, not
    /// even confirmation that the matter exists.
    #[tokio::test]
    async fn a_lawyer_without_participation_is_refused_as_not_found() {
        let surreal = mem_surreal().await;
        let matter = project(&surreal, "Private Matter").await;
        deadline(&surreal, matter, "triage:private").await;
        let person = lawyer(&surreal, None).await;

        let error = call(
            &surreal,
            &ReadScope::Membership {
                person_id: person.id,
                role: person.role,
            },
            &json!({ "project_id": matter }),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, crate::tools::ToolError::NotFound(_)));
    }

    /// A genuinely nonexistent project id is refused identically — the
    /// point being that the two cases cannot be told apart.
    #[tokio::test]
    async fn an_unknown_project_id_is_also_refused_as_not_found() {
        let surreal = mem_surreal().await;
        let person = lawyer(&surreal, None).await;

        let error = call(
            &surreal,
            &ReadScope::Membership {
                person_id: person.id,
                role: person.role,
            },
            &json!({ "project_id": Uuid::now_v7() }),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, crate::tools::ToolError::NotFound(_)));
    }

    /// Owner/Admin's directory lens is metadata-only and an unauthenticated
    /// or unlinked caller has no participation boundary to check — neither
    /// scope may reach matter contents through this tool.
    #[tokio::test]
    async fn no_non_membership_scope_reaches_matter_contents() {
        let surreal = mem_surreal().await;
        let matter = project(&surreal, "Scoped Matter").await;

        for scope in [
            ReadScope::Deployment,
            ReadScope::Directory {
                role: store::persons::Role::Owner,
            },
            ReadScope::Unlinked,
        ] {
            let error = call(&surreal, &scope, &json!({ "project_id": matter }))
                .await
                .unwrap_err();
            assert!(matches!(error, crate::tools::ToolError::NotFound(_)));
        }
    }

    #[tokio::test]
    async fn rejects_unknown_fields_and_missing_project_id() {
        let surreal = mem_surreal().await;
        let person = lawyer(&surreal, None).await;
        let scope = ReadScope::Membership {
            person_id: person.id,
            role: person.role,
        };

        let error = call(&surreal, &scope, &json!({})).await.unwrap_err();
        assert!(matches!(
            error,
            crate::tools::ToolError::InvalidArguments(_)
        ));

        let error = call(
            &surreal,
            &scope,
            &json!({ "project_id": Uuid::now_v7(), "extra": true }),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            crate::tools::ToolError::InvalidArguments(_)
        ));
    }
}
