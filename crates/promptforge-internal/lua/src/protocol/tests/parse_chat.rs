//! Yield parsing for the `chat` request the `models.loop` shim yields: the
//! handle and the `messages.new()` list, with every author-argument
//! failure as the call's own answer. The per-record rules run as the list
//! adds a record, so their tests sit with the list.

use super::*;
use crate::MessageList;

/// The refusal for a `messages` value that is not a `messages.new()` list.
const NOT_A_LIST: &str = "models.loop needs a messages.new() list; build one with \
                          messages.new() and :user, :append, or :replace";

/// A `chat` request over a `messages.new()` list holding the records of
/// the Lua array `records`, as the loop shim yields it.
fn chat_request(lua: &Lua, records: &str) -> mlua::Table {
    let list = lua
        .create_userdata(MessageList::default())
        .expect("userdata creation cannot fail");
    lua.load(format!(
        "local list = ...\nfor _, record in ipairs({records}) do list:append(record) end"
    ))
    .call::<()>(list.clone())
    .expect("the test records build a list");
    let table = request_table(lua, "chat");
    table.raw_set("messages", list).expect("raw_set");
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
        Request::Chat { list, binding } => {
            assert!(binding.is_none(), "a round without a handle has no binding");
            let messages = list.records();
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
        Request::Chat { list, .. } => {
            let messages = list.records();
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
        Request::Chat { list, .. } => {
            let messages = list.records();
            assert_eq!(messages[1].role, MessageRole::Tool);
            assert_eq!(messages[1].tool_call_id.as_deref(), Some("call_1"));
            assert_eq!(messages[2].role, MessageRole::Tool);
            assert_eq!(messages[2].tool_call_id.as_deref(), Some("call_2"));
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn messages_that_are_not_a_list_are_the_calls_error() {
    let lua = Lua::new();
    // Absent, a non-table, a hand-written array, an empty table, and a
    // userdata of another type are all refused with the list error.
    let missing = request_table(&lua, "chat");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(missing)),
        NOT_A_LIST,
    );
    let numeric = request_table(&lua, "chat");
    numeric.raw_set("messages", 42).expect("raw_set");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(numeric)),
        NOT_A_LIST,
    );
    for source in [r#"{ { role = "user", content = "hi" } }"#, "{}"] {
        let plain = request_table(&lua, "chat");
        plain
            .raw_set("messages", lua_table(&lua, source))
            .expect("raw_set");
        expect_chat_call_error(Request::from_yield(&lua, &Value::Table(plain)), NOT_A_LIST);
    }
    let other = request_table(&lua, "chat");
    other
        .raw_set("messages", handle_userdata(&lua))
        .expect("raw_set");
    expect_chat_call_error(Request::from_yield(&lua, &Value::Table(other)), NOT_A_LIST);
}

#[test]
fn an_empty_list_is_the_calls_error() {
    let lua = Lua::new();
    let table = chat_request(&lua, "{}");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "messages must not be empty",
    );
}

#[test]
fn chat_with_the_loops_handle_holds_its_frozen_binding() {
    // A handle's `loop` yields its receiver beside the messages; the
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
