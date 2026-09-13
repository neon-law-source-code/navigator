//! `bulk_import` MCP tool.
//!
//! Unlike most of the catalog (one row per call, the LLM loops), this
//! tool takes a whole document — a list of organizations and the people
//! who work at them — and find-or-creates `entities`, `persons`, and
//! the links between them in one shot. The unit of work is the document,
//! the same shape `create_notation` already accepts. All the logic
//! lives in the shared `import` crate so the `cli import-contacts`
//! subcommand and a future `web` upload route run the exact same engine.
//!
//! Writing clients into the system of record is privileged: the call is
//! refused unless the authenticated [`Principal`] resolves to a
//! lawyer-or-admin `persons` row. The contract and the LLM-generation
//! instructions live in
//! [`docs/bulk-contact-import.md`](../../../docs/bulk-contact-import.md).

use serde_json::{json, Value};

use super::ToolError;
use crate::principal::Principal;

/// Tool descriptor advertised by `tools/list`.
#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "bulk_import",
        "description": "Bulk-import organizations and the people who work at them into \
                        Neon Law Navigator. Find-or-creates an entity per organization, a person per \
                        contact, and a client_contact link between them. Idempotent: re-running \
                        the same payload changes nothing. Lawyer/admin only. Use this when a user \
                        hands you a list of contacts to load. Returns a per-row created/updated/ \
                        unchanged/failed report.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "version": { "type": "integer", "description": "Contract version. Must be 1." },
                "source": {
                    "type": "string",
                    "description": "Free-text provenance (e.g. \"partner-outreach-2026-06\"). \
                                    Recorded in telemetry, not stored on a row."
                },
                "organizations": {
                    "type": "array",
                    "description": "The organizations to create as entities.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "key": { "type": "string", "description": "Stable in-file id the people reference." },
                            "name": { "type": "string" },
                            "entity_type": { "type": "string", "description": "e.g. \"501(c)(3) Non-Profit\". Must already exist." },
                            "jurisdiction": { "type": "string", "description": "Two-letter code, e.g. \"WA\"." },
                            "phone": { "type": "string" },
                            "url": { "type": "string", "description": "Website URL; canonicalized to https on the way in." }
                        },
                        "required": ["key", "name", "entity_type", "jurisdiction"]
                    }
                },
                "people": {
                    "type": "array",
                    "description": "The people to create as persons, each linked to one organization.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "key": { "type": "string" },
                            "name": { "type": "string" },
                            "email": { "type": "string", "format": "email", "description": "Unique upsert key." },
                            "title": { "type": "string" },
                            "phone": { "type": "string" },
                            "organization": { "type": "string", "description": "A key from this payload's organizations." },
                            "entity_role": { "type": "string", "description": "Link role; defaults to client_contact." }
                        },
                        "required": ["key", "name", "email", "organization"]
                    }
                }
            },
            "required": ["organizations", "people"]
        }
    })
}

/// Validate the caller's tier, then apply the payload via the shared
/// engine and return a per-row report.
pub async fn call(
    surreal: &store::surreal::SurrealDb,
    principal: Option<&Principal>,
    arguments: &Value,
) -> Result<Value, ToolError> {
    require_lawyer(surreal, principal).await?;

    let payload: import::Payload = super::decode_args(arguments)?;
    let report = import::apply(surreal, &payload)
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?;

    let structured = serde_json::to_value(&report)
        .map_err(|e| ToolError::Internal(format!("serialize report: {e}")))?;

    // The tally alone ("0 created … 0 failed") reads as a silent
    // non-result to a client that renders only the text Part and drops
    // `structuredContent` — which is exactly what Gemini Enterprise does.
    // When anything went wrong, fold the diagnostics and per-row reasons
    // into the text so the caller sees *why*. See
    // [`docs/mcp-a2a-interaction.md`](../../../docs/mcp-a2a-interaction.md).
    let text = match report.problem_lines() {
        Some(problems) => format!(
            "Bulk import: {}.\n\nProblems:\n{problems}",
            report.summary()
        ),
        None => format!("Bulk import: {}.", report.summary()),
    };

    Ok(json!({
        "content": [{
            "type": "text",
            "text": text
        }],
        "structuredContent": structured
    }))
}

/// Resolve the principal to a `persons` row and require a lawyer-or-admin
/// tier. Writing clients into the system of record is not something an
/// anonymous or client-tier caller may do.
async fn require_lawyer(
    surreal: &store::surreal::SurrealDb,
    principal: Option<&Principal>,
) -> Result<(), ToolError> {
    let email = principal
        .map(|p| p.email.trim())
        .filter(|e| !e.is_empty())
        .ok_or_else(|| {
            ToolError::Forbidden("bulk import requires an authenticated caller".into())
        })?;

    match store::persons::find_by_email_ci(surreal, email).await? {
        Some(p) if p.role.is_lawyer_tier() => Ok(()),
        Some(_) => Err(ToolError::Forbidden(format!(
            "{email} is not lawyer or admin; bulk import is restricted"
        ))),
        None => Err(ToolError::Forbidden(format!(
            "{email} has no Neon Law Navigator account; bulk import is restricted"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::principal::Principal;
    use crate::tools::ToolError;
    use serde_json::json;

    use store::test_support::mem_surreal;
    async fn db_with_refs() -> store::surreal::SurrealDb {
        let surreal = mem_surreal().await;
        store::entity_types::create(&surreal, "501(c)(3) Non-Profit")
            .await
            .unwrap();
        store::jurisdictions::create(
            &surreal,
            &store::jurisdictions::NewJurisdiction::new("Washington", "WA", "state"),
        )
        .await
        .unwrap();
        surreal
    }

    async fn seed_lawyer(surreal: &store::surreal::SurrealDb, email: &str) {
        store::persons::create(
            surreal,
            &store::persons::NewPerson::with_role(
                "Lawyer",
                email.to_string(),
                store::persons::Role::Lawyer,
            ),
        )
        .await
        .unwrap();
    }

    fn payload() -> serde_json::Value {
        json!({
            "version": 1,
            "organizations": [
                { "key": "ejp", "name": "Example Justice Project",
                  "entity_type": "501(c)(3) Non-Profit", "jurisdiction": "WA",
                  "url": "http://Justice.example/?ref=x" }
            ],
            "people": [
                { "key": "ada", "name": "Ada Counsel", "email": "acounsel@justice.example",
                  "title": "Executive Director", "organization": "ejp" }
            ]
        })
    }

    #[test]
    fn descriptor_names_the_tool_and_requires_orgs_and_people() {
        let d = descriptor();
        assert_eq!(d["name"], "bulk_import");
        let required = d["inputSchema"]["required"].as_array().unwrap();
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"organizations"));
        assert!(names.contains(&"people"));
    }

    #[tokio::test]
    async fn lawyer_caller_imports_and_canonicalizes() {
        let surreal = db_with_refs().await;
        seed_lawyer(&surreal, "lawyer@neonlaw.com").await;
        let principal = Principal::new("lawyer@neonlaw.com");

        let result = call(&surreal, Some(&principal), &payload()).await.unwrap();
        assert_eq!(
            result["structuredContent"]["organizations"][0]["status"],
            "created"
        );
        assert_eq!(
            result["structuredContent"]["people"][0]["status"],
            "created"
        );

        // The org landed with a canonicalized URL, and the link exists.
        let ejp = store::entities::find_by_name(&surreal, "Example Justice Project")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ejp.url.as_deref(), Some("https://justice.example"));
        let links = store::entity_roles::all(&surreal).await.unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].role, "client_contact");
    }

    #[tokio::test]
    async fn validation_reject_explains_why_in_the_text_content() {
        // The silent-failure case the user hit: a structurally invalid
        // payload (unsupported version) writes nothing. The tally alone
        // would read as a message-less non-result; the text Part must
        // carry the diagnostic so a text-only client (Gemini Enterprise)
        // surfaces the reason.
        let surreal = db_with_refs().await;
        seed_lawyer(&surreal, "lawyer@neonlaw.com").await;
        let principal = Principal::new("lawyer@neonlaw.com");
        let mut bad = payload();
        bad["version"] = json!(2);

        let result = call(&surreal, Some(&principal), &bad).await.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Problems:"), "no problems block: {text}");
        assert!(
            text.contains("unsupported contract version"),
            "diagnostic not surfaced: {text}"
        );
    }

    #[tokio::test]
    async fn failed_row_reason_reaches_the_text_content() {
        // An unknown jurisdiction passes structural validation (ZZ is
        // two letters) but fails at apply time as a per-row failure. The
        // reason must reach the rendered text, not just structuredContent.
        let surreal = db_with_refs().await;
        seed_lawyer(&surreal, "lawyer@neonlaw.com").await;
        let principal = Principal::new("lawyer@neonlaw.com");
        let bad = json!({
            "version": 1,
            "organizations": [
                { "key": "ejp", "name": "Example Justice Project",
                  "entity_type": "501(c)(3) Non-Profit", "jurisdiction": "ZZ" }
            ],
            "people": [
                { "key": "ada", "name": "Ada Counsel",
                  "email": "acounsel@justice.example", "organization": "ejp" }
            ]
        });

        let result = call(&surreal, Some(&principal), &bad).await.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Problems:"), "no problems block: {text}");
        assert!(
            text.contains("unknown jurisdiction"),
            "row failure reason not surfaced: {text}"
        );
        // The structured detail is still there for programmatic clients.
        assert_eq!(
            result["structuredContent"]["organizations"][0]["status"],
            "failed"
        );
    }

    #[tokio::test]
    async fn anonymous_caller_is_forbidden() {
        let surreal = db_with_refs().await;
        let err = call(&surreal, None, &payload()).await.unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)));
        // Nothing was written.
        assert!(store::entities::all(&surreal).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn client_tier_caller_is_forbidden() {
        let surreal = db_with_refs().await;
        // A client-tier person (the default role) may not bulk-import.
        store::persons::create(
            &surreal,
            &store::persons::NewPerson::new("Aries", "aries@example.com"),
        )
        .await
        .unwrap();
        let principal = Principal::new("aries@example.com");
        let err = call(&surreal, Some(&principal), &payload())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)));
    }
}
