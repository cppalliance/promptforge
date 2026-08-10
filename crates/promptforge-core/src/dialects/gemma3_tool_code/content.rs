//! Classify model content into prose, tool calls, or malformed protocol.
//!
//! Gemma emits tool calls inside content fences rather than a wire `tool_calls`
//! array. This module recognizes a leading ` ```tool_code ` fence (or an interim
//! ` ```json ` `tool_calls` blob), scans fences quote-aware, and distinguishes a
//! recognized-but-malformed fence from ordinary prose so malformed control
//! syntax cannot masquerade as final text.

use serde_json::Value;

use crate::Error;
use crate::client::ToolCall;

use super::codec::parse_tool_code_call;

/// The three-way outcome of classifying model content.
///
/// Distinguishing malformed protocol from ordinary prose is the whole point: a
/// recognized tool fence whose contents are invalid must surface as an error,
/// not collapse to `None` alongside genuine prose and become final text.
pub(crate) enum ContentParse {
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
pub(crate) fn parse_content_tool_dialect(content: &str) -> ContentParse {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_close_ignores_backticks_inside_quotes() {
        // A ``` inside a quoted value must not be treated as the closing fence.
        let input = "echo(value=\"a ``` b\")\n```\ntrailing";
        let (body, after) = split_fence_close_standalone(input).expect("closes at standalone line");
        assert_eq!(body, "echo(value=\"a ``` b\")\n");
        assert_eq!(after, "trailing");
    }

    #[test]
    fn unterminated_fence_has_no_standalone_close() {
        assert!(split_fence_close_standalone("search(query=\"a\")").is_none());
    }
}
