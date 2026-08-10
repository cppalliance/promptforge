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

use super::{
    DetectScore, DialectEvidence, DialectRequest, ToolDialect, ToolDialectId,
    correlate_tool_results,
};

/// The Gemma-3 `tool_code` fence dialect.
///
/// - `detect`: never matches an endpoint with explicit native tool-call support
///   (`supports_tool_calls == Some(true)`); otherwise it scores a Gemma-
///   fingerprinted model (template, model-id, or source), adding weight for an
///   explicit `Some(false)`. Unknown capability plus a Gemma fingerprint still
///   scores, since Gemma via llama.cpp never emits native `tool_calls`.
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
        let reasoning_content = crate::normalize::extract_reasoning(message)?;

        // Gemma never emits wire tool_calls; go straight to content parsing.
        if let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.trim().is_empty())
        {
            // Three-way classification: recognized calls become a tool turn,
            // ordinary prose stays text, and a recognized-but-malformed tool
            // fence propagates as a concrete error instead of masquerading as
            // final text.
            match parse_content_tool_dialect(content) {
                ContentParse::Calls(calls) => {
                    return Ok(NormalizedTurn {
                        outcome: crate::client::CompletionResult::ToolCalls(calls),
                        finish_reason,
                        reasoning_content,
                    });
                }
                ContentParse::NotProtocol => {
                    return Ok(NormalizedTurn {
                        outcome: crate::client::CompletionResult::Text(content.to_string()),
                        finish_reason,
                        reasoning_content,
                    });
                }
                ContentParse::Malformed(error) => return Err(error),
            }
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
    ///
    /// # Errors
    /// Returns an error, leaving the conversation unmodified, when `calls` and
    /// `results` fail [`correlate_tool_results`]. Correlation is validated in
    /// every build (not just debug) before either message is appended, so a
    /// count, ordering, or id-correlation break can never silently truncate or
    /// mislabel a result via `zip`.
    fn echo_tool_results(
        &self,
        conversation: &mut Vec<Message>,
        calls: &[ToolCall],
        results: &[(String, String)],
    ) -> Result<()> {
        correlate_tool_results(calls, results)?;

        let fence = render_tool_code_fence(calls)?;
        conversation.push(Message::assistant(fence));

        let mut parts: Vec<String> = Vec::with_capacity(results.len());
        for (call, (id, content)) in calls.iter().zip(results.iter()) {
            parts.push(format!("TOOL RESULT {} ({}):\n{}", call.name, id, content));
        }
        let mut follow_up = parts.join("\n\n");
        follow_up.push_str(
            "\n\nContinue the protocol. Call another tool with a tool_code fence, or write the final evidence from fetch bodies only.",
        );
        conversation.push(Message::user(follow_up));
        Ok(())
    }
}

// --- Content-fence parsing (moved from normalize.rs) ---

/// The three-way outcome of classifying model content.
///
/// Distinguishing malformed protocol from ordinary prose is the whole point: a
/// recognized tool fence whose contents are invalid must surface as an error,
/// not collapse to `None` alongside genuine prose and become final text.
enum ContentParse {
    /// The content is ordinary prose with no recognized tool fence.
    NotProtocol,
    /// The content is one or more well-formed tool fences.
    Calls(Vec<ToolCall>),
    /// The content opened a recognized tool fence but its contents are invalid.
    Malformed(Error),
}

/// The outcome of peeling one leading fence.
enum Peel<'a> {
    /// A valid tool fence with its parsed calls and the remaining input.
    Calls(Vec<ToolCall>, &'a str),
    /// A recognized tool-protocol fence whose contents are invalid.
    Malformed(Error),
    /// No tool-protocol fence at this position (ordinary prose or data fence).
    NotAFence,
}

/// Classify model content as prose, tool calls, or malformed protocol.
///
/// The content is protocol only when it begins with a recognized tool fence;
/// prose that merely mentions a fence later stays text. Once protocol intent is
/// established, every fence must parse and no trailing non-fence content may
/// remain, or the whole turn is malformed.
fn parse_content_tool_dialect(content: &str) -> ContentParse {
    let mut rest = content.trim();
    let mut calls = Vec::new();
    // One monotonic counter threads across every `tool_code` fence so synthetic
    // ids stay unique instead of restarting at zero per fence.
    let mut next_id = 0usize;
    let mut saw_fence = false;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        match peel_tool_code_fence(rest, &mut next_id) {
            Peel::Calls(parsed, remain) => {
                saw_fence = true;
                calls.extend(parsed);
                rest = remain;
                continue;
            }
            Peel::Malformed(error) => return ContentParse::Malformed(error),
            Peel::NotAFence => {}
        }
        match peel_json_tool_calls_fence(rest) {
            Peel::Calls(parsed, remain) => {
                saw_fence = true;
                calls.extend(parsed);
                rest = remain;
                continue;
            }
            Peel::Malformed(error) => return ContentParse::Malformed(error),
            Peel::NotAFence => {}
        }
        // No tool fence here. If we already consumed one, this is trailing junk
        // in an otherwise-protocol turn; otherwise it is ordinary prose.
        if saw_fence {
            return ContentParse::Malformed(Error::MalformedResponse(
                "trailing content after tool_code fence".into(),
            ));
        }
        return ContentParse::NotProtocol;
    }
    if calls.is_empty() {
        ContentParse::NotProtocol
    } else {
        ContentParse::Calls(calls)
    }
}

/// Peel one leading ` ```tool_code ` fence into Python-style `name(k=v)` calls.
///
/// `next_id` is a run-wide monotonic counter used to mint each call's synthetic
/// id; it is advanced once per parsed call so ids stay unique across fences.
/// A `tool_code` opener commits to protocol intent: an unterminated fence, a
/// malformed call line, or an empty fence is [`Peel::Malformed`], never text.
fn peel_tool_code_fence<'a>(input: &'a str, next_id: &mut usize) -> Peel<'a> {
    let Some(rest) = strip_fence_open(input, "tool_code") else {
        return Peel::NotAFence;
    };
    let Some((body, after)) = split_fence_close_standalone(rest) else {
        return Peel::Malformed(Error::MalformedResponse(
            "unterminated tool_code fence".into(),
        ));
    };
    let mut calls = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(call) = parse_tool_code_call(line, *next_id) else {
            return Peel::Malformed(Error::MalformedResponse(
                "malformed tool_code call line".into(),
            ));
        };
        *next_id += 1;
        calls.push(call);
    }
    if calls.is_empty() {
        return Peel::Malformed(Error::MalformedResponse(
            "tool_code fence contained no calls".into(),
        ));
    }
    Peel::Calls(calls, after)
}

/// Peel one leading ` ```json ` / ` ``` ` fence that holds OpenAI `tool_calls`.
///
/// A code fence is only tool protocol when its body decodes to a JSON object
/// carrying a non-empty `tool_calls` array; anything else is an ordinary data
/// fence ([`Peel::NotAFence`]) that stays text. Once the fence *is* recognized
/// as tool protocol, malformed calls are [`Peel::Malformed`] and preserve the
/// concrete decode error rather than falling back to text.
fn peel_json_tool_calls_fence(input: &str) -> Peel<'_> {
    let Some(rest) = strip_fence_open(input, "json").or_else(|| strip_fence_open(input, "")) else {
        return Peel::NotAFence;
    };
    let Some((body, after)) = split_fence_close_standalone(rest) else {
        return Peel::NotAFence;
    };
    let Ok(value) = serde_json::from_str::<Value>(body.trim()) else {
        return Peel::NotAFence;
    };
    let Some(raw_calls) = value.get("tool_calls").and_then(Value::as_array) else {
        return Peel::NotAFence;
    };
    if raw_calls.is_empty() {
        return Peel::NotAFence;
    }
    match crate::normalize::parse_openai_tool_calls(raw_calls) {
        Ok(calls) => Peel::Calls(calls, after),
        Err(error) => Peel::Malformed(error),
    }
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

/// Split `input` at the first standalone closing fence line (a line whose
/// trimmed content is exactly ```` ``` ````), returning the body before it and
/// the text after it.
///
/// Scanning is line-oriented and quote-aware: a ```` ``` ```` that appears
/// inside a quoted argument value is not a close, so a value like
/// `x="```"` cannot terminate the fence early. Returns `None` when no standalone
/// closing line exists.
fn split_fence_close_standalone(input: &str) -> Option<(&str, &str)> {
    let mut offset = 0usize;
    let mut in_quote: Option<char> = None;
    for line in input.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        // A standalone closing fence only counts outside any open quote.
        if in_quote.is_none() && content.trim() == "```" {
            return Some((&input[..line_start], &input[offset..]));
        }
        // Advance quote state across this line. JSON strings never span a raw
        // newline, so quote state effectively resets at each line boundary for
        // well-formed calls; an unterminated quote simply prevents an early
        // close and yields an unterminated-fence error upstream.
        let mut escaped = false;
        for ch in content.chars() {
            if let Some(q) = in_quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == q {
                    in_quote = None;
                }
            } else if ch == '"' || ch == '\'' {
                in_quote = Some(ch);
            }
        }
    }
    None
}

/// True when `s` is a `tool_code` identifier: `[A-Za-z_][A-Za-z0-9_]*`.
///
/// Enforced on both tool names and keyword-argument keys so a name cannot start
/// with a digit and a key cannot smuggle punctuation or control characters.
fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Parse one `name(args)` call line into a [`ToolCall`].
///
/// The name must be an identifier, the arguments live between the first `(` and
/// the final `)`, and the `)` must end the non-whitespace input so trailing text
/// after the call is rejected rather than silently ignored.
fn parse_tool_code_call(line: &str, index: usize) -> Option<ToolCall> {
    let line = line.trim();
    let open = line.find('(')?;
    // The close paren must be the last non-whitespace character: `name(...)x` and
    // `name(...) then more` are rejected, not truncated.
    if !line.ends_with(')') {
        return None;
    }
    let close = line.len() - 1;
    if close <= open {
        return None;
    }
    let name = &line[..open];
    if !is_identifier(name) {
        return None;
    }
    let args_src = &line[open + 1..close];
    let arguments = parse_tool_code_args(name, args_src)?;
    Some(ToolCall {
        id: format!("call_tool_code_{index}"),
        name: name.to_string(),
        arguments,
    })
}

/// Parse the argument list into a JSON object.
///
/// Arguments are either all keyword (`key=<json>`) or all positional
/// (`<json>`); mixing the two forms is rejected, as is a duplicate keyword key.
/// Every value is a complete JSON value, so strings decode their escapes and
/// null, numbers, booleans, arrays, and objects all round-trip.
fn parse_tool_code_args(tool_name: &str, src: &str) -> Option<Value> {
    let src = src.trim();
    if src.is_empty() {
        return Some(Value::Object(Map::new()));
    }
    let parts = split_top_level_commas(src)?;
    // A blank part (e.g. a trailing comma) is malformed, not skippable.
    if parts.iter().any(|part| part.trim().is_empty()) {
        return None;
    }
    // Assignment is only an assignment at top level, outside quotes and nested
    // delimiters; a `=` inside a quoted value or a nested object never flips the
    // call into keyword mode.
    let assignments: Vec<Option<usize>> = parts
        .iter()
        .map(|part| top_level_assignment(part))
        .collect();
    let any_keyword = assignments.iter().any(Option::is_some);
    let all_keyword = assignments.iter().all(Option::is_some);
    if any_keyword && !all_keyword {
        // F8: mixed positional and keyword arguments are diagnosed, not guessed.
        return None;
    }

    if all_keyword {
        let mut map = Map::new();
        for (part, eq) in parts.iter().zip(assignments) {
            let eq = eq?;
            let key = part[..eq].trim();
            if !is_identifier(key) {
                return None;
            }
            let value = parse_json_value(&part[eq + 1..])?;
            // F8: a duplicate keyword key is rejected before insertion rather
            // than silently overwriting the earlier value.
            if map.insert(key.to_string(), value).is_some() {
                return None;
            }
        }
        return Some(Value::Object(map));
    }

    // All positional: Gemma often emits `search("C++ Alliance")`.
    let mut values = Vec::with_capacity(parts.len());
    for part in &parts {
        values.push(parse_json_value(part)?);
    }
    let keys = positional_arg_keys(tool_name, values.len())?;
    let mut map = Map::new();
    for (key, value) in keys.iter().zip(values) {
        map.insert((*key).to_string(), value);
    }
    Some(Value::Object(map))
}

/// Byte offset of the first top-level `=` in `part`, outside quotes and nested
/// delimiters, or `None` when the part carries no top-level assignment.
fn top_level_assignment(part: &str) -> Option<usize> {
    let mut expected_closers: Vec<char> = Vec::new();
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in part.char_indices() {
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
            '(' => expected_closers.push(')'),
            '[' => expected_closers.push(']'),
            '{' => expected_closers.push('}'),
            ')' | ']' | '}' => {
                expected_closers.pop();
            }
            '=' if expected_closers.is_empty() => return Some(idx),
            _ => {}
        }
    }
    None
}

/// Decode one argument token as a complete JSON value.
///
/// Using JSON as the value grammar gives one symmetric codec with the renderer:
/// strings decode their escapes, and null, numbers, booleans, arrays, and
/// objects parse to the same [`Value`] the renderer emits. A bare word, an
/// unterminated string, or any other non-JSON token is rejected.
fn parse_json_value(token: &str) -> Option<Value> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(token).ok()
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
            for key in params.keys() {
                if required.contains(&key.as_str()) {
                    arg_bits.push(format!("{key}=..."));
                } else {
                    arg_bits.push(format!("{key}=...?"));
                }
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

/// Splits `src` on top-level commas, rejecting malformed argument syntax.
///
/// Returns `None` when a closer does not match its most recent opener (for
/// example `[` closed by `)`), when a closer is unmatched, when an opener is
/// left unclosed, when a quote is left open, or when an escape is left dangling,
/// so a corrupted argument list can never be split into valid-looking parts.
///
/// Delimiter nesting is tracked with a stack of expected closers rather than a
/// single depth counter, so a mismatched pair like `foo(a=[1)]` is rejected even
/// though its opener and closer counts happen to balance.
fn split_top_level_commas(src: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut expected_closers: Vec<char> = Vec::new();
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
            '(' => expected_closers.push(')'),
            '[' => expected_closers.push(']'),
            '{' => expected_closers.push('}'),
            ')' | ']' | '}' => {
                // A closer must match the most recent unmatched opener; a bare
                // or mismatched closer corrupts the argument structure.
                if expected_closers.pop() != Some(ch) {
                    return None;
                }
            }
            ',' if expected_closers.is_empty() => {
                parts.push(&src[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    if !expected_closers.is_empty() || in_quote.is_some() || escaped {
        return None;
    }
    parts.push(&src[start..]);
    Some(parts)
}

// --- Rendering helpers ---

/// Render parsed tool calls as the Gemma ` ```tool_code ` content dialect.
///
/// The renderer is symmetric with the parser: every argument is emitted as
/// `key=<json>` using the same JSON value grammar the parser decodes, so a
/// parsed call round-trips back to the identical [`ToolCall`] arguments. String
/// values are JSON-escaped, so quotes and control characters inside a value
/// cannot break the call syntax.
///
/// # Errors
/// Returns [`Error::Internal`] when a tool name or an argument key is not a
/// valid identifier, or when the arguments are not a JSON object - shapes the
/// `tool_code` grammar cannot faithfully represent.
pub(crate) fn render_tool_code_fence(calls: &[ToolCall]) -> Result<String> {
    let mut body = String::from("```tool_code\n");
    for call in calls {
        if !is_identifier(&call.name) {
            return Err(Error::Internal(
                "tool_code render: tool name is not a valid identifier",
            ));
        }
        body.push_str(&call.name);
        body.push('(');
        let Value::Object(map) = &call.arguments else {
            return Err(Error::Internal(
                "tool_code render: arguments must be a JSON object",
            ));
        };
        let mut first = true;
        for (key, value) in map {
            if !is_identifier(key) {
                return Err(Error::Internal(
                    "tool_code render: argument key is not a valid identifier",
                ));
            }
            if !first {
                body.push_str(", ");
            }
            first = false;
            body.push_str(key);
            body.push('=');
            body.push_str(&render_json_value(value)?);
        }
        body.push_str(")\n");
    }
    body.push_str("```");
    Ok(body)
}

/// Render one argument value as its JSON text (the symmetric inverse of
/// [`parse_json_value`]).
fn render_json_value(value: &Value) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|_| Error::Internal("tool_code render: value could not be serialized"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::CompletionResult;

    #[test]
    fn split_top_level_commas_rejects_malformed_syntax() {
        assert_eq!(
            split_top_level_commas("a, b"),
            Some(vec!["a", " b"]),
            "balanced input splits"
        );
        assert_eq!(
            split_top_level_commas("query=\"a\""),
            Some(vec!["query=\"a\""])
        );
        // Unbalanced / malformed forms must be rejected, not silently accepted.
        assert_eq!(split_top_level_commas("a, (b"), None, "open delimiter");
        assert_eq!(split_top_level_commas("a)b"), None, "unmatched close");
        assert_eq!(split_top_level_commas("\"unterminated"), None, "open quote");
        assert_eq!(split_top_level_commas("\"a\\"), None, "dangling escape");
        // Mismatched delimiters whose open/close counts balance must still be
        // rejected: a single depth counter would wrongly accept these.
        assert_eq!(
            split_top_level_commas("(a=[1)]"),
            None,
            "paren closed by bracket"
        );
        assert_eq!(
            split_top_level_commas("[a}"),
            None,
            "bracket closed by brace"
        );
        assert_eq!(split_top_level_commas("{a)"), None, "brace closed by paren");
        assert_eq!(
            split_top_level_commas("([)]"),
            None,
            "interleaved delimiters"
        );
        // A correctly nested list must still split at top level.
        assert_eq!(
            split_top_level_commas("a=[1, 2], b={x: 1}"),
            Some(vec!["a=[1, 2]", " b={x: 1}"]),
            "correctly nested delimiters split only at top level"
        );
    }

    #[test]
    fn parse_tool_code_call_rejects_malformed_argument_syntax() {
        // A call whose arguments have an unbalanced delimiter must not parse
        // into a ToolCall with corrupted arguments.
        assert!(parse_tool_code_call("search(query=\"a\", nested(b)", 0).is_none());
        assert!(parse_tool_code_call("search(a, (b)", 0).is_none());
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "id".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn value_grammar_round_trips_all_json_shapes() {
        // A call carrying string (with escapes), number, bool, null, array, and
        // object arguments must render and re-parse to the identical arguments.
        let original = call(
            "tool",
            serde_json::json!({
                "s": "he said \"hi\"\nbye",
                "n": 42,
                "f": 3.5,
                "b": true,
                "nul": null,
                "arr": [1, "two", false],
                "obj": { "k": "v", "nested": [1, 2] }
            }),
        );
        let fence = render_tool_code_fence(std::slice::from_ref(&original)).expect("renders");
        // Strip the fence framing and re-parse the single call line.
        let inner = fence
            .trim_start_matches("```tool_code\n")
            .trim_end_matches("\n```");
        let reparsed = parse_tool_code_call(inner, 0).expect("re-parses");
        assert_eq!(
            reparsed.arguments, original.arguments,
            "arguments must round-trip losslessly: {fence}"
        );
    }

    #[test]
    fn equals_inside_quoted_value_stays_positional() {
        // A `=` inside a quoted value must not flip the call to keyword mode.
        let parsed = parse_tool_code_call("search(\"a=b\")", 0).expect("parses");
        assert_eq!(parsed.arguments, serde_json::json!({ "query": "a=b" }));
    }

    #[test]
    fn string_escapes_are_decoded() {
        let parsed = parse_tool_code_call("echo(value=\"line1\\nline2\")", 0).expect("parses");
        assert_eq!(
            parsed.arguments,
            serde_json::json!({ "value": "line1\nline2" })
        );
    }

    #[test]
    fn mixed_positional_and_keyword_is_rejected() {
        assert!(parse_tool_code_call("search(\"a\", count=3)", 0).is_none());
    }

    #[test]
    fn duplicate_keyword_key_is_rejected() {
        assert!(parse_tool_code_call("search(query=\"a\", query=\"b\")", 0).is_none());
    }

    #[test]
    fn names_and_keys_must_be_identifiers_and_close_must_end_input() {
        // Name starting with a digit.
        assert!(parse_tool_code_call("3search(query=\"a\")", 0).is_none());
        // Punctuation in a keyword key.
        assert!(parse_tool_code_call("search(a-b=\"x\")", 0).is_none());
        // Trailing text after the close paren.
        assert!(parse_tool_code_call("search(query=\"a\") extra", 0).is_none());
        // Bare-word value is not valid JSON.
        assert!(parse_tool_code_call("search(query=bareword)", 0).is_none());
    }

    #[test]
    fn render_rejects_shapes_the_grammar_cannot_represent() {
        // Non-identifier argument key.
        let bad_key = call("tool", serde_json::json!({ "a-b": 1 }));
        assert!(matches!(
            render_tool_code_fence(std::slice::from_ref(&bad_key)),
            Err(Error::Internal(_))
        ));
        // Non-identifier tool name.
        let bad_name = call("3tool", serde_json::json!({}));
        assert!(matches!(
            render_tool_code_fence(std::slice::from_ref(&bad_name)),
            Err(Error::Internal(_))
        ));
        // Non-object arguments.
        let bad_args = call("tool", serde_json::json!([1, 2]));
        assert!(matches!(
            render_tool_code_fence(std::slice::from_ref(&bad_args)),
            Err(Error::Internal(_))
        ));
    }

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
    fn fenced_json_with_malformed_arguments_is_malformed_not_text() {
        // A fenced tool_calls blob (protocol intent) whose arguments are invalid
        // must surface as a malformed error, preserving the concrete decode
        // failure, rather than silently falling back to final text.
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```json\n{\"tool_calls\":[{\"id\":\"1\",\"type\":\"function\",\"function\":{\"name\":\"fetch\",\"arguments\":\"not json\"}}]}\n```"
                }
            }]
        });
        assert!(
            matches!(dialect.parse_turn(&body), Err(Error::MalformedResponse(_))),
            "malformed fenced arguments must be a malformed error, not text"
        );
    }

    #[test]
    fn malformed_tool_code_fence_is_malformed_not_text() {
        let dialect = Gemma3ToolCodeDialect;
        // Leading tool_code fence commits to protocol; a bad call line is an error.
        let bad_call = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\nnot a call\n```" } }]
        });
        assert!(matches!(
            dialect.parse_turn(&bad_call),
            Err(Error::MalformedResponse(_))
        ));

        // Unterminated fence is malformed, not text.
        let unterminated = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\nsearch(query=\"a\")" } }]
        });
        assert!(matches!(
            dialect.parse_turn(&unterminated),
            Err(Error::MalformedResponse(_))
        ));

        // Trailing prose after a valid fence in a protocol turn is malformed.
        let trailing = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\nsearch(query=\"a\")\n```\nthen prose" } }]
        });
        assert!(matches!(
            dialect.parse_turn(&trailing),
            Err(Error::MalformedResponse(_))
        ));
    }

    #[test]
    fn backticks_inside_quoted_value_do_not_close_fence() {
        // A ``` inside a quoted argument value must not terminate the fence at
        // the first triple-backtick; only a standalone ``` line closes it.
        let dialect = Gemma3ToolCodeDialect;
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "```tool_code\necho(value=\"a ``` b\")\n```" } }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "value": "a ``` b" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn ordinary_json_data_fence_stays_text() {
        // A ```json code block that is not a tool_calls blob is ordinary data.
        let dialect = Gemma3ToolCodeDialect;
        let content = "```json\n{\"answer\": 42}\n```";
        let body = serde_json::json!({
            "choices": [{ "message": { "content": content } }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        assert!(matches!(turn.outcome, CompletionResult::Text(_)));
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
    fn consecutive_fences_mint_unique_ids() {
        let dialect = Gemma3ToolCodeDialect;
        let content =
            "```tool_code\nsearch(query=\"a\")\n```\n\n```tool_code\nfetch(\"https://x\")\n```";
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": content } }]
        });
        let turn = dialect.parse_turn(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].id, "call_tool_code_0");
                assert_eq!(
                    calls[1].id, "call_tool_code_1",
                    "ids must not restart per fence"
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn echo_rejects_uncorrelated_results() {
        let dialect = Gemma3ToolCodeDialect;
        let calls = vec![ToolCall {
            id: "call_tool_code_0".into(),
            name: "search".into(),
            arguments: serde_json::json!({ "query": "rust" }),
        }];
        // Wrong id breaks correlation; the conversation must stay untouched.
        let results = vec![("mismatched_id".to_string(), "text".to_string())];
        let mut conversation = Vec::new();
        let error = dialect
            .echo_tool_results(&mut conversation, &calls, &results)
            .expect_err("uncorrelated results must be rejected");
        assert!(matches!(error, Error::Internal(_)));
        assert!(
            conversation.is_empty(),
            "no message may be appended on failure"
        );
    }

    #[test]
    fn optional_params_survive_in_guide() {
        let dialect = Gemma3ToolCodeDialect;
        let mut body = serde_json::json!({
            "model": "gemma-3",
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "search",
                    "parameters": {
                        "type": "object",
                        "properties": { "query": { "type": "string" }, "count": { "type": "number" } },
                        "required": ["query"]
                    }
                }
            }]
        });
        let mut req = DialectRequest { body: &mut body };
        dialect.prepare_request(&mut req).unwrap();
        let guide = body["messages"][0]["content"].as_str().unwrap();
        assert!(
            guide.contains("query=..."),
            "required param present: {guide}"
        );
        assert!(
            guide.contains("count=...?"),
            "optional param must survive marked: {guide}"
        );
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
        dialect
            .echo_tool_results(&mut conversation, &calls, &results)
            .expect("correlated results echo cleanly");

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
