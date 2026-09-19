//! Yield parsing for the agent-only `chat` request: message-list and opts
//! validation, with every author-argument failure as the call's own answer.

use super::*;

fn chat_request(lua: &Lua, messages: &str, opts: Option<&str>) -> mlua::Table {
    let table = request_table(lua, "chat");
    table
        .raw_set("messages", lua_table(lua, messages))
        .expect("raw_set");
    if let Some(opts) = opts {
        table
            .raw_set("opts", lua_table(lua, opts))
            .expect("raw_set");
    }
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
fn chat_parses_messages_model_and_tools() {
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
        Some(r#"{ model = "fast", tools = { "echo", "search" } }"#),
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat {
            messages,
            model,
            tools,
        } => {
            assert_eq!(model.as_deref(), Some("fast"));
            assert_eq!(
                tools,
                Some(vec!["echo".to_owned(), "search".to_owned()]),
                "an explicit list is the agent's advertised set"
            );
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
fn an_assistant_message_carries_visible_text_plus_multiple_normalized_tool_calls() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{
            { role = "assistant", content = "working on it", tool_calls = {
                { id = "call_1", name = "echo", arguments = { value = "hi" } },
                { id = "call_2", name = "search" },
            } },
        }"#,
        None,
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { messages, .. } => {
            assert_eq!(
                messages[0].content,
                MessageContent::Text("working on it".to_owned()),
                "visible text rides alongside the calls"
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
fn correlated_tool_results_carry_the_matching_call_ids() {
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
        None,
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
            "messages[1] tool_calls[1] must carry a string id",
        ),
        (
            r#"{ { role = "assistant", content = "", tool_calls = { { id = "call_1" } } } }"#,
            "messages[1] tool_calls[1] must carry a string name",
        ),
        (
            r#"{ { role = "assistant", content = "", tool_calls = { { id = "call_1", name = "echo", arguments = "raw" } } } }"#,
            "messages[1] tool_calls[1] arguments must be a table",
        ),
    ];
    for (messages, expected) in cases {
        let table = chat_request(&lua, messages, None);
        expect_chat_call_error(Request::from_yield(&lua, &Value::Table(table)), expected);
    }
}

#[test]
fn content_parts_validate_each_variants_payload() {
    let lua = Lua::new();
    let cases: [(&str, &str); 3] = [
        (
            r#"{ { role = "user", content = { { type = "text" } } } }"#,
            "messages[1] content part 1 is a text part and must carry a string \
             text field",
        ),
        (
            r#"{ { role = "user", content = { { type = "image_url" } } } }"#,
            "messages[1] content part 1 is an image_url part and must carry an \
             image_url table with a string url field",
        ),
        (
            r#"{ { role = "user", content = { { type = "image_url", image_url = { detail = "high" } } } } }"#,
            "messages[1] content part 1 is an image_url part and must carry an \
             image_url table with a string url field",
        ),
    ];
    for (messages, expected) in cases {
        let table = chat_request(&lua, messages, None);
        expect_chat_call_error(Request::from_yield(&lua, &Value::Table(table)), expected);
    }
}

#[test]
fn a_non_string_tool_call_id_is_a_typed_call_error() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{ { role = "user", content = "ok", tool_call_id = 7 } }"#,
        None,
    );
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "messages[1] tool_call_id must be a string",
    );
}

#[test]
fn chat_without_opts_parses_no_model_and_the_tools_none_shape() {
    // The `tools: None` shape: a section VM's chat yield carries no tool
    // list, and the driver resolves the section's current tool scope.
    let lua = Lua::new();
    let table = chat_request(&lua, r#"{ { role = "user", content = "hi" } }"#, None);
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { model, tools, .. } => {
            assert_eq!(model, None);
            assert_eq!(
                tools, None,
                "an absent tools list is the None shape, not an empty explicit list"
            );
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn chat_with_opts_but_no_tools_still_parses_the_tools_none_shape() {
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{ { role = "user", content = "hi" } }"#,
        Some(r#"{ model = "fast" }"#),
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { model, tools, .. } => {
            assert_eq!(model.as_deref(), Some("fast"));
            assert_eq!(tools, None, "opts without tools is still the None shape");
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn chat_with_an_empty_tools_list_parses_an_explicit_empty_set() {
    // The agent VM's explicit list survives even when empty: `{}` means
    // "advertise nothing", which the driver must never widen to the
    // section scope the None shape names.
    let lua = Lua::new();
    let table = chat_request(
        &lua,
        r#"{ { role = "user", content = "hi" } }"#,
        Some("{ tools = {} }"),
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Chat { tools, .. } => {
            assert_eq!(
                tools,
                Some(Vec::new()),
                "an explicit empty list is Some(empty), distinct from None"
            );
        }
        other => panic!("expected a chat request, got {other:?}"),
    }
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
            "messages[2] is a tool message and must carry a string tool_call_id",
        ),
    ];
    for (messages, expected) in cases {
        let table = chat_request(&lua, messages, None);
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
        None,
    );
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "messages[1] tool_calls must be an array",
    );
}

#[test]
fn chat_opts_validation_is_the_calls_error() {
    let lua = Lua::new();
    let valid = r#"{ { role = "user", content = "hi" } }"#;
    let non_table = request_table(&lua, "chat");
    non_table
        .raw_set("messages", lua_table(&lua, valid))
        .expect("raw_set");
    non_table.raw_set("opts", "loud").expect("raw_set");
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(non_table)),
        "opts must be a table, got string",
    );
    let bad_model = chat_request(&lua, valid, Some("{ model = 42 }"));
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(bad_model)),
        "opts.model must be a string, got integer",
    );
    let bad_tools = chat_request(&lua, valid, Some(r#"{ tools = "echo" }"#));
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(bad_tools)),
        "opts.tools must be an array of tool alias strings, got string",
    );
    let bad_alias = chat_request(&lua, valid, Some(r#"{ tools = { "echo", 7 } }"#));
    expect_chat_call_error(
        Request::from_yield(&lua, &Value::Table(bad_alias)),
        "opts.tools[2] must be a string tool alias, got integer",
    );
}
