//! `list_tools` MCP tool.
//!
//! A meta-tool: returns the name and description of every other tool
//! Navigator MCP advertises. MCP clients already learn this via the protocol's
//! `tools/list` method, but A2A free-text callers and humans asking
//! "what can you do?" benefit from a callable surface that returns the
//! same information in one structured response. Sorted by name.

use serde_json::{json, Value};

use super::{list_tools, ToolError};

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "list_tools",
        "description": "List every other tool Navigator MCP exposes, returning each tool's name and \
                        description. Use this when the user asks what Navigator MCP can do, what tools or \
                        skills are available, or which tool to reach for next. Takes no arguments.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }
    })
}

#[allow(clippy::unused_async)]
pub async fn call(_arguments: &Value) -> Result<Value, ToolError> {
    let mut entries: Vec<(String, String)> = list_tools()
        .iter()
        .filter_map(|t| {
            let name = t["name"].as_str()?.to_string();
            let description = t["description"].as_str().unwrap_or("").to_string();
            Some((name, description))
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let tools: Vec<Value> = entries
        .iter()
        .map(|(name, description)| {
            json!({
                "name": name,
                "description": description,
            })
        })
        .collect();

    let listed = entries
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let summary = format!("{} tools: {listed}.", entries.len());

    Ok(json!({
        "content": [{ "type": "text", "text": summary }],
        "structuredContent": {
            "count": tools.len(),
            "tools": tools,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor};
    use crate::tools::list_tools;
    use serde_json::json;

    #[test]
    fn descriptor_names_the_tool_and_takes_no_arguments() {
        let d = descriptor();
        assert_eq!(d["name"], "list_tools");
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        let props = d["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.is_empty());
    }

    #[tokio::test]
    async fn returns_one_entry_per_advertised_tool_including_itself() {
        let r = call(&json!({})).await.unwrap();
        let count = usize::try_from(r["structuredContent"]["count"].as_u64().unwrap()).unwrap();
        assert_eq!(count, list_tools().len());
        let names: Vec<&str> = r["structuredContent"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"list_tools"));
        assert!(names.contains(&"list_projects"));
        assert!(names.contains(&"list_entities"));
    }

    #[tokio::test]
    async fn entries_are_sorted_by_name() {
        let r = call(&json!({})).await.unwrap();
        let names: Vec<String> = r["structuredContent"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[tokio::test]
    async fn each_entry_carries_a_bare_name_and_non_empty_description() {
        let r = call(&json!({})).await.unwrap();
        for tool in r["structuredContent"]["tools"].as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let description = tool["description"].as_str().unwrap();
            assert!(
                !name.starts_with("aida_"),
                "`{name}` still carries the retired vendor prefix"
            );
            assert!(
                tool.get("short_name").is_none(),
                "`short_name` duplicated `name` once the prefix went away"
            );
            assert!(!description.is_empty(), "{name} has an empty description");
        }
    }

    #[tokio::test]
    async fn summary_lists_bare_names_and_starts_with_count() {
        let r = call(&json!({})).await.unwrap();
        let text = r["content"][0]["text"].as_str().unwrap();
        let count = list_tools().len();
        assert!(text.starts_with(&format!("{count} tools:")));
        assert!(text.contains("list_projects"));
        assert!(!text.contains("aida_list_projects"));
    }

    #[tokio::test]
    async fn ignores_arguments_silently() {
        let r = call(&json!({ "garbage": 42 })).await.unwrap();
        let count = usize::try_from(r["structuredContent"]["count"].as_u64().unwrap()).unwrap();
        assert_eq!(count, list_tools().len());
    }
}
