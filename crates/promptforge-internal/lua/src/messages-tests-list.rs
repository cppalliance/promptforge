//! Tests for the Rust-backed [`MessageList`]: its builders, `replace`, the
//! leading-system rule, and the send record `commit` keeps.

use mlua::Lua;
use promptforge_model_client::client::Message;
use serde_json::json;

use crate::messages::MessageList;
use crate::protocol::{MessageContent, MessageRecord, MessageRole, ToolCallRecord};
use crate::{Error, project_messages};

#[path = "messages-tests-list-commit.rs"]
mod commit;

/// A VM holding a fresh list as the global `msgs`, and the Rust handle
/// that shares it.
pub(super) fn vm() -> (Lua, MessageList) {
    let lua = Lua::new();
    let list = MessageList::default();
    let userdata = lua
        .create_userdata(list.clone())
        .expect("userdata creation cannot fail");
    lua.globals()
        .set("msgs", userdata)
        .expect("a global set cannot fail");
    (lua, list)
}

pub(super) fn run(lua: &Lua, source: &str) {
    lua.load(source).exec().expect("test source runs");
}

/// The message of the crate error a list method raised, not its
/// flattened traceback.
pub(super) fn refusal(lua: &Lua, source: &str) -> String {
    let error = lua
        .load(source)
        .exec()
        .expect_err("the list must refuse the call");
    let cause = match &error {
        mlua::Error::CallbackError { cause, .. } => cause.as_ref(),
        other => other,
    };
    match cause {
        mlua::Error::ExternalError(cause) => match cause.downcast_ref::<Error>() {
            Some(Error::Lua(message)) => message.clone(),
            other => panic!("expected an Error::Lua refusal, got {other:?}"),
        },
        other => panic!("expected the list's refusal, got {other:?}"),
    }
}

fn text(role: MessageRole, content: &str) -> MessageRecord {
    MessageRecord {
        role,
        content: MessageContent::Text(content.to_owned()),
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

fn system(content: &str) -> MessageRecord {
    text(MessageRole::System, content)
}

fn user(content: &str) -> MessageRecord {
    text(MessageRole::User, content)
}

fn assistant(content: &str) -> MessageRecord {
    text(MessageRole::Assistant, content)
}

/// The list's records by role and text, for compact assertions.
fn texts(list: &MessageList) -> Vec<(MessageRole, String)> {
    list.records()
        .into_iter()
        .map(|record| match record.content {
            MessageContent::Text(content) => (record.role, content),
            MessageContent::Parts(_) => panic!("these tests build text records only"),
        })
        .collect()
}

/// The roles and texts `entries` names.
fn expect(entries: &[(MessageRole, &str)]) -> Vec<(MessageRole, String)> {
    entries
        .iter()
        .map(|(role, content)| (*role, (*content).to_owned()))
        .collect()
}

/// The wire request a round would send for the list right now.
fn request(list: &MessageList) -> Vec<Message> {
    project_messages(&list.records()).expect("the test list projects")
}

#[test]
fn each_builder_chains_and_appends_its_record() {
    let (lua, list) = vm();
    let same: bool = lua
        .load(
            "return msgs\
             :system('be terse')\
             :user('hi')\
             :assistant('working', { { id = 'call_1', name = 'echo', arguments = { value = 'hi' } } })\
             :tool('echoed', 'call_1')\
             :assistant('done')\
             :append({ role = 'user', content = 'thanks' }) == msgs",
        )
        .eval()
        .expect("test source evaluates");
    assert!(same, "every builder must return the list it was called on");
    assert_eq!(
        list.records(),
        vec![
            system("be terse"),
            user("hi"),
            MessageRecord {
                tool_calls: vec![ToolCallRecord {
                    id: "call_1".to_owned(),
                    name: "echo".to_owned(),
                    arguments: json!({ "value": "hi" }),
                }],
                ..assistant("working")
            },
            MessageRecord {
                tool_call_id: Some("call_1".to_owned()),
                ..text(MessageRole::Tool, "echoed")
            },
            assistant("done"),
            user("thanks"),
        ]
    );
}

/// Fills the list with `u1 a1 u2 a2`.
fn four(lua: &Lua) {
    run(
        lua,
        "msgs:user('u1'):assistant('a1'):user('u2'):assistant('a2')",
    );
}

#[test]
fn replace_with_no_records_deletes_the_range() {
    use MessageRole::{Assistant, User};
    let (lua, list) = vm();
    four(&lua);
    run(&lua, "msgs:replace(2, 3)");
    assert_eq!(texts(&list), expect(&[(User, "u1"), (Assistant, "a2")]));
}

#[test]
fn replace_with_first_one_past_last_inserts_before_first() {
    use MessageRole::{Assistant, System, User};
    let (lua, list) = vm();
    four(&lua);
    run(
        &lua,
        "msgs:replace(3, 2, { role = 'user', content = 'mid' })",
    );
    run(
        &lua,
        "msgs:replace(1, 0, { role = 'system', content = 'front' })",
    );
    run(
        &lua,
        "msgs:replace(#msgs + 1, #msgs, { role = 'user', content = 'end' })",
    );
    assert_eq!(
        texts(&list),
        expect(&[
            (System, "front"),
            (User, "u1"),
            (Assistant, "a1"),
            (User, "mid"),
            (User, "u2"),
            (Assistant, "a2"),
            (User, "end"),
        ])
    );
}

#[test]
fn replace_over_the_whole_list_swaps_every_record() {
    use MessageRole::{System, User};
    let (lua, list) = vm();
    four(&lua);
    run(
        &lua,
        "msgs:replace(1, #msgs, { role = 'system', content = 's' }, \
         { role = 'user', content = 'summary' })",
    );
    assert_eq!(texts(&list), expect(&[(System, "s"), (User, "summary")]));
}

#[test]
fn replace_out_of_bounds_is_refused_naming_the_bounds() {
    let (lua, list) = vm();
    four(&lua);
    let before = list.records();
    for (call, range) in [
        ("msgs:replace(0, 0)", "replace(0, 0)"),
        ("msgs:replace(3, 1)", "replace(3, 1)"),
        ("msgs:replace(2, 5)", "replace(2, 5)"),
        ("msgs:replace(6, 5)", "replace(6, 5)"),
        ("msgs:replace(-1, 2)", "replace(-1, 2)"),
    ] {
        assert_eq!(
            refusal(&lua, call),
            format!(
                "{range} is out of bounds on a list of 4 records: it needs \
                 1 <= first <= last + 1 <= 5"
            ),
            "{call}"
        );
    }
    assert_eq!(
        list.records(),
        before,
        "a refused replace leaves the list unchanged"
    );
}

#[test]
fn replace_takes_integral_floats_and_refuses_other_bounds() {
    let (lua, list) = vm();
    four(&lua);
    let before = list.records();
    assert_eq!(
        refusal(&lua, "msgs:replace(1.5, 2)"),
        "replace first must be an integer, got 1.5"
    );
    assert_eq!(
        refusal(&lua, "msgs:replace(1, '2')"),
        "replace last must be an integer, got string"
    );
    assert_eq!(
        refusal(&lua, "msgs:replace(1)"),
        "replace last must be an integer, got nil"
    );
    assert_eq!(
        list.records(),
        before,
        "a refused replace leaves the list unchanged"
    );
    run(&lua, "msgs:replace(2.0, 4.0)");
    assert_eq!(list.records(), vec![user("u1")]);
}

#[test]
fn a_system_record_after_a_non_system_record_is_refused_by_every_edit() {
    let (lua, list) = vm();
    run(&lua, "msgs:system('s1'):system('s2'):user('u')");
    let before = list.records();
    let late = |index: usize| {
        format!(
            "messages[{index}] is a system message after a non-system message; \
             system messages lead the list, so change the leading block with \
             replace(first, last, records...)"
        )
    };
    assert_eq!(refusal(&lua, "msgs:system('late')"), late(4));
    assert_eq!(
        refusal(&lua, "msgs:append({ role = 'system', content = 'late' })"),
        late(4)
    );
    assert_eq!(
        refusal(
            &lua,
            "msgs:replace(4, 3, { role = 'system', content = 'late' })"
        ),
        late(4)
    );
    // Replacing the first system record with a user record leaves the
    // second one behind a non-system record.
    assert_eq!(
        refusal(
            &lua,
            "msgs:replace(1, 1, { role = 'user', content = 'u0' })"
        ),
        late(2)
    );
    assert_eq!(
        list.records(),
        before,
        "a refused edit leaves the list unchanged"
    );
    run(
        &lua,
        "msgs:replace(1, 2, { role = 'system', content = 'one' }, \
         { role = 'system', content = 'two' }, { role = 'system', content = 'three' })",
    );
    assert_eq!(
        list.records(),
        vec![system("one"), system("two"), system("three"), user("u")],
        "edits inside the leading block are allowed"
    );
}

#[test]
fn a_record_that_breaks_a_per_record_rule_is_refused_at_its_position() {
    let (lua, list) = vm();
    assert_eq!(
        refusal(&lua, "msgs:user()"),
        "messages[1] content must be a string or a non-empty array of content parts"
    );
    run(&lua, "msgs:user('u')");
    let before = list.records();
    assert_eq!(
        refusal(&lua, "msgs:tool('result')"),
        "messages[2] is a tool message and must set a string tool_call_id"
    );
    assert_eq!(
        refusal(&lua, "msgs:assistant('a', { { name = 'echo' } })"),
        "messages[2] tool_calls[1] must set a string id"
    );
    assert_eq!(
        refusal(&lua, "msgs:append({ role = 'robot', content = 'beep' })"),
        "messages[2] role \"robot\" is unknown; known roles: system, user, assistant, tool"
    );
    assert_eq!(
        refusal(&lua, "msgs:append('hello')"),
        "messages[2] must be a message table"
    );
    assert_eq!(
        refusal(
            &lua,
            "msgs:replace(1, 1, { role = 'user', content = 'ok' }, { role = 'user' })"
        ),
        "messages[2] content must be a string or a non-empty array of content parts"
    );
    assert_eq!(
        list.records(),
        before,
        "a refused record leaves the list unchanged"
    );
}

#[test]
fn a_value_that_cannot_convert_to_json_is_refused_naming_where_it_was_given() {
    let (lua, list) = vm();
    run(&lua, "msgs:user('u')");
    let before = list.records();
    let unsupported = "unsupported value type `function`";
    for (call, subject) in [
        ("msgs:system(print)", "messages[2] content"),
        ("msgs:user({ print })", "messages[2] content"),
        (
            "msgs:assistant('a', { { id = 'call_1', name = 'echo', arguments = print } })",
            "messages[2] tool_calls",
        ),
        ("msgs:tool('result', print)", "messages[2] tool_call_id"),
    ] {
        assert_eq!(
            refusal(&lua, call),
            format!("{subject} must be JSON-representable: {unsupported}"),
            "{call}"
        );
    }
    for (call, index) in [
        ("msgs:append(print)", 2),
        ("msgs:append({ role = 'user', content = print })", 2),
        (
            "msgs:replace(1, 1, { role = 'user', content = 'ok' }, print)",
            2,
        ),
    ] {
        assert_eq!(
            refusal(&lua, call),
            format!("messages[{index}] must be a JSON-representable message table: {unsupported}"),
            "{call}"
        );
    }
    assert_eq!(
        list.records(),
        before,
        "a refused value leaves the list unchanged"
    );
}

#[test]
fn append_stores_a_record_without_its_extra_fields() {
    let (lua, list) = vm();
    run(
        &lua,
        "msgs:append({ role = 'user', content = 'raw', extra = 1, secret = 'key' })",
    );
    assert_eq!(list.records(), vec![user("raw")]);
}

#[test]
fn index_assignment_is_refused_and_len_counts_the_records() {
    let (lua, list) = vm();
    let empty: usize = lua.load("return #msgs").eval().expect("len evaluates");
    assert_eq!(empty, 0);
    run(&lua, "msgs:user('u'):assistant('a')");
    let len: usize = lua.load("return #msgs").eval().expect("len evaluates");
    assert_eq!(len, 2);
    let refused = "a messages.new() list cannot be assigned to; add records with \
                   append(record) and change them with replace(first, last, records...)";
    for assignment in [
        "msgs[1] = { role = 'user', content = 'x' }",
        "msgs[3] = { role = 'user', content = 'x' }",
        "msgs.extra = 1",
    ] {
        assert_eq!(refusal(&lua, assignment), refused, "{assignment}");
    }
    assert_eq!(list.records(), vec![user("u"), assistant("a")]);
}
