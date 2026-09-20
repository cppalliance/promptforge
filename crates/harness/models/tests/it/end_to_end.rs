//! Checkpoint 4: a fixture prompt through `prepare_run` and `drive_run`
//! with the real chat performer against the axum mock gateway and an
//! in-memory log. The prompt writes to the store, spawns a task, waits on
//! it, and asks the model once, so the record stream holds two effects
//! under the main task and a second task's events beside them. The suite
//! asserts the whole stream: the row's outcome, one answer row per
//! effect, the `Provenance` columns per task, the payloads the effects
//! and answers record, and that every logged event reached the sink.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Json, State};
use axum::http::HeaderMap;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::routing::post;
use harness_log::{RecordFilter, RecordKind, RunId, RunLog, RunOutcome, StoredRecord};
use harness_models::{GatewayChatPerformer, GatewayClient, GatewayEndpoint, SecretString};
use harness_runner::effect_loop::{SharedLog, drive_run};
use harness_runner::performers::{BoxFuture, InputPerformer};
use harness_runner::prepare::{Prepared, Services, prepare_run};
use harness_runner::spawn::spawn_tagged;
use harness_runner::test_support::mock_tag;
use promptforge_api_runtime::input::{InputError, InputOutcome};
use promptforge_api_types::cancel::CancelHandle;
use promptforge_api_types::event::Event;
use promptforge_api_types::models::{ModelDescriptor, ModelId, ThinkingMode};
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// The reply the mock gateway streams for every round.
const REPLY: &str = "hello from the mock";

/// The model name the mock gateway's chunks report.
const SERVED_MODEL: &str = "qwen3-30b";

/// The fixture: the `writer` role parked as the default, a main section
/// that writes to the store, spawns `## Child`, waits on it, and asks the
/// model about its prose, and a child section that returns at once.
const FIXTURE: &str = "---\nname: end-to-end\ndescription: the checkpoint fixture\n\
    promptforge: 0\nmodels:\n  writer: {}\n---\n\n\
    # End to End\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Main\n\n```lua\nstore.write('notes.md', 'kept')\n\
    local t = tasks.spawn('## Child')\n\
    local _first, _ok, child = tasks.when_any({ t })\nvar.child = child\n```\n\n\
    Ask the model.\n\n```lua\nreturn models.infer(prose) .. '|' .. var.child\n```\n\n\
    ## Child\n\n```lua\nreturn 'child-done'\n```\n";

/// Writes the fixture as a prompt file in `dir` and returns its path.
fn prompt_file(dir: &Path) -> PathBuf {
    let path = dir.join("agent.md");
    std::fs::write(&path, FIXTURE).expect("the fixture prompt is written");
    path
}

/// One round as the mock gateway saw it: the bearer and the request body.
type Request = (Option<String>, Value);

/// What the mock gateway saw, in arrival order.
#[derive(Clone, Default)]
struct Seen {
    requests: Arc<Mutex<Vec<Request>>>,
}

/// Renders `events` as SSE `data:` lines closed by the `[DONE]` sentinel.
fn sse_body(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// One round's stream: the reply in one content chunk, then a stop.
fn reply_stream() -> String {
    sse_body(&[
        json!({
            "model": SERVED_MODEL,
            "choices": [{ "index": 0, "delta": { "content": REPLY }, "finish_reason": null }]
        }),
        json!({
            "model": SERVED_MODEL,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ])
}

/// Serves the mock gateway on a loopback port, spawned under the
/// harness's tagged wrapper, and returns a keyed client at its `/v1`
/// root beside what it saw.
async fn mock_gateway() -> (GatewayClient, Seen) {
    async fn completions(
        State(seen): State<Seen>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> ([(axum::http::HeaderName, &'static str); 1], String) {
        let bearer = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        seen.requests.lock().unwrap().push((bearer, body));
        ([(CONTENT_TYPE, "text/event-stream")], reply_stream())
    }

    let seen = Seen::default();
    let app = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_tagged(mock_tag(), async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    (client, seen)
}

/// The fixture issues no input wait; reaching this is the test's failure.
struct NoInput;

impl InputPerformer for NoInput {
    fn wait(
        &self,
        _execution: String,
        _section: String,
    ) -> BoxFuture<Result<InputOutcome, InputError>> {
        unreachable!("the fixture issues no UserInput effect")
    }
}

/// The host's current model: what every declared role binds to.
fn current_model() -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("m").expect("the model id is valid"),
        "The host's current model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Switchable,
    )
}

/// Every record of the run, in loop order.
async fn records(log: &SharedLog, run_id: RunId) -> Vec<StoredRecord> {
    log.lock()
        .await
        .records(run_id, RecordFilter::default())
        .await
        .unwrap()
}

/// The records of `kind`, in loop order.
fn of_kind(records: &[StoredRecord], kind: RecordKind) -> Vec<&StoredRecord> {
    records
        .iter()
        .filter(|stored| stored.record.kind == kind)
        .collect()
}

/// Asserts every effect record has exactly one answer record, that the
/// answer comes after its effect, and that the two carry one provenance.
fn assert_one_answer_per_effect(records: &[StoredRecord]) {
    let effects = of_kind(records, RecordKind::Effect);
    let answers = of_kind(records, RecordKind::Answer);
    assert_eq!(effects.len(), answers.len(), "one answer per effect");
    for effect in effects {
        let id = effect
            .record
            .effect_id
            .expect("an effect record names its id");
        let matching: Vec<&&StoredRecord> = answers
            .iter()
            .filter(|answer| answer.record.effect_id == Some(id))
            .collect();
        assert_eq!(matching.len(), 1, "effect {id} has exactly one answer");
        let answer = matching[0];
        assert!(answer.seq > effect.seq, "the answer follows its effect");
        assert_eq!(answer.record.task_id, effect.record.task_id);
        assert_eq!(answer.record.task_seq, effect.record.task_seq);
    }
}

/// Asserts the `Provenance` columns over the whole record stream, the
/// parse events included: the effects and events of each task carry that
/// task's id and a `task_seq` that rises strictly in loop order, so a
/// reader can slice the stream by task and order within it, and no two
/// stamped records share a `(task_id, task_seq)`. The parse events are
/// stamped under the main task from zero by the parser; preparation
/// seeds the run's main-task counter past them, so the run's first
/// main-task record continues the parse's sequence rather than
/// restarting it.
fn assert_provenance_orders_each_task(records: &[StoredRecord]) {
    let mut last_seq: std::collections::BTreeMap<&str, u32> = std::collections::BTreeMap::new();
    let mut keys: std::collections::BTreeSet<(&str, u32)> = std::collections::BTreeSet::new();
    for stored in records {
        // An answer repeats its effect's provenance; the effect's own row
        // is the one the counter stamped.
        if stored.record.kind == RecordKind::Answer {
            continue;
        }
        let task = stored.record.task_id.as_str();
        if let Some(previous) = last_seq.get(task) {
            assert!(
                stored.record.task_seq > *previous,
                "task {task}: seq {} follows {previous} in loop order",
                stored.record.task_seq
            );
        }
        last_seq.insert(task, stored.record.task_seq);
        assert!(
            keys.insert((task, stored.record.task_seq)),
            "task {task}: seq {} is stamped once across the whole stream",
            stored.record.task_seq
        );
    }
}

/// Asserts the `parsed` parse events lead the stream as events, and that
/// the run's first main-task record continues the main task's sequence
/// where the parse left it, rather than restarting at zero.
fn assert_parse_events_lead_and_the_run_continues_their_sequence(
    records: &[StoredRecord],
    parsed: usize,
) {
    assert!(
        records[..parsed]
            .iter()
            .all(|stored| stored.record.kind == RecordKind::Event),
        "preparation records the parse events ahead of the run"
    );
    let parse_task = records[0].record.task_id.as_str();
    let first_of_run = records[parsed..]
        .iter()
        .find(|stored| stored.record.task_id == parse_task)
        .expect("the run records under the main task");
    assert_eq!(
        first_of_run.record.task_seq,
        u32::try_from(parsed).unwrap(),
        "the run's first main-task record continues the parse's sequence"
    );
}

/// Asserts the effects in loop order (the store write, then the model
/// round whose messages are `request`'s) and each one's answer payload,
/// and returns the main task's id, which both effects carry: the child
/// issued none.
fn assert_effects_and_answers(records: &[StoredRecord], request: &Value) -> String {
    let effects = of_kind(records, RecordKind::Effect);
    assert_eq!(effects.len(), 2, "one store effect, one chat effect");
    let store = effects[0];
    let chat = effects[1];
    assert_eq!(
        store.record.payload,
        json!({ "Store": { "op": { "Write": { "path": "notes.md", "contents": "kept" } } } })
    );
    assert_eq!(chat.record.payload["Chat"]["model"], "m");
    assert_eq!(chat.record.payload["Chat"]["alias"], "writer");
    assert_eq!(chat.record.payload["Chat"]["tools"], json!([]));
    assert_eq!(
        chat.record.payload["Chat"]["messages"]
            .as_array()
            .map(Vec::len),
        request["messages"].as_array().map(Vec::len),
        "the record's messages are the request's"
    );
    assert_eq!(
        store.record.task_id, chat.record.task_id,
        "both effects are the main task's"
    );
    assert!(
        chat.record.task_seq > store.record.task_seq,
        "the round follows the write within the task"
    );

    // The answers, each under its effect's id.
    let answer_for = |effect: &StoredRecord| -> Value {
        of_kind(records, RecordKind::Answer)
            .into_iter()
            .find(|answer| answer.record.effect_id == effect.record.effect_id)
            .expect("the effect is answered")
            .record
            .payload
            .clone()
    };
    assert_eq!(answer_for(store), json!({ "Store": { "Ok": "Unit" } }));
    assert_eq!(
        answer_for(chat),
        json!({ "Chat": { "Ok": {
            "model": SERVED_MODEL,
            "finish_reason": "stop",
            "reply": REPLY,
            "tool_calls": [],
        } } })
    );
    store.record.task_id.clone()
}

/// Asserts the task columns hold the main task and its one child, that
/// the child's id extends the main task's, and that the child's section
/// events are recorded under the child's own task.
fn assert_task_columns(records: &[StoredRecord], main_task: &str) {
    let mut tasks: Vec<&str> = records
        .iter()
        .map(|stored| stored.record.task_id.as_str())
        .collect();
    tasks.sort_unstable();
    tasks.dedup();
    assert_eq!(tasks.len(), 2, "the main task and its one child: {tasks:?}");
    let child_task = tasks
        .iter()
        .find(|task| **task != main_task)
        .expect("the child has its own task id");
    assert!(
        child_task.starts_with(&format!("{main_task}.")),
        "the child's id extends the main task's: {child_task}"
    );
    assert!(
        of_kind(records, RecordKind::Event)
            .iter()
            .any(|event| event.record.task_id == *child_task),
        "the child's section events are recorded under the child's task"
    );
}

#[tokio::test]
async fn a_prepared_run_drives_end_to_end_and_records_the_whole_stream() {
    let dir = tempfile::tempdir().unwrap();
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(RunLog::in_memory().await.unwrap()));
    let (client, seen) = mock_gateway().await;
    let (deltas, mut delta_rx) = mpsc::unbounded_channel();
    let services = Services {
        registry: None,
        vfs: shared_vfs::VfsRef::builder().build(),
        cancel: CancelHandle::new(),
        log: Arc::clone(&log),
        chat: Arc::new(GatewayChatPerformer::new(client, deltas)),
        input: Arc::new(NoInput),
        session_id: "session-e2e".to_owned(),
        agent: "end-to-end".to_owned(),
        model: Some(current_model()),
        ui: None,
    };

    let Prepared {
        run,
        run_id,
        performers,
        parse_events,
        ..
    } = prepare_run(&prompt_file(dir.path()), "", services)
        .await
        .expect("the fixture prepares");
    let seen_by_sink: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen_by_sink);
    let outcome = drive_run(
        run,
        performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        move |event| sink.lock().unwrap().push(event),
    )
    .await
    .expect("the drive completes");

    // The run's product: the model's reply beside the child's result.
    assert_eq!(
        outcome,
        RunOutcome::Completed {
            final_text: format!("{REPLY}|child-done"),
        }
    );
    let row = log.lock().await.run(run_id).await.unwrap();
    assert!(row.ended_at.is_some(), "the loop closes the row");
    assert_eq!(row.outcome, Some(outcome));

    // The mock gateway saw one round, keyed, for the bound model.
    let requests = seen.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "the one infer is the one round");
    let (bearer, body) = &requests[0];
    assert_eq!(bearer.as_deref(), Some("Bearer tok"));
    assert_eq!(body["model"], "m", "the round names the bound model");
    let asked = body["messages"]
        .as_array()
        .expect("the request carries messages")
        .iter()
        .any(|message| {
            message["content"]
                .as_str()
                .is_some_and(|content| content.contains("Ask the model."))
        });
    assert!(
        asked,
        "the section's prose is what the model was asked: {body}"
    );
    assert!(
        delta_rx.try_recv().is_err(),
        "a nested infer has no live consumer, so no delta reaches the sink"
    );

    // The whole record stream: events, effects, and answers.
    let records = records(&log, run_id).await;
    assert_eq!(
        records[0].record.kind,
        RecordKind::Event,
        "the stream opens with an event"
    );
    assert_eq!(
        records.last().unwrap().record.kind,
        RecordKind::Event,
        "the run's end is an event"
    );
    assert_one_answer_per_effect(&records);
    assert_parse_events_lead_and_the_run_continues_their_sequence(&records, parse_events.len());
    assert_provenance_orders_each_task(&records);

    let main_task = assert_effects_and_answers(&records, body);
    assert_task_columns(&records, &main_task);

    // Every logged event reached the sink after the parse events, in order.
    let logged: Vec<Value> = of_kind(&records, RecordKind::Event)
        .iter()
        .map(|stored| stored.record.payload.clone())
        .collect();
    let delivered: Vec<Value> = parse_events
        .iter()
        .chain(seen_by_sink.lock().unwrap().iter())
        .map(|event| serde_json::to_value(event).unwrap())
        .collect();
    assert_eq!(logged, delivered);
}
