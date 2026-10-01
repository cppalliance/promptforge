//! Effects as values: every leaf request kind a section yields - `infer`
//! and `chat`, `tool_call`, `store`, `timer` - issues
//! exactly one `Effect` out of the run's `step`, and each effect's record
//! round-trips through serde; only a `chat` round streams its deltas to
//! the Harness. The run's answer rules (a drop, an orphan, a wrong kind) are
//! pinned beside `Run` itself.

use promptforge_types::wire::StreamDelta;

use super::models_loop::{echo_tools, loop_models, loop_prompt};
use super::scheduler::scheduler_context_on;
use super::*;
use crate::execute::protocol::StoreOp;
use crate::execute::run::{EffectRecord, ToolCallOrigin, ToolCaller};
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;

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

/// Builds the run context and its Harness for an effect test: the parsed
/// prompt, an empty shared library, and the shared model and tool sets
/// pre-filled (the scheduler tests bypass the live H1 pass that would fill
/// them), under the given Harness.
fn effect_context(
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
        .expect("the model set mutex is not poisoned") = loop_models();
    let host = tools.into().install(&ctx, host);
    (ctx, host)
}

/// The origin of a call made by `caller` in `section` of the test run.
fn origin(section: &str, caller: ToolCaller) -> ToolCallOrigin {
    ToolCallOrigin {
        execution: EXECUTION.to_owned(),
        section: section.to_owned(),
        caller,
    }
}

/// Records every streamed delta the run forwards to the Harness.
fn delta_hook() -> (Arc<Mutex<Vec<StreamDelta>>>, RunHost) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let host = RunHost::new().on_delta(Arc::new(move |delta| {
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(delta);
    }));
    (seen, host)
}

#[tokio::test(flavor = "current_thread")]
async fn models_infer_issues_exactly_one_chat_effect_over_one_user_message() {
    let gateway = ScriptedGateway::start(vec![resp_text("answer")]).await;
    let prompt = parse(&loop_prompt("return models.infer('ask')"));
    let (ctx, host) = effect_context(&prompt, ToolSet::default(), RunHost::new());
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
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
    let (ctx, host) = effect_context(&prompt, echo_tools(), RunHost::new());
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
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
            origin: origin("Only", ToolCaller::Model),
        },
        "the model's call is one tool_call effect naming the bound identity and the model"
    );
    assert!(
        matches!(&records[2], EffectRecord::Chat { messages, .. } if messages.len() == 3),
        "the second round includes the assistant and tool records: {:?}",
        records[2]
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_issues_exactly_one_tool_call_effect() {
    let prompt = parse(&loop_prompt("return tools.call('echo', { value = 'hi' })"));
    let (ctx, host) = effect_context(&prompt, echo_tools(), RunHost::new());
    let mut scheduler = TokioDriver::new(&ctx, host, None);
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
            origin: origin("Only", ToolCaller::Script),
        }]
    );
    assert_round_trips(&records);
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tool_call_records_the_section_that_made_it() {
    let prompt = parse(
        "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n\
         ## First\n\n```lua\ntools.call('echo', { value = 'a' })\n```\n\n\
         ## Second\n\n```lua\nreturn tools.call('echo', { value = 'b' })\n```\n",
    );
    let (ctx, host) = effect_context(&prompt, echo_tools(), RunHost::new());
    let mut scheduler = TokioDriver::new(&ctx, host, None);
    let records = scheduler.record_effects_for_test();
    let out = scheduler.drive().await.expect("both calls complete");
    assert_eq!(out, "echoed: b");

    let origins: Vec<ToolCallOrigin> = records
        .lock()
        .expect("the tap mutex is not poisoned")
        .iter()
        .filter_map(|record| match record {
            EffectRecord::ToolCall { origin, .. } => Some(origin.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        origins,
        vec![
            origin("First", ToolCaller::Script),
            origin("Second", ToolCaller::Script),
        ],
        "each call names the section it was made in"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_store_operation_issues_exactly_one_store_effect() {
    let prompt = parse(&loop_prompt(
        "store.write('notes.md', 'kept')\n\
         return store.read('notes.md')",
    ));
    let (ctx, host) = effect_context(&prompt, ToolSet::default(), RunHost::new());
    let mut scheduler = TokioDriver::new(&ctx, host, None);
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
        "each store call is one store effect holding its validated op"
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
        local first, ok, result = tasks.join_any({ t }, { timeout = 30 })\n\
        return result\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\nreturn 'quick'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let mut scheduler = TokioDriver::new(&ctx, host, None);
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
    let (seen, delta_host) = delta_hook();
    let (ctx, host) = effect_context(&prompt, ToolSet::default(), delta_host);
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
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
    // fragments have no consumer and never reach the Harness's hook.
    let gateway = ScriptedGateway::start(vec![resp_text("answer")]).await;
    let prompt = parse(&loop_prompt("return models.infer('ask')"));
    let (seen, delta_host) = delta_hook();
    let (ctx, host) = effect_context(&prompt, ToolSet::default(), delta_host);
    let mut scheduler = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    let out = scheduler.drive().await.expect("the infer completes");
    assert_eq!(out, "answer");
    assert!(
        seen.lock()
            .expect("the delta log mutex is not poisoned")
            .is_empty(),
        "an infer round forwards no deltas"
    );
}
