//! Pure Notation template evaluation shared by document assembly and the
//! browser's live preview.

use std::collections::BTreeMap;

const FOR_OPEN: &str = "{{#for ";
const FOR_CLOSE: &str = "{{/for}}";
const IF_OPEN: &str = "{{#if ";
const IF_CLOSE: &str = "{{/if}}";

#[must_use]
pub fn fill(body: &str, context: &BTreeMap<String, String>) -> String {
    fill_with_display(body, context, context)
}

#[must_use]
pub fn fill_with_display(
    body: &str,
    context: &BTreeMap<String, String>,
    display: &BTreeMap<String, String>,
) -> String {
    let conditional = expand_conditionals(body, context);
    let expanded = expand_loops(&conditional, context);
    let mut filled = expanded;
    for (key, value) in display {
        filled = filled.replace(&format!("{{{{{key}}}}}"), value);
    }
    filled
}

fn expand_conditionals(body: &str, context: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find(IF_OPEN) {
        let after_open = &rest[start + IF_OPEN.len()..];
        let Some(header_len) = after_open.find("}}") else {
            break;
        };
        let condition = after_open[..header_len].trim();
        let block_start = start + IF_OPEN.len() + header_len + 2;
        let Some(close_rel) = matching_close(&rest[block_start..], IF_OPEN, IF_CLOSE) else {
            break;
        };
        let block = &rest[block_start..block_start + close_rel];
        let close_end = block_start + close_rel + IF_CLOSE.len();
        out.push_str(&rest[..start]);
        let (key, expected) = condition
            .split_once('=')
            .map_or((condition, None), |(key, value)| {
                (key.trim(), Some(value.trim()))
            });
        let actual = context.get(key).map(String::as_str).unwrap_or_default();
        let include = expected.map_or_else(
            || !actual.is_empty() && !matches!(actual, "false" | "no" | "0"),
            |expected| actual == expected,
        );
        if include {
            out.push_str(&expand_conditionals(block, context));
        }
        rest = &rest[close_end..];
    }
    out.push_str(rest);
    out
}

fn expand_loops(body: &str, context: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some(start) = rest.find(FOR_OPEN) {
        let after_open = &rest[start + FOR_OPEN.len()..];
        let Some(header_len) = after_open.find("}}") else {
            break;
        };
        let header = after_open[..header_len].trim();
        let block_start = start + FOR_OPEN.len() + header_len + 2;
        let Some(close_rel) = matching_close(&rest[block_start..], FOR_OPEN, FOR_CLOSE) else {
            break;
        };
        let block = &rest[block_start..block_start + close_rel];
        let close_end = block_start + close_rel + FOR_CLOSE.len();
        out.push_str(&rest[..start]);
        if let Some((var, state)) = header.split_once(" in ") {
            out.push_str(&render_loop(var.trim(), state.trim(), block, context));
        }
        rest = &rest[close_end..];
    }
    out.push_str(rest);
    out
}

fn matching_close(source: &str, open: &str, close: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let open = open.as_bytes();
    let close = close.as_bytes();
    let mut depth = 1usize;
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index..].starts_with(open) {
            depth += 1;
            index += open.len();
        } else if bytes[index..].starts_with(close) {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
            index += close.len();
        } else {
            index += 1;
        }
    }
    None
}

fn render_loop(var: &str, state: &str, block: &str, context: &BTreeMap<String, String>) -> String {
    let Some(json) = context.get(state) else {
        return String::new();
    };
    let rows: Vec<BTreeMap<String, serde_json::Value>> =
        serde_json::from_str(json).unwrap_or_default();
    let mut out = String::new();
    for row in &rows {
        let mut piece = block.to_string();
        for (part, value) in row {
            let needle = format!("{{{{{var}.{part}}}}}");
            piece = piece.replace(&needle, &json_scalar(value));
        }
        out.push_str(&expand_loops(&piece, context));
    }
    out
}

fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
