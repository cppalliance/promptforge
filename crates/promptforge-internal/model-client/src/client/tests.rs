//! Tests for the wire message constructors and tool schema validation.

use serde_json::Value;

use super::*;
use crate::detail::{message_from_validated_parts, tool_schema_new};

#[test]
fn from_validated_parts_serializes_role_and_content_verbatim() {
    // The engine's chat-round seam: a `system` role and a content-parts array
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
    // F8: the public arguments view surfaces typed accessors, never a
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
fn tool_schema_new_validates_wire_name_and_object_schema() {
    // F7: a valid name and object schema are accepted.
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
