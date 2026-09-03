//! `aida_list_projects` MCP tool.
//!
//! Scoped to the caller's own matters. Which matters those are is the
//! caller's [`ReadScope`], and the two authenticated lenses answer
//! genuinely different questions, so they return different shapes:
//!
//! - **Membership** (`lawyer`, `clerk`, `client`) — the matters the caller
//!   holds a participation row on, through
//!   [`store::access::visible_projects`], with the resolved Entity name and
//!   the `project_id` the write tools take. This is the same predicate
//!   `/app/projects` renders from.
//! - **Directory** (`owner`, `admin`) — oversight over every matter: code,
//!   name, status, and the lawyer DRIs, through
//!   [`store::projects::matter_directory`]. No `project_id`, deliberately:
//!   the lens says a matter exists and who owns it, and nothing a matter
//!   contains.
//!
//! An unauthenticated caller — KIND, local dev, the browser harness —
//! keeps the deployment-wide list this tool has always returned.
//!
//! Sorted by `name`. No pagination: the matter set for a single practice
//! stays small enough to ship in one response.

use serde_json::{json, Value};

use super::{ReadScope, ToolError};

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_list_projects",
        "description": "List the Projects (matters) the signed-in caller may see. A firm or \
                        client participant gets the matters they are on, each with id, name, \
                        status, brand (which house brand's storefront the matter was opened \
                        through), and the bound Entity's id and name when one is attached — use \
                        this to pick a Project by name (e.g. \"ShookEstate\") before linking a \
                        Person or attaching a Notation. An owner or admin gets the oversight \
                        directory instead: every matter's code, name, status, and lawyer DRIs, \
                        with no id, because that lens shows which matters exist and who is \
                        accountable for each without reaching what any matter contains. The \
                        `lens` field on the response says which one you received. Takes no \
                        arguments.",
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
    let rows = match scope {
        // Oversight is a different question with a different answer, so it
        // returns before the membership projection below rather than
        // filtering it.
        ReadScope::Directory { role } => return directory(surreal, *role).await,
        // An authenticated email with no `persons` row is scoped to nothing.
        // `visible_projects` would answer the same way for a `None`
        // `person_id`, but there is no role to ask it with.
        ReadScope::Unlinked => Vec::new(),
        ReadScope::Membership { person_id, role } => {
            store::access::visible_projects(surreal, Some(*person_id), *role)
                .await
                .map_err(ToolError::Database)?
        }
        ReadScope::Deployment => store::projects::all(surreal)
            .await
            .map_err(|error| ToolError::Database(error.to_string()))?,
    };
    let entities = store::entities::all(surreal).await?;

    let entity_name = |id: uuid::Uuid| {
        entities
            .iter()
            .find(|e| e.id == id)
            .map_or("(unknown)", |e| e.name.as_str())
    };

    let projects: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "name": row.name,
                "status": row.status,
                "brand": row.brand,
                "entity_id": row.entity_id,
                "entity_name": entity_name(row.entity_id),
            })
        })
        .collect();

    let summary = if rows.is_empty() {
        match scope {
            // Literally true only here. Under a lens the database's
            // contents are not what was asked, so saying it is empty
            // would be a claim this read never made.
            ReadScope::Deployment => "No projects in the database.".to_string(),
            _ => "No projects you participate in.".to_string(),
        }
    } else {
        let listed = rows
            .iter()
            .map(|r| format!("{} ({}, {})", r.name, r.status, entity_name(r.entity_id)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} projects: {listed}.", rows.len())
    };

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "lens": "membership",
            "count": projects.len(),
            "projects": projects,
        }
    }))
}

/// The Owner/Admin oversight directory over every matter.
///
/// Four fields per matter and no `project_id` — see
/// [`store::projects::MatterDirectoryEntry`] for why the handle is the
/// unique `code`. `lawyer_dris` is a list because a matter may name more
/// than one accountable lawyer, and empty when it names none; an
/// unassigned matter is what this lens exists to surface, so the summary
/// says so in words rather than leaving the model to read an empty array.
async fn directory(
    surreal: &store::surreal::SurrealDb,
    role: store::persons::Role,
) -> Result<Value, ToolError> {
    let entries = store::projects::matter_directory(surreal, role)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?;

    let summary = if entries.is_empty() {
        "No projects in the database.".to_string()
    } else {
        let listed = entries
            .iter()
            .map(|e| {
                let dris = if e.lawyer_dris.is_empty() {
                    "unassigned".to_string()
                } else {
                    e.lawyer_dris.join(" & ")
                };
                format!("{} [{}] ({}, DRI {dris})", e.name, e.code, e.status)
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{} projects, directory lens: {listed}. This lens carries no matter contents.",
            entries.len()
        )
    };

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "lens": "directory",
            "count": entries.len(),
            "projects": entries,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::ReadScope;
    use serde_json::json;
    use uuid::Uuid;

    use store::test_support::mem_surreal;
    async fn db() -> store::surreal::SurrealDb {
        let surreal = mem_surreal().await;
        surreal
    }

    async fn seed_entity(surreal: &store::surreal::SurrealDb, name: &str) -> Uuid {
        // The engine does not validate a `record<>` link and nothing in
        // this tool resolves either reference, so the fixture points them
        // at rows it never writes.
        let jur_id = Uuid::now_v7();
        let et_id = Uuid::now_v7();
        store::entities::create(
            surreal,
            &store::entities::NewEntity {
                name: name.into(),
                entity_type_id: et_id,
                jurisdiction_id: jur_id,
                phone: None,
                url: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_project(
        surreal: &store::surreal::SurrealDb,
        name: &str,
        status: &str,
        entity_id: Option<Uuid>,
    ) -> Uuid {
        // projects.entity_id is NOT NULL: open against the given entity, or
        // a fresh throwaway one when the test doesn't care which.
        let entity_id = match entity_id {
            Some(e) => e,
            None => store::test_support::seed_entity(surreal).await,
        };
        store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: format!("test-{}", Uuid::now_v7()),
                name: name.into(),
                status: status.into(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    #[test]
    fn descriptor_names_the_tool_and_takes_no_arguments() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_list_projects");
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        let props = d["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.is_empty());
    }

    #[tokio::test]
    async fn empty_database_returns_zero_count_not_an_error() {
        let surreal = db().await;
        let r = call(&surreal, &ReadScope::Deployment, &json!({}))
            .await
            .unwrap();
        assert_eq!(r["structuredContent"]["count"], 0);
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("No projects"));
    }

    #[tokio::test]
    async fn returns_seeded_projects_sorted_by_name() {
        let surreal = db().await;
        seed_project(&surreal, "Zeta Settlement", "open", None).await;
        seed_project(&surreal, "Alpha Matter", "open", None).await;
        let r = call(&surreal, &ReadScope::Deployment, &json!({}))
            .await
            .unwrap();
        let names: Vec<&str> = r["structuredContent"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Alpha Matter", "Zeta Settlement"]);
    }

    #[tokio::test]
    async fn row_carries_status_and_bound_entity_name() {
        let surreal = db().await;
        let eid = seed_entity(&surreal, "shook.family").await;
        seed_project(&surreal, "ShookEstate", "open", Some(eid)).await;
        let r = call(&surreal, &ReadScope::Deployment, &json!({}))
            .await
            .unwrap();
        let row = &r["structuredContent"]["projects"][0];
        assert_eq!(row["name"], "ShookEstate");
        assert_eq!(row["status"], "open");
        assert_eq!(row["entity_id"], eid.to_string());
        assert_eq!(row["entity_name"], "shook.family");
    }

    #[tokio::test]
    async fn summary_lists_status_and_bound_entity() {
        let surreal = db().await;
        let eid = seed_entity(&surreal, "shook.family").await;
        seed_project(&surreal, "ShookEstate", "open", Some(eid)).await;
        seed_project(&surreal, "Sison", "closed", Some(eid)).await;
        let r = call(&surreal, &ReadScope::Deployment, &json!({}))
            .await
            .unwrap();
        let text = r["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("2 projects:"));
        assert!(text.contains("ShookEstate (open, shook.family)"));
        assert!(text.contains("Sison (closed, shook.family)"));
    }

    /// A person at `tier`, put on `project_id` when one is given.
    ///
    /// The participation word is never chosen here — `add_participant`
    /// derives it from the person's tier, which is the only way one is
    /// written.
    async fn participant(
        surreal: &store::surreal::SurrealDb,
        email: &str,
        tier: store::persons::Role,
        project_id: Option<Uuid>,
        dri: store::participation::DriRequest,
    ) -> store::persons::Person {
        let person = store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(email, email, tier),
        )
        .await
        .unwrap();
        if let Some(project_id) = project_id {
            store::participation::add_participant(
                surreal,
                &store::participation::AddParticipantCommand {
                    project_id,
                    person_id: person.id,
                    dri,
                    actor: store::participation::DriActor::System,
                },
            )
            .await
            .unwrap();
        }
        person
    }

    fn membership(person: &store::persons::Person) -> ReadScope {
        ReadScope::Membership {
            person_id: person.id,
            role: person.role,
        }
    }

    fn names(result: &serde_json::Value) -> Vec<String> {
        result["structuredContent"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// The issue's first Done-when: a lawyer sees the matter they are on
    /// and not the one they are not.
    #[tokio::test]
    async fn a_lawyer_receives_only_the_matter_they_participate_in() {
        let surreal = db().await;
        let matter_a = seed_project(&surreal, "Matter A", "open", None).await;
        seed_project(&surreal, "Matter B", "open", None).await;
        let lawyer = participant(
            &surreal,
            "lawyer@example.com",
            store::persons::Role::Lawyer,
            Some(matter_a),
            store::participation::DriRequest::Unchanged,
        )
        .await;

        let r = call(&surreal, &membership(&lawyer), &json!({}))
            .await
            .unwrap();
        assert_eq!(r["structuredContent"]["lens"], "membership");
        assert_eq!(names(&r), vec!["Matter A"]);
        // The unscoped read returned both, so the filter is what is being
        // observed here rather than a database that only holds one row.
        let unscoped = call(&surreal, &ReadScope::Deployment, &json!({}))
            .await
            .unwrap();
        assert_eq!(names(&unscoped), vec!["Matter A", "Matter B"]);
    }

    /// A client-side row is a different lens, not a narrower firm one: a
    /// lawyer's firm-side matter must not appear in a client's list.
    #[tokio::test]
    async fn a_client_receives_only_their_own_client_side_matter() {
        let surreal = db().await;
        let theirs = seed_project(&surreal, "Their Matter", "open", None).await;
        let other = seed_project(&surreal, "Someone Else", "open", None).await;
        let client = participant(
            &surreal,
            "client@example.com",
            store::persons::Role::Client,
            Some(theirs),
            store::participation::DriRequest::Unchanged,
        )
        .await;
        participant(
            &surreal,
            "lawyer@example.com",
            store::persons::Role::Lawyer,
            Some(other),
            store::participation::DriRequest::Unchanged,
        )
        .await;

        let r = call(&surreal, &membership(&client), &json!({}))
            .await
            .unwrap();
        assert_eq!(names(&r), vec!["Their Matter"]);
    }

    /// A firm tier with no participation row anywhere sees nothing. There
    /// is no privileged bypass on the membership lens — that is what the
    /// directory lens exists for, and a `lawyer` does not get it.
    #[tokio::test]
    async fn a_lawyer_on_no_matter_receives_an_empty_list() {
        let surreal = db().await;
        seed_project(&surreal, "Matter A", "open", None).await;
        let lawyer = participant(
            &surreal,
            "unassigned@example.com",
            store::persons::Role::Lawyer,
            None,
            store::participation::DriRequest::Unchanged,
        )
        .await;

        let r = call(&surreal, &membership(&lawyer), &json!({}))
            .await
            .unwrap();
        assert_eq!(r["structuredContent"]["count"], 0);
        // The database is not empty; their scope is. Saying otherwise
        // would be a claim this read never made.
        let text = r["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "No projects you participate in.");
    }

    /// The issue's second Done-when, for both admin-tier roles: every
    /// matter, four columns, no `project_id` to reach contents with.
    #[tokio::test]
    async fn owner_and_admin_receive_the_directory_over_every_matter() {
        for tier in [store::persons::Role::Owner, store::persons::Role::Admin] {
            let surreal = db().await;
            let eid = seed_entity(&surreal, "shook.family").await;
            let matter_a = seed_project(&surreal, "Matter A", "open", Some(eid)).await;
            seed_project(&surreal, "Matter B", "closed", Some(eid)).await;
            // Accountability on one matter and not the other, so the
            // unassigned case is observed rather than assumed.
            participant(
                &surreal,
                "dri@example.com",
                store::persons::Role::Lawyer,
                Some(matter_a),
                store::participation::DriRequest::Designate(store::projects::DriSide::Lawyer),
            )
            .await;
            // The oversight caller holds no participation row at all.
            let overseer = participant(
                &surreal,
                "overseer@example.com",
                tier,
                None,
                store::participation::DriRequest::Unchanged,
            )
            .await;

            let r = call(
                &surreal,
                &ReadScope::Directory {
                    role: overseer.role,
                },
                &json!({}),
            )
            .await
            .unwrap();
            assert_eq!(r["structuredContent"]["lens"], "directory", "{tier:?}");
            assert_eq!(r["structuredContent"]["count"], 2, "{tier:?}");
            let rows = r["structuredContent"]["projects"].as_array().unwrap();
            let a = rows.iter().find(|p| p["name"] == "Matter A").unwrap();
            assert_eq!(a["status"], "open");
            assert!(a["code"].is_string(), "the handle is the unique code: {a}");
            assert_eq!(a["lawyer_dris"][0], "dri@example.com");
            // No id, deliberately: the lens says a matter exists and who
            // owns it, and carries nothing a matter contains.
            assert!(a["id"].is_null(), "the directory lens carries no id: {a}");
            assert!(a["entity_id"].is_null(), "{a}");
            let b = rows.iter().find(|p| p["name"] == "Matter B").unwrap();
            assert_eq!(b["status"], "closed");
            assert_eq!(
                b["lawyer_dris"].as_array().unwrap().len(),
                0,
                "an unassigned matter is what this lens exists to surface: {b}"
            );
            let text = r["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("DRI unassigned"), "got: {text}");
            assert!(text.contains("no matter contents"), "got: {text}");
        }
    }

    /// An authenticated email with no `persons` row reaches no matter.
    /// Fail-closed with no privileged exception.
    #[tokio::test]
    async fn an_unlinked_caller_receives_nothing() {
        let surreal = db().await;
        seed_project(&surreal, "Matter A", "open", None).await;
        let r = call(&surreal, &ReadScope::Unlinked, &json!({}))
            .await
            .unwrap();
        assert_eq!(r["structuredContent"]["count"], 0);
        assert_eq!(r["structuredContent"]["lens"], "membership");
        let text = r["content"][0]["text"].as_str().unwrap();
        assert_eq!(text, "No projects you participate in.");
    }

    #[tokio::test]
    async fn ignores_arguments_silently() {
        let surreal = db().await;
        seed_project(&surreal, "Sison", "open", None).await;
        let r = call(&surreal, &ReadScope::Deployment, &json!({ "garbage": 42 }))
            .await
            .unwrap();
        assert_eq!(r["structuredContent"]["count"], 1);
    }
}
