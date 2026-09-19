//! Record-to-table rendering: `append_message_record` onto an author's list.

use super::*;

#[test]
fn append_message_record_renders_every_record_shape() {
    let lua = Lua::new();
    let list = lua_table(&lua, r#"{ { role = "user", content = "hi" } }"#);
    let key = lua.create_registry_value(list.clone()).expect("stash");
    let calls = MessageRecord {
        role: MessageRole::Assistant,
        content: MessageContent::Text(String::new()),
        tool_calls: vec![ToolCallRecord {
            id: "call_1".to_owned(),
            name: "echo".to_owned(),
            arguments: json!({ "value": "hi" }),
        }],
        tool_call_id: None,
    };
    append_message_record(&lua, &key, &calls).expect("the assistant record appends");
    let result = MessageRecord {
        role: MessageRole::Tool,
        content: MessageContent::Text("echoed: hi".to_owned()),
        tool_calls: Vec::new(),
        tool_call_id: Some("call_1".to_owned()),
    };
    append_message_record(&lua, &key, &result).expect("the tool record appends");
    let parts = MessageRecord {
        role: MessageRole::User,
        content: MessageContent::Parts(vec![
            ContentPart::Text("look".to_owned()),
            ContentPart::ImageUrl("data:image/png;base64,AA".to_owned()),
        ]),
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    append_message_record(&lua, &key, &parts).expect("the parts record appends");
    let (length, call_id, call_name, call_arg, answer_id, answer, part_type, part_url): (
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = lua
        .load(
            "local m = ...; return #m, \
             m[2].tool_calls[1].id, m[2].tool_calls[1].name, m[2].tool_calls[1].arguments.value, \
             m[3].tool_call_id, m[3].content, \
             m[4].content[1].type, m[4].content[2].image_url.url",
        )
        .call(list)
        .expect("the appended records read back through Lua");
    assert_eq!(length, 4);
    assert_eq!(call_id, "call_1");
    assert_eq!(call_name, "echo");
    assert_eq!(call_arg, "hi");
    assert_eq!(answer_id, "call_1");
    assert_eq!(answer, "echoed: hi");
    assert_eq!(part_type, "text");
    assert_eq!(part_url, "data:image/png;base64,AA");
}
