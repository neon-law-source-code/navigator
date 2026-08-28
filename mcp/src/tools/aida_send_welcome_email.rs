//! `aida_send_welcome_email` MCP tool.
//!
//! Re-fires the firm's welcome email at an existing person — the same
//! "Welcome to Neon Law" message the `/app/admin/people/{id}` "Send welcome"
//! button and `POST /app/api/people/{id}/welcome` send.
//!
//! All three go through **one command**,
//! [`workflows::email::welcome::send_welcome`]. That matters more here
//! than ordinary de-duplication would suggest: this is one of the three
//! tools `requires_confirmation` classifies as a supervised act, so a
//! behaviour that lands on the command — a suppression rule, a bounce
//! record, an audit line — must reach the agent door too. Until ENG-317
//! this tool called `trigger_welcome` and did its own person lookup, and
//! the concrete cost was the audit row: the Restate worker runs a bare
//! backend with no `LoggingEmail`, so an agent-initiated welcome left no
//! `sent_emails` row while the other two doors wrote one.
//!
//! **Trust boundary (per the council's Scorpio note):** the tool takes
//! a `person_id`, never a free-text email address. You can only welcome
//! someone already seeded in `persons`, so AIDA can't be turned into a
//! sender for arbitrary inboxes. Unknown id → `NotFound`. The name and
//! email are read from the row inside the command, so the model can
//! neither spoof who the greeting names nor where it lands.
//!
//! The send is synchronous, which is the other thing convergence
//! bought: the tool now reports whether the mail actually went out
//! rather than whether a workflow started.

use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use workflows::email::welcome::{send_welcome, welcome_subject};

use super::ToolError;
use crate::server::McpState;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_send_welcome_email",
        "description": "Send the firm's \"Welcome to Neon Law\" email to an existing \
                        person. This is the correct tool for any \"send/email a welcome\" \
                        request, even when the user names the recipient only by email \
                        address. Identify the recipient by their Neon Law Navigator person_id: \
                        when you were given an email or name instead, call aida_show_person \
                        FIRST to resolve the person_id, then call this — do NOT create a \
                        new person. The email and name are read from that record, so you \
                        can only welcome someone already in the system, never an arbitrary \
                        address. Each call sends: calling it twice emails the person \
                        twice, so do not retry on your own initiative.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "person_id": {
                    "type": "string",
                    "format": "uuid",
                    "description": "UUID of the person to welcome. Must already exist \
                                    in Neon Law Navigator (see aida_show_person)."
                }
            },
            "required": ["person_id"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
struct Args {
    person_id: Uuid,
}

pub async fn call(state: &McpState, arguments: &Value) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;

    // No mailer means no `LoggingEmail`, and an unaudited send of a
    // supervised act is worse than no send. Refuse rather than fall back
    // to a path that leaves no `sent_emails` row.
    let email = state.email.as_ref().ok_or_else(|| {
        ToolError::Internal("no email service is configured for this deployment".into())
    })?;

    // The one command every door goes through. It reads the recipient
    // from the `persons` row itself, so the name and email are never the
    // caller's to choose, and it writes the audit row on the way out.
    let person = send_welcome(
        &state.surreal,
        email.as_ref(),
        &workflows::email::base_url_from_env(),
        args.person_id,
    )
    .await
    .map_err(|e| match e {
        store::people_commands::PeopleCommandError::NotFound => {
            ToolError::NotFound(format!("person_id={}", args.person_id))
        }
        other => ToolError::Internal(other.user_message()),
    })?;

    let summary = format!(
        "Sent the welcome email to {} <{}> (id={}).",
        person.name, person.email, person.id
    );
    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "person_id": person.id,
            "name": person.name,
            "email": person.email,
            "subject": welcome_subject(),
            "status": "sent",
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::server::McpState;
    use crate::tools::ToolError;
    use serde_json::json;
    use std::sync::Arc;
    use uuid::Uuid;
    use workflows::InMemoryRuntime;

    use store::test_support::mem_surreal;
    use workflows::email::CapturingEmail;

    /// State carrying a capturing mailer, standing in for the
    /// `LoggingEmail`-wrapped service `web` injects. The audit row that
    /// decorator writes is asserted in `portal`, where the decorator
    /// lives; here the assertion is that the tool hands the command a
    /// message tagged so that row is attributable.
    async fn state_with_mailer() -> (McpState, Arc<CapturingEmail>) {
        let surreal = mem_surreal().await;
        let runtime: Arc<dyn workflows::StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
        let mut st = McpState::new(surreal, runtime);
        let mailer = Arc::new(CapturingEmail::new());
        st.email = Some(mailer.clone());
        (st, mailer)
    }

    async fn state() -> McpState {
        state_with_mailer().await.0
    }

    async fn seed_person(surreal: &store::surreal::SurrealDb, name: &str, email: &str) -> Uuid {
        store::persons::create(surreal, &store::persons::NewPerson::new(name, email))
            .await
            .unwrap()
            .id
    }

    #[test]
    fn descriptor_names_the_tool_and_requires_person_id() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_send_welcome_email");
        let required = d["inputSchema"]["required"].as_array().unwrap();
        assert_eq!(required, &vec![json!("person_id")]);
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
    }

    #[tokio::test]
    async fn happy_path_sends_through_the_command_and_returns_sent_status() {
        let (state, mailer) = state_with_mailer().await;
        let pid = seed_person(&state.surreal, "Aries", "aries@example.com").await;
        let r = call(&state, &json!({ "person_id": pid })).await.unwrap();

        // The message the shared command built — tagged with the template
        // slug and the person id, which is what makes the `sent_emails`
        // row the mailer's decorator writes attributable to this person.
        let sent = mailer.captured();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, "aries@example.com");
        assert_eq!(sent[0].template_slug.as_deref(), Some("welcome"));
        assert_eq!(sent[0].person_id.as_deref(), Some(pid.to_string().as_str()));
        assert!(
            sent[0].html_body.is_some(),
            "the HTML alternative must be set"
        );

        assert_eq!(r["structuredContent"]["person_id"], pid.to_string());
        assert_eq!(r["structuredContent"]["name"], "Aries");
        assert_eq!(r["structuredContent"]["email"], "aries@example.com");
        assert_eq!(r["structuredContent"]["subject"], "Welcome to Neon Law");
        assert_eq!(r["structuredContent"]["status"], "sent");
        assert!(r["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("aries@example.com"));
    }

    #[tokio::test]
    async fn name_and_email_come_from_the_row_not_the_caller() {
        // The schema forbids extra fields, but even if a caller smuggles
        // an `email`, the greeting must target the DB row's address.
        let state = state().await;
        let pid = seed_person(&state.surreal, "Real Person", "real@example.com").await;
        let r = call(
            &state,
            &json!({ "person_id": pid, "email": "attacker@evil.test" }),
        )
        .await
        .unwrap();
        assert_eq!(r["structuredContent"]["email"], "real@example.com");
    }

    #[tokio::test]
    async fn unknown_person_returns_not_found_and_sends_nothing() {
        let (state, mailer) = state_with_mailer().await;
        let missing = Uuid::now_v7();
        let err = call(&state, &json!({ "person_id": missing }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound(_)));
        assert!(mailer.captured().is_empty());
    }

    #[tokio::test]
    async fn a_deployment_with_no_mailer_refuses_rather_than_sending_unaudited() {
        let surreal = mem_surreal().await;
        let runtime: Arc<dyn workflows::StateMachineRuntime> = Arc::new(InMemoryRuntime::new());
        let state = McpState::new(surreal, runtime);
        let pid = seed_person(&state.surreal, "Aries", "aries@example.com").await;
        let err = call(&state, &json!({ "person_id": pid }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Internal(_)));
    }

    #[tokio::test]
    async fn missing_person_id_is_invalid_arguments() {
        let state = state().await;
        let err = call(&state, &json!({})).await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    /// The guard ENG-317 exists to hold: this tool reaches the mailer
    /// through the shared command, not through the Restate trigger. If a
    /// future edit reintroduces `trigger_welcome` here, no message lands
    /// in the injected service and this fails.
    #[tokio::test]
    async fn the_tool_sends_through_the_injected_mailer_not_the_workflow_trigger() {
        let (state, mailer) = state_with_mailer().await;
        let pid = seed_person(&state.surreal, "Libra", "libra@example.com").await;
        call(&state, &json!({ "person_id": pid })).await.unwrap();
        assert_eq!(
            mailer.captured().len(),
            1,
            "the welcome must go through the injected EmailService — a send that \
             leaves it empty went via the worker, which writes no sent_emails row"
        );
    }
}
