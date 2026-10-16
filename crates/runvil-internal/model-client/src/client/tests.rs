//! Tests for the wire message constructors and tool schema validation.

use runvil_types::metrics::{CallMetrics, Usage};
use serde_json::{Value, json};

use super::*;
use crate::detail::{message_from_validated_parts, tool_schema_new};
use crate::model::CompletionErrorKind;
use crate::test_support::{assert_json, pong};

#[test]
fn from_validated_parts_serializes_role_and_content_verbatim() {
    // The Engine's chat-round seam: a `system` role and a content-parts array
    // must reach the wire exactly as validated, and the inherent constructors'
    // string form must stay byte-identical to the pre-seam shape.
    let parts = json!([
        { "type": "text", "text": "look at this" },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
    ]);
    let multimodal = message_from_validated_parts(Role::User, parts.clone(), None, None);
    assert_json(&multimodal, &json!({ "role": "user", "content": parts }));
    assert_eq!(
        multimodal.content(),
        "",
        "parts content has no text form; the accessor reports empty"
    );
    let system = message_from_validated_parts(
        Role::System,
        Value::String("be terse".to_owned()),
        None,
        None,
    );
    assert_json(&system, &json!({ "role": "system", "content": "be terse" }));
    assert_eq!(system.content(), "be terse");
    let plain = Message::user("hello");
    assert_json(&plain, &json!({ "role": "user", "content": "hello" }));
}

#[test]
fn each_constructor_names_its_role() {
    assert_eq!(Message::user("u").role(), Role::User);
    assert_eq!(Message::assistant("a").role(), Role::Assistant);
    assert_eq!(Message::tool("call_1", "t").role(), Role::Tool);
}

#[test]
fn a_role_round_trips_through_serde_as_its_lowercase_name() {
    for (role, wire) in [
        (Role::System, "\"system\""),
        (Role::User, "\"user\""),
        (Role::Assistant, "\"assistant\""),
        (Role::Tool, "\"tool\""),
    ] {
        assert_eq!(
            serde_json::to_string(&role).expect("a role serializes"),
            wire
        );
        assert_eq!(
            serde_json::from_str::<Role>(wire).expect("a role deserializes"),
            role
        );
        assert_eq!(format!("\"{}\"", role.as_str()), wire);
    }
    assert!(
        serde_json::from_str::<Role>("\"System\"").is_err(),
        "the wire names are exact, never case-folded"
    );
}

#[test]
fn a_role_parses_from_its_name_and_a_refusal_lists_the_accepted_roles() {
    for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
        assert_eq!(role.as_str().parse::<Role>(), Ok(role));
    }
    let error = "wizard"
        .parse::<Role>()
        .expect_err("an unknown role is refused");
    assert_eq!(error.input(), "wizard");
    assert_eq!(
        error.to_string(),
        "invalid role `wizard`: required one of `system`, `user`, `assistant`, `tool`"
    );
}

#[test]
fn tool_arguments_view_exposes_no_raw_value() {
    // The public arguments view surfaces typed accessors, never a
    // serde_json::Value.
    let call = ToolCall {
        id: "call_1".to_owned(),
        name: "search".to_owned(),
        arguments: json!({"query": "rust", "limit": 5}),
    };
    let args = call.arguments();
    assert!(!args.is_empty());
    assert!(args.contains("query"));
    assert!(!args.contains("absent"));
    let iter: ArgumentNames<'_> = args.names();
    let mut names: Vec<_> = iter.collect();
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
    assert_eq!(empty.arguments().names().count(), 0);
}

#[test]
fn from_parts_refuses_a_blank_id_a_blank_name_and_non_object_arguments() {
    let object = || json!({ "url": "x" });
    for (id, name, arguments, label) in [
        (" ", "fetch", object(), "blank id"),
        ("call_1", "\t", object(), "blank name"),
        ("call_1", "fetch", Value::Null, "null arguments"),
        ("call_1", "fetch", json!("{}"), "string arguments"),
        ("call_1", "fetch", json!([1]), "array arguments"),
    ] {
        let error = ToolCall::from_parts(id, name, arguments).expect_err(label);
        assert_eq!(
            error.kind(),
            CompletionErrorKind::InvalidParts,
            "{label} must be refused"
        );
        assert!(
            !error.is_retryable(),
            "{label}: rebuilding the same parts fails the same way"
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
    assert_eq!(duplicate.kind(), CompletionErrorKind::InvalidParts);
    assert!(!duplicate.is_retryable());
    assert!(
        duplicate.to_string().contains("\"call_1\""),
        "the message names the repeated id: {duplicate}"
    );
    assert_eq!(pong("m").model(), "m");
    Completion::from_result(CompletionResult::ToolCalls(vec![call]), "m")
        .expect("a batch of distinct calls is accepted");
}

#[test]
fn a_completion_from_a_bare_result_carries_no_metrics_and_no_raw_exchange() {
    let completion = pong("m");
    assert_eq!(completion.metrics(), None);
    assert_eq!(completion.raw(), None);
    assert_eq!(completion.finish_reason(), None);
    assert_eq!(completion.reasoning_content(), None);
}

#[test]
fn the_builders_set_metrics_the_raw_exchange_and_the_finish_reason() {
    let metrics = CallMetrics::new().with_usage(Some(Usage::new(7, 3, 10)));
    let request = json!({ "model": "m", "messages": [] });
    let response = json!({ "choices": [] });
    let completion = pong("m")
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
    let bare = pong("m");
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
fn with_model_replaces_the_served_model_name_and_keeps_the_rest() {
    let completion = pong("reported")
        .with_finish_reason("stop")
        .with_model("routed");
    assert_eq!(completion.model(), "routed");
    assert_eq!(completion.finish_reason(), Some("stop"));
    assert_eq!(
        completion.result(),
        &CompletionResult::Text("pong".to_owned()),
        "the label leaves the outcome as it was"
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
        tool_schema_new("ok", "d", json!([1, 2, 3])),
        Err(ToolSchemaError::NonObjectSchema { .. })
    ));
    assert!(matches!(
        tool_schema_new("ok", "d", json!("scalar")),
        Err(ToolSchemaError::NonObjectSchema { .. })
    ));
}

fn json_object() -> Value {
    json!({"type": "object", "properties": {}})
}
