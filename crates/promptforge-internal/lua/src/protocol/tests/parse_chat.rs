//! Yield parsing for the `chat` request the `models.loop` shim yields: the
//! leading handle and message-list validation, with every author-argument
//! failure as the call's own answer.

use super::*;

fn chat_request(lua: &Lua, messages: &str) -> mlua::Table {
    let table = request_table(lua, "chat");
    table
        .raw_set("messages", lua_table(lua, messages))
        .expect("raw_set");
    table
}

fn expect_chat_call_error(parse: YieldParse, expected: &str) {
    match parse {
        YieldParse::Call(Answer::Chat(Err(Error::Lua(message)))) => {
            assert_eq!(message, expected);
        }
        other => panic!("expected the chat call error {expected:?}, got {other:?}"),
    }
}

#[test]
fn chat_parses_every_message_shape() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{
            { role = "system", content = "be terse" },
            { role = "user", content = {
                { type = "text", text = "look" },
                { type = "image_url", image_url = { url = "data:image/png;base64,AA" } },
            } },
            { role = "assistant", content = "", tool_calls = {
                { id = "call_1", name = "echo", arguments = { value = "hi" } },
            } },
            { role = "tool", content = "result", tool_call_id = "call_1" },
        }"#,
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { messages, binding } => {
            assert!(binding.is_none(), "a round without a handle has no binding");
            assert_eq!(messages.len(), 4);
            assert_eq!(messages[0].role, MessageRole::System);
            assert_eq!(
                messages[0].content,
                MessageContent::Text("be terse".to_owned())
            );
            assert_eq!(
                messages[1].content,
                MessageContent::Parts(vec![
                    ContentPart::Text("look".to_owned()),
                    ContentPart::ImageUrl("data:image/png;base64,AA".to_owned()),
                ]),
                "content parts must survive the parse as typed variants"
            );
            assert_eq!(messages[2].role, MessageRole::Assistant);
            assert_eq!(
                messages[2].tool_calls,
                vec![ToolCallRecord {
                    id: "call_1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: json!({ "value": "hi" }),
                }]
            );
            assert_eq!(messages[3].role, MessageRole::Tool);
            assert_eq!(messages[3].tool_call_id.as_deref(), Some("call_1"));
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn an_assistant_message_holds_visible_text_plus_multiple_normalized_tool_calls() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{
            { role = "assistant", content = "working on it", tool_calls = {
                { id = "call_1", name = "echo", arguments = { value = "hi" } },
                { id = "call_2", name = "search" },
            } },
        }"#,
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { messages, .. } => {
            assert_eq!(
                messages[0].content,
                MessageContent::Text("working on it".to_owned()),
                "visible text stays on the same turn as the calls"
            );
            assert_eq!(
                messages[0].tool_calls,
                vec![
                    ToolCallRecord {
                        id: "call_1".to_owned(),
                        name: "echo".to_owned(),
                        arguments: json!({ "value": "hi" }),
                    },
                    ToolCallRecord {
                        id: "call_2".to_owned(),
                        name: "search".to_owned(),
                        arguments: json!({}),
                    },
                ],
                "an absent arguments normalizes to the empty object"
            );
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn correlated_tool_results_hold_the_matching_call_ids() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{
            { role = "assistant", content = "", tool_calls = {
                { id = "call_1", name = "echo" },
                { id = "call_2", name = "search" },
            } },
            { role = "tool", content = "echoed", tool_call_id = "call_1" },
            { role = "tool", content = "found", tool_call_id = "call_2" },
        }"#,
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { messages, .. } => {
            assert_eq!(messages[1].role, MessageRole::Tool);
            assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_1"));
            assert_eq!(messages[2].role, MessageRole::Tool);
            assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_2"));
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn malformed_tool_calls_are_typed_call_errors_naming_the_index() {
    let lua = Lua::new();
    let cases: [(&str, &str); 4] = [
        (
            r#"{ { role = "assistant", content = "", tool_calls = { "raw" } } }"#,
            "messages[1] tool_calls[1] must be a table",
        ),
        (
            r#"{ { role = "assistant", content = "", tool_calls = { { name = "echo" } } } }"#,
            "messages[1] tool_calls[1] must set a string id",
        ),
        (
            r#"{ { role = "assistant", content = "", tool_calls = { { id = "call_1" } } } }"#,
            "messages[1] tool_calls[1] must set a string name",
        ),
        (
            r#"{ { role = "assistant", content = "", tool_calls = { { id = "call_1", name = "echo", arguments = "raw" } } } }"#,
            "messages[1] tool_calls[1] arguments must be a table",
        ),
    ];
    for (messages, expected) in cases {
        let table = chat_request(&lua, messages);
        expect_chat_call_error(Request::from_yield(&lua, &Value::Table(table)), expected);
    }
}

#[test]
fn content_parts_validate_each_variants_payload() {
    let lua = Lua::new();
    let cases: [(&str, &str); 3] = [
        (
            r#"{ { role = "user", content = { { type = "text" } } } }"#,
            "messages[1] content part 1 is a text part and must set a string \
             text field",
        ),
        (
            r#"{ { role = "user", content = { { type = "image_url" } } } }"#,
            "messages[1] content part 1 is an image_url part and must set an \
             image_url table with a string url field",
        ),
        (
            r#"{ { role = "user", content = { { type = "image_url", image_url = { detail = "high" } } } } }"#,
            "messages[1] content part 1 is an image_url part and must set an \
             image_url table with a string url field",
        ),
    ];
    for (messages, expected) in cases {
        let table = chat_request(&lua, messages);
        expect_chat_call_error(Request::from_yield(&lua, &Value::Table(table)), expected);
    }
}

#[test]
fn a_non_string_tool_call_id_is_a_typed_call_error() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{ { role = "user", content = "ok", tool_call_id = 7 } }"#,
    );
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "messages[1] tool_call_id must be a string",
    );
}

#[test]
fn chat_message_validation_names_the_offending_index() {
    let lua = Lua::new();
    let cases: [(&str, &str); 8] = [
        ("{}", "messages must not be empty"),
        (
            r#"{ "not a table" }"#,
            "messages[1] must be a message table",
        ),
        (
            r#"{ { role = "user", content = "ok" }, { role = "wizard", content = "x" } }"#,
            "messages[2] role \"wizard\" is unknown; known roles: system, user, assistant, tool",
        ),
        (
            r#"{ { content = "no role" } }"#,
            "messages[1] role must be a string, one of: system, user, assistant, tool",
        ),
        (
            r#"{ { role = "user" } }"#,
            "messages[1] content must be a string or a non-empty array of content parts",
        ),
        (
            r#"{ { role = "user", content = { "bare string part" } } }"#,
            "messages[1] content part 1 must be a table with a string type field",
        ),
        (
            r#"{ { role = "user", content = { { type = "text", text = "ok" }, { type = "video" } } } }"#,
            "messages[1] content part 2 has unknown type \"video\"; known types: text, image_url",
        ),
        (
            r#"{ { role = "user", content = "ok" }, { role = "tool", content = "r" } }"#,
            "messages[2] is a tool message and must set a string tool_call_id",
        ),
    ];
    for (messages, expected) in cases {
        let table = chat_request(&lua, messages);
        expect_chat_call_error(Request::from_yield(&lua, &Value::Table(table)), expected);
    }
    // A non-table messages argument, absent included, is the call's error.
    let missing = request_table(&lua, "chat");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(missing)),
        "messages must be a table of message tables, got nil",
    );
    let numeric = request_table(&lua, "chat");
    numeric.raw_set("messages", 42).expect("raw_set");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(numeric)),
        "messages must be a table of message tables, got integer",
    );
    // A present tool_calls of the wrong shape is rejected in place.
    let table = chat_request(
        &lua,
        r#"{ { role = "assistant", content = "", tool_calls = "raw" } }"#,
    );
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "messages[1] tool_calls must be an array",
    );
}

#[test]
fn chat_with_the_loops_leading_handle_holds_its_frozen_binding() {
    // The loop shim yields its leading handle beside the messages; the
    // binding is cloned out of the userdata at the parse, so the round runs
    // on the handle's model rather than the section default.
    let lua = Lua::new();
    let table = chat_request(&lua, r#"{ { role = "user", content = "hi" } }"#);
    table
        .raw_set("handle", handle_userdata(&lua))
        .expect("raw_set");
    match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
        Request::Chat {
            binding: Some(binding),
            ..
        } => assert_eq!(binding.alias(), "fast"),
        other => panic!("expected a chat request on the handle's binding, got {other:?}"),
    }
}

#[test]
fn chat_handle_validation_names_the_loop_and_is_the_calls_error() {
    // The loop shim sets `handle` only for a userdata first argument, so a
    // userdata that is not a model handle is the loop's own argument error;
    // the parse still refuses any other shape.
    let lua = Lua::new();
    let valid = r#"{ { role = "user", content = "hi" } }"#;
    let wrong_userdata = chat_request(&lua, valid);
    wrong_userdata
        .raw_set(
            "handle",
            lua.create_userdata(OtherUserData)
                .expect("userdata creation cannot fail"),
        )
        .expect("raw_set");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(wrong_userdata)),
        "models.loop handle must be a model handle",
    );
    let wrong_type = chat_request(&lua, valid);
    wrong_type.raw_set("handle", "fast").expect("raw_set");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(wrong_type)),
        "models.loop handle must be a model handle, got string",
    );
}
