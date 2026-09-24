//! Tests for the scheduler's `Chat` dispatch arm from a section VM: one
//! stateless tool-capable round whose events the scheduler emits when it
//! applies the answer. The section fixtures reach the arm through the
//! test-only `models.chat` install (`expose_raw_shims_for_test`); in
//! production only the loop shim yields `chat`. The round's tool-scope
//! resolution (absent, explicit, empty, and unbound lists) is covered in
//! `chat_scope`.

use super::models_loop::{echo_tools, loop_models, loop_prompt};
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;
use promptforge_types::event::ReplyOrigin;
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};

/// Records every observation and every content report as one rendered
/// line, so two runs can be compared as whole sequences: the boundary
/// events, the turn numbers, the model name, the metrics presence, and the
/// text or calls the model produced.
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

/// The loop context with `models.chat` exposed to the section and the
/// given observer installed on the run.
pub(super) fn chat_context(
    prompt: &Prompt,
    tools: impl Into<FixtureTools>,
    observer: Arc<dyn Observer>,
) -> (RunState, RunHost) {
    let mut ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &test_context(EXECUTION),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    let host = tools
        .into()
        .install(&ctx, RunHost::new().observer(observer));
    ctx.expose_raw_shims_for_test();
    (ctx, host)
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

#[tokio::test(flavor = "current_thread")]
async fn a_chat_round_reports_the_same_sequence_as_the_rust_loop_for_a_text_reply() {
    // The Rust loop's one-round observation sequence is the reference: a
    // `chat` yield answered with the same mock reply must produce it
    // event for event, including the content reports.
    let loop_gateway = ScriptedGateway::start(vec![rich_text_reply("final answer")]).await;
    let loop_md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return 'ok'",
    );
    let loop_prompt_parsed = parse(&loop_md);
    let loop_recorder = Arc::new(RoundRecorder::default());
    let (loop_ctx, loop_host) = chat_context(
        &loop_prompt_parsed,
        echo_tools(),
        Arc::clone(&loop_recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(
        &loop_ctx,
        loop_host,
        Some(gateway_client(loop_gateway.addr())),
    )
    .drive()
    .await
    .expect("the reference loop runs one text round");
    assert_eq!(out, "ok");

    let chat_gateway = ScriptedGateway::start(vec![rich_text_reply("final answer")]).await;
    let chat_md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         local r = models.chat(msgs)\n\
         assert(r.overflow == false, 'a served round is not an overflow')\n\
         assert(r.reply == 'final answer', 'the reply text resumes')\n\
         assert(r.tool_calls == nil, 'a text round leaves tool_calls nil')\n\
         assert(r.finish_reason == 'stop', 'the finish reason resumes')\n\
         assert(r.model == 'served-model', 'the serving model resumes')\n\
         assert(r.metrics ~= nil, 'the usage metrics resume')\n\
         return 'ok'",
    );
    let chat_prompt_parsed = parse(&chat_md);
    let chat_recorder = Arc::new(RoundRecorder::default());
    let (chat_ctx, chat_host) = chat_context(
        &chat_prompt_parsed,
        echo_tools(),
        Arc::clone(&chat_recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(
        &chat_ctx,
        chat_host,
        Some(gateway_client(chat_gateway.addr())),
    )
    .drive()
    .await
    .expect("one chat round resumes the reply");
    assert_eq!(out, "ok");

    let reference = loop_recorder.lines();
    assert!(
        reference
            .iter()
            .any(|line| line.contains("reply") && line.contains("text=final answer")),
        "the reference sequence includes the reply report: {reference:?}"
    );
    assert_eq!(
        chat_recorder.lines(),
        reference,
        "the chat arm reports exactly the loop's one-round sequence"
    );
    let chat_lines = chat_recorder.lines();
    assert!(
        chat_lines
            .iter()
            .any(|line| line.contains("reply origin=Chat")),
        "the chat arm reports a chat-origin reply, pinning the emit site's `ReplyOrigin::Chat`: {chat_lines:?}"
    );
    assert_eq!(
        chat_gateway.requests()[0]["tools"][0]["function"]["name"],
        "echo",
        "an absent tool list advertises the section's scope"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_chat_round_resumes_the_requested_tool_calls_unexecuted() {
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('call the tool')\n\
         local r = models.chat(msgs)\n\
         assert(r.overflow == false, 'a served round is not an overflow')\n\
         assert(r.reply == nil, 'a tool round leaves reply nil')\n\
         assert(#r.tool_calls == 1, 'one call resumes')\n\
         assert(r.tool_calls[1].id == 'call_1', 'the call keeps its id')\n\
         assert(r.tool_calls[1].name == 'echo', 'the call keeps its wire name')\n\
         assert(r.tool_calls[1].arguments.value == 'hi', 'the arguments stay parsed')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = chat_context(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a tool round resumes its calls");
    assert_eq!(out, "ok");
    assert_eq!(gateway.call_count(), 1, "the arm runs one round and stops");
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.contains("tool_calls") && line.contains("calls=[\"echo\"]")),
        "the requested calls are reported unexecuted: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("tool_result")),
        "the arm dispatches nothing: {lines:?}"
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
         local ok, err = pcall(models.chat, msgs)\n\
         assert(not ok, 'an out-of-scope call raises')\n\
         return err.kind .. '|' .. err.name .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = chat_context(
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
         models.chat(msgs)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let (ctx, host) = chat_context(&prompt, echo_tools(), Arc::new(NullObserver::default()));
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
async fn an_empty_reply_resumes_as_a_completed_round_with_the_reply_absent() {
    let gateway = ScriptedGateway::start(vec![resp_text_finish("", "stop")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('say nothing')\n\
         local r = models.chat(msgs)\n\
         assert(r.overflow == false, 'an empty reply is not an overflow')\n\
         assert(r.reply == nil, 'the empty reply is absent, never an empty string')\n\
         assert(r.tool_calls == nil, 'no calls')\n\
         assert(r.finish_reason == 'stop', 'the finish reason resumes for the exit rules')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = chat_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an empty reply is a completed round");
    assert_eq!(out, "ok");
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(&format!(": {}", detail::MODEL_TURN_COMPLETED))),
        "the empty round is a completed turn: {lines:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_context_overflow_resumes_as_an_overflow_round_without_raising() {
    // The precheck refuses before any request leaves.
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         local r = models.chat(msgs)\n\
         assert(r.overflow == true, 'the precheck overflow resumes as a flag')\n\
         assert(r.reply == nil and r.tool_calls == nil, 'no round ran')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let (ctx, host) = chat_context(
        &prompt,
        ToolSet::default(),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the overflow is the round's answer, not a raise");
    assert_eq!(out, "ok");
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck fires before dispatch"
    );

    // The provider's rejection is the same flag after one request.
    let gateway = ScriptedGateway::start(vec![resp_status(
        400,
        "This model's maximum context length is 4096 tokens.",
    )])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         local r = models.chat(msgs)\n\
         assert(r.overflow == true, 'the provider overflow resumes as a flag')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(RoundRecorder::default());
    let (ctx, host) = chat_context(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the provider overflow is the round's answer");
    assert_eq!(out, "ok");
    assert_eq!(gateway.call_count(), 1, "the request left and was refused");
    let lines = recorder.lines();
    assert!(
        lines
            .iter()
            .any(|line| line.ends_with(&format!(": {}", detail::MODEL_TURN_FAILED))),
        "a refused round is a failed turn: {lines:?}"
    );
}
