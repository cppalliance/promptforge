//! Tests for the generic input broker: direct `user_input()` (operator
//! text with the availability flag, the unspoofable fallback sentence,
//! host failure, cancellation) and the contract that a configured broker
//! advertises no `user_input` tool to the model.

use super::*;
use crate::input::{INPUT_UNAVAILABLE_FALLBACK, InputError, InputOutcome};
use crate::lua::ToolSet;
use crate::model::{ModelBinding, ModelId};
use crate::test_support::TestBroker;
use crate::test_support::tokio_driver::TokioDriver;
use promptforge_model_client::model::ModelInvocation;

/// The model set an input test's run uses: `writer` (the prompt-wide
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

/// Builds the run context and its host for an input test: the parsed
/// prompt, an empty shared library, and the shared model and tool sets
/// pre-filled (the scheduler tests bypass the live H1 pass that would fill
/// them). The broker and observer arrive through `host`.
fn input_context(
    prompt: &Prompt,
    tools: impl Into<FixtureTools>,
    host: RunHost,
) -> (RunState, RunHost) {
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &test_context(EXECUTION),
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = input_models();
    let host = tools.into().install(&ctx, host);
    (ctx, host)
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
impl TestBroker for TextBroker {
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
impl TestBroker for UnavailableBroker {
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
impl TestBroker for FailingBroker {
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
impl TestBroker for PendingBroker {
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
    let input_host = RunHost::new()
        .observer(Arc::clone(&recorder) as Arc<dyn Observer>)
        .input_broker(Arc::new(TextBroker("hello operator")));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let out = TokioDriver::new(&ctx, host, None)
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
    let input_host = RunHost::new().input_broker(Arc::new(TextBroker(INPUT_UNAVAILABLE_FALLBACK)));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let out = TokioDriver::new(&ctx, host, None)
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
    let input_host = RunHost::new().observer(Arc::clone(&recorder) as Arc<dyn Observer>);
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let out = TokioDriver::new(&ctx, host, None)
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
    // The engine cannot know the host has no operator: it issues the
    // wait as an effect and reports it, and the broker-less host answers
    // with the unavailable fallback.
    assert!(
        recorder
            .events()
            .contains(&detail::USER_INPUT_WAIT_STARTED.to_string()),
        "the wait is issued as an effect the host answers"
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
    let input_host = RunHost::new()
        .observer(Arc::clone(&recorder) as Arc<dyn Observer>)
        .input_broker(Arc::new(UnavailableBroker));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let out = TokioDriver::new(&ctx, host, None)
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
         return tostring(err)",
    );
    let prompt = parse(&md);
    let input_host = RunHost::new().input_broker(Arc::new(FailingBroker));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let out = TokioDriver::new(&ctx, host, None)
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
    let input_host = RunHost::new().input_broker(Arc::new(FailingBroker));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let error = TokioDriver::new(&ctx, host, None)
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
    use std::time::{Duration, Instant};

    let md = input_prompt("user_input()\nreturn 'unreachable'");
    let prompt = parse(&md);
    let input_host = RunHost::new().input_broker(Arc::new(PendingBroker));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let mut scheduler = TokioDriver::new(&ctx, host, None);
    let canceller = scheduler.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let start = Instant::now();
    let result = scheduler.drive().await;
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
async fn a_brokered_loop_with_no_prompt_tools_advertises_no_tools_to_the_model() {
    let gateway = ScriptedGateway::start(vec![resp_text("all done")]).await;
    let md = input_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    );
    let prompt = parse(&md);
    let input_host = RunHost::new().input_broker(Arc::new(TextBroker("never asked")));
    let (ctx, host) = input_context(&prompt, ToolSet::default(), input_host);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a tool-free loop runs to its terminal turn");
    assert_eq!(out, "all done");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 1, "one terminal turn is one request");
    assert!(
        bodies[0].get("tools").is_none() || bodies[0]["tools"].is_null(),
        "a configured input broker advertises no user_input tool: {bodies:?}"
    );
}
