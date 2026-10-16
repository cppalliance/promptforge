//! Tests for the read-only record views a list's index and iteration
//! return, and the record JSON form they serialize through.

use mlua::{LuaSerdeExt, Value};
use serde_json::{Value as Json, json};

use super::list::refusal;
use crate::messages::MessageList;
use crate::protocol::{
    ContentPart, MessageContent, MessageRecord, NonEmpty, ToolCallRecord, parse_record,
};
use crate::test_vm::{BuiltVm, TestVm};
use crate::tests::{assistant, system, tool_result, user};

/// One record table of each shape, held in the global `shapes` and
/// appended to `msgs` in order: plain text, content parts, tool calls,
/// and a tool result.
#[track_caller]
fn shapes(vm: &BuiltVm) {
    vm.exec(
        "shapes = { \
           { role = 'system', content = 'be terse' }, \
           { role = 'user', content = { \
             { type = 'text', text = 'look' }, \
             { type = 'image_url', image_url = { url = 'data:image/png;base64,AA==' } } } }, \
           { role = 'assistant', content = 'working', \
             tool_calls = { { id = 'call_1', name = 'echo', arguments = { value = 'hi' } } } }, \
           { role = 'tool', content = 'echoed: hi', tool_call_id = 'call_1' } } \
         for _, record in ipairs(shapes) do msgs:append(record) end",
    );
}

/// What `expr` evaluates to in its JSON form, or `None` when it is nil.
#[track_caller]
fn read(vm: &BuiltVm, expr: &str) -> Option<Json> {
    match vm.eval::<Value>(&format!("return {expr}")) {
        Value::Nil => None,
        value => Some(
            vm.lua()
                .from_value(value)
                .expect("the value converts to JSON"),
        ),
    }
}

#[test]
fn to_json_round_trips_every_record_shape_through_parse_record() {
    let records = [
        system("be terse"),
        MessageRecord {
            content: MessageContent::Parts(NonEmpty::new(
                ContentPart::Text("look".to_owned()),
                vec![ContentPart::ImageUrl(
                    "data:image/png;base64,AA==".to_owned(),
                )],
            )),
            ..user("")
        },
        MessageRecord {
            tool_calls: vec![ToolCallRecord {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({ "value": "hi" }),
            }],
            ..assistant("working")
        },
        tool_result("call_1", "echoed: hi"),
    ];
    assert_eq!(
        records
            .iter()
            .map(MessageRecord::to_json)
            .collect::<Vec<_>>(),
        vec![
            json!({ "role": "system", "content": "be terse" }),
            json!({ "role": "user", "content": [
                { "type": "text", "text": "look" },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,AA==" } },
            ] }),
            json!({ "role": "assistant", "content": "working", "tool_calls": [
                { "id": "call_1", "name": "echo", "arguments": { "value": "hi" } },
            ] }),
            json!({ "role": "tool", "content": "echoed: hi", "tool_call_id": "call_1" }),
        ],
        "absent tool_calls and tool_call_id stay out of the JSON form"
    );
    for (position, record) in records.iter().enumerate() {
        assert_eq!(
            parse_record(position + 1, &record.to_json()).as_ref(),
            Ok(record),
            "{record:?}"
        );
    }
}

#[test]
fn each_view_field_reads_like_the_record_table_and_a_view_is_userdata() {
    let vm = TestVm::new().messages().build();
    shapes(&vm);
    for index in 1..=4 {
        assert_eq!(
            read(&vm, &format!("type(msgs[{index}])")),
            Some(json!("userdata"))
        );
        for field in ["role", "content", "tool_calls", "tool_call_id"] {
            assert_eq!(
                read(&vm, &format!("msgs[{index}].{field}")),
                read(&vm, &format!("shapes[{index}].{field}")),
                "msgs[{index}].{field}"
            );
        }
    }
}

#[test]
fn an_integral_float_index_reads_like_its_integer_and_other_keys_read_nil() {
    let vm = TestVm::new().messages().build();
    vm.exec("msgs:user('u1'):append({ role = 'user', content = 'u2', extra = 1 })");
    assert_eq!(
        read(&vm, "msgs[2.0]"),
        Some(json!({ "role": "user", "content": "u2" }))
    );
    assert_eq!(read(&vm, "msgs[2]"), read(&vm, "msgs[2.0]"));
    for expr in [
        "msgs[1.5]",
        "msgs.nope",
        "msgs[true]",
        "msgs[0]",
        "msgs[-1]",
        "msgs[3]",
        "msgs[2].extra",
    ] {
        assert_eq!(read(&vm, expr), None, "{expr}");
    }
}

#[test]
fn assigning_any_view_field_raises_the_read_only_error() {
    let vm = TestVm::new().messages().build();
    shapes(&vm);
    let before = vm.messages().to_records();
    for field in ["role", "content", "tool_calls", "tool_call_id", "extra"] {
        assert_eq!(
            refusal(&vm, &format!("msgs[3].{field} = 'x'")),
            "records read from a messages.new() list are read-only; change the list with \
             replace(first, last, records...)",
            "{field}"
        );
    }
    assert_eq!(vm.messages().to_records(), before);
}

#[test]
fn pairs_over_a_view_visits_its_present_fields_in_order() {
    let vm = TestVm::new().messages().build();
    shapes(&vm);
    let visits: Vec<String> = vm.eval(
        "local visits = {} \
         for i = 1, #msgs do \
           local fields = {} \
           for key, value in pairs(msgs[i]) do \
             fields[#fields + 1] = key .. ':' .. type(value) \
           end \
           visits[i] = table.concat(fields, ' ') \
         end \
         return visits",
    );
    assert_eq!(
        visits,
        [
            "role:string content:string",
            "role:string content:table",
            "role:string content:string tool_calls:table",
            "role:string content:string tool_call_id:string",
        ]
    );
}

#[test]
fn a_view_appended_or_converted_gives_the_same_record_as_its_table_form() {
    let vm = TestVm::new().messages().build();
    shapes(&vm);
    let other = MessageList::default();
    vm.lua()
        .globals()
        .set(
            "other",
            vm.lua()
                .create_userdata(other.clone())
                .expect("userdata creation cannot fail"),
        )
        .expect("a global set cannot fail");
    vm.exec("for i = 1, #msgs do other:append(msgs[i]) end");
    assert_eq!(other.to_records(), vm.messages().to_records());
    for index in 1..=4 {
        let view = read(&vm, &format!("msgs[{index}]")).expect("the view is present");
        let table = read(&vm, &format!("shapes[{index}]")).expect("the table is present");
        assert_eq!(
            parse_record(index, &view),
            parse_record(index, &table),
            "msgs[{index}]"
        );
    }
}

#[test]
fn writing_into_a_views_nested_tables_leaves_the_record_unchanged() {
    let vm = TestVm::new().messages().build();
    shapes(&vm);
    let before = vm.messages().to_records();
    let (content, tool_calls): (Value, Value) = vm.eval(
        "local parts_view, calls_view = msgs[2], msgs[3] \
         local parts = parts_view.content \
         parts[1].text = 'changed' \
         parts[3] = { type = 'text', text = 'more' } \
         local calls = calls_view.tool_calls \
         calls[1].id = 'call_9' \
         calls[1].arguments.value = 'changed' \
         calls[2] = calls[1] \
         return parts_view.content, calls_view.tool_calls",
    );
    assert_eq!(vm.messages().to_records(), before);
    let converted = |value: Value| -> Json {
        vm.lua()
            .from_value(value)
            .expect("the value converts to JSON")
    };
    assert_eq!(
        Some(converted(content)),
        read(&vm, "shapes[2].content"),
        "the same view reads its original content again"
    );
    assert_eq!(
        Some(converted(tool_calls)),
        read(&vm, "shapes[3].tool_calls"),
        "the same view reads its original tool calls again"
    );
}

#[test]
fn pairs_ipairs_and_a_counted_loop_visit_every_record_in_order_as_views() {
    let vm = TestVm::new().messages().build();
    let visit = "local seen = { pairs = {}, ipairs = {}, counted = {} } \
                 local function note(into, i, view) \
                   into[#into + 1] = i .. ':' .. type(view) .. ':' .. view.content \
                 end \
                 for i, view in pairs(msgs) do note(seen.pairs, i, view) end \
                 for i, view in ipairs(msgs) do note(seen.ipairs, i, view) end \
                 for i = 1, #msgs do note(seen.counted, i, msgs[i]) end \
                 return table.concat(seen.pairs, ' '), table.concat(seen.ipairs, ' '), \
                   table.concat(seen.counted, ' ')";
    let empty: (String, String, String) = vm.eval(visit);
    assert_eq!(empty, (String::new(), String::new(), String::new()));
    vm.exec("msgs:user('u1'):assistant('a1'):user('u2')");
    let full: (String, String, String) = vm.eval(visit);
    let every = "1:userdata:u1 2:userdata:a1 3:userdata:u2".to_owned();
    assert_eq!(full, (every.clone(), every.clone(), every));
}

#[test]
fn a_pairs_loop_that_appends_ends_after_the_records_it_started_with() {
    let vm = TestVm::new().messages().build();
    vm.exec("msgs:user('u1'):assistant('a1'):user('u2')");
    let ended: (i64, i64) = vm.eval(
        "local n = 0 \
         for _ in pairs(msgs) do \
           n = n + 1 \
           if n > 10 then error('runaway pairs') end \
           msgs:user('x') \
         end \
         return n, #msgs",
    );
    assert_eq!(ended, (3, 6));
}

#[test]
fn a_pairs_loop_stops_early_when_the_list_shrinks_below_its_next_index() {
    let vm = TestVm::new().messages().build();
    vm.exec("msgs:user('u1'):assistant('a1'):user('u2')");
    let seen: String = vm.eval(
        "local seen = {} \
         for i, view in pairs(msgs) do \
           if i == 1 then msgs:replace(2, 3) end \
           seen[#seen + 1] = i .. ':' .. view.content \
         end \
         return table.concat(seen, ' ')",
    );
    assert_eq!(seen, "1:u1");
}

#[test]
fn pairs_keeps_its_starting_length_while_ipairs_reads_the_live_list() {
    let walk = |iterate: &str| -> String {
        let vm = TestVm::new().messages().build();
        vm.exec("msgs:user('u1'):assistant('a1'):user('u2')");
        vm.eval(&format!(
            "local seen = {{}} \
             for i, view in {iterate}(msgs) do \
               if i == 1 then msgs:append({{ role = 'user', content = 'u3' }}) end \
               if i == 2 then msgs:replace(3, 3, {{ role = 'user', content = 'u2b' }}) end \
               seen[#seen + 1] = i .. ':' .. view.content \
             end \
             return table.concat(seen, ' ')"
        ))
    };
    assert_eq!(walk("pairs"), "1:u1 2:a1 3:u2b", "pairs");
    assert_eq!(walk("ipairs"), "1:u1 2:a1 3:u2b 4:u3", "ipairs");
}
