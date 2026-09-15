use mlua::{Lua, Value};
use serde_json::json;

use super::project_messages;
use crate::Error;
use crate::protocol::{
    ContentPart, MessageContent, MessageRecord, MessageRole, Request, ToolCallRecord, YieldParse,
};

fn record(role: MessageRole, text: &str) -> MessageRecord {
    MessageRecord {
        role,
        content: MessageContent::Text(text.to_owned()),
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

fn system(text: &str) -> MessageRecord {
    record(MessageRole::System, text)
}

fn user(text: &str) -> MessageRecord {
    record(MessageRole::User, text)
}

fn assistant(text: &str) -> MessageRecord {
    record(MessageRole::Assistant, text)
}

fn tool(id: &str, text: &str) -> MessageRecord {
    MessageRecord {
        role: MessageRole::Tool,
        content: MessageContent::Text(text.to_owned()),
        tool_calls: Vec::new(),
        tool_call_id: Some(id.to_owned()),
    }
}

fn call(id: &str, name: &str) -> ToolCallRecord {
    ToolCallRecord {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: json!({}),
    }
}

fn assistant_calls(text: &str, calls: Vec<ToolCallRecord>) -> MessageRecord {
    MessageRecord {
        role: MessageRole::Assistant,
        content: MessageContent::Text(text.to_owned()),
        tool_calls: calls,
        tool_call_id: None,
    }
}

/// Projects and serializes, so every assertion is about what the provider
/// would receive on the wire.
fn wire(records: &[MessageRecord]) -> serde_json::Value {
    serde_json::to_value(project_messages(records).expect("the list must project"))
        .expect("wire messages serialize")
}

fn projection_error(records: &[MessageRecord]) -> String {
    match project_messages(records) {
        Err(Error::Lua(message)) => message,
        other => panic!("expected a projection Lua error, got {other:?}"),
    }
}

/// Parses a Lua-authored message list through the chat protocol boundary,
/// exactly as a `models.chat` yield would, so the projection sees the same
/// records dispatch sees.
fn lua_parse(lua: &Lua, messages: &str) -> Vec<MessageRecord> {
    let request = lua.create_table().expect("table creation cannot fail");
    request.raw_set("op", "chat").expect("raw_set");
    request
        .raw_set(
            "messages",
            lua.load(messages)
                .eval::<Value>()
                .expect("test table source evaluates"),
        )
        .expect("raw_set");
    match Request::from_yield(lua, &Value::Table(request)) {
        YieldParse::Request(Request::Chat { messages, .. }) => messages,
        other => panic!("expected a chat request, got {other:?}"),
    }
}

#[test]
fn a_well_formed_conversation_projects_unchanged() {
    let records = vec![
        system("be terse"),
        user("hi"),
        assistant("hello"),
        user("thanks"),
    ];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "system", "content": "be terse" },
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "hello" },
            { "role": "user", "content": "thanks" },
        ])
    );
}

#[test]
fn multiple_leading_system_messages_compose_into_one_without_mutating_the_source() {
    let records = vec![system("be terse"), system("answer in English"), user("hi")];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "system", "content": "be terse\n\nanswer in English" },
            { "role": "user", "content": "hi" },
        ]),
        "the provider's single-system-message shape is composed, not rejected"
    );
    assert_eq!(
        records[0].content,
        MessageContent::Text("be terse".to_owned()),
        "the source array is the author's continuity state: projection never mutates it"
    );
    assert_eq!(records.len(), 3);
}

#[test]
fn a_single_leading_system_message_passes_through_verbatim() {
    let parts = MessageRecord {
        role: MessageRole::System,
        content: MessageContent::Parts(vec![
            ContentPart::Text("look".to_owned()),
            ContentPart::ImageUrl("data:image/png;base64,AA".to_owned()),
        ]),
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    let records = vec![parts, user("hi")];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "system", "content": [
                { "type": "text", "text": "look" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AA" } },
            ] },
            { "role": "user", "content": "hi" },
        ]),
        "no composition means no flattening: one system message keeps its parts"
    );
}

#[test]
fn consecutive_same_role_user_messages_normalize_into_one() {
    let records = vec![user("one"), user("two"), assistant("a"), user("three")];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "user", "content": "one\n\ntwo" },
            { "role": "assistant", "content": "a" },
            { "role": "user", "content": "three" },
        ]),
        "two user utterances join with a blank line; alternation holds by construction"
    );
}

#[test]
fn streaming_fragments_coalesce_into_one_assistant_result() {
    let records = vec![
        user("hi"),
        assistant("Hello, "),
        assistant("world"),
        assistant("!"),
        user("next"),
    ];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "Hello, world!" },
            { "role": "user", "content": "next" },
        ]),
        "fragments of one reply reassemble with no separator"
    );
    // Parts and text fragments interleave into one parts array.
    let mixed = vec![
        assistant(""),
        MessageRecord {
            role: MessageRole::Assistant,
            content: MessageContent::Parts(vec![ContentPart::Text("a".to_owned())]),
            tool_calls: Vec::new(),
            tool_call_id: None,
        },
        assistant("b"),
    ];
    assert_eq!(
        wire(&mixed),
        json!([{ "role": "assistant", "content": [
            { "type": "text", "text": "a" },
            { "type": "text", "text": "b" },
        ] }]),
        "an empty fragment is absorbed and parts concatenate in arrival order"
    );
}

#[test]
fn a_complete_tool_exchange_projects_verbatim() {
    let records = vec![
        user("call the tools"),
        assistant_calls(
            "working",
            vec![call("call_1", "echo"), call("call_2", "search")],
        ),
        tool("call_2", "found"),
        tool("call_1", "echoed"),
        assistant("done"),
    ];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "user", "content": "call the tools" },
            { "role": "assistant", "content": "working", "tool_calls": [
                { "id": "call_1", "name": "echo", "arguments": {} },
                { "id": "call_2", "name": "search", "arguments": {} },
            ] },
            { "role": "tool", "content": "found", "tool_call_id": "call_2" },
            { "role": "tool", "content": "echoed", "tool_call_id": "call_1" },
            { "role": "assistant", "content": "done" },
        ]),
        "results may answer in any order within the atomic block; nothing coalesces across it"
    );
}

#[test]
fn abnormal_edges_heal_into_a_clean_alternation() {
    let records = vec![
        system("s1"),
        system("s2"),
        user(""),
        user("hi"),
        assistant(""),
        assistant("answer"),
        assistant(""),
        user("next"),
    ];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "system", "content": "s1\n\ns2" },
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "answer" },
            { "role": "user", "content": "next" },
        ]),
        "empty fragments vanish into their runs instead of poisoning the join"
    );
}

#[test]
fn a_text_fragment_merges_into_a_following_tool_call_turn() {
    let records = vec![
        user("hi"),
        assistant("let me check"),
        assistant_calls("", vec![call("call_1", "echo")]),
        tool("call_1", "echoed"),
        assistant("done"),
    ];
    assert_eq!(
        wire(&records),
        json!([
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "let me check", "tool_calls": [
                { "id": "call_1", "name": "echo", "arguments": {} },
            ] },
            { "role": "tool", "content": "echoed", "tool_call_id": "call_1" },
            { "role": "assistant", "content": "done" },
        ]),
        "visible text rides the tool-call turn rather than breaking alternation"
    );
}

#[test]
fn metadata_beyond_the_contract_is_stripped_from_the_wire() {
    let lua = Lua::new();
    let records = lua_parse(
        &lua,
        r#"{
            { role = "user", content = "hi", name = "bob", weight = 3 },
            { role = "assistant", content = "hello", extra = { nested = true } },
        }"#,
    );
    assert_eq!(
        wire(&records),
        json!([
            { "role": "user", "content": "hi" },
            { "role": "assistant", "content": "hello" },
        ]),
        "exactly role, content, tool_call_id, and tool_calls may reach the provider"
    );
}

#[test]
fn malformed_records_fail_at_projection_naming_the_index() {
    let mut with_calls = user("hi");
    with_calls.tool_calls = vec![call("call_1", "echo")];
    assert_eq!(
        projection_error(&[with_calls]),
        "messages[1] carries tool_calls but is not an assistant message"
    );
    let mut with_id = user("hi");
    with_id.tool_call_id = Some("call_1".to_owned());
    assert_eq!(
        projection_error(&[with_id]),
        "messages[1] carries a tool_call_id but is not a tool message"
    );
    assert_eq!(
        projection_error(&[user("hi"), system("late")]),
        "messages[2] is a system message outside the leading system block"
    );
    let parts_system = MessageRecord {
        role: MessageRole::System,
        content: MessageContent::Parts(vec![ContentPart::Text("look".to_owned())]),
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    assert_eq!(
        projection_error(&[parts_system, system("be terse"), user("hi")]),
        "messages[1] is a system message with content parts; only plain text \
         system messages can be composed"
    );
}

#[test]
fn an_orphan_tool_record_is_rejected() {
    assert_eq!(
        projection_error(&[user("hi"), tool("call_9", "loose")]),
        "messages[2] is an orphan tool record: no pending assistant tool call \
         with id \"call_9\""
    );
    // An id outside the open batch is just as orphaned.
    assert_eq!(
        projection_error(&[
            assistant_calls("", vec![call("call_1", "echo")]),
            tool("call_2", "wrong"),
        ]),
        "messages[2] is an orphan tool record: no pending assistant tool call \
         with id \"call_2\""
    );
    // A second answer to an answered call is an orphan too.
    assert_eq!(
        projection_error(&[
            assistant_calls("", vec![call("call_1", "echo")]),
            tool("call_1", "echoed"),
            tool("call_1", "again"),
        ]),
        "messages[3] is an orphan tool record: no pending assistant tool call \
         with id \"call_1\""
    );
}

#[test]
fn a_duplicate_tool_call_id_is_rejected() {
    assert_eq!(
        projection_error(&[assistant_calls(
            "",
            vec![call("call_1", "echo"), call("call_1", "search")],
        )]),
        "messages[1] tool call id \"call_1\" duplicates an earlier tool call"
    );
    assert_eq!(
        projection_error(&[
            assistant_calls("", vec![call("call_1", "echo")]),
            tool("call_1", "echoed"),
            assistant_calls("", vec![call("call_1", "search")]),
        ]),
        "messages[3] tool call id \"call_1\" duplicates an earlier tool call",
        "ids stay unique across the whole list, not only within one batch"
    );
}

#[test]
fn an_unanswered_tool_call_is_rejected() {
    assert_eq!(
        projection_error(&[
            user("hi"),
            assistant_calls("", vec![call("call_1", "echo")])
        ]),
        "messages[2] tool call \"call_1\" has no tool result",
        "a list may not end on an open batch"
    );
    assert_eq!(
        projection_error(&[
            assistant_calls("", vec![call("call_1", "echo"), call("call_2", "search")]),
            tool("call_1", "echoed"),
            user("moving on"),
        ]),
        "messages[1] tool call \"call_2\" has no tool result",
        "any other role before the batch completes breaks atomic pairing"
    );
}

#[test]
fn secrets_in_stripped_fields_never_reach_the_wire() {
    // Regression: an author record carrying a credential in a field outside
    // the contract must not leak it into the provider request.
    let lua = Lua::new();
    let records = lua_parse(
        &lua,
        r#"{
            { role = "system", content = "be terse", api_key = "super-secret-token" },
            { role = "user", content = "hi", authorization = "super-secret-token" },
        }"#,
    );
    let rendered = wire(&records).to_string();
    assert!(
        !rendered.contains("super-secret-token"),
        "the projection must strip every field outside the contract: {rendered}"
    );
    assert_eq!(
        wire(&records),
        json!([
            { "role": "system", "content": "be terse" },
            { "role": "user", "content": "hi" },
        ])
    );
}
