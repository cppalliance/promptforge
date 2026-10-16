//! Tests for the scheduler's `Chat` dispatch arm, driven through
//! `models.loop`, the only source of `chat` yields: the events the
//! scheduler emits when it applies a round's answer, the tool-call batch
//! it resumes for the shim to dispatch, the out-of-scope refusal, and the
//! empty and overflow rounds it resumes rather than raises, which the
//! shim's exit rules and compactor then act on. The round's tool scope is
//! covered in `chat_scope`.

use promptforge_parser::test_support::one_section;
use promptforge_types::event::{Event, ReplyOrigin};
use promptforge_types::metrics::{CallMetrics, Usage};

use super::fixtures::echo_tool;
use super::gateway::overflowing;
use super::models_loop::loop_scenario;
use super::*;

/// A text reply with everything a round can report: a model name, a
/// reasoning side channel, a finish reason, and usage metrics.
fn rich_text_reply(content: &str) -> ScriptedReply {
    ScriptedReply::Text {
        model: "served-model".to_owned(),
        content: content.to_owned(),
        finish_reason: Some("stop".to_owned()),
        reasoning: Some("let me think".to_owned()),
        metrics: Some(Box::new(
            CallMetrics::new().with_usage(Some(Usage::new(3, 2, 5))),
        )),
    }
}

/// The index of the first of `events` that `matches`, panicking with
/// `missing` and every event when there is none.
#[track_caller]
fn position(events: &[Event], missing: &str, matches: impl Fn(&Event) -> bool) -> usize {
    events
        .iter()
        .position(matches)
        .unwrap_or_else(|| panic!("{missing}: {events:?}"))
}

#[test]
fn a_text_round_reports_its_reply_and_the_loop_appends_it() {
    let gateway = ScriptedChat::new(vec![rich_text_reply("final answer")]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('hello')
         models.loop(msgs)
         assert(#msgs == 2, 'the loop appends the reply')
         assert(msgs[2].role == 'assistant', 'the appended record is the assistant reply')
         assert(msgs[2].content == 'final answer', 'the reply text resumes')
         return 'ok'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    let events = trace.events();
    let thinking = position(events, "the reasoning side channel is reported", |event| {
        matches!(event, Event::Thinking { .. })
    });
    assert!(
        matches!(
            &events[thinking],
            Event::Thinking { model, text, .. } if model == "served-model" && text == "let me think"
        ),
        "{events:?}"
    );
    let reply = position(events, "the arm reports a chat-origin reply", |event| {
        matches!(
            event,
            Event::AssistantReply {
                origin: ReplyOrigin::Chat,
                ..
            }
        )
    });
    assert!(
        matches!(
            &events[reply],
            Event::AssistantReply { text, finish_reason, model, metrics, .. }
                if text == "final answer"
                    && finish_reason.as_deref() == Some("stop")
                    && model == "served-model"
                    && metrics.is_some()
        ),
        "the reply carries its finish reason, serving model, and metrics: {events:?}"
    );
    assert!(thinking < reply, "thinking precedes the reply: {events:?}");
    assert_eq!(
        gateway.requests()[0].tools[0].name(),
        "echo",
        "the round advertises the section's scope"
    );
}

#[test]
fn a_tool_round_reports_the_batch_before_the_shim_dispatches_it() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("done"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('call the tool')
         models.loop(msgs)
         local call = msgs[2].tool_calls[1]
         assert(call.id == 'call_1', 'the call keeps its id')
         assert(call.name == 'echo', 'the call keeps its wire name')
         assert(call.arguments.value == 'hi', 'the arguments stay parsed')
         assert(msgs[3].role == 'tool' and msgs[3].tool_call_id == 'call_1', 'the result answers the call')
         return 'ok'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    assert_eq!(gateway.call_count(), 2, "one tool round, one closing round");
    let events = trace.events();
    let batch = position(events, "the requested calls are reported", |event| {
        matches!(
            event,
            Event::AssistantToolCalls { calls, .. }
                if calls.iter().map(|call| call.name.as_str()).eq(["echo"])
        )
    });
    let result = position(events, "the shim dispatches the call", |event| {
        matches!(event, Event::ToolResult { .. })
    });
    assert!(
        matches!(
            &events[result],
            Event::ToolResult { tool_call_id, alias, .. } if tool_call_id == "call_1" && alias == "echo"
        ),
        "the result fires under the model's call id: {events:?}"
    );
    assert!(
        batch < result,
        "the arm reports the batch unexecuted, then the shim dispatches it: {events:?}"
    );
}

#[test]
fn a_batch_names_the_tool_behind_a_bound_alias_and_none_behind_a_local_one() {
    let gateway = ScriptedChat::new(vec![
        calls(&[
            ("call_1", "grab", "{\"value\":\"x\"}"),
            ("call_2", "echo", "{\"value\":\"y\"}"),
        ]),
        text("done"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)
           return 'grabbed ' .. args.value
         end)
         local msgs = messages.new()
         msgs:user('use both tools')
         models.loop(msgs)
         return 'ok'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    let events = trace.events();
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::AssistantToolCalls { calls, .. }
                if calls.iter().map(|call| call.name.as_str()).eq(["grab", "echo"])
                    && calls
                        .iter()
                        .map(|call| call.tool.as_ref().map(ToString::to_string))
                        .eq([None, Some("tools/echo".to_owned())])
        )),
        "the bound alias names its tool and the local one names none: {events:?}"
    );
}

#[test]
fn an_out_of_scope_tool_name_fails_the_round_with_out_of_scope_tool() {
    // Readable at the call site as the `out_of_scope_tool` kind with the
    // loop's exact message, and typed when it escapes the section.
    let gateway = ScriptedChat::new(vec![calls(&[("call_1", "rogue", "{}")])]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('go rogue')
         local ok, err = pcall(models.loop, msgs)
         assert(not ok, 'an out-of-scope call raises')
         return err.kind .. '|' .. err.name .. '|' .. tostring(err)",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(
        trace.text(),
        "out_of_scope_tool|rogue|tool \"rogue\" is not in this section's scope; \
         in-scope aliases: [\"echo\"]"
    );
    let events = trace.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ToolCallFailed { .. })),
        "the rejected call reports a failed tool call: {events:?}"
    );

    let gateway = ScriptedChat::new(vec![calls(&[("call_1", "rogue", "{}")])]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('go rogue')
         models.loop(msgs)
         return 'unreachable'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    match trace.error().inner() {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "rogue");
            assert!(!global_exists, "rogue is bound nowhere");
            assert_eq!(in_scope, &["echo".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
}

#[test]
fn an_empty_reply_is_a_completed_round_the_loop_raises_as_empty_model_reply() {
    // The arm resumes the empty round with the reply absent and its finish
    // reason; with no answered tool call before it, the loop's exit rules
    // raise `empty_model_reply` carrying that finish reason.
    let gateway = ScriptedChat::new(vec![text_finish("", "stop")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('say nothing')
         local ok, err = pcall(models.loop, msgs)
         assert(not ok, 'an empty first reply raises')
         assert(#msgs == 1, 'the empty round appends nothing')
         return err.kind .. '|' .. tostring(err.finish_reason)",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "empty_model_reply|stop");
    let events = trace.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ModelTurnCompleted { .. })),
        "the empty round is a completed turn: {events:?}"
    );
}

#[test]
fn a_context_overflow_resumes_with_its_reason_for_the_compactor() {
    // The arm answers an overflow as a flag, not a raise, so the loop hands
    // the reason to the compactor. The precheck refuses before any request
    // leaves.
    let compacting_loop = "local seen
         local ok = pcall(models.loop, msgs, function(reason)
           seen = reason
           error('compacted', 0)
         end)
         assert(not ok, 'the compactor raise ends the loop')
         return seen";
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(&format!(
        "local msgs = messages.new()
         msgs:user(string.rep('x', 100000))
         {compacting_loop}"
    ));
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "precheck");
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck fires before dispatch"
    );

    // A broker-reported context overflow is the same flag after one
    // request, and a failed turn.
    let rounds = Arc::new(AtomicUsize::new(0));
    let source = one_section(&format!(
        "local msgs = messages.new()
         msgs:user('a small prompt')
         {compacting_loop}"
    ));
    let trace = loop_scenario(source).chat_with(overflowing(&rounds)).run();
    assert_eq!(trace.text(), "provider");
    assert_eq!(
        rounds.load(Ordering::SeqCst),
        1,
        "the request left and was refused"
    );
    let events = trace.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ModelTurnFailed { .. })),
        "a refused round is a failed turn: {events:?}"
    );
}
