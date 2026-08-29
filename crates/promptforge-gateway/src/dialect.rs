//! Emulated tool-calling dialects: the Gemma3 `tool_code` content-fence
//! protocol, ported from `promptforge-core`'s `dialects::gemma3_tool_code`.
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
//! field carries the reason (also logged at warn). A malformed fence is a
//! post-receipt parse failure, kept distinct from pre-call translation
//! errors (a non-array `tools` request field is a malformed request, rejected
//! before the upstream call).

use serde_json::{Map, Value};

use crate::error::GatewayError;
use crate::wire::{ChatRequest, ChatResponse};

/// The `tool_dialect` config value selecting this dialect.
pub(crate) use promptforge_gateway_routing::GEMMA3_TOOL_CODE;

/// Translate an outgoing request for the emulated dialect: strip the tool
/// surface the backend cannot honor and prepend the tool-code system guide.
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

/// Parse each choice's message content for tool fences and rewrite the
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

/// One parsed tool call, rendered to the OpenAI wire shape by [`to_wire`].
struct ParsedCall {
    id: String,
    name: String,
    arguments: Value,
}

impl ParsedCall {
    /// Render as an OpenAI `tool_calls` entry: `function.arguments` is the
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
            Peel::Malformed(reason) => return ContentParse::Malformed(reason),
            Peel::NotAFence => {}
        }
        match peel_json_tool_calls_fence(rest) {
            Peel::Calls(parsed, remain) => {
                saw_fence = true;
                calls.extend(parsed);
                rest = remain;
                continue;
            }
            Peel::Malformed(reason) => return ContentParse::Malformed(reason),
            Peel::NotAFence => {}
        }
        // No tool fence here. If we already consumed one, this is trailing junk
        // in an otherwise-protocol turn; otherwise it is ordinary prose.
        if saw_fence {
            return ContentParse::Malformed("trailing content after tool_code fence".to_owned());
        }
        return ContentParse::NotProtocol;
    }
    if calls.is_empty() {
        ContentParse::NotProtocol
    } else {
        ContentParse::Calls(calls)
    }
}

/// The outcome of peeling one leading fence.
enum Peel<'a> {
    /// A valid tool fence with its parsed calls and the remaining input.
    Calls(Vec<ParsedCall>, &'a str),
    /// A recognized tool-protocol fence whose contents are invalid.
    Malformed(String),
    /// No tool-protocol fence at this position (ordinary prose or data fence).
    NotAFence,
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
        return Peel::Malformed("unterminated tool_code fence".to_owned());
    };
    let mut calls = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(call) = parse_tool_code_call(line, *next_id) else {
            return Peel::Malformed("malformed tool_code call line".to_owned());
        };
        *next_id += 1;
        calls.push(call);
    }
    if calls.is_empty() {
        return Peel::Malformed("tool_code fence contained no calls".to_owned());
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
    match parse_openai_tool_calls(raw_calls) {
        Ok(calls) => Peel::Calls(calls, after),
        Err(reason) => Peel::Malformed(reason),
    }
}

/// Parse the OpenAI `message.tool_calls` array into [`ParsedCall`]s.
///
/// Each call must be an object with a nonblank string `id`, a `type` of
/// `"function"`, an object `function` carrying a nonblank string `name`, and
/// an `arguments` field that is present, a JSON-encoded string, and decodes
/// to a JSON object. Blank identifiers, duplicate ids within the turn, missing
/// or null arguments, and arguments that do not decode to an object are all
/// rejected rather than coerced.
fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ParsedCall>, String> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for raw in raw_calls {
        if !raw.is_object() {
            return Err("tool call was not an object".to_owned());
        }
        match raw.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            _ => return Err("tool call `type` must be the string \"function\"".to_owned()),
        }
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "tool call had no string id".to_owned())?;
        if id.trim().is_empty() {
            return Err("tool call id was blank".to_owned());
        }
        if !seen_ids.insert(id) {
            return Err(format!("duplicate tool call id {id:?} within one turn"));
        }
        let function = raw
            .get("function")
            .ok_or_else(|| "tool call had no function".to_owned())?;
        if !function.is_object() {
            return Err("tool call `function` was not an object".to_owned());
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "tool call had no string name".to_owned())?;
        if name.trim().is_empty() {
            return Err("tool call name was blank".to_owned());
        }
        // OpenAI encodes `function.arguments` as a JSON string. It must be
        // present, a string, and decode to a JSON object - the shape tools
        // accept. Missing, null, non-string, invalid-JSON, and non-object
        // decoded values are all rejected rather than coerced.
        let arguments = match function.get("arguments") {
            Some(Value::String(raw_args)) => {
                let decoded = serde_json::from_str::<Value>(raw_args)
                    .map_err(|error| format!("tool call arguments were not valid JSON: {error}"))?;
                if !decoded.is_object() {
                    return Err("tool call arguments did not decode to an object".to_owned());
                }
                decoded
            }
            _ => return Err("tool call arguments were missing or not a string".to_owned()),
        };
        calls.push(ParsedCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
        });
    }
    Ok(calls)
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

/// Parse one `name(args)` call line into a [`ParsedCall`].
///
/// The name must be an identifier, the arguments live between the first `(` and
/// the final `)`, and the `)` must end the non-whitespace input so trailing text
/// after the call is rejected rather than silently ignored.
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
/// delimiters, or `None` when the part carries no top-level assignment.
fn top_level_assignment(part: &str) -> Option<usize> {
    scan_top_level(part, false, |idx, ch| (ch == '=').then_some(idx))
        .ok()
        .flatten()
}

/// Decode one argument token as a complete JSON value.
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
fn render_tool_guide(list: &[Value]) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str, properties: &Value, required: &[&str]) -> Value {
        let required: Vec<&str> = required.to_vec();
        serde_json::json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }
            }
        })
    }

    #[test]
    fn guide_lists_every_parameter_with_optional_markers() {
        let tools = vec![tool(
            "search",
            "search the web",
            &serde_json::json!({
                "query": { "type": "string" },
                "count": { "type": "integer" }
            }),
            &["query"],
        )];
        let guide = render_tool_guide(&tools).expect("renders");
        assert!(guide.contains("tool_code"), "teaches the fence: {guide}");
        assert!(
            guide.contains("- search(count=...?, query=...): search the web"),
            "optional parameter marked, required unmarked: {guide}"
        );
    }

    #[test]
    fn guide_is_none_for_empty_or_unusable_lists() {
        assert_eq!(render_tool_guide(&[]), None);
        assert_eq!(
            render_tool_guide(&[serde_json::json!({ "type": "function" })]),
            None,
            "a tool without a function name lists nothing"
        );
    }

    #[test]
    fn split_top_level_commas_rejects_malformed_syntax() {
        assert_eq!(split_top_level_commas("a, b"), Some(vec!["a", " b"]));
        // Unbalanced / malformed forms must be rejected, not silently accepted.
        assert_eq!(split_top_level_commas("a, (b"), None, "open delimiter");
        assert_eq!(split_top_level_commas("a)b"), None, "unmatched close");
        assert_eq!(split_top_level_commas("\"unterminated"), None, "open quote");
        assert_eq!(split_top_level_commas("\"a\\"), None, "dangling escape");
        // Mismatched delimiters whose open/close counts balance must still be
        // rejected: a single depth counter would wrongly accept these.
        assert_eq!(split_top_level_commas("(a=[1)]"), None);
        assert_eq!(split_top_level_commas("([)]"), None);
        assert_eq!(
            split_top_level_commas("a=[1, 2], b={x: 1}"),
            Some(vec!["a=[1, 2]", " b={x: 1}"]),
            "correctly nested delimiters split only at top level"
        );
    }

    #[test]
    fn parses_keyword_and_positional_calls() {
        let keyword = parse_tool_code_call("search(query=\"a\", count=3)", 0).expect("parses");
        assert_eq!(keyword.name, "search");
        assert_eq!(
            keyword.arguments,
            serde_json::json!({ "query": "a", "count": 3 })
        );
        let positional = parse_tool_code_call("search(\"a=b\")", 1).expect("parses");
        assert_eq!(positional.id, "call_tool_code_1");
        assert_eq!(positional.arguments, serde_json::json!({ "query": "a=b" }));
    }

    #[test]
    fn rejects_malformed_call_lines() {
        // Mixed positional and keyword.
        assert!(parse_tool_code_call("search(\"a\", count=3)", 0).is_none());
        // Duplicate keyword key.
        assert!(parse_tool_code_call("search(query=\"a\", query=\"b\")", 0).is_none());
        // Name starting with a digit.
        assert!(parse_tool_code_call("3search(query=\"a\")", 0).is_none());
        // Trailing text after the close paren.
        assert!(parse_tool_code_call("search(query=\"a\") extra", 0).is_none());
        // Bare-word value is not valid JSON.
        assert!(parse_tool_code_call("search(query=bareword)", 0).is_none());
        // Unbalanced argument delimiters.
        assert!(parse_tool_code_call("search(query=\"a\", nested(b)", 0).is_none());
    }

    #[test]
    fn content_classification_is_three_way() {
        // Ordinary prose, including prose that mentions a fence mid-text.
        assert!(matches!(
            parse_content_tool_dialect("just prose"),
            ContentParse::NotProtocol
        ));
        assert!(matches!(
            parse_content_tool_dialect("let me call ```tool_code\nsearch()\n``` now"),
            ContentParse::NotProtocol
        ));
        // A well-formed fence parses to calls.
        match parse_content_tool_dialect("```tool_code\nsearch(query=\"a\")\n```") {
            ContentParse::Calls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search");
            }
            other => panic!("expected calls, got {}", variant_name(&other)),
        }
        // Recognized-but-malformed fences never masquerade as prose.
        let unterminated = parse_content_tool_dialect("```tool_code\nsearch(query=\"a\")\n");
        assert!(
            matches!(unterminated, ContentParse::Malformed(_)),
            "unterminated fence"
        );
        let empty = parse_content_tool_dialect("```tool_code\n```");
        assert!(matches!(empty, ContentParse::Malformed(_)), "empty fence");
        let trailing = parse_content_tool_dialect("```tool_code\nsearch(query=\"a\")\n```\nafter");
        assert!(
            matches!(trailing, ContentParse::Malformed(_)),
            "trailing content after a protocol turn"
        );
    }

    #[test]
    fn json_tool_calls_fence_is_recognized() {
        let content = "```json\n{\"tool_calls\": [{\"id\": \"c1\", \"type\": \"function\", \"function\": {\"name\": \"search\", \"arguments\": \"{\\\"query\\\": \\\"a\\\"}\"}}]}\n```";
        match parse_content_tool_dialect(content) {
            ContentParse::Calls(calls) => {
                assert_eq!(calls[0].id, "c1");
                assert_eq!(calls[0].arguments, serde_json::json!({ "query": "a" }));
            }
            other => panic!("expected calls, got {}", variant_name(&other)),
        }
        // A json fence without a tool_calls payload is an ordinary data fence.
        assert!(matches!(
            parse_content_tool_dialect("```json\n{\"answer\": 42}\n```"),
            ContentParse::NotProtocol
        ));
    }

    fn variant_name(parse: &ContentParse) -> &'static str {
        match parse {
            ContentParse::NotProtocol => "not-protocol",
            ContentParse::Calls(_) => "calls",
            ContentParse::Malformed(_) => "malformed",
        }
    }

    fn request_with_tools(tools: Value) -> ChatRequest {
        let mut request = ChatRequest {
            model: "m".to_owned(),
            messages: vec![serde_json::json!({ "role": "user", "content": "hi" })],
            stream: false,
            rest: Map::new(),
        };
        request.rest.insert("tools".to_owned(), tools);
        request
            .rest
            .insert("tool_choice".to_owned(), Value::String("auto".to_owned()));
        request
    }

    #[test]
    fn prepare_request_strips_tools_and_prepends_the_guide() {
        let tools = serde_json::json!([tool(
            "search",
            "search the web",
            &serde_json::json!({ "query": { "type": "string" } }),
            &["query"]
        )]);
        let mut request = request_with_tools(tools);
        prepare_request(&mut request).expect("valid tools");
        assert!(!request.rest.contains_key("tools"));
        assert!(!request.rest.contains_key("tool_choice"));
        let first = &request.messages[0];
        assert_eq!(first.get("role").and_then(Value::as_str), Some("system"));
        let guide = first
            .get("content")
            .and_then(Value::as_str)
            .expect("guide content");
        assert!(guide.contains("search(query=...)"), "guide: {guide}");
    }

    #[test]
    fn prepare_request_without_tools_only_strips() {
        let mut request = request_with_tools(Value::Null);
        prepare_request(&mut request).expect("null tools");
        assert!(!request.rest.contains_key("tools"));
        assert!(!request.rest.contains_key("tool_choice"));
        assert_eq!(request.messages.len(), 1, "no guide without usable tools");
    }

    #[test]
    fn prepare_request_rejects_non_array_tools() {
        let mut request = request_with_tools(serde_json::json!("nope"));
        assert!(matches!(
            prepare_request(&mut request),
            Err(GatewayError::MalformedRequest(_))
        ));
        assert!(
            request.rest.contains_key("tools"),
            "a failed preparation leaves the request unmodified"
        );
    }

    fn response_with_content(content: &str) -> ChatResponse {
        ChatResponse {
            model: "m".to_owned(),
            choices: vec![serde_json::json!({
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            })],
            rest: Map::new(),
        }
    }

    #[test]
    fn apply_response_rewrites_a_fence_into_tool_calls() {
        let mut response = response_with_content("```tool_code\nsearch(query=\"a\")\n```");
        apply_response(&mut response, "m");
        let choice = &response.choices[0];
        let message = choice.get("message").expect("message");
        assert_eq!(message.get("content"), Some(&Value::Null));
        assert_eq!(
            choice.get("finish_reason").and_then(Value::as_str),
            Some("tool_calls")
        );
        let calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .expect("tool_calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].pointer("/function/name").and_then(Value::as_str),
            Some("search")
        );
        // OpenAI encodes arguments as a JSON string.
        assert_eq!(
            calls[0]
                .pointer("/function/arguments")
                .and_then(Value::as_str),
            Some("{\"query\":\"a\"}")
        );
    }

    #[test]
    fn apply_response_warns_and_empties_a_malformed_fence() {
        let mut response = response_with_content("```tool_code\nsearch(query=bareword)\n```");
        apply_response(&mut response, "m");
        let message = response.choices[0].get("message").expect("message");
        assert_eq!(
            message.get("content").and_then(Value::as_str),
            Some(""),
            "malformed protocol never masquerades as final text"
        );
        assert!(
            message.get("gateway_warning").is_some(),
            "the warning is always present on recovery"
        );
    }

    #[test]
    fn apply_response_leaves_prose_untouched() {
        let mut response = response_with_content("just a reply");
        apply_response(&mut response, "m");
        let message = response.choices[0].get("message").expect("message");
        assert_eq!(
            message.get("content").and_then(Value::as_str),
            Some("just a reply")
        );
        assert!(message.get("tool_calls").is_none());
        assert!(message.get("gateway_warning").is_none());
    }
}
