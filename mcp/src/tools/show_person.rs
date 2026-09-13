//! `show_person` MCP tool.
//!
//! Fuzzy-find people by name and/or email. Both fields are matched
//! case-insensitively as substrings (`LOWER(col) LIKE '%needle%'`),
//! so the model can call this with whatever fragment the user
//! mentions ("libra", "@neonlaw.com", "ari") and get back every row
//! that matches. When both fields are supplied they are combined
//! with a logical AND. Returns the full list of matches so the
//! model can disambiguate in a follow-up turn. Zero matches is a
//! successful empty result (`count: 0`), not an error — fuzzy
//! searches succeed even when they find nothing.
//!
//! The directory it searches is the firm's own, so the caller has to be
//! on the firm's side of it — see [`ReadScope::reads_firm_directory`]. A
//! client or clerk caller is refused rather than answered with an empty
//! list: nothing they could type would make the read succeed, and saying
//! "no matches" would teach the model the directory is empty.

use serde::Deserialize;
use serde_json::{json, Value};
use store::people_commands::search_people;

use super::{ReadScope, ToolError};

/// Cap on the number of rows returned in a single response. Keeps the
/// MCP payload bounded when the model passes an overly broad needle
/// (e.g. a single letter). Picked to be generous for human-scale
/// directories without flooding the model's context.
const MAX_RESULTS: u64 = 50;

/// Tool descriptor advertised by `tools/list`.
#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "show_person",
        "description": "Fuzzy-find people in Neon Law Navigator by name and/or email. \
                        Both fields are matched case-insensitively as substrings, \
                        so partial fragments (\"libra\", \"@neonlaw.com\") work. \
                        At least one of `name` or `email` is required; when both \
                        are given they are ANDed. Returns up to 50 matches with \
                        id, name, email, role, and oidc_subject.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Substring of the person's name. Case-insensitive."
                },
                "email": {
                    "type": "string",
                    "description": "Substring of the person's email. Case-insensitive."
                }
            },
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
struct Args {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/// Run the fuzzy search and return the MCP `result` payload.
pub async fn call(
    surreal: &store::surreal::SurrealDb,
    scope: &ReadScope,
    arguments: &Value,
) -> Result<Value, ToolError> {
    if !scope.reads_firm_directory() {
        return Err(ToolError::Forbidden(
            "the people directory is firm-side; this caller's tier does not reach it".into(),
        ));
    }
    let args: Args = super::decode_args(arguments)?;

    let name = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let email = args
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if name.is_none() && email.is_none() {
        return Err(ToolError::InvalidArguments(
            "provide at least one of `name` or `email`".into(),
        ));
    }

    // The `LIKE` predicate, ordering, and cap live in the shared People
    // command boundary; this tool owns only the arg validation above and
    // the model-facing summary below.
    let rows = search_people(surreal, name, email, MAX_RESULTS).await?;

    let persons: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "name": row.name,
                "email": row.email,
                "role": row.role.as_str(),
                "oidc_subject": row.oidc_subject,
            })
        })
        .collect();

    let summary = match rows.len() {
        0 => format!("No people matched {}.", describe_needle(name, email)),
        1 => format!(
            "Found 1 person: {} <{}> (id={}).",
            rows[0].name, rows[0].email, rows[0].id
        ),
        n => {
            let listed = rows
                .iter()
                .map(|r| format!("{} <{}>", r.name, r.email))
                .collect::<Vec<_>>()
                .join(", ");
            format!("Found {n} people: {listed}.")
        }
    };

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "count": persons.len(),
            "persons": persons,
        }
    }))
}

fn describe_needle(name: Option<&str>, email: Option<&str>) -> String {
    match (name, email) {
        (Some(n), Some(e)) => format!("name~={n} AND email~={e}"),
        (Some(n), None) => format!("name~={n}"),
        (None, Some(e)) => format!("email~={e}"),
        (None, None) => "<unknown>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor, MAX_RESULTS};
    use crate::tools::{ReadScope, ToolError};
    use serde_json::json;

    /// `show_person` reads only `persons`, so its tests need only
    /// the engine that owns the table.
    async fn surreal() -> store::surreal::SurrealDb {
        store::test_support::mem_surreal().await
    }

    async fn seed(
        surreal: &store::surreal::SurrealDb,
        name: &str,
        email: &str,
    ) -> store::persons::Person {
        store::persons::create(surreal, &store::persons::NewPerson::new(name, email))
            .await
            .unwrap()
    }

    #[test]
    fn descriptor_names_the_tool_under_tool_namespace() {
        let d = descriptor();
        assert_eq!(d["name"], "show_person");
        // Neither key is hard-required at the schema level — the
        // handler enforces "at least one" so the model gets a clearer
        // error than a JSON-Schema mismatch.
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        assert!(d["inputSchema"]["properties"]["name"].is_object());
        assert!(d["inputSchema"]["properties"]["email"].is_object());
    }

    #[tokio::test]
    async fn fuzzy_name_match_is_case_insensitive_and_substring() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@example.com").await;
        // Lowercased fragment of the *middle* of the name.
        let result = call(&surreal, &ReadScope::Deployment, &json!({ "name": "ibr" }))
            .await
            .unwrap();
        assert_eq!(result["structuredContent"]["count"], 1);
        assert_eq!(result["structuredContent"]["persons"][0]["name"], "Libra");
    }

    #[tokio::test]
    async fn fuzzy_email_match_works_on_domain_fragment() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@neonlaw.com").await;
        seed(&surreal, "Taurus", "taurus@example.com").await;
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "email": "neonlaw" }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["count"], 1);
        assert_eq!(
            result["structuredContent"]["persons"][0]["email"],
            "libra@neonlaw.com"
        );
    }

    #[tokio::test]
    async fn name_match_returns_multiple_rows_sorted_by_name() {
        let surreal = surreal().await;
        // Insert out of alphabetical order so we can prove the sort.
        // All three signs share the substring "ari".
        seed(&surreal, "Sagittarius", "sagittarius@example.com").await;
        seed(&surreal, "Aquarius", "aquarius@example.com").await;
        seed(&surreal, "Aries", "aries@example.com").await;
        let result = call(&surreal, &ReadScope::Deployment, &json!({ "name": "ari" }))
            .await
            .unwrap();
        assert_eq!(result["structuredContent"]["count"], 3);
        let names: Vec<&str> = result["structuredContent"]["persons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Aquarius", "Aries", "Sagittarius"]);
    }

    #[tokio::test]
    async fn both_name_and_email_are_anded() {
        let surreal = surreal().await;
        seed(&surreal, "Aquarius", "aquarius@neonlaw.com").await;
        seed(&surreal, "Aries", "aries@example.com").await;
        seed(&surreal, "Sagittarius", "sagittarius@neonlaw.com").await;
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "ari", "email": "neonlaw" }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["count"], 2);
        let names: Vec<&str> = result["structuredContent"]["persons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["Aquarius", "Sagittarius"]);
    }

    #[tokio::test]
    async fn missing_both_keys_is_invalid_arguments() {
        let surreal = surreal().await;
        let err = call(&surreal, &ReadScope::Deployment, &json!({}))
            .await
            .unwrap_err();
        match err {
            ToolError::InvalidArguments(m) => {
                assert!(m.contains("name") && m.contains("email"));
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn whitespace_only_keys_are_treated_as_missing() {
        let surreal = surreal().await;
        let err = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "   ", "email": "" }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn whitespace_around_needle_is_trimmed_before_matching() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@example.com").await;
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "  libra\n" }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["count"], 1);
    }

    #[tokio::test]
    async fn no_match_returns_empty_result_not_an_error() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@example.com").await;
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "ghost" }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["count"], 0);
        assert_eq!(
            result["structuredContent"]["persons"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("No people matched"), "got: {text}");
        assert!(text.contains("name~=ghost"), "got: {text}");
    }

    #[tokio::test]
    async fn no_match_with_both_keys_describes_both_in_summary() {
        let surreal = surreal().await;
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "ghost", "email": "void" }),
        )
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["count"], 0);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("name~=ghost"), "got: {text}");
        assert!(text.contains("email~=void"), "got: {text}");
    }

    #[tokio::test]
    async fn results_are_capped_at_max_results() {
        let surreal = surreal().await;
        let total = MAX_RESULTS + 5;
        for i in 0..total {
            seed(&surreal, "Aries", &format!("user{i:03}@example.com")).await;
        }
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "aries" }),
        )
        .await
        .unwrap();
        assert_eq!(
            result["structuredContent"]["count"].as_u64().unwrap(),
            MAX_RESULTS
        );
    }

    /// The people directory is the firm's own, so a client or clerk
    /// caller is refused rather than answered with an empty list —
    /// nothing they could type would make the read succeed, and "no
    /// matches" would teach the model the directory is empty.
    #[tokio::test]
    async fn the_client_and_clerk_tiers_are_refused_the_firm_directory() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@neonlaw.com").await;
        for role in [store::persons::Role::Client, store::persons::Role::Clerk] {
            let scope = ReadScope::Membership {
                person_id: uuid::Uuid::now_v7(),
                role,
            };
            let err = call(&surreal, &scope, &json!({ "email": "neonlaw" }))
                .await
                .unwrap_err();
            match err {
                ToolError::Forbidden(m) => assert!(m.contains("firm-side"), "{role:?}: {m}"),
                other => panic!("expected Forbidden for {role:?}, got {other:?}"),
            }
        }
    }

    /// An authenticated email with no `persons` row reaches nothing.
    #[tokio::test]
    async fn an_unlinked_caller_is_refused_the_firm_directory() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@neonlaw.com").await;
        let err = call(
            &surreal,
            &ReadScope::Unlinked,
            &json!({ "email": "neonlaw" }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ToolError::Forbidden(_)));
    }

    /// The firm tiers do read it: a lawyer needs a `person_id` before
    /// they can link one to a matter, and oversight legitimately looks a
    /// person up.
    #[tokio::test]
    async fn the_firm_tiers_read_the_directory() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@neonlaw.com").await;
        let scopes = [
            ReadScope::Membership {
                person_id: uuid::Uuid::now_v7(),
                role: store::persons::Role::Lawyer,
            },
            ReadScope::Directory {
                role: store::persons::Role::Owner,
            },
            ReadScope::Directory {
                role: store::persons::Role::Admin,
            },
        ];
        for scope in &scopes {
            let r = call(&surreal, scope, &json!({ "email": "neonlaw" }))
                .await
                .unwrap();
            assert_eq!(r["structuredContent"]["count"], 1, "{scope:?}");
        }
    }

    #[tokio::test]
    async fn summary_text_uses_singular_for_one_match() {
        let surreal = surreal().await;
        seed(&surreal, "Libra", "libra@example.com").await;
        let result = call(
            &surreal,
            &ReadScope::Deployment,
            &json!({ "name": "libra" }),
        )
        .await
        .unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Found 1 person:"), "got: {text}");
        assert!(text.contains("Libra"));
    }

    #[tokio::test]
    async fn summary_text_uses_plural_for_multiple_matches() {
        let surreal = surreal().await;
        seed(&surreal, "Aquarius", "aquarius@example.com").await;
        seed(&surreal, "Aries", "aries@example.com").await;
        let result = call(&surreal, &ReadScope::Deployment, &json!({ "name": "ari" }))
            .await
            .unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Found 2 people:"), "got: {text}");
    }
}
