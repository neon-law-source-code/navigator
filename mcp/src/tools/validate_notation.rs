//! `aida_validate_notation` MCP tool.
//!
//! Lints markdown without persisting anything. Mirrors the `cli
//! validate` rule-set selection so a Template that passes the CLI
//! passes here and vice versa — same `rules::navigator_default_rules`
//! / `rules::navigator_markdown_only_rules` as the REST handler at
//! `POST /app/api/templates/validate`.
//!
//! Conversational use: the model drafts markdown, calls this tool,
//! reads back the `violations` array, fixes the markdown, and tries
//! again. No database state changes — safe to invoke speculatively.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use super::ToolError;

/// Default pretend filename used when the caller does not pass one.
/// `snake_case` so the default does not itself trip the `N103`
/// filename rule.
const DEFAULT_PATH: &str = "template.md";

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_validate_notation",
        "description":
            "Lint markdown for Neon Law Navigator notation rules and return the \
             list of violations. Does NOT persist anything — safe to \
             call repeatedly while drafting. Pass `contents` (the raw \
             markdown, including any YAML frontmatter), optionally a \
             `path` (used by filename-aware rules like N103 and to \
             label the response), and `markdown_only: true` to skip \
             the N-family notation-template rules (use this for plain \
             prose). Returns `clean: true` when the file has no blocking \
             errors, or `clean: false` otherwise; `violations` lists one \
             entry per finding (`code`, `line`, `message`), including \
             yellow advisories like N112 (\"step allowed but not built \
             yet\") that do not affect `clean`.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "contents": {
                    "type": "string",
                    "description":
                        "Raw markdown body, including any YAML \
                         frontmatter. Required."
                },
                "path": {
                    "type": "string",
                    "description":
                        "Optional pretend filename so rules that key \
                         off the path (`N103` snake_case) and the \
                         response have something meaningful to report. \
                         Defaults to `template.md`."
                },
                "markdown_only": {
                    "type": "boolean",
                    "description":
                        "When true, lint with the Markdown-only rule \
                         set (drops the N-family, adds `S102` line \
                         packing) — use this for plain prose. Defaults to \
                         false: the full Neon Law Navigator notation rule set \
                         runs."
                }
            },
            "required": ["contents"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
struct Args {
    contents: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    markdown_only: bool,
}

// Async to match the uniform `tools::call_tool` dispatch shape —
// every tool is awaited there — even though the body is pure CPU
// work with no `.await`. Don't drop the `async`.
#[allow(clippy::unused_async)]
pub async fn call(arguments: &Value) -> Result<Value, ToolError> {
    let args: Args = super::decode_args(arguments)?;

    let path = args
        .path
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PATH.to_string());

    let rule_set = if args.markdown_only {
        rules::navigator_markdown_only_rules()
    } else {
        rules::navigator_default_rules()
    };

    let file = rules::SourceFile {
        path: PathBuf::from(&path),
        contents: args.contents,
    };

    let mut violations: Vec<Value> = Vec::new();
    let mut error_count = 0usize;
    for rule in &rule_set {
        for v in rule.lint(&file) {
            if rules::severity_for_code(v.code) == rules::Severity::Error {
                error_count += 1;
            }
            violations.push(json!({
                "code": v.code,
                "line": v.line,
                "message": v.message,
            }));
        }
    }

    // `clean` means no *blocking* errors. Yellow advisories (e.g. N112,
    // "step allowed but not built yet" — which every lawyer_review gate
    // earns) are still returned in `violations` but don't flip `clean`.
    let clean = error_count == 0;
    let warning_count = violations.len() - error_count;
    let text = if violations.is_empty() {
        format!("`{path}` is clean: 0 violations.")
    } else {
        let preview = violations
            .iter()
            .take(5)
            .map(|v| {
                format!(
                    "{}:{}",
                    v["code"].as_str().unwrap_or(""),
                    v["line"].as_u64().unwrap_or(0)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let suffix = if violations.len() > 5 { ", …" } else { "" };
        format!(
            "`{path}` has {error_count} error(s), {warning_count} warning(s): {preview}{suffix}"
        )
    };

    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": {
            "path": path,
            "clean": clean,
            "violations": violations,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::ToolError;
    use serde_json::json;

    #[test]
    fn descriptor_names_the_tool_under_aida_namespace() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_validate_notation");
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        let required = d["inputSchema"]["required"].as_array().unwrap();
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(required_names, vec!["contents"]);
        let props = d["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("contents"));
        assert!(props.contains_key("path"));
        assert!(props.contains_key("markdown_only"));
    }

    #[tokio::test]
    async fn missing_contents_is_invalid_arguments() {
        let err = call(&json!({})).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "expected InvalidArguments, got {err:?}"
        );
    }

    #[tokio::test]
    async fn clean_notation_returns_clean_true_with_only_the_not_built_advisory() {
        // Minimal notation that satisfies every N-rule — copied from
        // the REST integration test so the two surfaces stay aligned.
        let contents = "---\n\
kind: trust\n\
title: Trust\n\
respondent_type: entity\n\
code: sample__trust\n\
confidential: false\n\
questionnaire:\n  \
  BEGIN:\n    \
    _: END\n  \
  END: {}\n\
workflow:\n  \
  BEGIN:\n    \
    next: lawyer_review\n  \
  lawyer_review:\n    \
    next: END\n  \
  END: {}\n\
---\n\n\
Body.\n";
        let result = call(&json!({ "contents": contents })).await.unwrap();
        let sc = &result["structuredContent"];
        assert_eq!(sc["clean"], true, "expected clean (no errors), got: {sc}");
        assert_eq!(sc["path"], "template.md");
        // The mandatory lawyer_review gate earns the yellow N112 advisory,
        // returned without flipping `clean` to false.
        let codes: Vec<&str> = sc["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert_eq!(
            codes,
            ["N112"],
            "expected only the N112 advisory, got: {sc}"
        );
    }

    #[tokio::test]
    async fn frontmatter_and_line_length_violations_show_up_with_codes() {
        let long_line = "x".repeat(150);
        let contents = format!("---\nfoo: bar\n---\n\n{long_line}\n");
        let result = call(&json!({ "contents": contents, "path": "trust.md" }))
            .await
            .unwrap();
        let sc = &result["structuredContent"];
        assert_eq!(sc["clean"], false);
        assert_eq!(sc["path"], "trust.md");
        let codes: Vec<&str> = sc["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(codes.contains(&"N101"), "expected N101, got {codes:?}");
        assert!(codes.contains(&"N102"), "expected N102, got {codes:?}");
        assert!(codes.contains(&"S101"), "expected S101, got {codes:?}");
    }

    #[tokio::test]
    async fn markdown_only_drops_the_n_family() {
        // No frontmatter at all — would trip N101 in the default set.
        let result = call(&json!({
            "contents": "# Heading\n\nBody paragraph.\n",
            "markdown_only": true,
        }))
        .await
        .unwrap();
        let codes: Vec<&str> = result["structuredContent"]["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["code"].as_str().unwrap())
            .collect();
        assert!(
            codes.iter().all(|c| !c.starts_with('N')),
            "N-family must not run when markdown_only=true, got {codes:?}"
        );
    }

    #[tokio::test]
    async fn empty_path_falls_back_to_default() {
        let result = call(&json!({
            "contents": "# H\n",
            "path": "   ",
            "markdown_only": true,
        }))
        .await
        .unwrap();
        assert_eq!(result["structuredContent"]["path"], "template.md");
    }
}
