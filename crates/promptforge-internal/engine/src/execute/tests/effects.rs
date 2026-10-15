//! Effects as values: every leaf request kind a section yields - `infer`
//! and `chat`, `tool_call`, `store`, `timer` - issues
//! exactly one `Effect` out of the run's `step`, and each effect's record
//! round-trips through serde; a section's round carries the `Chat` origin
//! and a nested infer round the `Infer` origin. The run's answer rules (a
//! drop, an orphan, a wrong kind) are pinned beside `Run` itself.

use promptforge_types::event::ReplyOrigin;
use promptforge_types::ids::RoundId;

use super::models_loop::{loop_prompt, loop_scenario};
use super::scheduler::scheduler_scenario;
use super::*;
use crate::execute::protocol::VfsOp;
use crate::execute::run::{EffectRecord, Round, ToolCallOrigin, ToolCaller};
use crate::test_support::rig::Rig;
use crate::test_support::scenario::round_trip;

/// Asserts every recorded effect survives the round trip unchanged.
#[track_caller]
fn assert_round_trips(records: &[EffectRecord]) {
    for record in records {
        assert_eq!(&round_trip(record), record, "the record round-trips");
    }
}

/// The origin of a call made by `caller` in `section` of the test run.
fn origin(section: &str, caller: ToolCaller) -> ToolCallOrigin {
    ToolCallOrigin {
        execution: EXECUTION.to_owned(),
        section: section.to_owned(),
        caller,
    }
}

#[test]
fn models_infer_issues_exactly_one_chat_effect_over_one_user_message() {
    let gateway = ScriptedChat::new(vec![text("answer")]);
    let source = loop_prompt("return models.infer('ask')");
    let rig = Rig::run(loop_scenario(source).chat(&gateway));
    assert_eq!(rig.trace().text(), "answer");

    assert_eq!(
        rig.issued(),
        vec![EffectRecord::Chat {
            round: RoundId::new(0),
            alias: "writer".to_owned(),
            after: None,
            keep: 0,
            messages: vec![json!({ "role": "user", "content": "ask" })],
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            thinking: None,
        }],
        "an infer is one tool-free chat effect"
    );
    assert_round_trips(rig.issued());
}

#[test]
fn a_models_loop_round_issues_one_chat_effect_and_one_tool_call_effect_per_call() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", r#"{"value":"hi"}"#)]),
        text("done"),
    ]);
    let source = loop_prompt(
        "tools.offer('tools/echo')\n\
         local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    );
    let rig = Rig::run(
        loop_scenario(source)
            .offer("echo", echo_tool())
            .chat(&gateway),
    );
    assert_eq!(rig.trace().text(), "done");

    let records = rig.issued();
    assert_eq!(records.len(), 3, "two rounds and one call: {records:?}");
    assert!(
        matches!(&records[0], EffectRecord::Chat { tools, after: None, keep: 0, messages, .. }
            if tools == &["echo".to_owned()] && messages.len() == 1),
        "the first round advertises the scope over the author's list, which starts here: {:?}",
        records[0]
    );
    assert_eq!(
        records[1],
        EffectRecord::ToolCall {
            tool: ToolId::parse("tools/echo").expect("a valid id"),
            alias: "echo".to_owned(),
            args: json!({ "value": "hi" }),
            origin: origin("Only", ToolCaller::Model),
        },
        "the model's call is one tool_call effect naming the bound identity and the model"
    );
    let EffectRecord::Chat {
        after,
        keep,
        messages,
        ..
    } = &records[2]
    else {
        panic!("the third effect is the second round: {:?}", records[2]);
    };
    assert_eq!(
        (*after, *keep),
        (Some(RoundId::new(0)), 1),
        "the second round extends the first, repeating its one message"
    );
    let roles: Vec<&Value> = messages.iter().map(|message| &message["role"]).collect();
    assert_eq!(
        roles,
        [&json!("assistant"), &json!("tool")],
        "the second round logs only the assistant tool-call record and the tool record: \
         {messages:?}"
    );
    assert!(
        messages[0]["tool_calls"].is_array() && messages[1]["tool_call_id"] == json!("call_1"),
        "the logged pair is the call and its result: {messages:?}"
    );
    assert_round_trips(records);
}

#[test]
fn a_script_tools_call_issues_exactly_one_tool_call_effect() {
    let source = loop_prompt(
        "tools.offer('tools/echo')\n\
         return tools.call('tools/echo', { value = 'hi' })",
    );
    let rig = Rig::run(loop_scenario(source).offer("echo", echo_tool()));
    assert_eq!(rig.trace().text(), "echoed: hi");

    assert_eq!(
        rig.issued(),
        vec![EffectRecord::ToolCall {
            tool: ToolId::parse("tools/echo").expect("a valid id"),
            alias: "echo".to_owned(),
            args: json!({ "value": "hi" }),
            origin: origin("Only", ToolCaller::Script),
        }]
    );
    assert_round_trips(rig.issued());
}

#[test]
fn a_script_tool_call_records_the_section_that_made_it() {
    let source = "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n\
         ## First\n\n```lua\ntools.offer('tools/echo')\ntools.call('tools/echo', { value = 'a' })\n```\n\n\
         ## Second\n\n```lua\ntools.offer('tools/echo')\n\
         return tools.call('tools/echo', { value = 'b' })\n```\n";
    let rig = Rig::run(loop_scenario(source).offer("echo", echo_tool()));
    assert_eq!(rig.trace().text(), "echoed: b");

    let origins: Vec<ToolCallOrigin> = rig
        .issued()
        .iter()
        .filter_map(|record| match record {
            EffectRecord::ToolCall { origin, .. } => Some(origin.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        origins,
        vec![
            origin("First", ToolCaller::Script),
            origin("Second", ToolCaller::Script),
        ],
        "each call names the section it was made in"
    );
}

#[test]
fn a_store_operation_issues_exactly_one_store_effect() {
    let source = loop_prompt(
        "store.write('notes.md', 'kept')\n\
         return store.read('notes.md')",
    );
    let rig = Rig::run(loop_scenario(source));
    assert_eq!(rig.trace().text(), "kept");

    assert_eq!(
        rig.issued(),
        vec![
            EffectRecord::Vfs {
                op: VfsOp::Write {
                    path: "notes.md".to_owned(),
                    contents: "kept".to_owned(),
                },
            },
            EffectRecord::Vfs {
                op: VfsOp::Read {
                    path: "notes.md".to_owned(),
                    start: None,
                    end: None,
                },
            },
        ],
        "each store call is one store effect holding its validated op"
    );
    assert_round_trips(rig.issued());
}

#[test]
fn a_timed_wait_issues_exactly_one_timer_effect() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Timer\n\n\
        ## Main\n\n\
        ```lua\n\
        local t = tasks.spawn('## Child')\n\
        local first, ok, result = tasks.join_any({ t }, { timeout = 30 })\n\
        return result\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\nreturn 'quick'\n```\n";
    let rig = Rig::run(scheduler_scenario(md));
    assert_eq!(rig.trace().text(), "quick");

    assert_eq!(
        rig.issued(),
        vec![EffectRecord::Timer { seconds: 30.0 }],
        "the wait's timeout is one timer effect; the child issued none"
    );
    assert_round_trips(rig.issued());
}

#[test]
fn a_sections_round_is_the_runs_first_with_the_chat_origin() {
    let gateway = ScriptedChat::new(vec![text("answer")]);
    let source = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    );
    let rig = Rig::run(loop_scenario(source).chat(&gateway));
    assert_eq!(rig.trace().text(), "answer");
    assert_eq!(
        rig.rounds(),
        vec![Round {
            id: RoundId::new(0),
            origin: ReplyOrigin::Chat,
        }],
        "the section's round is the run's first and has the chat origin"
    );
}

#[test]
fn a_nested_infer_round_is_the_runs_first_with_the_infer_origin() {
    let gateway = ScriptedChat::new(vec![text("answer")]);
    let source = loop_prompt("return models.infer('ask')");
    let rig = Rig::run(loop_scenario(source).chat(&gateway));
    assert_eq!(rig.trace().text(), "answer");
    assert_eq!(
        rig.rounds(),
        vec![Round {
            id: RoundId::new(0),
            origin: ReplyOrigin::Infer,
        }],
        "the nested round is the run's first and has the infer origin"
    );
}
