//! Emulated tool-calling dialects: the Gemma3 `tool_code` content-fence
//! protocol.
//!
//! Gemma has no native tool array, so a model configured with
//! `tool_dialect = "gemma3_tool_code"` gets tool calling emulated at the
//! gateway boundary: the outgoing request has its OpenAI `tools` translated
//! into a plain-language system guide (and `tools`/`tool_choice` stripped),
//! and the reply's content is scanned for a ` ```tool_code ` fence whose
//! `name(key=<json>)` lines become OpenAI `tool_calls` objects.
//!
//! Recovery discipline: the gateway is terminal with no fallback, so a
//! recognized-but-malformed fence never fails the turn and never masquerades
//! as final text - the choice's content is emptied and a `gateway_warning`
//! field holds the reason (also logged at warn). A malformed fence is a
//! post-receipt parse failure, kept distinct from pre-call translation
//! errors (a non-array `tools` request field is a malformed request, rejected
//! before the upstream call).

use serde_json::{Map, Value};

use crate::error::GatewayError;
use crate::upstream::StreamedChunks;
use crate::wire::{ChatChunk, ChatChunkChoice, ChatRequest, ChatResponse};

mod content;
mod guide;

use self::content::parse_content_tool_dialect;
use self::guide::render_tool_guide;

/// The `tool_dialect` config value selecting this dialect.
pub(crate) use gateway_routing::GEMMA3_TOOL_CODE;

/// Translates an outgoing request for the emulated dialect: strips the tool
/// surface the backend cannot honor and prepends the tool-code system guide.
///
/// Mutation is atomic: the guide is fully rendered before anything is
/// removed, so a preparation failure leaves the request unmodified.
///
/// # Errors
/// Returns [`GatewayError::MalformedRequest`] when `tools` is present but not
/// an array - a pre-call translation error, never confused with a post-receipt
/// parse failure.
pub(crate) fn prepare_request(request: &mut ChatRequest) -> Result<(), GatewayError> {
    let guide = match request.rest.get("tools") {
        None | Some(Value::Null) => None,
        Some(Value::Array(tools)) => render_tool_guide(tools),
        Some(_) => {
            return Err(GatewayError::MalformedRequest(
                "request `tools` was present but not an array".to_owned(),
            ));
        }
    };
    request.rest.remove("tools");
    request.rest.remove("tool_choice");
    if let Some(guide) = guide {
        request.messages.insert(
            0,
            serde_json::json!({
                "role": "system",
                "content": guide,
            }),
        );
    }
    Ok(())
}

/// Parses each choice's message content for tool fences and rewrites the
/// response in place: well-formed fences become wire `tool_calls` with a
/// `tool_calls` finish reason; a malformed fence empties the content and
/// attaches a `gateway_warning`, logged at warn; ordinary prose is untouched.
pub(crate) fn apply_response(response: &mut ChatResponse, model: &str) {
    for choice in &mut response.choices {
        let Some(choice_object) = choice.as_object_mut() else {
            continue;
        };
        let Some(message) = choice_object
            .get_mut("message")
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.trim().is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        match parse_content_tool_dialect(&content) {
            ContentParse::NotProtocol => {}
            ContentParse::Calls(calls) => {
                message.insert("content".to_owned(), Value::Null);
                message.insert(
                    "tool_calls".to_owned(),
                    calls.iter().map(ParsedCall::to_wire).collect(),
                );
                choice_object.insert(
                    "finish_reason".to_owned(),
                    Value::String("tool_calls".to_owned()),
                );
            }
            ContentParse::Malformed(reason) => {
                tracing::warn!(
                    model = %model,
                    warning = %reason,
                    "emulated tool call failed to parse; returning empty content"
                );
                message.insert("content".to_owned(), Value::String(String::new()));
                message.insert("gateway_warning".to_owned(), Value::String(reason));
            }
        }
    }
}

/// Re-emits a dialect-rewritten buffered response as a synthetic chunk
/// stream, so the emulated dialect serves `stream: true` callers.
///
/// The tool-code fence can only be parsed from the whole reply, so the
/// streaming path buffers one upstream round trip and re-emits the rewritten
/// response: one chunk holds each choice's message as its delta (tool-call
/// entries gain the fragment `index` streaming clients merge by), and a
/// trailing empty-choices summary chunk includes the response's top-level
/// `usage`/`timings`/`metrics` passthrough fields, so
/// `stream_options.include_usage` semantics survive the buffered round trip.
pub(crate) fn response_chunks(response: ChatResponse) -> StreamedChunks {
    use futures_util::StreamExt as _;

    let mut choices: Vec<ChatChunkChoice> = Vec::new();
    for (position, choice) in response.choices.into_iter().enumerate() {
        let Some(choice_object) = choice.as_object() else {
            continue;
        };
        let index = choice_object
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| u32::try_from(index).ok())
            .unwrap_or(u32::try_from(position).unwrap_or(u32::MAX));
        let mut delta = choice_object.get("message").cloned().unwrap_or(Value::Null);
        if let Some(calls) = delta.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for (call_index, call) in calls.iter_mut().enumerate() {
                if let Some(call) = call.as_object_mut() {
                    call.insert("index".to_owned(), Value::from(call_index));
                }
            }
        }
        let mut rest = Map::new();
        if let Some(finish) = choice_object.get("finish_reason") {
            rest.insert("finish_reason".to_owned(), finish.clone());
        }
        choices.push(ChatChunkChoice { index, delta, rest });
    }
    let mut chunks = vec![ChatChunk {
        model: response.model.clone(),
        choices,
        rest: Map::new(),
    }];
    let mut summary = Map::new();
    for key in ["usage", "timings", "metrics"] {
        if let Some(section) = response.rest.get(key).filter(|section| !section.is_null()) {
            summary.insert(key.to_owned(), section.clone());
        }
    }
    if !summary.is_empty() {
        chunks.push(ChatChunk {
            model: response.model,
            choices: Vec::new(),
            rest: summary,
        });
    }
    let items: Vec<_> = chunks.into_iter().map(Ok).collect();
    StreamedChunks {
        content_type: None,
        cache_control: None,
        chunks: futures_util::stream::iter(items).boxed(),
    }
}

/// One parsed tool call, rendered to the OpenAI wire shape by [`to_wire`].
struct ParsedCall {
    id: String,
    name: String,
    arguments: Value,
}

impl ParsedCall {
    /// Renders as an OpenAI `tool_calls` entry: `function.arguments` is the
    /// arguments object JSON-encoded into a string, as the wire shape requires.
    fn to_wire(&self) -> Value {
        serde_json::json!({
            "id": self.id,
            "type": "function",
            "function": {
                "name": self.name,
                "arguments": self.arguments.to_string(),
            }
        })
    }
}

/// The three-way outcome of classifying model content.
///
/// Distinguishing malformed protocol from ordinary prose is the whole point: a
/// recognized tool fence whose contents are invalid must surface as a warning,
/// not collapse to `NotProtocol` alongside genuine prose and become final text.
enum ContentParse {
    /// The content is ordinary prose with no recognized tool fence.
    NotProtocol,
    /// The content is one or more well-formed tool fences.
    Calls(Vec<ParsedCall>),
    /// The content opened a recognized tool fence but its contents are invalid.
    Malformed(String),
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

/// Parses one `name(args)` call line into a [`ParsedCall`].
///
/// The name must be an identifier, the arguments sit between the first `(`
/// and the final `)`, and the `)` must end the non-whitespace input so
/// trailing text after the call is rejected rather than silently ignored.
fn parse_tool_code_call(line: &str, index: usize) -> Option<ParsedCall> {
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
    Some(ParsedCall {
        id: format!("call_tool_code_{index}"),
        name: name.to_string(),
        arguments,
    })
}

/// Parses the argument list into a JSON object.
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
        // Mixed positional and keyword arguments are diagnosed, not guessed.
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
            // A duplicate keyword key is rejected before insertion rather
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

/// Scans quote and delimiter state, invoking `visit_top_level` only outside
/// quotes and nested delimiters.
fn scan_top_level<T>(
    src: &str,
    validate_structure: bool,
    mut visit_top_level: impl FnMut(usize, char) -> Option<T>,
) -> std::result::Result<Option<T>, ()> {
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
                let expected = expected_closers.pop();
                if validate_structure && expected != Some(ch) {
                    return Err(());
                }
            }
            _ if expected_closers.is_empty() => {
                if let Some(result) = visit_top_level(idx, ch) {
                    return Ok(Some(result));
                }
            }
            _ => {}
        }
    }
    if validate_structure && (!expected_closers.is_empty() || in_quote.is_some() || escaped) {
        return Err(());
    }
    Ok(None)
}

/// Byte offset of the first top-level `=` in `part`, outside quotes and nested
/// delimiters, or `None` when the part has no top-level assignment.
fn top_level_assignment(part: &str) -> Option<usize> {
    scan_top_level(part, false, |idx, ch| (ch == '=').then_some(idx))
        .ok()
        .flatten()
}

/// Decodes one argument token as a complete JSON value.
///
/// Strings decode their escapes, and null, numbers, booleans, arrays, and
/// objects parse to the same [`Value`] the wire renderer emits. A bare word, an
/// unterminated string, or any other non-JSON token is rejected.
fn parse_json_value(token: &str) -> Option<Value> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    serde_json::from_str::<Value>(token).ok()
}

/// Maps positional `tool_code` args onto schema-ish parameter names.
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
    scan_top_level(src, true, |idx, ch| {
        if ch == ',' {
            parts.push(&src[start..idx]);
            start = idx + ch.len_utf8();
        }
        None::<()>
    })
    .ok()?;
    parts.push(&src[start..]);
    Some(parts)
}

#[cfg(test)]
mod tests;
