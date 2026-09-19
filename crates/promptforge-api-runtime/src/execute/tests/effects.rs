//! Effects as values: every leaf request kind a section yields - `infer`
//! and `chat`, `tool_call`, `user_input`, `store`, `timer` - issues
//! exactly one `Effect` out of the run's `step`, and each effect's record
//! round-trips through serde; only a `chat` round streams its deltas to
//! the host. The run's answer rules (a drop, an orphan, a wrong kind) are
//! pinned beside `Run` itself.

use super::models_loop::{echo_tools, loop_models, loop_prompt};
use super::scheduler::scheduler_context_on;
use super::*;
use crate::client::StreamDelta;
use crate::execute::protocol::StoreOp;
use crate::execute::run::EffectRecord;
use crate::execute::tokio_driver::TokioDriver;
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
        Arc::new(prompt.clone()),
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

/// Records every streamed delta the run forwards to the host.
fn delta_hook() -> (Arc<Mutex<Vec<StreamDelta>>>, RunContext) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let config = test_context(EXECUTION).on_delta(Arc::new(move |delta| {
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
    let ctx = effect_context(&prompt, ToolSet::default(), &test_context(EXECUTION));
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
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
    let ctx = effect_context(&prompt, echo_tools(), &test_context(EXECUTION));
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
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
    let ctx = effect_context(&prompt, echo_tools(), &test_context(EXECUTION));
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    let config = test_context(EXECUTION).input_broker(Arc::new(TextBroker("typed")));
    let ctx = effect_context(&prompt, ToolSet::default(), &config);
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    let ctx = effect_context(&prompt, ToolSet::default(), &test_context(EXECUTION));
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    let mut scheduler = TokioDriver::new(&ctx, None);
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
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
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
    let mut scheduler = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let out = scheduler.drive().await.expect("the infer completes");
    assert_eq!(out, "answer");
    assert!(
        seen.lock()
            .expect("the delta log mutex is not poisoned")
            .is_empty(),
        "an infer round forwards no deltas"
    );
}
