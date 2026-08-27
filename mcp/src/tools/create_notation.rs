//! `aida_create_notation` MCP tool.
//!
//! Kick off a conversational notation from a template. The tool
//! creates the Notation row, starts the questionnaire runtime,
//! and returns the first question so the LLM can ask the user
//! and then call `aida_answer_notation` with the answer. The
//! server is the sole owner of questionnaire state; the LLM is
//! the UI.
//!
//! A lawyer chooses the existing Project explicitly. When the MCP
//! boundary has populated a [`crate::Principal`], the tool verifies that
//! principal is lawyer and has scope for the chosen Project. The notation's
//! respondent is always the Project's client-side DRI; it is never inferred
//! from the authenticated lawyer.
//!
//! This door opens the notation through the policy-free
//! [`workflows::notation_session::start_notation`] primitive, so the
//! engagement-first rule the `web` and CLI create path applies
//! ([`workflows::notation_session::create_notation_from_repo`]) does NOT
//! constrain AIDA: a lawyer directing the agent may bind any kind as a
//! matter's first notation. Gating this door would forbid AIDA from ever
//! opening a filing or letter on a fresh matter, which is the agent's
//! ordinary use. The authorization that does apply is the project scope
//! check above — the actor must be lawyer and in scope.
//!
//! The principal is absent only in the pass-through dev path (KIND, where
//! no auth layer ran). A deployed environment cannot reach this code
//! without one: `GOOGLE_OAUTH_CLIENT_IDS` is a boot invariant
//! (`store::deployment::WEB_REQUIREMENTS`), so `portal::google_oauth` is
//! always enforced and `portal::mcp_principal` always injects — the scope
//! check below cannot be skipped by a missing env var.

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use workflows::{notation_session, NextStep, NotationSessionError, StateMachineRuntime};

use crate::principal::Principal;

use super::ToolError;

/// Tool descriptor advertised by `tools/list`.
#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_create_notation",
        "description":
            "Start a conversational notation from a template. Looks up the \
             existing Project, creates a Notation for that Project's client \
             DRI, starts \
             the questionnaire state machine, and returns the first question \
             to ask. Reply to the user with the returned `prompt` verbatim; \
             once they answer, call `aida_answer_notation` with \
             `notation_id`, `question_code`, and `value` to advance. \
             Returns `next_question` (with `code`, `prompt`, `answer_type`) \
             while the questionnaire is in progress, or `status: \"complete\"` \
             if the template has no questions. The caller must choose an \
             existing Project; creating a Project is a separate lawyer action.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "template_code": {
                    "type": "string",
                    "description":
                        "Stable template code, e.g. `onboarding__retainer`, \
                         `ca__llc_operating_agreement`. Required."
                },
                "project_id": {
                    "type": "string",
                    "description":
                        "UUID of the existing Project (matter) on which to \
                         start the notation. Required."
                }
            },
            "required": ["template_code", "project_id"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
struct Args {
    template_code: String,
    project_id: Uuid,
}

pub async fn call(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: Option<&std::sync::Arc<dyn cloud::StorageService>>,
    principal: Option<&Principal>,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;

    let template_code = args.template_code.trim();
    if template_code.is_empty() {
        return Err(ToolError::InvalidArguments(
            "`template_code` must not be blank".into(),
        ));
    }

    let project = store::projects::find_by_id(surreal, args.project_id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?
        .ok_or_else(|| ToolError::NotFound(format!("project_id={}", args.project_id)))?;
    if let Some(principal) = principal {
        let actor = resolve_person(surreal, &principal.email).await?;
        if !store::projects::can_access_as_lawyer_in_surreal(
            surreal,
            Some(actor.id),
            actor.role,
            project.id,
        )
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?
        {
            return Err(ToolError::Forbidden(format!(
                "the authenticated lawyer cannot act on project_id={}",
                project.id
            )));
        }
    }

    // The respondent is always a client-side DRI — AIDA cannot name one.
    // Resolved after the authorization check so an unauthorized caller never
    // triggers the lookup.
    //
    // A matter may have several client DRIs, and a notation is addressed to one
    // person, so this picks the **longest-standing** one rather than whichever
    // row the scan returned first: an arbitrary pick would send the same
    // questionnaire to a different client contact on different days. Letting the
    // caller name a respondent among them is a separate decision.
    let mut client_dris: Vec<_> = store::projects::participations_for_project(surreal, project.id)
        .await
        .map_err(|error| ToolError::Database(error.to_string()))?
        .into_iter()
        .filter(|row| row.is_client_dri)
        .collect();
    client_dris.sort_by(|a, b| {
        a.inserted_at
            .cmp(&b.inserted_at)
            .then_with(|| a.person_id.cmp(&b.person_id))
    });
    let client_dri_person_id = client_dris
        .first()
        .map(|row| row.person_id)
        .ok_or_else(|| {
            ToolError::InvalidArguments(format!("project_id={} has no client-side DRI", project.id))
        })?;

    let outcome = notation_session::start_notation(
        surreal,
        runtime,
        storage,
        template_code,
        client_dri_person_id,
        project.id,
        Some(project.entity_id),
    )
    .await
    .map_err(map_notation_err)?;

    let payload = match outcome.next {
        NextStep::NeedsAnswer { question } => json!({
            "notation_id": outcome.notation_id,
            "status": "needs_answer",
            "next_question": {
                "code": question.code,
                "prompt": question.prompt,
                "answer_type": question.answer_type,
            }
        }),
        NextStep::QuestionnaireComplete => json!({
            "notation_id": outcome.notation_id,
            "status": "complete",
        }),
    };

    let summary = match payload["status"].as_str() {
        Some("needs_answer") => format!(
            "Started notation {}. Ask the user: {}",
            outcome.notation_id, payload["next_question"]["prompt"]
        ),
        _ => format!(
            "Started notation {} (template has no questions; ready for workflow).",
            outcome.notation_id
        ),
    };

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": payload,
    }))
}

/// Look up the unique person matching `email` case-insensitively.
async fn resolve_person(
    surreal: &store::surreal::SurrealDb,
    email: &str,
) -> Result<store::persons::Person, ToolError> {
    store::persons::find_by_email_ci(surreal, email)
        .await?
        .ok_or_else(|| ToolError::NotFound(format!("person with email `{email}`")))
}

fn map_notation_err(err: NotationSessionError) -> ToolError {
    match err {
        NotationSessionError::TemplateNotFound(c) => ToolError::NotFound(format!("template `{c}`")),
        NotationSessionError::EngagementMustBeFirst { code, kind } => {
            ToolError::InvalidArguments(format!(
                "the first notation on a matter must be the engagement that opens it — a retainer \
                 or an onboarding — not `{code}` (kind: {kind})"
            ))
        }
        NotationSessionError::TemplateHasNoQuestionnaire(c) => {
            ToolError::InvalidArguments(format!("template `{c}` has no questionnaire to walk"))
        }
        NotationSessionError::NotationNotFound(id) => {
            ToolError::NotFound(format!("notation `{id}`"))
        }
        NotationSessionError::QuestionMismatch { expected, got } => ToolError::InvalidArguments(
            format!("questionnaire is currently asking `{expected}`, not `{got}`"),
        ),
        NotationSessionError::AlreadyComplete => {
            ToolError::InvalidArguments("questionnaire is already complete".into())
        }
        NotationSessionError::Db(e) => ToolError::Database(e),
        // The questionnaire's questions, answers, and the notation itself
        // live in SurrealDB (ENG-121), so a session can now fail in either
        // engine.
        NotationSessionError::Question(e) => ToolError::Internal(e.to_string()),
        NotationSessionError::Template(e) => ToolError::Internal(e.to_string()),
        NotationSessionError::Answer(e) => ToolError::Internal(e.to_string()),
        NotationSessionError::Notation(e) => ToolError::Internal(e.to_string()),
        NotationSessionError::Reask(e) => ToolError::Internal(e.to_string()),
        NotationSessionError::QuestionNotSeeded(c) => {
            ToolError::Internal(format!("question `{c}` not seeded in store"))
        }
        NotationSessionError::QuestionNotClientFacing(c) => {
            ToolError::InvalidArguments(format!("question `{c}` is not a client-facing question"))
        }
        NotationSessionError::QuestionNotFlagged(c) => ToolError::InvalidArguments(format!(
            "question `{c}` was not flagged for re-collection by the lawyer review"
        )),
        NotationSessionError::Runtime(e) => ToolError::Internal(format!("workflow runtime: {e}")),
        NotationSessionError::Spec(e) => ToolError::Internal(format!("spec parse: {e}")),
        NotationSessionError::SnapshotEncode(e) | NotationSessionError::SnapshotDecode(e) => {
            ToolError::Internal(format!("questionnaire snapshot: {e}"))
        }
        NotationSessionError::TemplateSource(e) => {
            ToolError::InvalidArguments(format!("reading template from the project repo: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::principal::Principal;
    use crate::tools::ToolError;
    use serde_json::json;
    use uuid::Uuid;
    use workflows::InMemoryRuntime;

    use store::test_support::mem_surreal;
    async fn db() -> store::surreal::SurrealDb {
        let surreal = mem_surreal().await;
        surreal
    }

    /// Seed the template these tests open, declaring `kind`.
    ///
    /// The code stays `onboarding__retainer` because its questionnaire is
    /// bundled at compile time (`workflows::specs::BUNDLED_SPEC_YAML`), so
    /// it is what resolves with no project repo and no stored body. `kind`
    /// is the axis under test: a matter's first notation may declare any
    /// kind, so a caller passing `filing` here is asserting exactly that.
    async fn seed_template(surreal: &store::surreal::SurrealDb, kind: &str) {
        store::templates::save_version(
            surreal,
            None,
            "onboarding__retainer",
            store::templates::Version {
                title: "Retainer".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: Some(kind.into()),
                source_commit_sha: None,
            },
        )
        .await
        .unwrap();
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new("person", "Who is the person?", "person"),
        )
        .await
        .unwrap();
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new("project", "What is the project?", "project"),
        )
        .await
        .unwrap();
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new("custom_text", "Prompt for custom text", "string"),
        )
        .await
        .unwrap();
    }

    /// Seed a `Role::Admin` person. Admin bypasses project scoping
    /// outright (`store::projects::can_access_as_lawyer`), so this is only
    /// for the bypass test — a scope test seeded with an admin can never
    /// observe a refusal.
    async fn seed_admin(surreal: &store::surreal::SurrealDb) -> Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Firm Principal",
                "principal@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await
        .unwrap()
        .id
    }

    /// Seed a `Role::Lawyer` person — the tier project scoping actually
    /// constrains. Lawyer reach a matter only through a firm-side participation
    /// row; the `lawyer_dri` participation names the accountable lawyer.
    async fn seed_lawyer(surreal: &store::surreal::SurrealDb, email: &str) -> Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(email, email, store::persons::Role::Lawyer),
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_person(surreal: &store::surreal::SurrealDb, email: &str) -> Uuid {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(email, email, store::persons::Role::Client),
        )
        .await
        .unwrap()
        .id
    }

    async fn seed_project(
        surreal: &store::surreal::SurrealDb,
        client_dri: Uuid,
        lawyer_dri: Option<Uuid>,
    ) -> Uuid {
        let project = store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: format!("notation-matter-{}", Uuid::now_v7()),
                name: "Matter".into(),
                status: "open".into(),
                entity_id: store::test_support::seed_entity(surreal).await,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::projects::designate_dri_in_surreal(
            surreal,
            project.id,
            client_dri,
            store::projects::DriSide::Client,
        )
        .await
        .unwrap();
        if let Some(lawyer_dri) = lawyer_dri {
            link_lawyer_dri(surreal, lawyer_dri, project.id).await;
        }
        project.id
    }

    async fn link_lawyer_dri(
        surreal: &store::surreal::SurrealDb,
        person_id: Uuid,
        project_id: Uuid,
    ) {
        store::projects::designate_dri_in_surreal(
            surreal,
            project_id,
            person_id,
            store::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();
    }

    async fn storage() -> std::sync::Arc<dyn cloud::StorageService> {
        std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-mcp-create-notation"))
                .await
                .unwrap(),
        )
    }

    #[test]
    fn descriptor_names_the_tool_under_aida_namespace() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_create_notation");
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        let required: Vec<&str> = d["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"template_code"));
        assert!(required.contains(&"project_id"));
        assert!(!required.contains(&"person_email"));
    }

    #[tokio::test]
    async fn creates_a_notation_on_an_explicit_project() {
        // The lawyer names the matter; AIDA opens no Project of its own.
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let project_id = seed_project(&surreal, client, None).await;
        let runtime = InMemoryRuntime::new();
        let storage = storage().await;

        let out = call(
            &surreal,
            &runtime,
            Some(&storage),
            None,
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap();

        assert_eq!(out["structuredContent"]["status"], "needs_answer");
        assert!(out["structuredContent"]["notation_id"].is_string());
        assert_eq!(
            out["structuredContent"]["next_question"]["code"],
            "person__client"
        );
    }

    #[tokio::test]
    async fn a_filing_may_open_a_matter_through_aida() {
        // AIDA is lawyer-directed and opens the notation through the
        // policy-free primitive, so the engagement-first rule that governs
        // the `web`/CLI create path does not apply here: a filing on a
        // fresh matter is the agent's ordinary use, not an error.
        let surreal = db().await;
        seed_template(&surreal, "filing").await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let project_id = seed_project(&surreal, client, None).await;
        let runtime = InMemoryRuntime::new();
        let storage = storage().await;

        let out = call(
            &surreal,
            &runtime,
            Some(&storage),
            None,
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap();
        assert_eq!(out["structuredContent"]["status"], "needs_answer");
    }

    #[tokio::test]
    async fn authenticated_lawyer_acts_for_the_projects_client_dri() {
        // A `Role::Lawyer` actor is in scope through the disclosed lawyer-DRI
        // participation — not admin, so the scope check admits this call.
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let lawyer = seed_lawyer(&surreal, "dri@example.com").await;
        let project_id = seed_project(&surreal, client, Some(lawyer)).await;
        link_lawyer_dri(&surreal, lawyer, project_id).await;
        let runtime = InMemoryRuntime::new();
        let principal = Principal::new("dri@example.com");
        let storage = storage().await;

        let out = call(
            &surreal,
            &runtime,
            Some(&storage),
            Some(&principal),
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap();
        assert_eq!(out["structuredContent"]["status"], "needs_answer");

        // The authenticated lawyer is the actor, not the respondent.
        let row = store::notations::find_by_id(
            &surreal,
            serde_json::from_value::<Uuid>(out["structuredContent"]["notation_id"].clone())
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(row.person_id, client);
    }

    #[tokio::test]
    async fn authenticated_principal_with_unknown_email_is_not_found() {
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let project_id = seed_project(&surreal, client, None).await;
        let runtime = InMemoryRuntime::new();
        let principal = Principal::new("ghost@example.com");
        let storage = storage().await;
        let err = call(
            &surreal,
            &runtime,
            Some(&storage),
            Some(&principal),
            &json!({ "template_code": "onboarding__retainer", "project_id": project_id }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::NotFound(m) => assert!(m.contains("person")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn authenticated_lawyer_outside_project_scope_is_forbidden() {
        // The actor is `Role::Lawyer` and the matter is someone else's: not
        // has no firm-side participation row. Seeding an admin here would
        // prove nothing — admin bypasses scoping entirely.
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let dri = seed_lawyer(&surreal, "dri@example.com").await;
        let _outsider = seed_lawyer(&surreal, "outsider@example.com").await;
        let project_id = seed_project(&surreal, client, Some(dri)).await;
        let runtime = InMemoryRuntime::new();
        let storage = storage().await;
        let err = call(
            &surreal,
            &runtime,
            Some(&storage),
            Some(&Principal::new("outsider@example.com")),
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)));
    }

    #[tokio::test]
    async fn an_admin_acts_on_a_matter_they_have_no_participation_on() {
        // Admin deliberately bypasses project scoping (see
        // `docs/access-model.md`); the matter names a different lawyer DRI
        // and the admin still opens the notation.
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let dri = seed_lawyer(&surreal, "dri@example.com").await;
        let _admin = seed_admin(&surreal).await;
        let project_id = seed_project(&surreal, client, Some(dri)).await;
        let runtime = InMemoryRuntime::new();
        let storage = storage().await;

        let out = call(
            &surreal,
            &runtime,
            Some(&storage),
            Some(&Principal::new("principal@example.com")),
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap();
        assert_eq!(out["structuredContent"]["status"], "needs_answer");
    }

    #[tokio::test]
    async fn project_must_exist() {
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let runtime = InMemoryRuntime::new();
        let err = call(
            &surreal,
            &runtime,
            None,
            None,
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": Uuid::nil(),
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
    }

    #[tokio::test]
    async fn unknown_template_is_not_found() {
        let surreal = db().await;
        let client = seed_person(&surreal, "libra@example.com").await;
        let project_id = seed_project(&surreal, client, None).await;
        let runtime = InMemoryRuntime::new();
        let storage = storage().await;
        let err = call(
            &surreal,
            &runtime,
            Some(&storage),
            None,
            &json!({
                "template_code": "does_not_exist",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::NotFound(m) => assert!(m.contains("template")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn project_without_client_dri_is_rejected() {
        let surreal = db().await;
        seed_template(&surreal, "onboarding").await;
        let entity_id = store::test_support::seed_entity(&surreal).await;
        let project_id = store::projects::create(
            &surreal,
            &store::projects::NewProject {
                code: format!("unassigned-matter-{}", Uuid::now_v7()),
                name: "Unassigned matter".into(),
                status: "open".into(),
                entity_id,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .id;
        let runtime = InMemoryRuntime::new();
        let err = call(
            &surreal,
            &runtime,
            None,
            None,
            &json!({
                "template_code": "onboarding__retainer",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("client-side DRI")),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_project_id_is_invalid_arguments() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let err = call(
            &surreal,
            &runtime,
            None,
            None,
            &json!({ "template_code": "onboarding__retainer" }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => assert!(m.contains("project_id")),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn blank_template_code_is_invalid_arguments() {
        let surreal = db().await;
        let runtime = InMemoryRuntime::new();
        let err = call(
            &surreal,
            &runtime,
            None,
            None,
            &json!({ "template_code": "  ", "project_id": Uuid::nil() }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
