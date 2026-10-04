//! Content classification: peeling the leading tool fences off a reply,
//! ` ```tool_code ` call lines or OpenAI `tool_calls` in a JSON or bare
//! code fence.

use serde_json::Value;

use super::{ContentParse, ParsedCall, parse_tool_code_call};

/// Classifies model content as prose, tool calls, or malformed protocol.
///
/// The content is protocol only when it begins with a recognized tool fence;
/// prose that merely mentions a fence later stays text. Once protocol intent is
/// established, every fence must parse and no trailing non-fence content may
/// remain, or the whole turn is malformed.
pub(super) fn parse_content_tool_dialect(content: &str) -> ContentParse {
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

/// Peels one leading ` ```tool_code ` fence into Python-style `name(k=v)` calls.
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

/// Peels one leading ` ```json ` / ` ``` ` fence that holds OpenAI `tool_calls`.
///
/// A code fence is only tool protocol when its body decodes to a JSON object
/// with a non-empty `tool_calls` array; anything else is an ordinary data
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
        Err(rejection) => Peel::Malformed(crate::error::error_chain(&rejection)),
    }
}

/// Why one OpenAI `tool_calls` entry was rejected rather than coerced.
///
/// The rendered `source()` chain becomes the turn's `gateway_warning`, so
/// each variant's message is the exact wire string and a cause-bearing
/// variant contributes its cause through `source()`, not its message.
#[derive(Debug, thiserror::Error)]
enum ToolCallRejection {
    /// The entry was not a JSON object.
    #[error("tool call was not an object")]
    NotObject,
    /// The entry's `type` was not the string `"function"`.
    #[error("tool call `type` must be the string \"function\"")]
    TypeNotFunction,
    /// The entry had no string `id`.
    #[error("tool call had no string id")]
    NoStringId,
    /// The entry's `id` was blank.
    #[error("tool call id was blank")]
    BlankId,
    /// The entry's `id` already appeared earlier in the same turn.
    #[error("duplicate tool call id {0:?} within one turn")]
    DuplicateId(String),
    /// The entry had no `function` member.
    #[error("tool call had no function")]
    NoFunction,
    /// The entry's `function` was not an object.
    #[error("tool call `function` was not an object")]
    FunctionNotObject,
    /// The function had no string `name`.
    #[error("tool call had no string name")]
    NoStringName,
    /// The function's `name` was blank.
    #[error("tool call name was blank")]
    BlankName,
    /// The function's `arguments` string did not decode as JSON. The decode
    /// failure is retained as the `source()`; the wire-warning renderer
    /// walks the chain to include its text.
    #[error("tool call arguments were not valid JSON")]
    ArgumentsNotJson(#[source] serde_json::Error),
    /// The decoded `arguments` were not a JSON object.
    #[error("tool call arguments did not decode to an object")]
    ArgumentsNotObject,
    /// The function's `arguments` were missing or not a string.
    #[error("tool call arguments were missing or not a string")]
    ArgumentsMissing,
}

/// Parses the OpenAI `message.tool_calls` array into [`ParsedCall`]s.
///
/// Each call must be an object with a nonblank string `id`, a `type` of
/// `"function"`, an object `function` with a nonblank string `name`, and
/// an `arguments` field that is present, a JSON-encoded string, and decodes
/// to a JSON object. Blank identifiers, duplicate ids within the turn, missing
/// or null arguments, and arguments that do not decode to an object are all
/// rejected rather than coerced.
fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ParsedCall>, ToolCallRejection> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for raw in raw_calls {
        if !raw.is_object() {
            return Err(ToolCallRejection::NotObject);
        }
        match raw.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            _ => return Err(ToolCallRejection::TypeNotFunction),
        }
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or(ToolCallRejection::NoStringId)?;
        if id.trim().is_empty() {
            return Err(ToolCallRejection::BlankId);
        }
        if !seen_ids.insert(id) {
            return Err(ToolCallRejection::DuplicateId(id.to_owned()));
        }
        let function = raw.get("function").ok_or(ToolCallRejection::NoFunction)?;
        if !function.is_object() {
            return Err(ToolCallRejection::FunctionNotObject);
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or(ToolCallRejection::NoStringName)?;
        if name.trim().is_empty() {
            return Err(ToolCallRejection::BlankName);
        }
        // OpenAI encodes `function.arguments` as a JSON string. It must be
        // present, a string, and decode to a JSON object - the shape tools
        // accept. Missing, null, non-string, invalid-JSON, and non-object
        // decoded values are all rejected rather than coerced.
        let arguments = match function.get("arguments") {
            Some(Value::String(raw_args)) => {
                let decoded = serde_json::from_str::<Value>(raw_args)
                    .map_err(ToolCallRejection::ArgumentsNotJson)?;
                if !decoded.is_object() {
                    return Err(ToolCallRejection::ArgumentsNotObject);
                }
                decoded
            }
            _ => return Err(ToolCallRejection::ArgumentsMissing),
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

/// Splits `input` at the first standalone closing fence line (a line whose
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
