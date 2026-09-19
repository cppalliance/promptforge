//! Yield parsing for the section-only `loop` request: the author's message
//! table and compactor are stashed for the driver, and argument errors are
//! the call's own answer.

use super::*;

fn loop_request(lua: &Lua, messages: &str, compactor: Option<&str>) -> mlua::Table {
    let table = request_table(lua, "loop");
    table
        .raw_set("messages", lua_table(lua, messages))
        .expect("raw_set");
    if let Some(compactor) = compactor {
        let function: Function = lua
            .load(compactor)
            .eval()
            .expect("compactor source evaluates");
        table.raw_set("compactor", function).expect("raw_set");
    }
    table
}

fn expect_loop_call_error(parse: YieldParse, expected: &str) {
    match parse {
        YieldParse::Call(Answer::Loop(Err(Error::Lua(message)))) => {
            assert_eq!(message, expected);
        }
        other => panic!("expected the loop call error {expected:?}, got {other:?}"),
    }
}

#[test]
fn loop_parses_messages_without_a_handle_or_compactor() {
    let lua = Lua::new();
    let table = loop_request(
        &lua,
        r#"{
            { role = "user", content = "hi" },
            { role = "assistant", content = "", tool_calls = {
                { id = "call_1", name = "echo" },
            } },
            { role = "tool", content = "done", tool_call_id = "call_1" },
        }"#,
        None,
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Loop {
            messages,
            binding,
            compactor,
            ..
        } => {
            assert_eq!(messages.len(), 3);
            assert_eq!(messages[1].tool_calls.len(), 1);
            assert_eq!(binding, None);
            assert!(compactor.is_none(), "an omitted compactor is the default");
        }
        other => panic!("expected a loop request, got {other:?}"),
    }
}

#[test]
fn loop_with_a_handle_clones_its_frozen_binding() {
    let lua = Lua::new();
    let table = loop_request(&lua, r#"{ { role = "user", content = "hi" } }"#, None);
    table
        .raw_set("handle", handle_userdata(&lua))
        .expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Loop {
            binding: Some(binding),
            ..
        } => {
            assert_eq!(binding.alias(), "fast");
            assert_eq!(binding.id().name(), "test-model");
        }
        other => panic!("expected a loop request with a binding, got {other:?}"),
    }
}

#[test]
fn loop_stashes_the_author_table_and_compactor_for_the_driver() {
    let lua = Lua::new();
    let messages = lua_table(&lua, r#"{ { role = "user", content = "hi" } }"#);
    let table = request_table(&lua, "loop");
    table
        .raw_set("messages", messages.clone())
        .expect("raw_set");
    let compactor: Function = lua
        .load("function(reason) error('stop:' .. reason, 0) end")
        .eval()
        .expect("compactor source evaluates");
    table.raw_set("compactor", compactor).expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Loop {
            messages_key,
            compactor: Some(compactor_key),
            ..
        } => {
            // The stashed table is the author's own: an append through
            // the key grows the table the author still holds.
            let record = MessageRecord {
                role: MessageRole::Assistant,
                content: MessageContent::Text("reply".to_owned()),
                tool_calls: Vec::new(),
                tool_call_id: None,
            };
            append_message_record(&lua, &messages_key, &record).expect("the append lands");
            let (length, role, content): (i64, String, String) = lua
                .load("local m = ...; return #m, m[2].role, m[2].content")
                .call(messages)
                .expect("the author's table reads back");
            assert_eq!(length, 2);
            assert_eq!(role, "assistant");
            assert_eq!(content, "reply");
            // The stashed compactor is the author's function.
            let stashed: Function = lua
                .registry_value(&compactor_key)
                .expect("the compactor key reads back");
            let error = stashed
                .call::<()>("precheck")
                .expect_err("the stashed compactor runs");
            assert!(
                error.to_string().contains("stop:precheck"),
                "the stashed callback is the author's own: {error}"
            );
        }
        other => panic!("expected a loop request with a compactor, got {other:?}"),
    }
}

#[test]
fn a_loop_with_a_wrong_handle_type_is_the_calls_error() {
    let lua = Lua::new();
    let as_string = loop_request(&lua, r#"{ { role = "user", content = "hi" } }"#, None);
    as_string
        .raw_set("handle", "not a handle")
        .expect("raw_set");
    expect_loop_call_error(
        Request::from_yield(&lua, &Value::Table(as_string)),
        "models.loop handle must be a model handle, got string",
    );
    let as_other_userdata = loop_request(&lua, r#"{ { role = "user", content = "hi" } }"#, None);
    let wrong = lua
        .create_userdata(LuaFanoutResult::success(json!(1), "x"))
        .expect("userdata creation cannot fail on a fresh VM");
    as_other_userdata.raw_set("handle", wrong).expect("raw_set");
    expect_loop_call_error(
        Request::from_yield(&lua, &Value::Table(as_other_userdata)),
        "models.loop handle must be a model handle",
    );
}

#[test]
fn a_loop_with_a_non_function_compactor_is_the_calls_error() {
    let lua = Lua::new();
    let table = loop_request(&lua, r#"{ { role = "user", content = "hi" } }"#, None);
    table.raw_set("compactor", 42).expect("raw_set");
    expect_loop_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "compactor must be a function, got integer",
    );
}

#[test]
fn loop_message_validation_is_the_calls_error() {
    let lua = Lua::new();
    // A non-table messages argument, absent included, is the call's error.
    let missing = request_table(&lua, "loop");
    expect_loop_call_error(
        Request::from_yield(&lua, &Value::Table(missing)),
        "messages must be a table of message tables, got nil",
    );
    // A malformed record names its 1-based index, as the chat parse does.
    let table = loop_request(&lua, r#"{ { role = "wizard", content = "x" } }"#, None);
    expect_loop_call_error(
        Request::from_yield(&lua, &Value::Table(table)),
        "messages[1] role \"wizard\" is unknown; known roles: system, user, assistant, tool",
    );
}
