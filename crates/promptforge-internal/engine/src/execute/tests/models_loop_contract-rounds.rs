//! Contract tests for `models.loop`'s rounds: the clean exit's call
//! count, the batch rule under a raising handler, a dropped model-issued
//! call, a local tool's return and its cancellation, and the full trace a
//! run reports for the loop.

use promptforge_types::event::ReplyOrigin;
use promptforge_types::ids::TaskId;
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};

use super::super::models_loop::{always_tool, echo_tools, loop_events};
use super::super::serial_driver::{text_of, tool_call_reply};
use super::super::tool_call_arm::{ToolRecorder, tool_result_lines};
use super::*;
use crate::execute::run::{Effect, EffectAnswer, EffectId, Run, Step};

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

/// The one effect `step` issued, with its id.
fn only_effect(step: Step) -> (EffectId, Effect) {
    let Step::Pending { mut effects, .. } = step else {
        panic!("the run parks on an effect");
    };
    assert_eq!(effects.len(), 1, "one effect is outstanding: {effects:?}");
    let (id, _, effect) = effects.remove(0);
    (id, effect)
}

#[tokio::test(flavor = "current_thread")]
async fn a_failing_bound_tool_counts_toward_the_clean_exit() {
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "fail", "{}"),
        resp_text_finish("", "stop"),
    ]);
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('try the tool')\n\
         local result = models.loop(msgs)\n\
         return tostring(result) .. '|' .. #msgs .. '|' .. msgs[3].role \
         .. '|' .. msgs[4].role .. ':' .. msgs[4].content",
    );
    let recorder = Arc::new(Recorder::default());
    let out = drive_observed(
        &md,
        always_tool("fail", Arc::new(FailingTool)),
        &gateway,
        Arc::clone(&recorder) as Arc<dyn Observer>,
    )
    .await
    .expect("the empty stop after a failed tool is the clean exit");
    assert_eq!(out, "nil|4|tool|assistant:");
    assert_eq!(
        loop_events(&recorder),
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_FAILED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
        ],
        "the counted call is the failed one"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_clean_exit_counts_only_the_calls_of_its_own_loop_call() {
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("done"),
        resp_text_finish("", "stop"),
    ]);
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('first')\n\
         models.loop(msgs)\n\
         msgs:user('second')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         return tostring(ok) .. '|' .. err.kind .. '|' .. tostring(err.finish_reason) \
         .. '|' .. #msgs .. '|' .. msgs[3].role",
    );
    let out = drive(&md, echo_tools(), &gateway)
        .await
        .expect("the second loop's raise is pcall-able");
    assert_eq!(out, "false|empty_model_reply|stop|5|tool");
}

#[tokio::test(flavor = "current_thread")]
async fn a_handler_raising_mid_batch_leaves_the_list_untouched_and_raises_its_own_table() {
    // The second call's handler raises its own table: the batch never
    // lands, the call reports a failed tool call with no `ToolResult`, and
    // the very table the handler raised reaches the loop's caller.
    let gateway = ScriptedChat::new(vec![resp_two_tool_calls(
        "grab",
        ("c1", "{\"value\":\"a\"}"),
        ("c2", "{\"value\":\"b\"}"),
    )]);
    let md = loop_prompt(
        "local own = { reason = 'mine' }\n\
         local count = 0\n\
         tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           count = count + 1\n\
           if count == 2 then error(own) end\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('grab twice')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         return tostring(ok) .. '|' .. tostring(err == own) .. '|' .. count \
         .. '|' .. #msgs .. '|' .. msgs[1].role",
    );
    let recorder = Arc::new(ToolRecorder::default());
    let out = drive_observed(
        &md,
        ToolSet::default(),
        &gateway,
        Arc::clone(&recorder) as Arc<dyn Observer>,
    )
    .await
    .expect("the handler's raise is pcall-able");
    assert_eq!(out, "false|true|2|1|user");
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
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('asked')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         return tostring(ok) .. ':' .. err.kind .. '|' .. #msgs .. '|' .. msgs[1].role",
    );
    let prompt = parse(&md);
    let (ctx, _fixture) = loop_context(&prompt, echo_tools());
    let mut run = Run::from_state(ctx);
    let (round, effect) = only_effect(run.step());
    assert!(
        matches!(effect, Effect::Chat { .. }),
        "the loop's round: {effect:?}"
    );
    run.resume(
        round,
        tool_call_reply("call_1", "echo", json!({ "value": "x" })),
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

#[tokio::test(flavor = "current_thread")]
async fn a_local_tools_first_return_value_is_its_text_and_nil_is_empty() {
    let gateway = ScriptedChat::new(vec![
        ScriptedReply::ToolCalls {
            model: MOCK_MODEL.to_owned(),
            calls: vec![
                scripted_call("c1", "none", "{}"),
                scripted_call("c2", "num", "{}"),
                scripted_call("c3", "two", "{}"),
            ],
        },
        resp_text("done"),
    ]);
    let md = loop_prompt(
        "tools.add_local('none', 'Returns nothing', {}, function() end)\n\
         tools.add_local('num', 'Returns a number', {}, function() return 42 end)\n\
         tools.add_local('two', 'Returns two values', {}, function() return 'first', 'second' end)\n\
         local msgs = messages.new()\n\
         msgs:user('call all three')\n\
         models.loop(msgs)\n\
         return '[' .. msgs[3].content .. ']|' .. msgs[4].content .. '|' .. msgs[5].content",
    );
    let out = drive(&md, ToolSet::default(), &gateway)
        .await
        .expect("every scalar return is a result");
    assert_eq!(out, "[]|42|first");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_tool_cancelled_mid_handler_reports_no_failed_tool_call() {
    use std::time::Duration;

    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("unreachable"),
    ]);
    let md = loop_prompt(
        "tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           while true do end\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('Use the tool.')\n\
         models.loop(msgs)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(Recorder::default());
    let (ctx, fixture) = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let mut driver = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)));
    let canceller = driver.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let result = driver.drive().await;
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "expected Interrupted, got {result:?}"
    );
    assert_eq!(
        loop_events(&recorder),
        vec![detail::MODEL_TURN_COMPLETED.to_string()],
        "the round completed and the cancelled handler reported nothing"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_scheduler_reports_the_whole_trace_of_a_bound_round_a_local_round_and_a_reply() {
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"a\"}"),
        resp_tool_call("call_2", "grab", "{\"value\":\"b\"}"),
        resp_text("done"),
    ]);
    let md = loop_prompt(
        "tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use both tools')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    );
    let recorder = Arc::new(TraceRecorder::default());
    let out = drive_observed(
        &md,
        echo_tools(),
        &gateway,
        Arc::clone(&recorder) as Arc<dyn Observer>,
    )
    .await
    .expect("the three rounds run");
    assert_eq!(out, "done");
    let validated = [
        "Only: Tool scope validation started",
        "Only: Tool scope validation succeeded",
        "Only: Model turn completed",
    ];
    let mut expected = vec![
        "Loop: Run started",
        "Only: Section started",
        "Only: Lua shared load started",
        "Only: Lua shared load succeeded",
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
