//! Tests for debug capture delivery and the `tools.calls` counters. The
//! infer-round reporting cases sit in `infer_rounds`.

use runvil_parser::test_support::{PromptSource, one_section};
use runvil_types::emitter::DebugMode;

use super::context::{arm_fanout, setup_prompt, with_writer};
use super::fixtures::{echo_tool, failing_tool, scoped_tool};
use super::models_loop::loop_scenario;
use super::*;
use crate::test_support::recording::DebugEvent;
use crate::test_support::scenario::EXECUTION;

/// The prompt whose one section, `Only`, asks the `writer` role about its
/// prose.
fn ask_the_model() -> String {
    with_writer(PromptSource::new().prose(
        "Only",
        "Ask the model.

         ```lua
         return models.infer(prose)
         ```",
    ))
    .build()
}

/// A bound run of `source` whose context turns debug reporting on.
fn debugged(source: String) -> Scenario {
    Scenario::new(source)
        .bound()
        .context(|ctx| ctx.report_debug(DebugMode::On))
}

#[test]
fn debug_capture_receives_request_and_response_when_set() {
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let trace = debugged(ask_the_model()).chat(&gateway).run();

    assert_eq!(trace.text(), "hello from the mock");
    let captures = trace.captures();
    assert_eq!(
        captures.len(),
        2,
        "one request and one response: {captures:#?}"
    );
    assert_eq!(captures[0].execution, EXECUTION);
    assert_eq!(captures[0].section, "Only");
    assert_eq!(captures[0].turn, 1);
    let round = gateway.last_request().expect("infer must reach the model");
    assert_eq!(round.options.model(), "claude-sonnet-4-6");
    assert!(!round.messages.is_empty());
    // A scripted completion carries no raw exchange, so both bodies are
    // the `null` a broker without one reports.
    match &captures[0].event {
        DebugEvent::Request { body } => {
            assert!(body.is_null(), "{body}");
        }
        other @ DebugEvent::Response { .. } => panic!("expected request first, got {other:?}"),
    }
    match &captures[1].event {
        DebugEvent::Response {
            body,
            finish_reason,
            reasoning_content,
        } => {
            assert_eq!(finish_reason, &None);
            assert_eq!(reasoning_content, &None);
            assert!(body.is_null(), "{body}");
        }
        other @ DebugEvent::Request { .. } => panic!("expected response second, got {other:?}"),
    }
}

#[test]
fn nested_model_infer_capture_reaches_the_debug_sink() {
    // A nested infer called from Lua must route its request/response
    // capture to the run's owned debug sink instead of dropping it.
    let gateway = ScriptedChat::new(vec![text("final answer")]);
    let source = setup_prompt(PromptSource::new().section(
        "Only",
        "local text = models.get('writer'):infer('say hello')
         return text",
    ));
    let trace = debugged(source).chat(&gateway).run();
    assert_eq!(trace.text(), "final answer");

    let captures = trace.captures();
    assert!(
        !captures.is_empty(),
        "nested handle-form infer must reach the debug sink (F4), got no events"
    );
    assert!(
        captures
            .iter()
            .any(|capture| matches!(capture.event, DebugEvent::Request { .. })),
        "nested inference must capture at least one request: {captures:#?}"
    );
    assert!(
        captures
            .iter()
            .any(|capture| matches!(capture.event, DebugEvent::Response { .. })),
        "nested inference must capture at least one response: {captures:#?}"
    );
}

#[test]
fn fanout_arm_debug_events_reach_the_run_sink() {
    // The arm's debug side channel: the fanout's run-context fork keeps
    // the run's own debug sink, so an arm's model-turn events land on the
    // run's sink under the worker's section name.
    let gateway = ScriptedChat::new(vec![text("arm reply")]);
    let source = setup_prompt(arm_fanout().prose(
        "### Worker",
        "Reply about {{ item }}.

         ```lua
         return models.infer(prose)
         ```",
    ));
    let trace = debugged(source).chat(&gateway).run();
    assert_eq!(trace.text(), "arm reply");

    let captures = trace.captures();
    let worker_captures: Vec<_> = captures
        .iter()
        .filter(|capture| capture.section == "Worker")
        .collect();
    assert!(
        worker_captures
            .iter()
            .any(|capture| matches!(capture.event, DebugEvent::Request { .. })),
        "the arm's request must forward to the run's sink: {captures:#?}"
    );
    assert!(
        worker_captures
            .iter()
            .any(|capture| matches!(capture.event, DebugEvent::Response { .. })),
        "the arm's response must forward to the run's sink: {captures:#?}"
    );
}

#[test]
fn debug_capture_none_changes_nothing() {
    let gateway = ScriptedChat::new(vec![text("hello from the mock")]);
    let trace = Scenario::new(ask_the_model()).bound().chat(&gateway).run();
    assert_eq!(trace.text(), "hello from the mock");
}

// --- Per-VM tools.calls count tests ---

#[test]
fn tool_calls_count_increments_on_successful_dispatch() {
    let (tool, called) = scoped_tool("echo", "Echo a test value.");
    let source = setup_prompt(PromptSource::new().front("plugins: [tools]").section(
        "Only",
        "tools.call('tools/echo', { value = 'x' })
         assert(tools.calls['tools/echo'] == 1, 'expected 1 call, got ' .. tostring(tools.calls['tools/echo']))
         return 'ok'",
    ));
    let trace = Scenario::new(source).bound().tool(tool).run();
    assert_eq!(trace.text(), "ok");
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[test]
fn tool_calls_count_increments_even_when_tool_errors() {
    // Drive the failing fixture tool through a `models.loop` round and prove
    // `tools.calls` records exactly one call even though the tool errors
    // (the count is incremented before dispatch). The tool's own failure is
    // the call's error result, so the loop continues to the terminal reply.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_x", "echo", "{\"value\":\"x\"}")]),
        text("final answer"),
    ]);
    let source = one_section(
        "tools.offer('tools/failing')
         local msgs = messages.new()
         msgs:user('ask the model')
         models.loop(msgs)
         return msgs[#msgs].content .. '|' .. tostring(tools.calls['tools/failing'])",
    );
    let trace = loop_scenario(source)
        .offer("echo", failing_tool())
        .chat(&gateway)
        .run();
    assert_eq!(
        trace.text(),
        "final answer|1",
        "the counter must record exactly one call even though the tool errored"
    );
}

#[test]
fn a_model_call_and_a_script_call_count_under_the_id_and_a_local_tool_under_its_alias() {
    let gateway = ScriptedChat::new(vec![
        calls(&[
            ("call_1", "echo", r#"{"value":"model"}"#),
            ("call_2", "grab", r#"{"value":"local"}"#),
        ]),
        text("final answer"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         tools.offer_local('grab', 'Grab a value', { value = 'string' }, function(args) return 'got ' .. args.value end)
         local msgs = messages.new()
         msgs:user('ask the model')
         models.loop(msgs)
         tools.call('tools/echo', { value = 'script' })
         local ok = pcall(function() return tools.calls.echo end)
         return tostring(tools.calls['tools/echo']) .. '|' .. tostring(tools.calls.grab) .. '|' .. tostring(ok)",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(
        trace.text(),
        "2|1|false",
        "the model's wire-name call and the script's id call count once each under \
         the id, the local tool under its alias, and the wire name is no key"
    );
}

#[test]
fn tool_calls_count_zero_for_uncalled_alias_fails_epilog_assert() {
    // The first script dispatch installs the counts seeded from the
    // effective scope, so an offered but uncalled tool reads as 0 and an
    // author assert on it fails the run with its own message.
    let source = setup_prompt(PromptSource::new().front("plugins: [tools]").prose(
        "Only",
        "```lua
         tools.offer('tools/search')
         local _ = tools.call('tools/other', { value = 'x' })
         ```

         ```lua
         assert(tools.calls['tools/search'] > 0, 'search was never called')
         return 'unreached'
         ```",
    ));
    let (search, _) = scoped_tool("search", "Search for things.");
    let (other, _) = scoped_tool("other", "Other things.");
    let trace = Scenario::new(source).bound().tool(search).tool(other).run();
    let error = trace.error();
    assert!(
        error.to_string().contains("search was never called"),
        "error must include the assert message: {error}"
    );
}

#[test]
fn tool_calls_typo_alias_is_a_hard_error_with_seeded_set() {
    let source = setup_prompt(PromptSource::new().front("plugins: [tools]").prose(
        "Only",
        "```lua
         tools.offer('tools/search')
         local _ = tools.call('tools/search', { value = 'x' })
         ```

         ```lua
         local _ = tools.calls['tools/serach']
         return 'unreached'
         ```",
    ));
    let (tool, _) = scoped_tool("search", "Search for things.");
    let trace = Scenario::new(source).bound().tool(tool).run();
    let msg = trace.error().to_string();
    assert!(
        msg.contains("tools/serach") && msg.contains("has no seeded count"),
        "error must name the bad key and state it was never seeded: {msg}"
    );
    assert!(
        msg.contains("seeded names: [\"tools/search\"]")
            && msg.contains(" - check for typos or offer it with tools.offer"),
        "error must list the seeded ids and the remedy: {msg}"
    );
}

#[test]
fn handle_infer_returns_text_without_touching_reply_or_sys() {
    // The one infer shape: `handle:infer(...)` returns the round's text and never
    // sets `reply` or `sys.reply_finish_reason`.
    let gateway = ScriptedChat::new(vec![text("pong")]);
    let source = setup_prompt(PromptSource::new().section(
        "Only",
        "local text = models.get('writer'):infer('say hello')
         assert(type(text) == 'string', 'infer must return text')
         assert(text == 'pong')
         assert(reply == nil, 'infer must not set reply')
         assert(not pcall(function() return sys.reply_finish_reason end),
             'infer must not touch sys')
         return text",
    ));
    let trace = Scenario::new(source).bound().chat(&gateway).run();
    assert_eq!(trace.text(), "pong");
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert!(
        body.tools.is_empty(),
        "handle-form infer advertises no tools: {body:?}"
    );
}

#[path = "debug_and_counts-infer-rounds.rs"]
mod infer_rounds;
