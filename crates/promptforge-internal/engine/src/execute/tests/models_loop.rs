//! Tests for the section-visible `models.loop`: the shim-driven model-tool
//! loop over an author-owned message list. One terminal turn with no tools,
//! repeated model-tool rounds with automatic assistant and tool-result
//! appends, the nil return, explicit terminal removal, local and bound
//! tools, call-time tool scope, explicit-handle calls on a frozen binding,
//! and the atomic append of a tool-call batch. The shared loop fixtures
//! (`loop_models`, `loop_scenario`, `catalog_scenario`, `LOOP_TO_TEXT`,
//! and the `loop_events` filter) sit here for every loop-driven sibling.
//! The compactor argument's tests sit in `models_loop_compactors`; the
//! loop's exit rules in `exit_rules`; its cap, scope gate, and
//! result-record trust in `tool_loop`.

use promptforge_parser::test_support::one_section;

use super::*;
use crate::test_support::scenario::test_binding;

#[path = "models_loop-arguments.rs"]
mod arguments;

#[path = "models_loop-author-shapes.rs"]
mod author_shapes;

/// The model set a loop test's run uses: `writer` (the prompt-wide
/// default, model `test-model`) and `other` (model `other-model`), so an
/// explicit handle provably runs on its own frozen binding.
pub(super) fn loop_models() -> ModelSet {
    ModelSet {
        bindings: vec![
            test_binding("writer", "A general model for tests", "test-model"),
            test_binding("other", "A second model", "other-model"),
        ],
        default: Some("writer".to_owned()),
    }
}

/// A run of `source` over the loop models, unprepared, so the run starts
/// with what the live H1 pass would leave and no catalog.
pub(super) fn loop_scenario(source: impl Into<String>) -> Scenario {
    Scenario::new(source).unprepared().models(loop_models())
}

/// A run of `source` over the loop models, prepared, so the scenario's
/// catalog tools are offered as the prepare offers them.
pub(super) fn catalog_scenario(source: impl Into<String>) -> Scenario {
    Scenario::new(source).models(loop_models())
}

/// The section body the cap, trust, scoping, exit-rule, and observation
/// tests run: one loop over a single user message, then the terminal
/// record's text.
pub(super) const LOOP_TO_TEXT: &str = "local msgs = messages.new()\n\
     msgs:user('ask the model')\n\
     models.loop(msgs)\n\
     return msgs[#msgs].content";

/// The model-turn and tool-call observations `recorder` saw, in order,
/// with every other boundary event (chunk, section, run) dropped, so a
/// prompt-level test reads exactly the sequence the loop's rounds report.
pub(super) fn loop_events(recorder: &Recorder) -> Vec<String> {
    let loop_details = [
        detail::MODEL_TURN_COMPLETED,
        detail::MODEL_TURN_FAILED,
        detail::MODEL_TURN_TRUNCATED,
        detail::TOOL_CALL_SUCCEEDED,
        detail::TOOL_CALL_FAILED,
    ]
    .map(|observation| observation.to_string());
    recorder
        .events()
        .into_iter()
        .map(|(_, detail)| detail)
        .filter(|detail| loop_details.contains(detail))
        .collect()
}

#[test]
fn models_loop_appends_the_terminal_assistant_record_and_returns_nil() {
    let gateway = ScriptedChat::new(vec![text("final answer")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('hello')
         local result = models.loop(msgs)
         assert(result == nil, 'models.loop returns nil')
         assert(#msgs == 2, 'the loop appended exactly the terminal record')
         assert(msgs[2].role == 'assistant', 'the terminal record is an assistant message')
         assert(msgs[2].content == 'final answer', 'the terminal record holds the reply text')
         msgs:replace(#msgs, #msgs)
         assert(#msgs == 1, 'explicit terminal removal shrinks the list')
         return 'ok'",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "ok");
    assert_eq!(gateway.call_count(), 1, "one terminal turn is one request");
    let bodies = gateway.requests();
    assert_eq!(bodies[0].messages[0].role(), "user");
    assert_eq!(bodies[0].messages[0].content(), "hello");
    assert!(
        bodies[0].tools.is_empty(),
        "no tools in scope means no tools on the wire: {bodies:?}"
    );
}

#[test]
fn models_loop_repeats_model_tool_rounds_and_appends_each_exchange() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"one\"}")]),
        calls(&[("call_2", "echo", "{\"value\":\"two\"}")]),
        text("both done"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('echo twice')
         models.loop(msgs)
         assert(#msgs == 6, 'user plus two exchanges plus the terminal record')
         assert(msgs[2].role == 'assistant', 'round one appends the assistant call')
         assert(msgs[2].tool_calls[1].id == 'call_1', 'the assistant record holds the normalized call')
         assert(msgs[2].tool_calls[1].name == 'echo', 'the call keeps its wire name')
         assert(msgs[2].tool_calls[1].arguments.value == 'one', 'the call arguments stay parsed')
         assert(msgs[3].role == 'tool', 'the correlated result follows its call')
         assert(msgs[3].tool_call_id == 'call_1', 'the result answers call_1')
         assert(msgs[3].content == 'echoed: one', 'the trusted result appends verbatim')
         assert(msgs[4].tool_calls[1].id == 'call_2', 'round two appends its own call')
         assert(msgs[5].tool_call_id == 'call_2', 'round two appends its own result')
         assert(msgs[6].role == 'assistant' and msgs[6].content == 'both done', 'terminal text is the final record')
         return 'ok'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 3, "two tool rounds plus the terminal round");
    let tool_turns: Vec<&str> = bodies[2]
        .messages
        .iter()
        .filter(|message| message.role() == "tool")
        .map(crate::model::Message::content)
        .collect();
    assert_eq!(
        tool_turns,
        ["echoed: one", "echoed: two"],
        "the terminal round's conversation includes both exchanges: {bodies:?}"
    );
}

#[test]
fn replayed_tool_calls_reach_the_chat_effect_in_the_openai_shape() {
    // The bug report's failing sequence: one round requests a tool, the
    // follow-up round replays the assistant call plus its result. The
    // assertions below pin the exact shape on the replayed round; the
    // request body carries the messages verbatim, which model-client's
    // `build_request_body` tests check against the OpenAI schema.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"one\"}")]),
        text("done"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('echo once')
         models.loop(msgs)
         return msgs[#msgs].content",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "done");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 2, "one tool round plus the terminal round");
    let messages = bodies[1].messages_json();
    let assistant = messages
        .as_array()
        .expect("a round's messages serialize to an array")
        .iter()
        .find(|message| message["role"] == "assistant" && message.get("tool_calls").is_some())
        .expect("the replayed request carries the assistant tool-call turn");
    let call = &assistant["tool_calls"][0];
    assert_eq!(call["id"], "call_1", "the call id replays unchanged");
    assert_eq!(call["type"], "function", "the OpenAI discriminator: {call}");
    assert_eq!(
        call["function"]["name"], "echo",
        "the name sits under function"
    );
    let arguments = call["function"]["arguments"]
        .as_str()
        .expect("arguments is a JSON-encoded string on the wire");
    assert_eq!(
        serde_json::from_str::<Value>(arguments).expect("the arguments re-decode"),
        json!({ "value": "one" }),
        "the arguments string decodes back to the original object"
    );
}

#[test]
fn models_loop_dispatches_local_and_bound_tools() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "grab", "{\"value\":\"x\"}")]),
        calls(&[("call_2", "echo", "{\"value\":\"y\"}")]),
        text("tools done"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)
           return 'grabbed ' .. args.value
         end)
         local msgs = messages.new()
         msgs:user('use both tools')
         models.loop(msgs)
         assert(#msgs == 6, 'both exchanges and the terminal record appended')
         assert(msgs[3].content == 'grabbed x', 'the local handler ran on the section VM')
         assert(msgs[3].tool_call_id == 'call_1', 'the local result correlates its call')
         assert(msgs[5].content == 'echoed: y', 'the bound tool ran through dispatch')
         return 'ok'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    let bodies = gateway.requests();
    assert_eq!(
        bodies[0].tools.len(),
        2,
        "both the local and the bound tool are advertised: {bodies:?}"
    );
}

#[test]
fn models_loop_reads_the_tool_scope_at_each_call() {
    let gateway = ScriptedChat::new(vec![
        text("no tools yet"),
        calls(&[("call_1", "echo", "{\"value\":\"late\"}")]),
        text("scoped in"),
    ]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('first')
         models.loop(msgs)
         tools.offer('tools/echo')
         msgs:user('second')
         models.loop(msgs)
         assert(msgs[#msgs].content == 'scoped in', 'the second loop converged')
         return 'ok'",
    );
    // Nothing in scope at first: the first call advertises no tools, the
    // `tools.offer` between calls scopes `echo` in for the second.
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    let bodies = gateway.requests();
    assert_eq!(
        bodies.len(),
        3,
        "one tool-free round, then a tool round and its terminal"
    );
    assert!(
        bodies[0].tools.is_empty(),
        "the first call predates the tools.offer: {bodies:?}"
    );
    assert_eq!(
        bodies[1].tools[0].name(),
        "echo",
        "the second call advertises the newly offered tool: {bodies:?}"
    );
}

#[test]
fn a_handles_loop_runs_on_its_frozen_binding() {
    let gateway = ScriptedChat::new(vec![text("first"), text("second")]);
    let source = one_section(
        "local other = models.get('other')
         local msgs = messages.new()
         msgs:user('one')
         other:loop(msgs)
         msgs:user('two')
         other:loop(msgs)
         assert(#msgs == 4, 'each call appends its terminal record')
         return msgs[2].content .. '|' .. msgs[4].content",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "first|second");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 2, "two loops, two requests");
    for body in &bodies {
        assert_eq!(
            body.options.model(),
            "other-model",
            "the handle's frozen binding serves, not the section default: {bodies:?}"
        );
    }
}

#[test]
fn the_author_list_never_shows_a_half_answered_tool_batch() {
    // Two calls in one round: each handler runs while its sibling is
    // unanswered, and the list it reads must not yet hold the batch's
    // assistant record. After the round the assistant record and both
    // results land together, in call order, ahead of the terminal text.
    let gateway = ScriptedChat::new(vec![
        calls(&[
            ("c1", "grab", "{\"value\":\"a\"}"),
            ("c2", "grab", "{\"value\":\"b\"}"),
        ]),
        text("done"),
    ]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('grab twice')
         local seen = {}
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)
           seen[#seen + 1] = #msgs
           for _, m in ipairs(msgs) do
             assert(m.tool_calls == nil, 'no assistant call record is visible mid-batch')
             assert(m.role ~= 'tool', 'no tool record is visible mid-batch')
           end
           return 'grabbed ' .. args.value
         end)
         models.loop(msgs)
         assert(#seen == 2, 'both calls in the batch ran')
         assert(seen[1] == 1 and seen[2] == 1, 'each handler saw only the user message')
         assert(#msgs == 5, 'user, the batch record, two results, and the terminal text')
         assert(msgs[2].role == 'assistant' and #msgs[2].tool_calls == 2, 'one record holds the whole batch')
         assert(msgs[2].tool_calls[1].id == 'c1' and msgs[2].tool_calls[2].id == 'c2', 'calls keep their order')
         assert(msgs[3].tool_call_id == 'c1' and msgs[3].content == 'grabbed a', 'the first result follows')
         assert(msgs[4].tool_call_id == 'c2' and msgs[4].content == 'grabbed b', 'the second result follows')
         assert(msgs[5].role == 'assistant' and msgs[5].content == 'done', 'the terminal text is last')
         return 'ok'",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "ok");
    let tool_turns = gateway.requests()[1]
        .messages
        .iter()
        .filter(|message| message.role() == "tool")
        .count();
    assert_eq!(tool_turns, 2, "both results appear in the terminal round");
}
