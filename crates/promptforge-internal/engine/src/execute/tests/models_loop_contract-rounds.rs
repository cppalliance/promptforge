//! Contract tests for `models.loop`'s rounds: the clean exit's call
//! count, the batch rule under a raising handler, a dropped model-issued
//! call, a local tool's return and its cancellation, and the full trace a
//! run reports for the loop.

use promptforge_parser::test_support::{PromptSource, one_section};
use promptforge_types::event::ReplyOrigin;
use promptforge_types::ids::TaskId;
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};

use super::super::models_loop::loop_events;
use super::super::tool_call_arm::{ToolRecorder, tool_result_lines};
use super::*;
use crate::execute::run::{Effect, EffectAnswer, Step};
use crate::test_support::scenario::{chat_answer, only_effect, text_of};

/// Records every observation and every content report as one line, in
/// order, so a test reads the whole trace a run reports.
#[derive(Default)]
struct TraceRecorder(Mutex<Vec<String>>);

impl TraceRecorder {
    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("the trace mutex is not poisoned")
            .push(line);
    }

    fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the trace mutex is not poisoned")
            .clone()
    }
}

impl Observer for TraceRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.push(format!("{section}: {event}"));
    }

    fn on_assistant_reply(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        _model: &str,
        _metrics: Option<&CallMetrics>,
        origin: ReplyOrigin,
    ) {
        self.push(format!(
            "{section}: reply turn={turn} origin={origin:?} finish={finish_reason:?} text={text}"
        ));
    }

    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        _model: &str,
        calls: &[ToolCallEvent],
    ) {
        let calls: Vec<String> = calls
            .iter()
            .map(|call| format!("{}:{}", call.id, call.name))
            .collect();
        self.push(format!("{section}: tool_calls turn={turn} calls={calls:?}"));
    }

    fn on_tool_result(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        tool_call_id: &str,
        alias: &str,
        content: &str,
        trusted: bool,
    ) {
        self.push(format!(
            "{section}: tool_result turn={turn} id={tool_call_id} alias={alias} \
             trusted={trusted} content={content}"
        ));
    }

    fn on_thinking(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        _model: &str,
        text: &str,
    ) {
        self.push(format!("{section}: thinking turn={turn} text={text}"));
    }

    fn on_task_notice(
        &self,
        _execution: &str,
        section: &str,
        _chain_id: u32,
        _depth: u32,
        turn: u32,
        task: &TaskId,
        text: &str,
    ) {
        self.push(format!(
            "{section}: notice turn={turn} task={task} text={text}"
        ));
    }
}

/// The model-turn and tool-call observations `trace` reports, in order.
fn trace_loop_events(trace: &Trace) -> Vec<String> {
    let recorder = Recorder::default();
    trace.replay(&recorder);
    loop_events(&recorder)
}

#[test]
fn a_failing_bound_tool_counts_toward_the_clean_exit() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "fail", "{}")]),
        text_finish("", "stop"),
    ]);
    let source = one_section(
        "tools.offer('tools/failing')
         local msgs = messages.new()
         msgs:user('try the tool')
         local result = models.loop(msgs)
         return tostring(result) .. '|' .. #msgs .. '|' .. msgs[3].role \
         .. '|' .. msgs[4].role .. ':' .. msgs[4].content",
    );
    let trace = loop_scenario(source)
        .offer("fail", failing_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "nil|4|tool|assistant:");
    assert_eq!(
        trace_loop_events(&trace),
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_FAILED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
        ],
        "the counted call is the failed one"
    );
}

#[test]
fn the_clean_exit_counts_only_the_calls_of_its_own_loop_call() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"hi\"}")]),
        text("done"),
        text_finish("", "stop"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('first')
         models.loop(msgs)
         msgs:user('second')
         local ok, err = pcall(models.loop, msgs)
         return tostring(ok) .. '|' .. err.kind .. '|' .. tostring(err.finish_reason) \
         .. '|' .. #msgs .. '|' .. msgs[3].role",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "false|empty_model_reply|stop|5|tool");
}

#[test]
fn a_handler_raising_mid_batch_leaves_the_list_untouched_and_raises_its_own_table() {
    // The second call's handler raises its own table: the batch never
    // lands, the call reports a failed tool call with no `ToolResult`, and
    // the very table the handler raised reaches the loop's caller.
    let gateway = ScriptedChat::new(vec![calls(&[
        ("c1", "grab", "{\"value\":\"a\"}"),
        ("c2", "grab", "{\"value\":\"b\"}"),
    ])]);
    let source = one_section(
        "local own = { reason = 'mine' }
         local count = 0
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)
           count = count + 1
           if count == 2 then error(own) end
           return 'grabbed ' .. args.value
         end)
         local msgs = messages.new()
         msgs:user('grab twice')
         local ok, err = pcall(models.loop, msgs)
         return tostring(ok) .. '|' .. tostring(err == own) .. '|' .. count \
         .. '|' .. #msgs .. '|' .. msgs[1].role",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "false|true|2|1|user");
    let recorder = ToolRecorder::default();
    trace.replay(&recorder);
    let lines = recorder.lines();
    assert!(
        lines.contains(&format!("Only: {}", detail::TOOL_CALL_FAILED)),
        "the raising handler is a failed tool call: {lines:?}"
    );
    assert_eq!(
        tool_result_lines(&recorder),
        vec!["Only: tool_result id=c1 alias=grab trusted=true content=grabbed a".to_owned()],
        "only the call that returned reports a result"
    );
    assert_eq!(gateway.call_count(), 1, "no round follows the raise");
}

#[test]
fn a_dropped_model_issued_call_raises_cancelled_at_the_loop_call_site() {
    let source = one_section(
        "tools.offer('tools/echo')
         local msgs = messages.new()
         msgs:user('asked')
         local ok, err = pcall(models.loop, msgs)
         return tostring(ok) .. ':' .. err.kind .. '|' .. #msgs .. '|' .. msgs[1].role",
    );
    let mut run = loop_scenario(source).offer("echo", echo_tool()).into_run();
    let (round, effect) = only_effect(run.step());
    assert!(
        matches!(effect, Effect::Chat { .. }),
        "the loop's round: {effect:?}"
    );
    run.resume(
        round,
        chat_answer(&calls(&[("call_1", "echo", r#"{"value":"x"}"#)])),
    );
    let (call, effect) = only_effect(run.step());
    assert!(
        matches!(&effect, Effect::ToolCall { alias, .. } if alias == "echo"),
        "the model's call: {effect:?}"
    );
    run.resume(call, EffectAnswer::Dropped);
    let Step::Done { result, .. } = run.step() else {
        panic!("the caught drop ends the section");
    };
    assert_eq!(text_of(result), "false:cancelled|1|user");
}

#[test]
fn a_local_tools_first_return_value_is_its_text_and_nil_is_empty() {
    let gateway = ScriptedChat::new(vec![
        calls(&[
            ("c1", "none", "{}"),
            ("c2", "num", "{}"),
            ("c3", "two", "{}"),
        ]),
        text("done"),
    ]);
    let source = one_section(
        "tools.offer_local('none', 'Returns nothing', {}, function() end)
         tools.offer_local('num', 'Returns a number', {}, function() return 42 end)
         tools.offer_local('two', 'Returns two values', {}, function() return 'first', 'second' end)
         local msgs = messages.new()
         msgs:user('call all three')
         models.loop(msgs)
         return '[' .. msgs[3].content .. ']|' .. msgs[4].content .. '|' .. msgs[5].content",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "[]|42|first");
}

#[test]
fn a_local_tool_cancelled_mid_handler_reports_no_failed_tool_call() {
    // The cancel lands between steps once the first round is answered, so
    // the step that hands the model's call to the looping handler runs
    // with the flag already set.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "grab", "{\"value\":\"hi\"}")]),
        text("unreachable"),
    ]);
    let source = one_section(
        "tools.offer_local('grab', 'Grab a value', { value = 'string' }, function(args)
           while true do end
         end)
         local msgs = messages.new()
         msgs:user('Use the tool.')
         models.loop(msgs)
         return 'unreachable'",
    );
    let mut driving = loop_scenario(source).chat(&gateway).start();
    while gateway.call_count() == 0 {
        assert!(driving.step(), "the run reaches its first round");
    }
    driving.cancel();
    let trace = driving.finish();
    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "expected the cancelled run, got {:?}",
        trace.result()
    );
    assert_eq!(
        trace_loop_events(&trace),
        vec![detail::MODEL_TURN_COMPLETED.to_string()],
        "the round completed and the cancelled handler reported nothing"
    );
}

#[test]
fn the_scheduler_reports_the_whole_trace_of_a_bound_round_a_local_round_and_a_reply() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "echo", "{\"value\":\"a\"}")]),
        calls(&[("call_2", "grab", "{\"value\":\"b\"}")]),
        text("done"),
    ]);
    let source = PromptSource::new().title("Loop").section(
        "Only",
        "tools.offer('tools/echo')
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)
           return 'grabbed ' .. args.value
         end)
         local msgs = messages.new()
         msgs:user('use both tools')
         models.loop(msgs)
         return msgs[#msgs].content",
    );
    let trace = loop_scenario(source.build())
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "done");
    let recorder = TraceRecorder::default();
    trace.replay(&recorder);
    let validated = [
        "Only: Tool scope validation started",
        "Only: Tool scope validation succeeded",
        "Only: Model turn completed",
    ];
    let mut expected = vec![
        "Prompt: Parse started",
        "Only: Lua compilation started",
        "Only: Lua compilation succeeded",
        "Prompt: Parse succeeded",
        "Loop: Run started",
        "Only: Section started",
        "Only: Lua chunk started",
    ];
    expected.extend(validated);
    expected.extend([
        "Only: tool_calls turn=1 calls=[\"call_1:echo\"]",
        "Only: Tool call succeeded",
        "Only: tool_result turn=1 id=call_1 alias=echo trusted=true content=echoed: a",
    ]);
    expected.extend(validated);
    expected.extend([
        "Only: tool_calls turn=2 calls=[\"call_2:grab\"]",
        "Only: Tool call succeeded",
        "Only: tool_result turn=2 id=call_2 alias=grab trusted=true content=grabbed b",
    ]);
    expected.extend(validated);
    expected.extend([
        "Only: reply turn=3 origin=Chat finish=None text=done",
        "Only: Lua chunk succeeded",
        "Only: Lua teardown started",
        "Only: Lua teardown succeeded",
        "Only: Section finished",
        "Loop: Run succeeded",
    ]);
    assert_eq!(recorder.lines(), expected);
}
