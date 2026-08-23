//! Web adapter for the People command boundary.
//!
//! The DTOs, validation, and the create/update/delete writes live in
//! `store::people_commands` so `web`, `cli`, and `mcp` share one command
//! boundary without duplicating persistence. This module re-exports that
//! surface for the JSON `/app/api/people*` routes and the browser lawyer forms.
//!
//! `send_welcome` — the one command that needs the mailer — lives in
//! `workflows::email::welcome` and is re-exported here. It moved down a
//! crate so the `aida_send_welcome_email` MCP tool can reach the same
//! command: `portal` depends on `mcp`, so a command defined here was one
//! the agent door structurally could not call, and it grew its own path
//! instead (ENG-317).

pub use store::people_commands::{
    create_person, delete_person, parse_role, update_person, CreatePersonCommand,
    PeopleCommandError, UpdateContext, UpdatePersonCommand,
};
pub use workflows::email::welcome::send_welcome;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use store::test_support::mem_surreal;
    use workflows::email::CapturingEmail;
    use workflows::{EmailService, InMemoryRuntime};

    use crate::email::LoggingEmail;

    async fn seed(surreal: &store::surreal::SurrealDb, name: &str, email: &str) -> uuid::Uuid {
        store::persons::create(surreal, &store::persons::NewPerson::new(name, email))
            .await
            .unwrap()
            .id
    }

    /// Both doors onto the welcome email go through one command, and the
    /// property that proves it is the `sent_emails` audit row.
    ///
    /// This is the guard ENG-317 exists for. Before it, the API route
    /// called `send_welcome` (synchronous, through the
    /// `LoggingEmail`-wrapped service, so one row per attempt) while the
    /// MCP tool called `trigger_welcome` (the Restate worker, which runs
    /// a deliberately bare backend with no `LoggingEmail`). The agent
    /// door therefore sent mail that the audit table never recorded.
    ///
    /// The test fails if either door grows a second path: route the tool
    /// back through the worker and its row disappears; change the
    /// command under the API route and its row changes shape.
    #[tokio::test]
    async fn both_doors_write_the_same_audit_row_through_one_command() {
        let surreal = mem_surreal().await;
        let mailer: Arc<dyn EmailService> = Arc::new(LoggingEmail::new(
            Arc::new(CapturingEmail::new()),
            surreal.clone(),
            "support@example.com",
        ));

        // Door 1: the JSON API route's command, called exactly as
        // `portal::api::send_welcome` calls it.
        let api_person = seed(&surreal, "Api Recipient", "api@example.com").await;
        super::send_welcome(&surreal, mailer.as_ref(), "http://localhost", api_person)
            .await
            .unwrap();

        // Door 2: the MCP tool, holding the same mailer `web` injects.
        let mut mcp_state = mcp::McpState::new(surreal.clone(), Arc::new(InMemoryRuntime::new()));
        mcp_state.email = Some(mailer.clone());
        let tool_person = seed(&surreal, "Agent Recipient", "agent@example.com").await;
        mcp::tools::aida_send_welcome_email::call(
            &mcp_state,
            &serde_json::json!({ "person_id": tool_person }),
        )
        .await
        .unwrap();

        let rows = store::sent_emails::all(&surreal).await.unwrap();
        assert_eq!(rows.len(), 2, "each door must journal exactly one row");
        for recipient in ["api@example.com", "agent@example.com"] {
            let row = rows
                .iter()
                .find(|r| r.recipient == recipient)
                .unwrap_or_else(|| panic!("no sent_emails row for {recipient}; got {rows:?}"));
            assert_eq!(row.template_slug.as_deref(), Some("welcome"));
            assert_eq!(row.outcome, "sent");
        }

        // Same subject from the same render path, so a copy change lands
        // on both doors at once.
        assert_eq!(rows[0].subject, rows[1].subject);
    }
}
