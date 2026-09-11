//! Tests for the generic input broker: direct `user_input()` (operator
//! text with the availability flag, the unspoofable fallback sentence,
//! host failure, cancellation) and the model-visible input tool adapting
//! the same broker through the correlated tool protocol inside
//! `models.loop`.

use super::*;
use crate::execute::scheduler::Scheduler;
use crate::input::{INPUT_UNAVAILABLE_FALLBACK, InputBroker, InputError, InputOutcome, InputTool};
use crate::lua::{ToolBinding, ToolSet};
use crate::model::{ModelBinding, ModelId, ModelInvocation};

/// The model set an input test's run carries: `writer` (the prompt-wide
/// default, model `test-model`), so `models.loop` resolves a binding.
fn input_models() -> ModelSet {
    ModelSet {
        bindings: vec![ModelBinding::new(
            "writer",
            "A general model for tests",
            ModelId::from_validated("gateway", "test-model"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )],
        default: Some("writer".to_owned()),
    }
}

/// Builds the run context for an input test: the parsed prompt, an empty
/// shared library, and the shared model and tool sets pre-filled (the
/// scheduler tests bypass the live H1 pass that would fill them). The
/// broker arrives through the [`RunConfig`].
fn input_context(prompt: &Prompt, tools: ToolSet, config: &RunConfig) -> RunContext {
    let ctx = RunContext::new(
        prompt,
        "",
        &StoreRef::memory(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        config,
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = input_models();
    *ctx.tool_set()
        .lock()
        .expect("the tool set mutex is not poisoned") = tools;
    ctx
}

/// The one-section prompt shell every input test drives.
fn input_prompt(lua: &str) -> String {
    format!(
        "---\nname: input\ndescription: d\npromptforge: 0\n---\n\n# Input\n\n## Only\n\n```lua\n{lua}\n```\n"
    )
}

/// A broker that always answers with the same operator text.
struct TextBroker(&'static str);

#[async_trait::async_trait]
impl InputBroker for TextBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        Ok(InputOutcome::Text(self.0.to_owned()))
    }
}

/// A broker reporting the host has no input to give.
struct UnavailableBroker;

#[async_trait::async_trait]
impl InputBroker for UnavailableBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        Ok(InputOutcome::Unavailable)
    }
}

/// A broker whose every request fails.
struct FailingBroker;

#[async_trait::async_trait]
impl InputBroker for FailingBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        Err(InputError::message("the input device is gone"))
    }
}

/// A broker that never answers, so only cancellation ends the wait.
struct PendingBroker;

#[async_trait::async_trait]
impl InputBroker for PendingBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        std::future::pending().await
    }
}

/// Records fixed observations and `on_user_input` content reports.
#[derive(Default)]
struct InputRecorder {
    events: Mutex<Vec<String>>,
    inputs: Mutex<Vec<String>>,
}

impl Observer for InputRecorder {
    fn observe(&self, _execution: &str, _section: &str, event: Observation) {
        self.events
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push(event.to_string());
    }

    fn on_user_input(&self, _execution: &str, _section: &str, text: &str) {
        self.inputs
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .push(text.to_owned());
    }
}

impl InputRecorder {
    fn events(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }

    fn inputs(&self) -> Vec<String> {
        self.inputs
            .lock()
            .expect("the recorder mutex must not be poisoned")
            .clone()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn user_input_returns_the_operator_text_with_available_true() {
    let md = input_prompt(
        "local before = 41\n\
         local text, available = user_input()\n\
         assert(available == true, 'operator text reports available')\n\
         return text .. ' ' .. (before + 1)",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(InputRecorder::default());
    let config = RunConfig::new(EXECUTION)
        .observer(Arc::clone(&recorder) as Arc<dyn Observer>)
        .input_broker(Arc::new(TextBroker("hello operator")));
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the wait completes with the operator's text");
    assert_eq!(
        out, "hello operator 42",
        "the section resumes with its VM state intact across the wait"
    );
    assert_eq!(
        recorder.inputs(),
        vec!["hello operator".to_owned()],
        "the response is recorded byte-exact through host observation"
    );
    assert!(
        recorder
            .events()
            .contains(&detail::USER_INPUT_WAIT_STARTED.to_string()),
        "the wait is recorded through host observation"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn identical_human_text_cannot_spoof_the_unavailable_fallback() {
    let md = input_prompt(
        "local text, available = user_input()\n\
         return tostring(available) .. '|' .. text",
    );
    let prompt = parse(&md);
    // The operator types exactly the fallback sentence: the availability
    // flag still distinguishes it from the unavailable policy's answer.
    let config =
        RunConfig::new(EXECUTION).input_broker(Arc::new(TextBroker(INPUT_UNAVAILABLE_FALLBACK)));
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the wait completes");
    assert_eq!(
        out,
        format!("true|{INPUT_UNAVAILABLE_FALLBACK}"),
        "human text byte-identical to the fallback sentence still reports available"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_run_without_a_broker_gets_the_unavailable_fallback() {
    let md = input_prompt(
        "local text, available = user_input()\n\
         return tostring(available) .. '|' .. text",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(InputRecorder::default());
    let config = RunConfig::new(EXECUTION).observer(Arc::clone(&recorder) as Arc<dyn Observer>);
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the unavailable fallback is a successful answer");
    assert_eq!(
        out,
        format!("false|{INPUT_UNAVAILABLE_FALLBACK}"),
        "no broker means the fixed fallback sentence with available false"
    );
    assert!(
        recorder.inputs().is_empty(),
        "the fallback records no operator input"
    );
    assert!(
        !recorder
            .events()
            .contains(&detail::USER_INPUT_WAIT_STARTED.to_string()),
        "the immediate fallback opens no wait"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_unavailable_broker_answer_is_the_fallback() {
    let md = input_prompt(
        "local text, available = user_input()\n\
         return tostring(available) .. '|' .. text",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(InputRecorder::default());
    let config = RunConfig::new(EXECUTION)
        .observer(Arc::clone(&recorder) as Arc<dyn Observer>)
        .input_broker(Arc::new(UnavailableBroker));
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the unavailable answer is not a failure");
    assert_eq!(out, format!("false|{INPUT_UNAVAILABLE_FALLBACK}"));
    assert!(
        recorder.inputs().is_empty(),
        "an unavailable answer records no operator input"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_broker_failure_raises_at_the_call_site() {
    let md = input_prompt(
        "local ok, err = pcall(user_input)\n\
         assert(not ok, 'a broker failure raises')\n\
         return err",
    );
    let prompt = parse(&md);
    let config = RunConfig::new(EXECUTION).input_broker(Arc::new(FailingBroker));
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let out = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect("the pcall catches the raised failure");
    assert!(
        out.contains("the input device is gone"),
        "the call site sees the broker's message, got: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_uncaught_broker_failure_fails_the_run_typed() {
    let md = input_prompt("user_input()\nreturn 'unreachable'");
    let prompt = parse(&md);
    let config = RunConfig::new(EXECUTION).input_broker(Arc::new(FailingBroker));
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let error = Scheduler::new(&ctx, None)
        .drive()
        .await
        .expect_err("an uncaught broker failure fails the run");
    assert!(
        matches!(error, Error::Input { .. }),
        "the failure stays typed all the way out, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_interrupts_a_pending_input_wait() {
    use crate::cancel::{self, CancelHandle};
    use std::time::{Duration, Instant};

    let md = input_prompt("user_input()\nreturn 'unreachable'");
    let prompt = parse(&md);
    let config = RunConfig::new(EXECUTION).input_broker(Arc::new(PendingBroker));
    let ctx = input_context(&prompt, ToolSet::default(), &config);
    let handle = CancelHandle::new();
    let canceller = handle.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let start = Instant::now();
    let mut scheduler = Scheduler::new(&ctx, None);
    let result = cancel::scope(handle, scheduler.drive()).await;
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "cancel during a pending input wait must return promptly, took {:?}",
        start.elapsed()
    );
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "a cancelled wait interrupts the run, got {result:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_model_visible_input_tool_adapts_the_broker_through_the_correlated_tool_protocol() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "user_input", "{}"),
        resp_text("all done"),
    ])
    .await;
    let recorder = Arc::new(InputRecorder::default());
    let tool = InputTool::new(
        Arc::new(TextBroker("from the operator")),
        EXECUTION,
        "Only",
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let tools = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "user_input",
            "operator input capability",
            Arc::new(tool),
        )],
        vec!["user_input".to_owned()],
    );
    let md = input_prompt(
        "local msgs = messages.new()\n\
         msgs:user('ask me something')\n\
         models.loop(msgs)\n\
         assert(#msgs == 4, 'user, assistant call, correlated result, terminal')\n\
         assert(msgs[2].role == 'assistant', 'the model asked through a tool call')\n\
         assert(msgs[3].role == 'tool', 'the input answer is a tool result')\n\
         assert(msgs[3].tool_call_id == 'call_1', 'the result is correlated to the call')\n\
         assert(msgs[3].content == 'from the operator', 'the operator text is the result content')\n\
         assert(msgs[4].role == 'assistant' and msgs[4].content == 'all done', 'the terminal record closes the loop')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let config = RunConfig::new(EXECUTION).observer(Arc::clone(&recorder) as Arc<dyn Observer>);
    let ctx = input_context(&prompt, tools, &config);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop runs the input tool round to its terminal turn");
    assert_eq!(out, "ok");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 2, "the tool round plus the terminal round");
    let tool_message = bodies[1]["messages"]
        .as_array()
        .expect("a request body must carry a messages array")
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the re-sent conversation must include the tool turn");
    assert_eq!(
        tool_message["tool_call_id"], "call_1",
        "the wire result answers the model's call id"
    );
    assert_eq!(tool_message["content"], "from the operator");
    assert_eq!(
        recorder.inputs(),
        vec!["from the operator".to_owned()],
        "the tool path records the response through host observation exactly once"
    );
}
