//! Gemma-3 `tool_code` fence dialect.
//!
//! Local Gemma models served via llama.cpp never emit a `message.tool_calls`
//! array. Instead they put a sole ` ```tool_code ` fence (Python-style call
//! syntax) in `message.content`. Results are echoed as a follow-up user turn
//! rather than `role=tool` messages, because Gemma chat templates reject the
//! OpenAI tool-result shape with HTTP 400.
//!
//! This dialect also handles the interim fenced-JSON `tool_calls` blob that
//! some Gemma quantizations emit.

use serde_json::{Map, Value};

use crate::client::{Message, ToolCall};
use crate::normalize::NormalizedTurn;
use crate::{Error, Result};

use super::{DetectScore, DialectEvidence, DialectRequest, ToolDialect, ToolDialectId};

/// The Gemma-3 `tool_code` fence dialect.
///
/// - `detect`: scores when `supports_tool_calls == Some(false)` combined with
///   Gemma template or model-id fingerprints.
/// - `prepare_request`: copies tool schemas into a system guide, then strips
///   `tools` / `tool_choice` (Gemma rejects the native OpenAI tool array).
/// - `parse_turn`: recognizes sole `tool_code` fences and fenced JSON
///   `tool_calls` blobs; mixed prose stays text.
/// - `echo_tool_results`: pushes the assistant's rendered `tool_code` fence
///   then a user message with `TOOL RESULT` blocks and a continue trailer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Gemma3ToolCodeDialect;

impl ToolDialect for Gemma3ToolCodeDialect {
    fn id(&self) -> ToolDialectId {
        ToolDialectId::Gemma3ToolCode
    }

    fn detect(&self, evidence: &DialectEvidence) -> Option<DetectScore> {
        // Native tool-call endpoints are never this dialect.
        if evidence.supports_tool_calls == Some(true) {
            return None;
        }

        // `<bos>` alone is too common; require the Gemma turn marker.
        let gemma_template = evidence
            .chat_template
            .as_deref()
            .is_some_and(|template| template.contains("<start_of_turn>"));
        let gemma_model = evidence
            .model_id
            .as_deref()
            .is_some_and(|id| id.to_ascii_lowercase().contains("gemma"));
        let gemma_source = evidence
            .source
            .as_deref()
            .is_some_and(|src| src.to_ascii_lowercase().contains("gemma"));

        // Require a Gemma fingerprint so other tools-unsupported models (e.g.
        // some Qwen GGUFs) do not resolve here from caps alone.
        if !gemma_template && !gemma_model && !gemma_source {
            return None;
        }

        let mut score: u8 = 0;
        if evidence.supports_tool_calls == Some(false) {
            score += 40;
        }
        if gemma_template {
            score += 30;
        }
        if gemma_model {
            score += 20;
        }
        if gemma_source {
            score += 10;
        }
        Some(DetectScore(score))
    }

    /// Emulate tools: copy schemas into a leading system guide, then strip the
    /// native OpenAI `tools` / `tool_choice` fields Gemma rejects.
    fn prepare_request(&self, request: &mut DialectRequest<'_>) -> Result<()> {
        let Some(obj) = request.body.as_object_mut() else {
            return Ok(());
        };
        let tools = obj.remove("tools");
        obj.remove("tool_choice");
        if let Some(tools) = tools.as_ref() {
            inject_tool_guide(obj, tools);
        }
        Ok(())
    }

    fn parse_turn(&self, body: &Value) -> Result<NormalizedTurn> {
        let choice = body
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| Error::MalformedResponse("no choices in response".into()))?;
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let message = choice
            .get("message")
            .ok_or_else(|| Error::MalformedResponse("choice had no message".into()))?;
        let reasoning_content = extract_reasoning(message);

        // Gemma never emits wire tool_calls; go straight to content parsing.
        if let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.is_empty())
        {
            if let Some(calls) = parse_content_tool_dialect(content) {
                return Ok(NormalizedTurn {
                    outcome: crate::client::CompletionResult::ToolCalls(calls),
                    finish_reason,
                    reasoning_content,
                });
            }
            return Ok(NormalizedTurn {
                outcome: crate::client::CompletionResult::Text(content.to_string()),
                finish_reason,
                reasoning_content,
            });
        }

        Err(Error::EmptyModelReply {
            detail: if reasoning_content.is_some() {
                "empty model reply: reasoning content was present but ignored"
            } else {
                "empty model reply"
            },
        })
    }

    /// Echo tool-call results in the Gemma content-fence style.
    ///
    /// Pushes an assistant message with the rendered `tool_code` fence, then a
    /// user message containing `TOOL RESULT` blocks and a continue trailer.
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[(String, String)],
    ) {
        conversation.push(Message::assistant(render_tool_code_fence(calls)));

        // The sole caller (the tool-call loop) pushes exactly one result per
        // call, in call order, so the positional pairing below is correct. This
        // asserts that crate-internal invariant rather than silently truncating.
        debug_assert_eq!(
            calls.len(),
            results.len(),
            "echo_tool_results requires one result per call, in order"
        );
        let mut parts: Vec<String> = Vec::with_capacity(results.len());
        for (call, (id, content)) in calls.iter().zip(results.iter()) {
            parts.push(format!("TOOL RESULT {} ({}):\n{}", call.name, id, content));
        }
        let mut follow_up = parts.join("\n\n");
        follow_up.push_str(
            "\n\nContinue the protocol. Call another tool with a tool_code fence, or write the final evidence from fetch bodies only.",
        );
        conversation.push(Message::user(follow_up));
    }
}

// --- Content-fence parsing (moved from normalize.rs) ---

/// When `content` is entirely a known tool-call dialect, return the calls.
///
/// Mixed prose (evidence text that happens to mention a fence) returns `None`
/// so the turn stays ordinary text.
fn parse_content_tool_dialect(content: &str) -> Option<Vec<ToolCall>> {
    let mut rest = content.trim();
    let mut calls = Vec::new();
    while !rest.is_empty() {
        if let Some((parsed, remain)) = peel_tool_code_fence(rest) {
            if parsed.is_empty() {
                return None;
            }
            calls.extend(parsed);
            rest = remain.trim();
            continue;
        }
        if let Some((parsed, remain)) = peel_json_tool_calls_fence(rest) {
            if parsed.is_empty() {
                return None;
            }
            calls.extend(parsed);
            rest = remain.trim();
            continue;
        }
        return None;
    }
    if calls.is_empty() { None } else { Some(calls) }
}

/// Peel one leading ` ```tool_code ` fence into Python-style `name(k=v)` calls.
fn peel_tool_code_fence(input: &str) -> Option<(Vec<ToolCall>, &str)> {
    let rest = strip_fence_open(input, "tool_code")?;
    let (body, after) = split_fence_close(rest)?;
    let mut calls = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let call = parse_tool_code_call(line, index)?;
        calls.push(call);
    }
    Some((calls, after))
}

/// Peel one leading ` ```json ` / ` ``` ` fence that holds OpenAI `tool_calls`.
fn peel_json_tool_calls_fence(input: &str) -> Option<(Vec<ToolCall>, &str)> {
    let rest = strip_fence_open(input, "json").or_else(|| strip_fence_open(input, ""))?;
    let (body, after) = split_fence_close(rest)?;
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let raw_calls = value
        .get("tool_calls")
        .and_then(Value::as_array)
        .filter(|calls| !calls.is_empty())?;
    let calls = parse_openai_tool_calls(raw_calls).ok()?;
    Some((calls, after))
}

fn strip_fence_open<'a>(input: &'a str, language: &str) -> Option<&'a str> {
    let trimmed = input.trim_start();
    let prefix = if language.is_empty() {
        "```".to_string()
    } else {
        format!("```{language}")
    };
    let rest = trimmed.strip_prefix(&prefix)?;
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n')?;
    Some(rest)
}

fn split_fence_close(input: &str) -> Option<(&str, &str)> {
    let idx = input.find("```")?;
    Some((&input[..idx], &input[idx + 3..]))
}

/// Parse `search(query="x", count=3)` into a [`ToolCall`].
fn parse_tool_code_call(line: &str, index: usize) -> Option<ToolCall> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let name = line[..open].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let args_src = line[open + 1..close].trim();
    let arguments = parse_tool_code_args(name, args_src)?;
    Some(ToolCall {
        id: format!("call_tool_code_{index}"),
        name: name.to_string(),
        arguments,
    })
}

fn parse_tool_code_args(tool_name: &str, src: &str) -> Option<Value> {
    if src.is_empty() {
        return Some(Value::Object(Map::new()));
    }
    // Keyword form `query="..."` wins when any part contains `=`.
    let parts = split_top_level_commas(src);
    let keyword = parts.iter().any(|part| part.contains('='));
    if keyword {
        let mut map = Map::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let eq = part.find('=')?;
            let key = part[..eq].trim();
            if key.is_empty() {
                return None;
            }
            let raw = part[eq + 1..].trim();
            map.insert(key.to_string(), parse_tool_code_value(raw)?);
        }
        return Some(Value::Object(map));
    }

    // Gemma often emits positionals: `search("C++ Alliance")`.
    let mut values = Vec::new();
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        values.push(parse_tool_code_value(part)?);
    }
    let keys = positional_arg_keys(tool_name, values.len())?;
    let mut map = Map::new();
    for (key, value) in keys.iter().zip(values) {
        map.insert(key.to_string(), value);
    }
    Some(Value::Object(map))
}

/// Map positional `tool_code` args onto schema-ish parameter names.
///
/// Gemma IT frequently emits `search("...")` / `fetch("https://...")` instead
/// of keyword form. Keep this table aligned with shipped tool aliases.
fn positional_arg_keys(tool_name: &str, count: usize) -> Option<&'static [&'static str]> {
    match (tool_name, count) {
        ("search" | "web_search", 1) => Some(&["query"]),
        ("fetch" | "web_fetch", 1) => Some(&["url"]),
        ("echo", 1) => Some(&["value"]),
        _ => None,
    }
}

/// Build a system guide from OpenAI-shaped `tools` and prepend it to messages.
fn inject_tool_guide(body: &mut Map<String, Value>, tools: &Value) {
    let Some(guide) = render_tool_guide(tools) else {
        return;
    };
    let messages = body
        .entry("messages")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = messages.as_array_mut() else {
        return;
    };
    list.insert(
        0,
        serde_json::json!({
            "role": "system",
            "content": guide,
        }),
    );
}

fn render_tool_guide(tools: &Value) -> Option<String> {
    let list = tools.as_array().filter(|list| !list.is_empty())?;
    let mut lines = Vec::new();
    lines.push(
        "You call tools by emitting a sole ```tool_code fence (no other prose) \
         with one call per line. Prefer keyword arguments."
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
            let keys: Vec<&str> = if required.is_empty() {
                params.keys().map(String::as_str).collect()
            } else {
                required.clone()
            };
            for key in keys {
                arg_bits.push(format!("{key}=..."));
            }
        }
        let sig = if arg_bits.is_empty() {
            format!("{name}()")
        } else {
            format!("{name}({})", arg_bits.join(", "))
        };
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

fn parse_tool_code_value(raw: &str) -> Option<Value> {
    if let Some(inner) = strip_quotes(raw, '"').or_else(|| strip_quotes(raw, '\'')) {
        return Some(Value::String(inner.to_string()));
    }
    if raw.eq_ignore_ascii_case("true") {
        return Some(Value::Bool(true));
    }
    if raw.eq_ignore_ascii_case("false") {
        return Some(Value::Bool(false));
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Some(Value::Number(n.into()));
    }
    if let Ok(n) = raw.parse::<f64>() {
        return serde_json::Number::from_f64(n).map(Value::Number);
    }
    None
}

fn strip_quotes(raw: &str, quote: char) -> Option<&str> {
    let raw = raw.strip_prefix(quote)?.strip_suffix(quote)?;
    Some(raw)
}

fn split_top_level_commas(src: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth: i32 = 0;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in src.char_indices() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                in_quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&src[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&src[start..]);
    parts
}

/// Parse the OpenAI `message.tool_calls` array into runtime [`ToolCall`]s.
fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    for raw in raw_calls {
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::MalformedResponse("tool call had no id".into()))?
            .to_string();
        let function = raw
            .get("function")
            .ok_or_else(|| Error::MalformedResponse("tool call had no function".into()))?;
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::MalformedResponse("tool call had no name".into()))?
            .to_string();
        let arguments = match function.get("arguments").and_then(Value::as_str) {
            Some(raw_args) => serde_json::from_str::<Value>(raw_args)
                .unwrap_or_else(|_| Value::String(raw_args.to_string())),
            None => Value::Null,
        };
        calls.push(ToolCall {
            id,
            name,
            arguments,
        });
    }
    Ok(calls)
}

// --- Rendering helpers ---

/// Render parsed tool calls as the Gemma ` ```tool_code ` content dialect.
pub(crate) fn render_tool_code_fence(calls: &[ToolCall]) -> String {
    let mut body = String::from("```tool_code\n");
    for call in calls {
        body.push_str(&call.name);
        body.push('(');
        match &call.arguments {
            Value::Object(map) => {
                let mut first = true;
                for (key, value) in map {
                    if !first {
                        body.push_str(", ");
                    }
                    first = false;
                    body.push_str(key);
                    body.push('=');
                    body.push_str(&render_tool_code_arg(value));
                }
            }
            other => {
                body.push_str("arguments=");
                body.push_str(&render_tool_code_arg(other));
            }
        }
        body.push_str(")\n");
    }
    body.push_str("```");
    body
}

fn render_tool_code_arg(value: &Value) -> String {
    match value {
        Value::String(s) => format!("\"{s}\""),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => "null".into(),
        other => format!("\"{other}\""),
    }
}

/// First non-empty string among the known reasoning field synonyms.
fn extract_reasoning(message: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning", "thinking"] {
        if let Some(text) = message
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            return Some(text.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::CompletionResult;

    #[test]
    fn sole_tool_code_fence_becomes_tool_calls() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nsearch(query=\"C++ Alliance founder\")\n```"
                },
                "finish_reason": "stop"
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_tool_code_0");
                assert_eq!(calls[0].name, "search");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "query": "C++ Alliance founder" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn positional_search_maps_to_query() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nsearch(\"C++ Alliance organization\")\n```"
                }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "search");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "query": "C++ Alliance organization" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn positional_fetch_maps_to_url() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nfetch(\"https://cppalliance.org\")\n```"
                }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "fetch");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "url": "https://cppalliance.org" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn mixed_prose_stays_text() {
        let dialect = Gemma3ToolCodeDialect;
        let content = "Here is evidence.\n\n```tool_code\nsearch(query=\"x\")\n```\n";
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": content }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::Text(text) => assert_eq!(text, content),
            CompletionResult::ToolCalls(_) => panic!("mixed prose must stay text"),
        }
    }

    #[test]
    fn fenced_json_tool_calls_parsed() {
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```json\n{\"tool_calls\":[{\"id\":\"1\",\"type\":\"function\",\"function\":{\"name\":\"fetch\",\"arguments\":\"{\\\"url\\\":\\\"https://example.com\\\"}\"}}]}\n```"
                }
            }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "fetch");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "url": "https://example.com" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn prepare_request_strips_tools_and_injects_guide() {
        let dialect = Gemma3ToolCodeDialect;
        let mut body = serde_json::json!({
            "model": "gemma-3",
            "messages": [{"role": "user", "content": "brief me"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "search",
                    "description": "Web search",
                    "parameters": {
                        "type": "object",
                        "properties": { "query": { "type": "string" } },
                        "required": ["query"]
                    }
                }
            }],
            "tool_choice": "auto"
        });
        let mut req = DialectRequest { body: &mut body };
        dialect.prepare_request(&mut req).unwrap();
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_eq!(body["model"], "gemma-3");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        let guide = messages[0]["content"].as_str().unwrap();
        assert!(guide.contains("search(query=...)"));
        assert!(guide.contains("```tool_code"));
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn detect_gemma_props_scores() {
        let dialect = Gemma3ToolCodeDialect;
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
            source: None,
        };
        let score = dialect.detect(&evidence).expect("should score");
        assert!(score.0 >= 80, "expected high score, got {}", score.0);
    }

    #[test]
    fn detect_with_native_tools_returns_none() {
        let dialect = Gemma3ToolCodeDialect;
        let evidence = DialectEvidence {
            supports_tool_calls: Some(true),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
            source: None,
        };
        assert!(dialect.detect(&evidence).is_none());
    }

    #[test]
    fn detect_tools_false_without_gemma_fingerprint_returns_none() {
        let dialect = Gemma3ToolCodeDialect;
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("{{ messages }}".to_string()),
            model_id: Some("qwen3-0.6b".to_string()),
            source: None,
        };
        assert!(dialect.detect(&evidence).is_none());
    }

    #[test]
    fn echo_tool_results_produces_user_turn() {
        let dialect = Gemma3ToolCodeDialect;
        let calls = vec![ToolCall {
            id: "call_tool_code_0".into(),
            name: "search".into(),
            arguments: serde_json::json!({"query": "rust"}),
        }];
        let results = vec![("call_tool_code_0".into(), "result text".into())];
        let mut conversation = Vec::new();
        dialect.echo_tool_results(&mut conversation, &calls, &results);

        assert_eq!(conversation.len(), 2);
        assert_eq!(conversation[0].role, "assistant");
        assert!(conversation[0].content.contains("```tool_code"));
        assert!(conversation[0].content.contains("search("));
        assert_eq!(conversation[1].role, "user");
        assert!(
            conversation[1]
                .content
                .contains("TOOL RESULT search (call_tool_code_0):")
        );
        assert!(conversation[1].content.contains("result text"));
        assert!(conversation[1].content.contains("Continue the protocol"));
    }
}
