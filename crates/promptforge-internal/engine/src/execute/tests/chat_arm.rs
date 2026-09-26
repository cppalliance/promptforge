//! Tests for the scheduler's `Chat` dispatch arm, driven through
//! `models.loop`, the only source of `chat` yields: the events the
//! scheduler emits when it applies a round's answer, the tool-call batch
//! it resumes for the shim to dispatch, the out-of-scope refusal, and the
//! empty and overflow rounds it resumes rather than raises, which the
//! shim's exit rules and compactor then act on. The round's tool scope is
//! covered in `chat_scope`.

use super::models_loop::{echo_tools, loop_context_observed, loop_prompt};
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;
use promptforge_types::event::ReplyOrigin;
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};

/// Records every observation and every content report as one rendered
/// line: the boundary events, the turn numbers, the model name, the
/// metrics presence, and the text or calls the model produced.
#[derive(Default)]
struct RoundRecorder(Mutex<Vec<String>>);

impl RoundRecorder {
    fn push(&self, line: String) {
        self.0
            .lock()
            .expect("the round recorder mutex is not poisoned")
            .push(line);
    }

    fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .expect("the round recorder mutex is not poisoned")
            .clone()
    }

    /// The index of the first line containing `needle`.
    fn position(&self, needle: &str) -> Option<usize> {
        self.lines().iter().position(|line| line.contains(needle))
    }
}

impl Observer for RoundRecorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.push(format!("{section}: {event}"));
    }

    fn on_thinking(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        text: &str,
    ) {
        self.push(format!(
            "{section}: thinking chain={chain_id} depth={depth} turn={turn} model={model} text={text}"
        ));
    }

    fn on_assistant_reply(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        metrics: Option<&CallMetrics>,
        origin: ReplyOrigin,
    ) {
        self.push(format!(
            "{section}: reply origin={origin:?} chain={chain_id} depth={depth} turn={turn} text={text} \
             finish={finish_reason:?} model={model} metrics={}",
            metrics.is_some()
        ));
    }

    fn on_assistant_tool_calls(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        model: &str,
        calls: &[ToolCallEvent],
    ) {
        let names: Vec<&str> = calls.iter().map(|call| call.name.as_str()).collect();
        self.push(format!(
            "{section}: tool_calls chain={chain_id} depth={depth} turn={turn} model={model} calls={names:?}"
        ));
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
        _content: &str,
        trusted: bool,
    ) {
        self.push(format!(
            "{section}: tool_result turn={turn} id={tool_call_id} alias={alias} trusted={trusted}"
        ));
    }
}

/// A text reply with everything a round can report: a model name, a
/// reasoning side channel, a finish reason, and usage metrics.
fn rich_text_reply(content: &str) -> GatewayReply {
    GatewayReply::Json(json!({
        "model": "served-model",
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "reasoning_content": "let me think",
                "content": content,
            }
        }],
        "usage": { "prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5 }
    }))
}

/// The provider's context-window rejection.
fn context_rejection() -> GatewayReply {
    resp_status(400, "This model's maximum context length is 4096 tokens.")
}

#[tokio::test(flavor = "current_thread")]
async fn a_text_round_reports_its_reply_and_the_loop_appends_it() {
    let gateway = ScriptedGateway::start(vec![rich_text_reply("final answer")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         assert(#msgs == 2, 'the loop appends the reply')\n\
         assert(msgs[2].role == 'assistant', 'the appended record is the assistant reply')\n\
         assert(msgs[2].content == 'final answer', 'the reply text resumes')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("one text round ends the loop");
    assert_eq!(out, "ok");
    let lines = recorder.lines();
    let thinking = recorder
        .position("thinking")
        .unwrap_or_else(|| panic!("the reasoning side channel is reported: {lines:?}"));
    assert!(
        lines[thinking].contains("model=served-model text=let me think"),
        "{lines:?}"
    );
    let reply = recorder
        .position("reply origin=Chat")
        .unwrap_or_else(|| panic!("the arm reports a chat-origin reply: {lines:?}"));
    assert!(
        lines[reply]
            .contains("text=final answer finish=Some(\"stop\") model=served-model metrics=true"),
        "the reply carries its finish reason, serving model, and metrics: {lines:?}"
    );
    assert!(thinking < reply, "thinking precedes the reply: {lines:?}");
    assert_eq!(
        gateway.requests()[0]["tools"][0]["function"]["name"],
        "echo",
        "the round advertises the section's scope"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_tool_round_reports_the_batch_before_the_shim_dispatches_it() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("done"),
    ])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('call the tool')\n\
         models.loop(msgs)\n\
         local call = msgs[2].tool_calls[1]\n\
         assert(call.id == 'call_1', 'the call keeps its id')\n\
         assert(call.name == 'echo', 'the call keeps its wire name')\n\
         assert(call.arguments.value == 'hi', 'the arguments stay parsed')\n\
         assert(msgs[3].role == 'tool' and msgs[3].tool_call_id == 'call_1', \
           'the result answers the call')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the tool round and the closing reply run");
    assert_eq!(out, "ok");
    assert_eq!(gateway.call_count(), 2, "one tool round, one closing round");
    let lines = recorder.lines();
    let batch = recorder
        .position("calls=[\"echo\"]")
        .unwrap_or_else(|| panic!("the requested calls are reported: {lines:?}"));
    let result = recorder
        .position("tool_result")
        .unwrap_or_else(|| panic!("the shim dispatches the call: {lines:?}"));
    assert!(
        lines[result].contains("id=call_1 alias=echo"),
        "the result fires under the model's call id: {lines:?}"
    );
    assert!(
        batch < result,
        "the arm reports the batch unexecuted, then the shim dispatches it: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_out_of_scope_tool_name_fails_the_round_with_out_of_scope_tool() {
    // Readable at the call site as the `out_of_scope_tool` kind with the
    // loop's exact message, and typed when it escapes the section.
    let gateway = ScriptedGateway::start(vec![resp_tool_call("call_1", "rogue", "{}")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('go rogue')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'an out-of-scope call raises')\n\
         return err.kind .. '|' .. err.name .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert_eq!(
        out,
        "out_of_scope_tool|rogue|tool \"rogue\" is not in this section's scope; \
         in-scope aliases: [\"echo\"]"
    );
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(&format!(": {}", detail::TOOL_CALL_FAILED))),
        "the rejected call reports a failed tool call: {lines:?}"
    );

    let gateway = ScriptedGateway::start(vec![resp_tool_call("call_1", "rogue", "{}")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('go rogue')\n\
         models.loop(msgs)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let (ctx, host) =
        loop_context_observed(&prompt, echo_tools(), Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("an uncaught out-of-scope call fails the section");
    match error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "rogue");
            assert!(!global_exists, "rogue is bound nowhere");
            assert_eq!(in_scope, vec!["echo".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_reply_is_a_completed_round_the_loop_raises_as_empty_model_reply() {
    // The arm resumes the empty round with the reply absent and its finish
    // reason; with no answered tool call before it, the loop's exit rules
    // raise `empty_model_reply` carrying that finish reason.
    let gateway = ScriptedGateway::start(vec![resp_text_finish("", "stop")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('say nothing')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'an empty first reply raises')\n\
         assert(#msgs == 1, 'the empty round appends nothing')\n\
         return err.kind .. '|' .. tostring(err.finish_reason)",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert_eq!(out, "empty_model_reply|stop");
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(&format!(": {}", detail::MODEL_TURN_COMPLETED))),
        "the empty round is a completed turn: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_context_overflow_resumes_with_its_reason_for_the_compactor() {
    // The arm answers an overflow as a flag, not a raise, so the loop hands
    // the reason to the compactor. The precheck refuses before any request
    // leaves.
    let compacting_loop = "local seen\n\
         local ok = pcall(models.loop, msgs, function(reason)\n\
           seen = reason\n\
           error('compacted', 0)\n\
         end)\n\
         assert(not ok, 'the compactor raise ends the loop')\n\
         return seen";
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(&format!(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         {compacting_loop}"
    ));
    let prompt = parse(&md);
    let (ctx, host) = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the compactor's raise is pcall-able");
    assert_eq!(out, "precheck");
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck fires before dispatch"
    );

    // The provider's rejection is the same flag after one request, and a
    // failed turn.
    let gateway = ScriptedGateway::start(vec![context_rejection()]).await;
    let md = loop_prompt(&format!(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         {compacting_loop}"
    ));
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the compactor's raise is pcall-able");
    assert_eq!(out, "provider");
    assert_eq!(gateway.call_count(), 1, "the request left and was refused");
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(&format!(": {}", detail::MODEL_TURN_FAILED))),
        "a refused round is a failed turn: {lines:?}"
    );
}
