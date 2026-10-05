//! Tests for the tool guide and the content tool-call parser.

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

#[test]
fn json_tool_calls_fence_malformed_arguments_warning_names_the_decode_error() {
    // `arguments` is a string, but not JSON: the fence is recognized as
    // tool protocol and the wire warning must name the decode failure
    // from the rejection's source chain, not just the rejection message.
    let content = "```json\n{\"tool_calls\": [{\"id\": \"c1\", \"type\": \"function\", \"function\": {\"name\": \"search\", \"arguments\": \"{not json\"}}]}\n```";
    let expected_cause = serde_json::from_str::<Value>("{not json")
        .expect_err("the fixture arguments must not decode")
        .to_string();
    match parse_content_tool_dialect(content) {
        ContentParse::Malformed(warning) => {
            assert!(
                warning.contains("tool call arguments were not valid JSON"),
                "warning must name the rejection: {warning}"
            );
            assert!(
                warning.contains(&expected_cause),
                "warning must include the decode error {expected_cause:?}: {warning}"
            );
        }
        other => panic!("expected malformed, got {}", variant_name(&other)),
    }
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

#[tokio::test]
async fn response_chunks_stream_the_rewritten_message_and_summary() {
    use futures_util::StreamExt as _;

    // A fence-rewritten response converted for a stream: true caller. The
    // delta must include the tool calls with fragment indices, the
    // finish reason must be set on the chunk choice, and the top-level
    // usage must arrive on a trailing empty-choices summary chunk.
    let mut response = response_with_content("```tool_code\nsearch(query=\"a\")\n```");
    response.rest.insert(
        "usage".to_owned(),
        serde_json::json!({ "prompt_tokens": 2, "completion_tokens": 5, "total_tokens": 7 }),
    );
    apply_response(&mut response, "m");
    let mut streamed = response_chunks(response);
    let mut chunks = Vec::new();
    while let Some(item) = streamed.chunks.next().await {
        chunks.push(item.expect("synthetic chunks never fail"));
    }
    assert_eq!(chunks.len(), 2, "one content chunk plus the summary");
    let content = &chunks[0];
    assert_eq!(content.choices.len(), 1);
    assert_eq!(content.choices[0].index, 0);
    assert_eq!(
        content.choices[0]
            .rest
            .get("finish_reason")
            .and_then(Value::as_str),
        Some("tool_calls")
    );
    let calls = content.choices[0]
        .delta
        .get("tool_calls")
        .and_then(Value::as_array)
        .expect("the delta includes tool calls");
    assert_eq!(
        calls[0].get("index").and_then(Value::as_u64),
        Some(0),
        "each delta tool-call entry includes the fragment index"
    );
    assert_eq!(
        calls[0].pointer("/function/name").and_then(Value::as_str),
        Some("search")
    );
    let summary = &chunks[1];
    assert!(
        summary.choices.is_empty(),
        "the summary chunk has no choices"
    );
    assert_eq!(
        summary
            .rest
            .get("usage")
            .and_then(|usage| usage.get("total_tokens"))
            .and_then(Value::as_u64),
        Some(7)
    );
}
