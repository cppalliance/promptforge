//! Effects as values: every leaf request kind a section yields - `infer`
//! and `chat`, `tool_call`, `user_input`, `store`, `timer` - issues
//! exactly one `Effect` through the scheduler's performer table, and each
//! effect's record round-trips through serde. A `Dropped` answer resumes
//! the parked chain with the cancelled error and aborts the performer,
//! whose late answer is discarded; an answer of the wrong kind fails the
//! run; only a `chat` round streams its deltas to the host.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::models_loop::{echo_tools, loop_models, loop_prompt};
use super::scheduler::scheduler_context_on;
use super::*;
use crate::client::StreamDelta;
use crate::execute::protocol::StoreOp;
use crate::execute::run::{EffectAnswer, EffectId, EffectRecord};
use crate::execute::scheduler::Scheduler;
use crate::input::{InputBroker, InputError, InputOutcome};
use crate::lua::ToolSet;

/// Serializes a record and reads it back: the round trip a run log and a
/// replay depend on.
fn round_trip(record: &EffectRecord) -> EffectRecord {
    let text = serde_json::to_string(record).expect("a record serializes");
    serde_json::from_str(&text).expect("a serialized record deserializes")
}

/// Asserts every recorded effect survives the round trip unchanged.
fn assert_round_trips(records: &[EffectRecord]) {
    for record in records {
        assert_eq!(&round_trip(record), record, "the record round-trips");
    }
}

/// Builds the run context for an effect test: the parsed prompt, an empty
/// shared library, and the shared model and tool sets pre-filled (the
/// scheduler tests bypass the live H1 pass that would fill them), under
/// the given run configuration.
fn effect_context(prompt: &Prompt, tools: ToolSet, config: &RunContext) -> RunState {
    let ctx = RunState::new(
        prompt,
        "",
        &TestStore::new().vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        config,
    );
    *ctx.model_set()
        .lock()
        .expect("the model set mutex is not poisoned") = loop_models();
    *ctx.tool_set()
        .lock()
        .expect("the tool set mutex is not poisoned") = tools;
    ctx
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

/// A broker that never answers, so only a dropped effect ends the wait.
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

/// Sets its flag when dropped, so a test can prove an aborted performer's
/// future was torn down rather than detached.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A broker that stages the race a cancel loses: its first wait posts the
/// host's `Dropped` for effect 0 and then its own answer for the same id
/// (the performer posted before the abort landed), then parks forever,
/// flagging when its future is dropped; every later wait answers at once.
struct LateBroker {
    calls: AtomicUsize,
    /// The scheduler's answer sender, installed once the scheduler exists
    /// (the broker is configured before the scheduler that owns the
    /// channel is built).
    answers: Mutex<Option<tokio::sync::mpsc::UnboundedSender<(EffectId, EffectAnswer)>>>,
    first_dropped: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl InputBroker for LateBroker {
    async fn user_input(
        &self,
        _execution: &str,
        _section: &str,
    ) -> std::result::Result<InputOutcome, InputError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let _guard = SetOnDrop(Arc::clone(&self.first_dropped));
            let answers = self
                .answers
                .lock()
                .expect("the sender mutex is not poisoned")
                .clone()
                .expect("the test installs the sender before the drive");
            let late = EffectAnswer::UserInput(Ok(InputOutcome::Text("late".to_owned())));
            for answer in [EffectAnswer::Dropped, late] {
                answers
                    .send((EffectId(0), answer))
                    .expect("the scheduler holds its receiver");
            }
            std::future::pending().await
        } else {
            Ok(InputOutcome::Text("second".to_owned()))
        }
    }
}

/// Records every streamed delta the run forwards to the host.
fn delta_hook() -> (Arc<Mutex<Vec<StreamDelta>>>, RunContext) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let config = RunContext::new(EXECUTION).on_delta(Arc::new(move |delta| {
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(delta);
    }));
    (seen, config)
}

#[tokio::test(flavor = "current_thread")]
async fn models_infer_issues_exactly_one_chat_effect_over_one_user_message() {
    let gateway = ScriptedGateway::start(vec![resp_text("answer")]).await;
    let prompt = parse(&loop_prompt("return models.infer('ask')"));
    let ctx = effect_context(&prompt, ToolSet::default(), &RunContext::new(EXECUTION));
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the infer completes");
    assert_eq!(out, "answer");

    let records = records.lock().expect("the tap mutex is not poisoned");
    assert_eq!(
        *records,
        vec![EffectRecord::Chat {
            model: "test-model".to_owned(),
            alias: "writer".to_owned(),
            messages: vec![json!({ "role": "user", "content": "ask" })],
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            thinking: None,
        }],
        "an infer is one tool-free chat effect"
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_models_loop_round_issues_one_chat_effect_and_one_tool_call_effect_per_call() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "echo", r#"{"value":"hi"}"#),
        resp_text("done"),
    ])
    .await;
    let prompt = parse(&loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    ));
    let ctx = effect_context(&prompt, echo_tools(), &RunContext::new(EXECUTION));
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the loop completes");
    assert_eq!(out, "done");

    let records = records.lock().expect("the tap mutex is not poisoned");
    assert_eq!(records.len(), 3, "two rounds and one call: {records:?}");
    assert!(
        matches!(&records[0], EffectRecord::Chat { tools, messages, .. }
            if tools == &["echo".to_owned()] && messages.len() == 1),
        "the first round advertises the scope over the author's list: {:?}",
        records[0]
    );
    assert_eq!(
        records[1],
        EffectRecord::ToolCall {
            tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
            alias: "echo".to_owned(),
            args: json!({ "value": "hi" }),
        },
        "the model's call is one tool_call effect naming the bound identity"
    );
    assert!(
        matches!(&records[2], EffectRecord::Chat { messages, .. } if messages.len() == 3),
        "the second round carries the assistant and tool records: {:?}",
        records[2]
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_issues_exactly_one_tool_call_effect() {
    let prompt = parse(&loop_prompt("return tools.call('echo', { value = 'hi' })"));
    let ctx = effect_context(&prompt, echo_tools(), &RunContext::new(EXECUTION));
    let mut scheduler = Scheduler::new(&ctx, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the call completes");
    assert_eq!(out, "echoed: hi");

    let records = records.lock().expect("the tap mutex is not poisoned");
    assert_eq!(
        *records,
        vec![EffectRecord::ToolCall {
            tool: ToolId::parse("tests/tools/echo").expect("a valid id"),
            alias: "echo".to_owned(),
            args: json!({ "value": "hi" }),
        }]
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn user_input_issues_exactly_one_user_input_effect() {
    let prompt = parse(&loop_prompt(
        "local text, available = user_input()\n\
         return text .. '|' .. tostring(available)",
    ));
    let config = RunContext::new(EXECUTION).input_broker(Arc::new(TextBroker("typed")));
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let mut scheduler = Scheduler::new(&ctx, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the wait completes");
    assert_eq!(out, "typed|true");

    let records = records.lock().expect("the tap mutex is not poisoned");
    assert_eq!(
        *records,
        vec![EffectRecord::UserInput {
            execution: EXECUTION.to_owned(),
            section: "Only".to_owned(),
        }]
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_store_operation_issues_exactly_one_store_effect() {
    let prompt = parse(&loop_prompt(
        "store.write('notes.md', 'kept')\n\
         return store.read('notes.md')",
    ));
    let ctx = effect_context(&prompt, ToolSet::default(), &RunContext::new(EXECUTION));
    let mut scheduler = Scheduler::new(&ctx, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the store ops complete");
    assert_eq!(out, "kept");

    let records = records.lock().expect("the tap mutex is not poisoned");
    assert_eq!(
        *records,
        vec![
            EffectRecord::Store {
                op: StoreOp::Write {
                    path: "notes.md".to_owned(),
                    contents: "kept".to_owned(),
                },
            },
            EffectRecord::Store {
                op: StoreOp::Read {
                    path: "notes.md".to_owned(),
                    start: None,
                    end: None,
                },
            },
        ],
        "each store call is one store effect carrying its validated op"
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_timed_wait_issues_exactly_one_timer_effect() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Timer\n\n\
        ## Main\n\n\
        ```lua\n\
        local t = tasks.spawn('## Child')\n\
        local first, ok, result = tasks.when_any({ t }, { timeout = 30 })\n\
        return result\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\nreturn 'quick'\n```\n";
    let prompt = parse(md);
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let mut scheduler = Scheduler::new(&ctx, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("the wait completes");
    assert_eq!(out, "quick");

    let records = records.lock().expect("the tap mutex is not poisoned");
    assert_eq!(
        *records,
        vec![EffectRecord::Timer { seconds: 30.0 }],
        "the wait's timeout is one timer effect; the child issued none"
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_dropped_answer_resumes_the_parked_chain_with_the_cancelled_error() {
    // The chain parks on a wait no broker ever answers; the host drops
    // the effect instead. The first effect the run issues is id 0, and
    // the answer is posted before the drive so the channel delivers it
    // the moment the driver awaits it.
    let prompt = parse(&loop_prompt("return user_input()"));
    let config = RunContext::new(EXECUTION).input_broker(Arc::new(PendingBroker));
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let scheduler = Scheduler::new(&ctx, None);
    scheduler.post_answer_for_test(0, EffectAnswer::Dropped);
    let mut scheduler = scheduler;
    let result = scheduler.drive().await;
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "a dropped wait resumes as the cancelled error, got {result:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_dropped_effects_performer_is_aborted_and_its_late_answer_discarded() {
    // The host drops effect 0 while its performer runs, and the
    // performer's own answer lands right behind the drop - the race a
    // cancel loses when the performer posted first. The broker stages
    // both from inside the running performer, so the channel delivers
    // them in that order: the drop resumes the caught wait, the chain
    // issues effect 1, and the late answer for 0 arrives with no pending
    // entry. It must be discarded, not fail the run; effect 1's answer
    // then completes it.
    let first_dropped = Arc::new(AtomicBool::new(false));
    let prompt = parse(&loop_prompt(
        "local ok = pcall(user_input)\n\
         local text = user_input()\n\
         return tostring(ok) .. '|' .. text",
    ));
    let broker = Arc::new(LateBroker {
        calls: AtomicUsize::new(0),
        answers: Mutex::new(None),
        first_dropped: Arc::clone(&first_dropped),
    });
    let config =
        RunContext::new(EXECUTION).input_broker(Arc::clone(&broker) as Arc<dyn InputBroker>);
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let mut scheduler = Scheduler::new(&ctx, None);
    *broker
        .answers
        .lock()
        .expect("the sender mutex is not poisoned") = Some(scheduler.answer_sender_for_test());
    let out = scheduler
        .drive()
        .await
        .expect("the late answer for a dropped effect is discarded");
    assert_eq!(
        out, "false|second",
        "the dropped wait failed under pcall and the second wait was answered"
    );
    // The drop aborted the first performer and the run-end drain joined
    // it: its future is gone, not detached on the runtime.
    assert!(
        first_dropped.load(Ordering::SeqCst),
        "the dropped effect's performer was aborted and joined"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_answer_of_the_wrong_kind_for_a_pending_effect_fails_loudly() {
    // Effect 0 is a user_input wait; a timer's firing posted under its id
    // has a pending entry but the wrong kind, so the run fails with the
    // kind-mismatch internal error rather than resuming the chain.
    let prompt = parse(&loop_prompt("return user_input()"));
    let config = RunContext::new(EXECUTION).input_broker(Arc::new(PendingBroker));
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let scheduler = Scheduler::new(&ctx, None);
    scheduler.post_answer_for_test(0, EffectAnswer::Timer);
    let mut scheduler = scheduler;
    let error = scheduler
        .drive()
        .await
        .expect_err("a wrong-kind answer must fail the run");
    assert!(
        matches!(&error, Error::Internal { message, .. } if message.contains("effect's own kind")),
        "the mismatch is a loud invariant failure: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_chat_round_streams_its_deltas_to_the_host() {
    // The scripted gateway serves every reply as two content fragments,
    // so a streaming round forwards exactly two text deltas.
    let gateway = ScriptedGateway::start(vec![resp_text("answer")]).await;
    let prompt = parse(&loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
    ));
    let (seen, config) = delta_hook();
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler.drive().await.expect("the loop completes");
    assert_eq!(out, "answer");
    assert_eq!(
        *seen.lock().expect("the delta log mutex is not poisoned"),
        vec![
            StreamDelta::Text("ans".to_owned()),
            StreamDelta::Text("wer".to_owned()),
        ],
        "a chat round's fragments reach the host's hook live, in order"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_nested_infer_round_streams_no_deltas_to_the_host() {
    // A nested `models.infer` consumes only the completed reply; its
    // fragments have no consumer and never reach the host's hook, exactly
    // as the legacy infer round behaved.
    let gateway = ScriptedGateway::start(vec![resp_text("answer")]).await;
    let prompt = parse(&loop_prompt("return models.infer('ask')"));
    let (seen, config) = delta_hook();
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let mut scheduler = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler.drive().await.expect("the infer completes");
    assert_eq!(out, "answer");
    assert!(
        seen.lock()
            .expect("the delta log mutex is not poisoned")
            .is_empty(),
        "an infer round forwards no deltas"
    );
}
