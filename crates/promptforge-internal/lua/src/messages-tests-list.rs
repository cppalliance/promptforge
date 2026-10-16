//! Tests for the Rust-backed [`MessageList`]: its builders, `replace`, the
//! leading-system rule, and the send record `commit` keeps.

use promptforge_model_client::client::{Message, Role};
use serde_json::json;

use crate::messages::{ListEditError, MessageList};
use crate::protocol::{MessageContent, MessageRecord, ToolCallRecord};
use crate::test_vm::{BuiltVm, TestVm};
use crate::tests::{assistant, system, tool_result, user};
use crate::{Error, project_messages};

#[path = "messages-tests-list-commit.rs"]
mod commit;

/// The message of the crate error a list method raised, not its
/// flattened traceback.
#[track_caller]
pub(super) fn refusal(vm: &BuiltVm, source: &str) -> String {
    let error = vm.eval_err(source);
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

/// The list's records by role and text, for compact assertions.
fn texts(list: &MessageList) -> Vec<(Role, String)> {
    list.to_records()
        .into_iter()
        .map(|record| match record.content {
            MessageContent::Text(content) => (record.role.kind(), content),
            MessageContent::Parts(_) => panic!("these tests build text records only"),
        })
        .collect()
}

/// The roles and texts `entries` names.
fn expect(entries: &[(Role, &str)]) -> Vec<(Role, String)> {
    entries
        .iter()
        .map(|(role, content)| (*role, (*content).to_owned()))
        .collect()
}

/// The wire request a round would send for the list right now.
fn request(list: &MessageList) -> Vec<Message> {
    project_messages(&list.to_records()).expect("the test list projects")
}

#[test]
fn each_builder_chains_and_appends_its_record() {
    let vm = TestVm::new().messages().build();
    let same: bool = vm.eval(
        "return msgs\
         :system('be terse')\
         :user('hi')\
         :assistant('working', { { id = 'call_1', name = 'echo', arguments = { value = 'hi' } } })\
         :tool('echoed', 'call_1')\
         :assistant('done')\
         :append({ role = 'user', content = 'thanks' }) == msgs",
    );
    assert!(same, "every builder must return the list it was called on");
    assert_eq!(
        vm.messages().to_records(),
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
            tool_result("call_1", "echoed"),
            assistant("done"),
            user("thanks"),
        ]
    );
}

#[test]
fn push_refuses_a_late_system_record_as_a_typed_edit_error_naming_its_position() {
    let list = MessageList::default();
    list.push(system("lead")).expect("a leading system record");
    list.push(user("hi")).expect("a user record after it");
    let refusal = list
        .push(system("late"))
        .expect_err("a system record behind a user record");
    assert_eq!(refusal, ListEditError::LateSystem { index: 3 });
    assert!(
        refusal
            .to_string()
            .starts_with("messages[3] is a system message"),
        "the refusal names the position: {refusal}"
    );
    assert_eq!(list.to_records(), vec![system("lead"), user("hi")]);
}

/// Fills the list with `u1 a1 u2 a2`.
#[track_caller]
fn four(vm: &BuiltVm) {
    vm.exec("msgs:user('u1'):assistant('a1'):user('u2'):assistant('a2')");
}

#[test]
fn replace_with_no_records_deletes_the_range() {
    use Role::{Assistant, User};
    let vm = TestVm::new().messages().build();
    four(&vm);
    vm.exec("msgs:replace(2, 3)");
    assert_eq!(
        texts(vm.messages()),
        expect(&[(User, "u1"), (Assistant, "a2")])
    );
}

#[test]
fn replace_with_first_one_past_last_inserts_before_first() {
    use Role::{Assistant, System, User};
    let vm = TestVm::new().messages().build();
    four(&vm);
    vm.exec("msgs:replace(3, 2, { role = 'user', content = 'mid' })");
    vm.exec("msgs:replace(1, 0, { role = 'system', content = 'front' })");
    vm.exec("msgs:replace(#msgs + 1, #msgs, { role = 'user', content = 'end' })");
    assert_eq!(
        texts(vm.messages()),
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
    use Role::{System, User};
    let vm = TestVm::new().messages().build();
    four(&vm);
    vm.exec(
        "msgs:replace(1, #msgs, { role = 'system', content = 's' }, \
         { role = 'user', content = 'summary' })",
    );
    assert_eq!(
        texts(vm.messages()),
        expect(&[(System, "s"), (User, "summary")])
    );
}

#[test]
fn replace_out_of_bounds_is_refused_naming_the_bounds() {
    let vm = TestVm::new().messages().build();
    let list = vm.messages();
    four(&vm);
    let before = list.to_records();
    for (call, range) in [
        ("msgs:replace(0, 0)", "replace(0, 0)"),
        ("msgs:replace(3, 1)", "replace(3, 1)"),
        ("msgs:replace(2, 5)", "replace(2, 5)"),
        ("msgs:replace(6, 5)", "replace(6, 5)"),
        ("msgs:replace(-1, 2)", "replace(-1, 2)"),
    ] {
        assert_eq!(
            refusal(&vm, call),
            format!(
                "{range} is out of bounds on a list of 4 records: it needs \
                 1 <= first <= last + 1 <= 5"
            ),
            "{call}"
        );
    }
    assert_eq!(
        list.to_records(),
        before,
        "a refused replace leaves the list unchanged"
    );
}

#[test]
fn replace_takes_integral_floats_and_refuses_other_bounds() {
    let vm = TestVm::new().messages().build();
    let list = vm.messages();
    four(&vm);
    let before = list.to_records();
    assert_eq!(
        refusal(&vm, "msgs:replace(1.5, 2)"),
        "replace first must be an integer, got 1.5"
    );
    assert_eq!(
        refusal(&vm, "msgs:replace(1, '2')"),
        "replace last must be an integer, got string"
    );
    assert_eq!(
        refusal(&vm, "msgs:replace(1)"),
        "replace last must be an integer, got nil"
    );
    assert_eq!(
        list.to_records(),
        before,
        "a refused replace leaves the list unchanged"
    );
    vm.exec("msgs:replace(2.0, 4.0)");
    assert_eq!(list.to_records(), vec![user("u1")]);
}

#[test]
fn a_system_record_after_a_non_system_record_is_refused_by_every_edit() {
    let vm = TestVm::new().messages().build();
    let list = vm.messages();
    vm.exec("msgs:system('s1'):system('s2'):user('u')");
    let before = list.to_records();
    let late = |index: usize| {
        format!(
            "messages[{index}] is a system message after a non-system message; \
             system messages lead the list, so change the leading block with \
             replace(first, last, records...)"
        )
    };
    assert_eq!(refusal(&vm, "msgs:system('late')"), late(4));
    assert_eq!(
        refusal(&vm, "msgs:append({ role = 'system', content = 'late' })"),
        late(4)
    );
    assert_eq!(
        refusal(
            &vm,
            "msgs:replace(4, 3, { role = 'system', content = 'late' })"
        ),
        late(4)
    );
    // Replacing the first system record with a user record leaves the
    // second one behind a non-system record.
    assert_eq!(
        refusal(&vm, "msgs:replace(1, 1, { role = 'user', content = 'u0' })"),
        late(2)
    );
    assert_eq!(
        list.to_records(),
        before,
        "a refused edit leaves the list unchanged"
    );
    vm.exec(
        "msgs:replace(1, 2, { role = 'system', content = 'one' }, \
         { role = 'system', content = 'two' }, { role = 'system', content = 'three' })",
    );
    assert_eq!(
        list.to_records(),
        vec![system("one"), system("two"), system("three"), user("u")],
        "edits inside the leading block are allowed"
    );
}

#[test]
fn a_record_that_breaks_a_per_record_rule_is_refused_at_its_position() {
    let vm = TestVm::new().messages().build();
    let list = vm.messages();
    assert_eq!(
        refusal(&vm, "msgs:user()"),
        "messages[1] content must be a string or a non-empty array of content parts"
    );
    vm.exec("msgs:user('u')");
    let before = list.to_records();
    assert_eq!(
        refusal(&vm, "msgs:tool('result')"),
        "messages[2] is a tool message and must set a string tool_call_id"
    );
    assert_eq!(
        refusal(&vm, "msgs:assistant('a', { { name = 'echo' } })"),
        "messages[2] tool_calls[1] must set a string id"
    );
    assert_eq!(
        refusal(&vm, "msgs:append({ role = 'robot', content = 'beep' })"),
        "messages[2] role \"robot\" is unknown; known roles: system, user, assistant, tool"
    );
    assert_eq!(
        refusal(&vm, "msgs:append('hello')"),
        "messages[2] must be a message table"
    );
    assert_eq!(
        refusal(
            &vm,
            "msgs:replace(1, 1, { role = 'user', content = 'ok' }, { role = 'user' })"
        ),
        "messages[2] content must be a string or a non-empty array of content parts"
    );
    assert_eq!(
        list.to_records(),
        before,
        "a refused record leaves the list unchanged"
    );
}

#[test]
fn a_value_that_cannot_convert_to_json_is_refused_naming_where_it_was_given() {
    let vm = TestVm::new().messages().build();
    let list = vm.messages();
    vm.exec("msgs:user('u')");
    let before = list.to_records();
    let unsupported = "unsupported value type `function`";
    for (call, subject) in [
        ("msgs:system(tostring)", "messages[2] content"),
        ("msgs:user({ tostring })", "messages[2] content"),
        (
            "msgs:assistant('a', { { id = 'call_1', name = 'echo', arguments = tostring } })",
            "messages[2] tool_calls",
        ),
        ("msgs:tool('result', tostring)", "messages[2] tool_call_id"),
    ] {
        assert_eq!(
            refusal(&vm, call),
            format!("{subject} must be JSON-representable: {unsupported}"),
            "{call}"
        );
    }
    for (call, index) in [
        ("msgs:append(tostring)", 2),
        ("msgs:append({ role = 'user', content = tostring })", 2),
        (
            "msgs:replace(1, 1, { role = 'user', content = 'ok' }, tostring)",
            2,
        ),
    ] {
        assert_eq!(
            refusal(&vm, call),
            format!("messages[{index}] must be a JSON-representable message table: {unsupported}"),
            "{call}"
        );
    }
    assert_eq!(
        list.to_records(),
        before,
        "a refused value leaves the list unchanged"
    );
}

#[test]
fn append_stores_a_record_without_its_extra_fields() {
    let vm = TestVm::new().messages().build();
    vm.exec("msgs:append({ role = 'user', content = 'raw', extra = 1, secret = 'key' })");
    assert_eq!(vm.messages().to_records(), vec![user("raw")]);
}

#[test]
fn index_assignment_is_refused_and_len_counts_the_records() {
    let vm = TestVm::new().messages().build();
    let empty: usize = vm.eval("return #msgs");
    assert_eq!(empty, 0);
    vm.exec("msgs:user('u'):assistant('a')");
    let len: usize = vm.eval("return #msgs");
    assert_eq!(len, 2);
    let refused = "a messages.new() list cannot be assigned to; add records with \
                   append(record) and change them with replace(first, last, records...)";
    for assignment in [
        "msgs[1] = { role = 'user', content = 'x' }",
        "msgs[3] = { role = 'user', content = 'x' }",
        "msgs.extra = 1",
    ] {
        assert_eq!(refusal(&vm, assignment), refused, "{assignment}");
    }
    assert_eq!(vm.messages().to_records(), vec![user("u"), assistant("a")]);
}
