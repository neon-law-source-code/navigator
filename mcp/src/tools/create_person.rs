//! `aida_create_person` MCP tool.
//!
//! `LibreChat` asks the LLM to call this tool when a user (a Neon Law
//! attorney, lawyer, or admin chatting through `LibreChat`) wants to
//! register a new human contact. The handler delegates to the shared
//! `store::people_commands::create_person` command — the same write the
//! `/app/api/people` REST route and the lawyer form travel — and returns the
//! new id + name + email so the model can confirm what landed. Every Neon
//! Law Navigator tool is namespaced under the `aida_` prefix so clients
//! can group them in their UI.

use serde::Deserialize;
use serde_json::{json, Value};
use store::people_commands::{create_person, CreatePersonCommand};

use super::ToolError;

/// Tool descriptor advertised by `tools/list`. The `inputSchema` is a
/// standard JSON Schema; `LibreChat` surfaces it to the model as the
/// function signature.
#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_create_person",
        "description": "Create a NEW person record in Neon Law Navigator. Use this ONLY when \
                        the user explicitly asks to add or register a new contact, \
                        client, prospect, or lawyer. Do NOT call this to look up, \
                        message, email, or welcome someone — a request that mentions an \
                        email address is not a request to create a person. To find or \
                        act on an existing person, call aida_show_person first. Returns \
                        the new id, name, and email so the caller can reference the row.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Full name of the person (e.g. \"Libra\")."
                },
                "email": {
                    "type": "string",
                    "format": "email",
                    "description": "Email address. Must be unique across all persons."
                }
            },
            "required": ["name", "email"],
            "additionalProperties": false
        }
    })
}

/// The MCP tool contract: `name` + `email` only. Decoding into this narrow
/// struct — rather than straight into `CreatePersonCommand` — is what keeps
/// a smuggled `role` (or a structured legal-name part) out of the tool.
/// Extra JSON keys are tolerated and ignored, matching the descriptor's
/// advisory `additionalProperties: false`.
#[derive(Debug, Deserialize)]
struct Args {
    name: String,
    email: String,
}

/// Create a Person through the shared command boundary and return the MCP
/// `result` payload. Validation, trimming, and the duplicate-email conflict
/// all live in `store::people_commands`, so this tool stays a thin adapter.
///
/// The role is pinned to the `client` default and the structured legal-name
/// parts are left unset: this machine-facing tool exposes only `name`/`email`
/// and has no `may_change_roles` gate, so it must not let a direct or
/// hallucinated call set `role: "admin"` the way the cookie/CSRF-gated
/// `/app/api/people` route can for an authorized admin.
pub async fn call(
    surreal: &store::surreal::SurrealDb,
    arguments: &Value,
) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;
    let command = CreatePersonCommand {
        name: args.name,
        email: args.email,
        role: String::new(),
        given_name: None,
        family_name: None,
        middle_name: None,
        notion_user_id: None,
    };
    let inserted = create_person(surreal, &command).await?;

    Ok(json!({
        "content": [{
            "type": "text",
            "text": format!(
                "Created person id={} ({} <{}>).",
                inserted.id, inserted.name, inserted.email
            )
        }],
        "structuredContent": {
            "id": inserted.id,
            "name": inserted.name,
            "email": inserted.email
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::ToolError;
    use serde_json::json;

    /// `aida_create_person` writes only `persons`, so its tests need only
    /// the engine that owns the table.
    async fn surreal() -> store::surreal::SurrealDb {
        store::test_support::mem_surreal().await
    }

    #[test]
    fn descriptor_names_the_tool_and_requires_name_and_email() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_create_person");
        let required = d["inputSchema"]["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"name"));
        assert!(names.contains(&"email"));
        // The schema must lock down extras so the model can't sneak in
        // unknown fields that we'd silently ignore.
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
    }

    #[tokio::test]
    async fn happy_path_inserts_and_returns_structured_content() {
        let surreal = surreal().await;
        let result = call(
            &surreal,
            &json!({ "name": "Libra", "email": "libra@example.com" }),
        )
        .await
        .unwrap();

        assert_eq!(result["structuredContent"]["name"], "Libra");
        assert_eq!(result["structuredContent"]["email"], "libra@example.com");
        // `id` is rendered as a UUID hex string in JSON.
        let id = result["structuredContent"]["id"].as_str().unwrap();
        uuid::Uuid::parse_str(id).expect("id is a valid UUID");

        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Libra"));
        assert!(text.contains("libra@example.com"));

        let all = store::persons::list_directory(&surreal, "", "", &[])
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].email, "libra@example.com");
        // The command defaults a role-less create to `client`.
        assert_eq!(all[0].role, store::persons::Role::Client);
    }

    #[tokio::test]
    async fn trims_surrounding_whitespace_on_name_and_email() {
        let surreal = surreal().await;
        let result = call(
            &surreal,
            &json!({ "name": "  Libra ", "email": "  libra@example.com\n" }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["name"], "Libra");
        assert_eq!(result["structuredContent"]["email"], "libra@example.com");
    }

    #[tokio::test]
    async fn missing_name_field_is_invalid_arguments() {
        let surreal = surreal().await;
        // A missing `name` fails the command's required-field validation.
        let err = call(&surreal, &json!({ "email": "libra@example.com" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn missing_email_field_is_invalid_arguments() {
        let surreal = surreal().await;
        let err = call(&surreal, &json!({ "name": "Libra" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn empty_name_is_invalid_arguments() {
        let surreal = surreal().await;
        let err = call(
            &surreal,
            &json!({ "name": "   ", "email": "libra@example.com" }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn at_less_email_is_invalid_arguments() {
        let surreal = surreal().await;
        let err = call(&surreal, &json!({ "name": "Libra", "email": "not-email" }))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn smuggled_role_is_ignored_and_person_stays_client() {
        // The tool exposes only name/email and has no may_change_roles
        // gate, so a direct/hallucinated call that smuggles role="admin"
        // (or a structured legal-name part) must not escalate — the row
        // lands as a plain client.
        let surreal = surreal().await;
        call(
            &surreal,
            &json!({
                "name": "Sneaky",
                "email": "sneaky@example.com",
                "role": "admin",
                "given_name": "Should",
                "family_name": "Ignore"
            }),
        )
        .await
        .unwrap();
        let all = store::persons::list_directory(&surreal, "", "", &[])
            .await
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].role, store::persons::Role::Client);
        assert!(all[0].given_name.is_none());
        assert!(all[0].family_name.is_none());
    }

    #[tokio::test]
    async fn duplicate_email_surfaces_a_conflict_error() {
        let surreal = surreal().await;
        call(
            &surreal,
            &json!({ "name": "Libra", "email": "dup@example.com" }),
        )
        .await
        .unwrap();
        let err = call(
            &surreal,
            &json!({ "name": "Other", "email": "dup@example.com" }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ToolError::Conflict(_)),
            "expected Conflict, got {err:?}"
        );
    }

    #[tokio::test]
    async fn distinct_emails_can_coexist() {
        let surreal = surreal().await;
        call(
            &surreal,
            &json!({ "name": "Libra", "email": "libra@example.com" }),
        )
        .await
        .unwrap();
        call(
            &surreal,
            &json!({ "name": "Taurus", "email": "taurus@example.com" }),
        )
        .await
        .unwrap();
        let all = store::persons::list_directory(&surreal, "", "", &[])
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn extra_field_is_tolerated() {
        // The schema declares additionalProperties=false for the model,
        // but serde tolerates extras at decode time; this pins that an
        // unknown key doesn't break the call.
        let surreal = surreal().await;
        let result = call(
            &surreal,
            &json!({ "name": "Libra", "email": "libra@example.com", "extra": "ignored" }),
        )
        .await;
        assert!(result.is_ok());
    }
}
