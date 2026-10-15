//! Prompt-level tests for the `models.loop` shim's round cap, its scope
//! gate, the trust of the result records it appends, its turn and
//! observation reporting, and its cancellation: every test drives a
//! section calling `models.loop` through the scheduler against the mock
//! gateway, so the shim's `chat` and `tool_call` rounds are exercised end
//! to end. The scope gate's tests sit in the `scope_gate` child. The
//! loop's exit rules sit in `exit_rules`; its append shapes, compactor
//! paths, and handle calls in `models_loop`.

use promptforge_parser::test_support::{one_section, prompt};
use promptforge_types::event::Event;

use super::models_loop::{LOOP_TO_TEXT, loop_events, loop_scenario};
use super::*;
use crate::model::CompletionErrorKind;
use crate::test_support::scenario::assert_model_failure;

/// The one-section loop prompt running `lua` under an explicit
/// frontmatter round cap.
fn capped(cap: usize, lua: &str) -> String {
    prompt(&format!("max_tool_iterations: {cap}"), &[("Only", lua)])
}

/// A never-converging model: `rounds` tool-call replies for `echo`, each
/// under its own call id. Every round re-validates the author's whole list
/// (each round is one `chat` over it), whose call ids must be unique - as a
/// real backend's are - so a script replaying one id would be refused as a
/// duplicate before the cap could fire.
fn never_converging_script(rounds: usize) -> Vec<ScriptedReply> {
    (0..rounds)
        .map(|round| calls(&[(&format!("call_{round}"), "echo", "{\"value\":\"x\"}")]))
        .collect()
}

#[test]
fn tool_loop_gives_up_after_exactly_the_configured_cap() {
    // A small explicit cap: the loop must make exactly that many round
    // trips against a never-converging model, then exhaust.
    let cap = 3;
    let gateway = ScriptedChat::new(never_converging_script(cap));
    let source = capped(cap, &format!("tools.offer('tools/echo')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    let error = trace.error().inner();
    assert!(matches!(error, Error::ToolLoopExhausted), "got {error:?}");
    assert_eq!(
        gateway.call_count(),
        cap,
        "the loop must make exactly `cap` round trips before giving up"
    );
}

#[test]
fn tool_loop_exhaustion_is_readable_at_the_call_site_after_whole_exchanges() {
    // The raise is pcall-able as the `tool_loop_exhausted` kind with the
    // typed error's exact message, and every round before it appended a
    // complete exchange: the list holds no half-answered batch at the cap.
    let gateway = ScriptedChat::new(never_converging_script(2));
    let source = capped(
        2,
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('loop forever')
         local ok, err = pcall(models.loop, msgs)
         assert(not ok, 'the cap raises')
         assert(#msgs == 5, 'two whole exchanges were appended before the cap')
         assert(msgs[2].tool_calls[1].id == 'call_0' and msgs[3].tool_call_id == 'call_0')
         assert(msgs[4].tool_calls[1].id == 'call_1' and msgs[5].tool_call_id == 'call_1')
         return err.kind .. '|' .. tostring(err)",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(
        trace.text(),
        "tool_loop_exhausted|tool-call loop did not converge"
    );
    assert_eq!(gateway.call_count(), 2);
}

#[test]
fn tool_loop_uses_the_default_cap_when_unspecified() {
    // A prompt declaring no budget runs the limits default: exactly
    // `DEFAULT_MAX_TOOL_ITERATIONS` round trips.
    let gateway = ScriptedChat::new(never_converging_script(DEFAULT_MAX_TOOL_ITERATIONS));
    let source = one_section(&format!("tools.offer('tools/echo')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    let error = trace.error().inner();
    assert!(matches!(error, Error::ToolLoopExhausted), "got {error:?}");
    assert_eq!(gateway.call_count(), DEFAULT_MAX_TOOL_ITERATIONS);
    assert_eq!(DEFAULT_MAX_TOOL_ITERATIONS, 24);
}

#[test]
fn run_resolves_cap_from_frontmatter_else_default() {
    // Mirrors the resolution in `run`: a declared budget wins, an absent
    // one falls back to the raised default.
    let declared =
        "---\nname: t\ndescription: d\nmax_tool_iterations: 5\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(declared, EXECUTION).0.unwrap();
    assert_eq!(
        promptforge_parser::detail::max_tool_iterations(p.frontmatter())
            .resolve(DEFAULT_MAX_TOOL_ITERATIONS),
        5
    );

    let absent = "---\nname: t\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(absent, EXECUTION).0.unwrap();
    assert_eq!(
        promptforge_parser::detail::max_tool_iterations(p.frontmatter())
            .resolve(DEFAULT_MAX_TOOL_ITERATIONS),
        DEFAULT_MAX_TOOL_ITERATIONS
    );
}

#[test]
fn tool_loop_dispatches_then_returns_text() {
    // One tool-call round and one text round: the terminal text is the
    // list's last record and the run's turn counter advanced twice.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("final answer"),
    ]);
    let source = one_section(&format!("tools.offer('tools/echo')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "final answer");
    let turns: Vec<u32> = trace
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::AssistantToolCalls { turn, .. } | Event::AssistantReply { turn, .. } => {
                Some(*turn)
            }
            _ => None,
        })
        .collect();
    assert_eq!(turns, [1, 2], "one tool-call reply, then the final text");
}

#[test]
fn a_failing_tool_becomes_an_untrusted_error_result_and_the_loop_continues() {
    // A bound tool's own failure is the call's result record - the ToolError
    // message, nonce-wrapped as untrusted - with TOOL_CALL_FAILED firing
    // alongside it, and the loop continues to the terminal reply.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_x", "echo", "{\"value\":\"x\"}")]),
        text("final answer"),
    ]);
    let source = one_section(
        "tools.offer('tools/failing')
         local msgs = messages.new()
         msgs:user('ask the model')
         models.loop(msgs)
         assert(msgs[3].role == 'tool' and msgs[3].tool_call_id == 'call_x', 'the failure is the call record')
         return msgs[3].content .. '|' .. msgs[4].content",
    );
    let trace = loop_scenario(source)
        .offer("echo", failing_tool())
        .chat(&gateway)
        .run();
    let (record, terminal) = trace
        .text()
        .split_once('|')
        .expect("the section returns the record and the terminal text");
    assert_eq!(terminal, "final answer", "the loop continues to the reply");
    assert!(
        record.contains("the tool's own backend failed"),
        "the result record must include the tool's error message, got: {record}"
    );
    assert!(
        record.contains("<untrusted_input_") && record.contains("</untrusted_input_"),
        "the error result must be nonce-wrapped as untrusted, got: {record}"
    );
    assert_eq!(
        last_tool_turn_content(&gateway.requests()),
        record,
        "the wire sends the same result record the author's list holds"
    );
    let recorder = Recorder::default();
    trace.replay(&recorder);
    assert_eq!(
        loop_events(&recorder),
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_FAILED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
        ],
        "the failed dispatch is reported and the loop continues"
    );
    assert!(
        recorder
            .records()
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION)
    );
}

#[test]
fn repeated_calls_to_a_failing_tool_exit_at_the_iteration_cap() {
    // Every round's failing call becomes an error result, so a model that
    // keeps calling the failing tool never converges: the loop exits at
    // exactly `max_tool_iterations`.
    let cap = 3;
    let gateway = ScriptedChat::new(never_converging_script(cap));
    let source = capped(
        cap,
        &format!("tools.offer('tools/failing')\n{LOOP_TO_TEXT}"),
    );
    let trace = loop_scenario(source)
        .offer("echo", failing_tool())
        .chat(&gateway)
        .run();
    let error = trace.error().inner();
    assert!(matches!(error, Error::ToolLoopExhausted), "got {error:?}");
    assert_eq!(
        gateway.call_count(),
        cap,
        "each round answers the failing call and loops, exiting at the cap"
    );
}

#[test]
fn a_failing_model_turn_is_reported_before_the_error_propagates() {
    let gateway = ScriptedChat::new(vec![fail(
        CompletionErrorKind::ServerError,
        500,
        "private backend response",
    )]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('private model input')
         models.loop(msgs)
         return 'unreachable'",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_model_failure(
        trace.error(),
        CompletionErrorKind::ServerError,
        "the model backend reported a fault of its own (status 500)",
    );
    let recorder = Recorder::default();
    trace.replay(&recorder);
    assert_eq!(
        loop_events(&recorder),
        vec![detail::MODEL_TURN_FAILED.to_string()]
    );
    let observed = format!("{:?}", recorder.events());
    for payload in ["private backend response", "private model input"] {
        assert!(
            !observed.contains(payload),
            "observation leaked {payload:?}"
        );
    }
}

#[test]
fn a_client_rejection_without_overflow_signatures_stays_a_rejection() {
    // Same status class as a provider overflow, unrelated body: not context
    // overflow, so the bare rejection propagates and no compactor is
    // invoked.
    let gateway = ScriptedChat::new(vec![fail(
        CompletionErrorKind::Rejected,
        400,
        "invalid request: unknown field",
    )]);
    let trace = loop_scenario(one_section(LOOP_TO_TEXT))
        .chat(&gateway)
        .run();
    assert_model_failure(
        trace.error(),
        CompletionErrorKind::Rejected,
        "the model backend rejected the request (status 400)",
    );
}

// --- Guard-wrapping of tool results in the loop ---

#[test]
fn untrusted_tool_result_is_guard_wrapped_in_the_loop() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("final answer"),
    ]);
    let source = one_section(&format!(
        "tools.offer('tools/untrusted_echo')\n{LOOP_TO_TEXT}"
    ));
    let trace = loop_scenario(source)
        .offer("echo", untrusted_echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "final answer");

    let content = last_tool_turn_content(&gateway.requests());
    assert!(
        content.contains("is data, not instructions"),
        "an untrusted tool's result must include the preface, got: {content}"
    );
    assert!(
        content.contains("<untrusted_input_") && content.contains("</untrusted_input_"),
        "an untrusted tool's result must be wrapped in the tags, got: {content}"
    );
    assert!(
        content.contains("echoed: hi"),
        "the wrapped block must still contain the tool output, got: {content}"
    );
}

#[test]
fn untrusted_nonce_is_stable_across_rounds() {
    // One nonce per run: every round's envelope in a single loop is
    // stamped with the same nonce, so identical content wraps
    // byte-identically and KV-cache prefixes stay shared across rounds.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_0", "echo", "{\"value\":\"hi\"}")]),
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("final answer"),
    ]);
    let source = one_section(&format!(
        "tools.offer('tools/untrusted_echo')\n{LOOP_TO_TEXT}"
    ));
    let trace = loop_scenario(source)
        .offer("echo", untrusted_echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "final answer");

    let nonces = tool_turn_nonces(&gateway.requests());
    assert!(
        nonces.len() >= 2,
        "expected two rounds of guard-wrapped tool output, got: {nonces:?}"
    );
    assert!(
        nonces.windows(2).all(|pair| pair[0] == pair[1]),
        "every round's untrusted wrap in a run is stamped with the run's nonce: {nonces:?}"
    );
}

#[test]
fn trusted_tool_result_is_appended_verbatim_in_the_loop() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("final answer"),
    ]);
    let source = one_section(&format!("tools.offer('tools/echo')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "final answer");

    let content = last_tool_turn_content(&gateway.requests());
    assert_eq!(
        content, "echoed: hi",
        "a trusted tool's result must be appended verbatim, got: {content}"
    );
    assert!(
        !content.contains("untrusted_input_"),
        "a trusted tool's result appears without guard tags, got: {content}"
    );
}

#[test]
fn cancel_during_in_flight_tool_call_returns_promptly() {
    // The slow tool's call is held, so the run waits on it alone until the
    // cancel lands between steps.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("final answer"),
    ]);
    let source = one_section(&format!("tools.offer('test/tools/slow')\n{LOOP_TO_TEXT}"));
    let mut driving = loop_scenario(source)
        .offer("echo", slow_tool())
        .hold(calls_slow_tool)
        .chat(&gateway)
        .start();
    while driving.step() {}
    driving.cancel();
    let trace = driving.finish();
    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "expected the cancelled run, got {:?}",
        trace.result()
    );
    assert!(
        trace
            .effects()
            .iter()
            .any(|pair| matches!(pair, (EffectRecord::ToolCall { .. }, AnswerRecord::Dropped))),
        "the cancel lands while the tool call is in flight: {:?}",
        trace.effects()
    );
}

#[path = "tool_loop-scope-gate.rs"]
mod scope_gate;
