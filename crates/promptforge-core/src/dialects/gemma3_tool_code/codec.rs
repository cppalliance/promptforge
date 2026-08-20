//! The symmetric `tool_code` call codec.
//!
//! One grammar parses and renders a `name(args)` call: arguments are all
//! keyword (`key=<json>`) or all positional (`<json>`), values are complete
//! JSON values, and identifiers are validated on both sides. Parsing and
//! rendering are inverses, so a parsed call round-trips to identical arguments.

use serde_json::{Map, Value};

use crate::client::ToolCall;
use crate::{Error, Result};

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
pub(crate) fn parse_tool_code_call(line: &str, index: usize) -> Option<ToolCall> {
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
}
