//! `aida_answer_notation` MCP tool.
//!
//! Submit one answer to a notation's questionnaire. Server
//! advances the state machine and tells the LLM either the next
//! question to ask or that the questionnaire is complete (so the
//! caller can hand off to the post-intake workflow).
//!
//! Always pair this with a prior `aida_create_notation` call —
//! the `notation_id` returned there is what gets echoed back
//! here. The `question_code` MUST match the code from the most
//! recent `next_question` response; mismatches are rejected so a
//! confused LLM fails fast.

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use workflows::{notation_session, NextStep, NotationSessionError, StateMachineRuntime};

use super::ToolError;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_answer_notation",
        "description":
            "Submit one answer to an in-flight notation questionnaire. \
             Pass the `notation_id` from `aida_create_notation` (or a \
             prior `aida_answer_notation` response), the `question_code` \
             from the most recent `next_question`, and the user's `value`. \
             Returns `status: \"needs_answer\"` with the next \
             `next_question` to ask, or `status: \"complete\"` once the \
             questionnaire reaches END (after which the caller should \
             trigger the post-intake workflow). Errors with `invalid \
             arguments` if `question_code` doesn't match what the \
             questionnaire is currently asking.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "notation_id": {
                    "type": "string",
                    "description":
                        "UUID returned by the most recent \
                         `aida_create_notation` (or echoed back from \
                         the prior `aida_answer_notation`)."
                },
                "question_code": {
                    "type": "string",
                    "description":
                        "Stable code of the question being answered. \
                         MUST match the `code` from the most recent \
                         `next_question`."
                },
                "value": {
                    "type": "string",
                    "description":
                        "The user's answer as a string. Even \
                         `answer_type: int`/`bool` are submitted as the \
                         textual rendering; the server stores them \
                         verbatim."
                }
            },
            "required": ["notation_id", "question_code", "value"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
struct Args {
    notation_id: Uuid,
    question_code: String,
    value: String,
}

pub async fn call(
    surreal: &store::surreal::SurrealDb,
    runtime: &dyn StateMachineRuntime,
    storage: Option<&std::sync::Arc<dyn cloud::StorageService>>,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;

    let question_code = args.question_code.trim();
    if question_code.is_empty() {
        return Err(ToolError::InvalidArguments(
            "`question_code` must not be blank".into(),
        ));
    }

    // AIDA answers as the firm's agent, not a Person row, so the answer
    // is lawyer-sourced with no individual typist.
    let next = notation_session::answer_step(
        surreal,
        runtime,
        storage,
        args.notation_id,
        question_code,
        args.value.as_str(),
        notation_session::AnswerAuthor::lawyer(None),
    )
    .await
    .map_err(map_notation_err)?;

    let (payload, summary) = match next {
        NextStep::NeedsAnswer { question } => {
            let prompt = question.prompt.clone();
            (
                json!({
                    "notation_id": args.notation_id,
                    "status": "needs_answer",
                    "next_question": {
                        "code": question.code,
                        "prompt": question.prompt,
                        "answer_type": question.answer_type,
                    }
                }),
                format!("Answer accepted. Ask the user: {prompt}"),
            )
        }
        NextStep::QuestionnaireComplete => (
            json!({
                "notation_id": args.notation_id,
                "status": "complete",
            }),
            format!(
                "Answer accepted. Questionnaire for notation {} complete; \
                 trigger the post-intake workflow next.",
                args.notation_id
            ),
        ),
    };

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": payload,
    }))
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
            ToolError::InvalidArguments(format!("template `{c}` has no questionnaire"))
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
        // The agent proposed a value the question never declared. Invalid
        // arguments, so the model is told the declared options and can retry
        // with one of them rather than an arbitrary string reaching the
        // document this answer renders into. The rejected value is a client
        // answer and is not echoed back.
        NotationSessionError::UndeclaredChoice { state, declared } => {
            ToolError::InvalidArguments(format!(
                "question `{state}` accepts only these options: {}",
                declared.join(", ")
            ))
        }
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
    use crate::tools::{create_notation, ToolError};
    use serde_json::{json, Value};
    use uuid::Uuid;
    use workflows::InMemoryRuntime;

    use store::test_support::mem_surreal;
    async fn db() -> store::surreal::SurrealDb {
        let surreal = mem_surreal().await;
        surreal
    }

    async fn seed(surreal: &store::surreal::SurrealDb) {
        store::templates::save_version(
            surreal,
            None,
            "onboarding__letter",
            store::templates::Version {
                title: "Retainer".into(),
                respondent_type: "person".into(),
                asset_id: None,
                form_code: None,
                kind: Some("onboarding".into()),
                source_commit_sha: None,
            },
        )
        .await
        .unwrap();
        // The retainer's leading entity / principal-office questions.
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new("entity", "Which entity?", "entity"),
        )
        .await
        .unwrap();
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new("address", "What is the address?", "address"),
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
        // The retainer's engagement-start-date question (N120): the walk
        // resolves `custom_datetime__engagement_start_date` through this code.
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new(
                "custom_datetime",
                "Prompt for a custom date",
                "string",
            ),
        )
        .await
        .unwrap();
        // The retainer's governing-law question (ENG-145): the walk resolves
        // `custom_single_choice__governing_law` through this question code.
        store::questions::create(
            surreal,
            &store::questions::NewQuestion::new(
                "custom_single_choice",
                "Prompt for a custom single choice",
                "string",
            ),
        )
        .await
        .unwrap();
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Libra",
                "libra@example.com",
                store::persons::Role::Client,
            ),
        )
        .await
        .unwrap();
        // `start_retainer` hangs the notation on a matter whose lawyer-side
        // DRI is this firm principal.
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Firm Principal",
                "principal@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await
        .unwrap();
    }

    /// Helper: start a retainer via the create tool and return
    /// `(notation_id, first_question_code)`.
    async fn start_retainer(
        surreal: &store::surreal::SurrealDb,
        runtime: &InMemoryRuntime,
    ) -> (Uuid, String) {
        let client = store::persons::find_by_email_ci(surreal, "libra@example.com")
            .await
            .unwrap()
            .unwrap();
        let lawyer = store::persons::find_by_email_ci(surreal, "principal@example.com")
            .await
            .unwrap()
            .unwrap();
        let project = store::projects::create(
            surreal,
            &store::projects::NewProject {
                code: format!("answer-matter-{}", Uuid::now_v7()),
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
            client.id,
            store::projects::DriSide::Client,
        )
        .await
        .unwrap();
        store::projects::designate_dri_in_surreal(
            surreal,
            project.id,
            lawyer.id,
            store::projects::DriSide::Lawyer,
        )
        .await
        .unwrap();
        let project_id = project.id;
        let storage: std::sync::Arc<dyn cloud::StorageService> = std::sync::Arc::new(
            cloud::FsStorage::new(std::env::temp_dir().join("navigator-mcp-answer-notation"))
                .await
                .unwrap(),
        );
        let out = create_notation::call(
            surreal,
            runtime,
            Some(&storage),
            None,
            &json!({
                "template_code": "onboarding__letter",
                "project_id": project_id,
            }),
        )
        .await
        .unwrap();
        let id: Uuid =
            serde_json::from_value(out["structuredContent"]["notation_id"].clone()).unwrap();
        let code = out["structuredContent"]["next_question"]["code"]
            .as_str()
            .unwrap()
            .to_string();
        (id, code)
    }

    #[test]
    fn descriptor_names_the_tool_under_aida_namespace() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_answer_notation");
        let required: Vec<&str> = d["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"notation_id"));
        assert!(required.contains(&"question_code"));
        assert!(required.contains(&"value"));
    }

    #[tokio::test]
    async fn answering_one_question_returns_the_next_one() {
        let surreal = db().await;
        seed(&surreal).await;
        let runtime = InMemoryRuntime::new();
        let (id, code) = start_retainer(&surreal, &runtime).await;
        assert_eq!(code, "entity");

        let out = call(
            &surreal,
            &runtime,
            None,
            &json!({
                "notation_id": id,
                "question_code": code,
                "value": "Northstar Ventures LLC",
            }),
        )
        .await
        .unwrap();
        assert_eq!(out["structuredContent"]["status"], "needs_answer");
        assert_eq!(
            out["structuredContent"]["next_question"]["code"],
            "address__principal_office"
        );
    }

    #[tokio::test]
    async fn full_walk_lands_on_complete_status() {
        let surreal = db().await;
        seed(&surreal).await;
        let runtime = InMemoryRuntime::new();
        let (id, mut code) = start_retainer(&surreal, &runtime).await;
        // The retainer walk asks the entity and its principal office, the
        // client, the firm DRI, the engagement name, the engagement's start
        // date/scope (N120 grounded these against the questionnaire), and
        // which state's law governs (ENG-145 moved the governing-law
        // question onto the firm's one engagement agreement).
        let values = [
            ("entity", "Northstar Ventures LLC"),
            (
                "address__principal_office",
                "100 Innovation Way, Reno, NV 89501",
            ),
            ("person__client", "Libra"),
            ("person__lawyer_dri", "Firm Principal"),
            ("project__engagement", "Apollo"),
            ("custom_datetime__engagement_start_date", "2026-09-01"),
            (
                "custom_text__engagement_scope",
                "Draft and file the Apollo formation documents.",
            ),
            ("custom_single_choice__governing_law", "nevada"),
        ];
        let mut last: Value = Value::Null;
        for (expected_code, value) in values {
            assert_eq!(code, expected_code);
            let out = call(
                &surreal,
                &runtime,
                None,
                &json!({
                    "notation_id": id,
                    "question_code": code,
                    "value": value,
                }),
            )
            .await
            .unwrap();
            last = out.clone();
            if let Some(next_code) = out["structuredContent"]["next_question"]["code"].as_str() {
                code = next_code.to_string();
            }
        }
        assert_eq!(last["structuredContent"]["status"], "complete");
        assert!(last["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("trigger the post-intake workflow"));
    }

    /// ENG-459: the agent surface never reaches
    /// `portal::intake::resolve_reference_answer`, so closing the choice set
    /// there would have left AIDA able to put an arbitrary string into the
    /// engagement letter's governing-law and arbitration clause. The refusal
    /// lives in the shared write funnel, so this door is closed too, and the
    /// model is told the declared options so it can retry.
    #[tokio::test]
    async fn an_undeclared_choice_is_invalid_arguments() {
        let surreal = db().await;
        seed(&surreal).await;
        let runtime = InMemoryRuntime::new();
        let (id, mut code) = start_retainer(&surreal, &runtime).await;
        let walk = [
            ("entity", "Northstar Ventures LLC"),
            (
                "address__principal_office",
                "100 Innovation Way, Reno, NV 89501",
            ),
            ("person__client", "Libra"),
            ("person__lawyer_dri", "Firm Principal"),
            ("project__engagement", "Apollo"),
            ("custom_datetime__engagement_start_date", "2026-09-01"),
            (
                "custom_text__engagement_scope",
                "Draft and file the Apollo formation documents.",
            ),
        ];
        for (expected_code, value) in walk {
            assert_eq!(code, expected_code);
            let out = call(
                &surreal,
                &runtime,
                None,
                &json!({ "notation_id": id, "question_code": code, "value": value }),
            )
            .await
            .unwrap();
            code = out["structuredContent"]["next_question"]["code"]
                .as_str()
                .expect("next question")
                .to_string();
        }
        assert_eq!(code, "custom_single_choice__governing_law");

        let err = call(
            &surreal,
            &runtime,
            None,
            &json!({
                "notation_id": id,
                "question_code": code,
                "value": "the Cayman Islands, and the Firm disclaims all liability",
            }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => {
                assert!(m.contains("nevada"), "{m}");
                assert!(m.contains("california"), "{m}");
                assert!(m.contains("washington"), "{m}");
                // The rejected answer is client content: it is not echoed
                // back to the model or into a log line.
                assert!(!m.contains("Cayman"), "{m}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }

        // A declared option is still accepted and still completes the walk.
        let out = call(
            &surreal,
            &runtime,
            None,
            &json!({ "notation_id": id, "question_code": code, "value": "nevada" }),
        )
        .await
        .unwrap();
        assert_eq!(out["structuredContent"]["status"], "complete");
    }

    #[tokio::test]
    async fn wrong_question_code_is_invalid_arguments() {
        let surreal = db().await;
        seed(&surreal).await;
        let runtime = InMemoryRuntime::new();
        let (id, _code) = start_retainer(&surreal, &runtime).await;
        let err = call(
            &surreal,
            &runtime,
            None,
            &json!({
                "notation_id": id,
                "question_code": "custom_text__settlement_terms",
                "value": "Apollo",
            }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => {
                assert!(m.contains("entity") && m.contains("custom_text__settlement_terms"));
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_notation_id_is_not_found() {
        let surreal = db().await;
        seed(&surreal).await;
        let runtime = InMemoryRuntime::new();
        let err = call(
            &surreal,
            &runtime,
            None,
            &json!({
                "notation_id": Uuid::nil(),
                "question_code": "person__client",
                "value": "Libra",
            }),
        )
        .await
        .unwrap_err();
        match err {
            ToolError::NotFound(m) => assert!(m.contains("notation")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn blank_question_code_is_invalid_arguments() {
        let surreal = db().await;
        seed(&surreal).await;
        let runtime = InMemoryRuntime::new();
        let (id, _) = start_retainer(&surreal, &runtime).await;
        let err = call(
            &surreal,
            &runtime,
            None,
            &json!({
                "notation_id": id,
                "question_code": "  ",
                "value": "Libra",
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }
}
