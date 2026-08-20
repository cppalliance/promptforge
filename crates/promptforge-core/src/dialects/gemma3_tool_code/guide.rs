//! Render the system-prompt tool guide injected for the Gemma dialect.
//!
//! Gemma has no native tool array, so the OpenAI-shaped `tools` are translated
//! into a plain-language guide that teaches the `tool_code` fence protocol and
//! lists each tool's signature (every property, optional ones marked `?`).

use serde_json::Value;

/// Collects a function's parameters and required set, then renders its signature.
fn render_signature(function: &Value, name: &str) -> String {
    let params = function
        .get("parameters")
        .and_then(|value| value.get("properties"))
        .and_then(Value::as_object);
    let required = function
        .get("parameters")
        .and_then(|value| value.get("required"))
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut arg_bits = Vec::new();
    if let Some(params) = params {
        // List every property, not just the required ones: a schema with any
        // required parameter must still advertise its optional parameters,
        // marked with a trailing `?`, so they never vanish from the guide.
        let mut keys = params.keys().collect::<Vec<_>>();
        keys.sort_unstable();
        for key in keys {
            if required.contains(&key.as_str()) {
                arg_bits.push(format!("{key}=..."));
            } else {
                arg_bits.push(format!("{key}=...?"));
            }
        }
    }
    if arg_bits.is_empty() {
        format!("{name}()")
    } else {
        format!("{name}({})", arg_bits.join(", "))
    }
}

/// Render a system guide from OpenAI-shaped `tools`, or `None` when the list is
/// empty or lists no usable tool.
pub(crate) fn render_tool_guide(list: &[Value]) -> Option<String> {
    if list.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    lines.push(
        "You call tools by emitting a sole ```tool_code fence (no other prose) \
         with one call per line. Prefer keyword arguments. A trailing ? marks an \
         optional argument."
            .to_owned(),
    );
    lines.push("Available tools:".to_owned());
    let mut listed = 0usize;
    for tool in list {
        let Some(function) = tool.get("function") else {
            continue;
        };
        let Some(name) = function.get("name").and_then(Value::as_str) else {
            continue;
        };
        let description = function
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let sig = render_signature(function, name);
        if description.is_empty() {
            lines.push(format!("- {sig}"));
        } else {
            lines.push(format!("- {sig}: {description}"));
        }
        listed += 1;
    }
    if listed == 0 {
        return None;
    }
    lines.push("Example: ```tool_code\nsearch(query=\"example\")\n```".to_owned());
    Some(lines.join("\n"))
}
