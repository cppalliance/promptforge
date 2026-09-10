use mlua::{Lua, LuaSerdeExt, Value};
use promptforge_core_support::observe::NullObserver;
use promptforge_core_support::untrusted::GuardNonce;
use promptforge_store::StoreRef;
use serde_json::json;

use super::install_messages;
use crate::protocol::{Answer, MessageRecord, Request, ToolCallRecord, YieldParse};
use crate::{Error, SectionVm};

fn lua_with_messages() -> Lua {
    let lua = Lua::new();
    let globals = lua.globals();
    install_messages(&lua, &globals).expect("messages install cannot fail on a fresh VM");
    lua
}

fn eval(lua: &Lua, source: &str) -> Value {
    lua.load(source).eval().expect("test source evaluates")
}

/// Converts builder output through the same serde boundary the chat
/// protocol's `messages` conversion uses.
fn eval_json(lua: &Lua, source: &str) -> serde_json::Value {
    lua.from_value(eval(lua, source))
        .expect("builder output must convert to JSON")
}

/// Parses a message list through the chat protocol boundary, exactly as a
/// `models.chat` yield would.
fn chat_parse(lua: &Lua, messages: Value) -> Vec<MessageRecord> {
    let request = lua.create_table().expect("table creation cannot fail");
    request.raw_set("op", "chat").expect("raw_set");
    request.raw_set("messages", messages).expect("raw_set");
    match Request::from_yield(lua, &Value::Table(request)) {
        YieldParse::Request(Request::Chat { messages, .. }) => messages,
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn new_returns_an_empty_numerically_indexed_list() {
    let lua = lua_with_messages();
    let (len, first_nil): (i64, bool) = lua
        .load("local l = messages.new(); return #l, l[1] == nil")
        .eval()
        .expect("test source evaluates");
    assert_eq!(len, 0);
    assert!(first_nil);
}

#[test]
fn builders_chain_into_plain_records_in_order() {
    let lua = lua_with_messages();
    let json = eval_json(
        &lua,
        "messages.new():system('be terse'):user('hi'):assistant('hello')",
    );
    // The serde conversion succeeding at all proves the chainable methods
    // live behind the metatable: as direct fields the functions would make
    // the list JSON-unrepresentable.
    assert_eq!(
        json,
        json!([
            { "role": "system", "content": "be terse" },
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "hello" },
        ])
    );
}

#[test]
fn chaining_returns_the_same_list_table() {
    let lua = lua_with_messages();
    let same: bool = lua
        .load(
            "local l = messages.new(); \
             return l:user('x') == l and l:append({ role = 'user', content = 'y' }) == l",
        )
        .eval()
        .expect("test source evaluates");
    assert!(
        same,
        "every builder method must return the list for chaining"
    );
}

#[test]
fn assistant_carries_tool_calls_only_when_given() {
    let lua = lua_with_messages();
    let without = eval_json(&lua, "messages.new():assistant('working on it')");
    assert_eq!(
        without,
        json!([{ "role": "assistant", "content": "working on it" }]),
        "an absent tool_calls argument must leave the field unset"
    );
    let with = eval_json(
        &lua,
        "messages.new():assistant('working', \
         { { id = 'call_1', name = 'echo', arguments = { value = 'hi' } } })",
    );
    assert_eq!(
        with,
        json!([{
            "role": "assistant",
            "content": "working",
            "tool_calls": [{ "id": "call_1", "name": "echo", "arguments": { "value": "hi" } }],
        }])
    );
}

#[test]
fn tool_records_carry_the_call_id() {
    let lua = lua_with_messages();
    let json = eval_json(&lua, "messages.new():tool('echoed: hi', 'call_1')");
    assert_eq!(
        json,
        json!([{ "role": "tool", "content": "echoed: hi", "tool_call_id": "call_1" }])
    );
}

#[test]
fn append_adds_a_raw_record_unchanged() {
    let lua = lua_with_messages();
    let json = eval_json(
        &lua,
        "messages.new():append({ role = 'user', content = 'raw', extra = 1 })",
    );
    assert_eq!(
        json,
        json!([{ "role": "user", "content": "raw", "extra": 1 }]),
        "append must not reshape the record; the protocol parse drops extra fields"
    );
}

#[test]
fn builder_output_parses_through_the_protocol_like_a_raw_array() {
    let lua = lua_with_messages();
    let built = eval(
        &lua,
        "messages.new()\
         :system('be terse')\
         :user('hi')\
         :assistant('working', { { id = 'call_1', name = 'echo' } })\
         :tool('echoed', 'call_1')",
    );
    let raw = eval(
        &lua,
        "{ { role = 'system', content = 'be terse' },\
           { role = 'user', content = 'hi' },\
           { role = 'assistant', content = 'working',\
             tool_calls = { { id = 'call_1', name = 'echo' } } },\
           { role = 'tool', content = 'echoed', tool_call_id = 'call_1' } }",
    );
    let built_records = chat_parse(&lua, built);
    assert_eq!(
        built_records,
        chat_parse(&lua, raw),
        "builder output and a hand-written array must validate to the same records"
    );
    assert_eq!(built_records.len(), 4);
    assert_eq!(
        built_records[2].tool_calls,
        vec![ToolCallRecord {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({}),
        }],
        "an absent arguments normalizes to the empty object"
    );
    assert_eq!(built_records[3].tool_call_id.as_deref(), Some("call_1"));
}

#[test]
fn host_validation_still_rejects_a_bad_builder_record() {
    // The builders validate nothing: a record missing its content fails in
    // the protocol parse, exactly as the same hand-written array does.
    let lua = lua_with_messages();
    let built = eval(&lua, "messages.new():user()");
    let request = lua.create_table().expect("table creation cannot fail");
    request.raw_set("op", "chat").expect("raw_set");
    request.raw_set("messages", built).expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(request)) {
        YieldParse::Call(Answer::Chat(Err(Error::Lua(message)))) => {
            assert_eq!(
                message,
                "messages[1] content must be a string or a non-empty \
                 array of content parts"
            );
        }
        other => panic!("expected the chat call error, got {other:?}"),
    }
}

#[test]
fn the_builders_run_under_the_hardened_section_sandbox() {
    let nonce = GuardNonce::fresh();
    let observer = NullObserver::default();
    let mut vm = SectionVm::new(&nonce, "test-run", &observer, "Test")
        .expect("section VM construction cannot fail");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host injection cannot fail");
    let json: serde_json::Value = vm
        .lua()
        .load("return messages.new():system('s'):user('u')")
        .eval::<Value>()
        .and_then(|value| vm.lua().from_value(value))
        .expect("builder output converts under the hardened sandbox");
    assert_eq!(
        json,
        json!([{ "role": "system", "content": "s" }, { "role": "user", "content": "u" }])
    );
    vm.teardown(&observer, "Test");
}
