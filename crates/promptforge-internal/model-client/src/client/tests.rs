//! Tests for the wire message constructors and tool schema validation.

use promptforge_types::metrics::{CallMetrics, Usage};
use serde_json::Value;

use super::*;
use crate::detail::{message_from_validated_parts, tool_schema_new};
use crate::model::CompletionErrorKind;

#[test]
fn from_validated_parts_serializes_role_and_content_verbatim() {
    // The Engine's chat-round seam: a `system` role and a content-parts array
    // must reach the wire exactly as validated, and the inherent constructors'
    // string form must stay byte-identical to the pre-seam shape.
    let parts = serde_json::json!([
        { "type": "text", "text": "look at this" },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
    ]);
    let multimodal = message_from_validated_parts("user", parts.clone(), None, None);
    assert_eq!(
        serde_json::to_value(&multimodal).expect("a message must serialize"),
        serde_json::json!({ "role": "user", "content": parts }),
    );
    assert_eq!(
        multimodal.content(),
        "",
        "parts content has no text form; the accessor reports empty"
    );
    let system =
        message_from_validated_parts("system", Value::String("be terse".to_owned()), None, None);
    assert_eq!(
        serde_json::to_value(&system).expect("a message must serialize"),
        serde_json::json!({ "role": "system", "content": "be terse" }),
    );
    assert_eq!(system.content(), "be terse");
    assert_eq!(
        serde_json::to_value(Message::user("hello")).expect("a message must serialize"),
        serde_json::json!({ "role": "user", "content": "hello" }),
        "the plain constructors keep their wire shape"
    );
}

#[test]
fn tool_arguments_view_exposes_no_raw_value() {
    // The public arguments view surfaces typed accessors, never a
    // serde_json::Value.
    let call = ToolCall {
        id: "call_1".to_owned(),
        name: "search".to_owned(),
        arguments: serde_json::json!({"query": "rust", "limit": 5}),
    };
    let args = call.arguments();
    assert!(!args.is_empty());
    assert!(args.contains("query"));
    assert!(!args.contains("absent"));
    let mut names: Vec<_> = args.names().collect();
    names.sort_unstable();
    assert_eq!(names, ["limit", "query"]);
    let json = args.to_json_string();
    assert!(json.contains("\"query\":\"rust\""), "got {json}");

    // A null payload reads as empty.
    let empty = ToolCall {
        id: "c2".to_owned(),
        name: "noop".to_owned(),
        arguments: Value::Null,
    };
    assert!(empty.arguments().is_empty());
    assert!(!empty.arguments().contains("anything"));
}

#[test]
fn from_parts_refuses_a_blank_id_a_blank_name_and_non_object_arguments() {
    let object = || serde_json::json!({ "url": "x" });
    for (id, name, arguments, label) in [
        (" ", "fetch", object(), "blank id"),
        ("call_1", "\t", object(), "blank name"),
        ("call_1", "fetch", Value::Null, "null arguments"),
        (
            "call_1",
            "fetch",
            serde_json::json!("{}"),
            "string arguments",
        ),
        ("call_1", "fetch", serde_json::json!([1]), "array arguments"),
    ] {
        let error = ToolCall::from_parts(id, name, arguments).expect_err(label);
        assert_eq!(
            error.kind(),
            CompletionErrorKind::MalformedResponse,
            "{label} must be refused"
        );
    }
    let call = ToolCall::from_parts("call_1", "fetch", object()).expect("a whole call is accepted");
    assert_eq!((call.id(), call.name()), ("call_1", "fetch"));
}

#[test]
fn from_result_refuses_an_empty_batch_and_duplicate_ids_and_accepts_text() {
    let empty = Completion::from_result(CompletionResult::ToolCalls(Vec::new()), "m")
        .expect_err("an empty batch is refused");
    assert_eq!(empty.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(empty.finish_reason(), None);
    let call = ToolCall::from_parts("call_1", "fetch", json_object()).expect("a whole call");
    let twice = CompletionResult::ToolCalls(vec![call.clone(), call.clone()]);
    let duplicate = Completion::from_result(twice, "m").expect_err("a duplicate id is refused");
    assert_eq!(duplicate.kind(), CompletionErrorKind::MalformedResponse);
    assert!(
        duplicate.to_string().contains("\"call_1\""),
        "the message names the repeated id: {duplicate}"
    );
    let text = Completion::from_result(CompletionResult::Text("pong".to_owned()), "m")
        .expect("a text result is accepted");
    assert_eq!(text.model(), "m");
    Completion::from_result(CompletionResult::ToolCalls(vec![call]), "m")
        .expect("a batch of distinct calls is accepted");
}

#[test]
fn a_completion_from_a_bare_result_carries_no_metrics_and_no_raw_exchange() {
    let completion = Completion::from_result(CompletionResult::Text("pong".to_owned()), "m")
        .expect("a text result is accepted");
    assert_eq!(completion.metrics(), None);
    assert_eq!(completion.raw(), None);
    assert_eq!(completion.finish_reason(), None);
    assert_eq!(completion.reasoning_content(), None);
}

#[test]
fn the_builders_set_metrics_the_raw_exchange_and_the_finish_reason() {
    let metrics = CallMetrics {
        usage: Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
            total_tokens: 10,
            cached_tokens: None,
            reasoning_tokens: None,
        }),
        llama: None,
        vllm: None,
        client: None,
    };
    let request = serde_json::json!({ "model": "m", "messages": [] });
    let response = serde_json::json!({ "choices": [] });
    let completion = Completion::from_result(CompletionResult::Text("pong".to_owned()), "m")
        .expect("a text result is accepted")
        .with_metrics(metrics.clone())
        .with_raw(RawExchange::new(request.clone(), response.clone()))
        .with_finish_reason("stop");
    assert_eq!(completion.metrics(), Some(&metrics));
    let raw = completion.raw().expect("the raw exchange was attached");
    assert_eq!(raw.request(), &request);
    assert_eq!(raw.response(), &response);
    assert_eq!(completion.finish_reason(), Some("stop"));
    assert_eq!(
        completion.result(),
        &CompletionResult::Text("pong".to_owned()),
        "the builders leave the outcome as it was"
    );
}

#[test]
fn the_builders_set_the_reasoning_content_and_the_metadata_diagnostics() {
    let bare = Completion::from_result(CompletionResult::Text("pong".to_owned()), "m")
        .expect("a text result is accepted");
    assert!(bare.metadata_diagnostics().is_empty());
    let diagnostics = vec!["usage: expected an object".to_owned()];
    let completion = bare
        .with_reasoning_content("scratch work")
        .with_metadata_diagnostics(vec!["timings: expected an object".to_owned()])
        .with_metadata_diagnostics(diagnostics.clone());
    assert_eq!(completion.reasoning_content(), Some("scratch work"));
    assert_eq!(
        completion.metadata_diagnostics(),
        diagnostics.as_slice(),
        "a second call replaces the first list"
    );
    assert_eq!(
        completion.result(),
        &CompletionResult::Text("pong".to_owned()),
        "the builders leave the outcome as it was"
    );
}

#[test]
fn a_tool_schema_reads_back_its_name_description_and_parameters() {
    let schema = tool_schema_new("web.search", "Search the web.", json_object())
        .expect("a valid schema is accepted");
    assert_eq!(schema.name(), "web.search");
    assert_eq!(schema.description(), "Search the web.");
    assert_eq!(schema.parameters(), &json_object());
}

#[test]
fn tool_schema_new_validates_wire_name_and_object_schema() {
    // A valid name and object schema are accepted.
    let schema =
        tool_schema_new("web.search-1", "desc", json_object()).expect("a valid schema is accepted");
    assert_eq!(schema.name, "web.search-1");
    // An empty or malformed name is rejected.
    assert!(matches!(
        tool_schema_new("", "d", json_object()),
        Err(ToolSchemaError::InvalidName { .. })
    ));
    assert!(matches!(
        tool_schema_new("bad name", "d", json_object()),
        Err(ToolSchemaError::InvalidName { .. })
    ));
    // A non-object JSON Schema is rejected.
    assert!(matches!(
        tool_schema_new("ok", "d", serde_json::json!([1, 2, 3])),
        Err(ToolSchemaError::NonObjectSchema { .. })
    ));
    assert!(matches!(
        tool_schema_new("ok", "d", serde_json::json!("scalar")),
        Err(ToolSchemaError::NonObjectSchema { .. })
    ));
}

fn json_object() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}
