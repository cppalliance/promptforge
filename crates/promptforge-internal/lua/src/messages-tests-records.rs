//! Tests for the per-record rules a list enforces as `append` adds a
//! record, each refusal naming the position the record would take.

use super::list::{refusal, run, vm};

/// Appends every record of the Lua array `records` but the last to a
/// fresh list, then returns the refusal of the last one.
fn refused_append(records: &str) -> String {
    let (lua, _list) = vm();
    run(
        &lua,
        &format!(
            "records = {records}\n\
             for i = 1, #records - 1 do msgs:append(records[i]) end"
        ),
    );
    refusal(&lua, "msgs:append(records[#records])")
}

fn assert_refusals(cases: &[(&str, &str)]) {
    for (records, expected) in cases {
        assert_eq!(refused_append(records), *expected, "{records}");
    }
}

#[test]
fn malformed_tool_calls_are_refused_naming_the_index() {
    assert_refusals(&[
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
        (
            r#"{ { role = "assistant", content = "", tool_calls = "raw" } }"#,
            "messages[1] tool_calls must be an array",
        ),
    ]);
}

#[test]
fn content_parts_validate_each_variants_payload() {
    assert_refusals(&[
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
    ]);
}

#[test]
fn a_non_string_tool_call_id_is_refused() {
    assert_refusals(&[(
        r#"{ { role = "user", content = "ok", tool_call_id = 7 } }"#,
        "messages[1] tool_call_id must be a string",
    )]);
}

#[test]
fn record_validation_names_the_offending_index() {
    assert_refusals(&[
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
    ]);
}
