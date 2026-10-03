//! The effect loop answers every `Chat` effect through the inference
//! broker: a section's own chat round is handed the run's delta callback,
//! a nested `models.infer` round is handed none, and the run resumes with
//! the broker's completion as each round's answer.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use harness_runner::effect_loop::drive_run;
use harness_runner::performers::{BoxFuture, InferenceBroker, OnDelta, Performers};
use harness_runner::recorder::{RecordKind, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, CompletionResult, Message, ModelBinding,
    ModelCatalog, ModelDescriptor, ModelId, StreamDelta, ThinkingMode, ToolSchema,
};
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, Run, RunContext};
use serde_json::{Value, json};

use super::begun_log;
use crate::support::{EXECUTION, unused};

/// A program that infers once and then chats once: the nested round
/// first, then the section's own round over its message list.
const INFERS_THEN_CHATS: &str = "---\nname: runner-broker\ndescription: infers then chats\n\
    promptforge: 0\nmodels:\n  writer: {}\n---\n\n\
    # Broker\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\n\
    local inferred = models.infer('infer round')\n\
    local msgs = messages.new()\n\
    msgs:user('chat round')\n\
    models.loop(msgs)\n\
    return inferred .. '|' .. msgs[#msgs].content\n\
    ```\n";

/// A run over [`INFERS_THEN_CHATS`] with every declared role bound to one
/// current model.
fn infers_then_chats() -> Run {
    let (prompt, _parse_events) = Prompt::parse(INFERS_THEN_CHATS, EXECUTION);
    let prompt = Arc::new(prompt.expect("the broker fixture parses"));
    let model = ModelDescriptor::new(
        ModelId::gateway("m").expect("the model id is valid"),
        "The host's current model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Never,
    );
    let ctx = RunContext::new(EXECUTION, 7, Timestamp::UNIX_EPOCH).model(model);
    let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
    assert!(
        requirements.is_satisfied(),
        "the bound model satisfies the fixture's one role: {requirements:?}"
    );
    Run::new(prompt, "", ctx)
}

/// One round as the broker saw it: the last message's text, and whether
/// the round was handed a delta callback.
type Round = (String, bool);

/// What a fake keeps, shared with the test that reads it.
type Kept<T> = Arc<Mutex<Vec<T>>>;

/// Replies `re: <last message>` to every round, sending the reply as one
/// text delta first when the round has a callback, and keeps each round
/// it was handed.
#[derive(Default)]
struct RecordingBroker {
    rounds: Kept<Round>,
}

impl InferenceBroker for RecordingBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        unreachable!("the effect loop never lists models")
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let asked = messages
            .last()
            .map(|message| message.content().to_owned())
            .unwrap_or_default();
        self.rounds
            .lock()
            .unwrap()
            .push((asked.clone(), on_delta.is_some()));
        Box::pin(async move {
            let reply = format!("re: {asked}");
            if let Some(on_delta) = on_delta {
                on_delta(StreamDelta::Text(reply.clone()));
            }
            Completion::from_result(CompletionResult::Text(reply), "m").map(Box::new)
        })
    }
}

/// The recording broker and a delta callback that keeps every delta, in
/// the run's performers, beside what each saw.
fn recording() -> (Performers, Kept<Round>, Kept<StreamDelta>) {
    let broker = RecordingBroker::default();
    let rounds = Arc::clone(&broker.rounds);
    let deltas: Kept<StreamDelta> = Arc::default();
    let kept = Arc::clone(&deltas);
    let mut performers = unused();
    performers.broker = Arc::new(broker);
    performers.on_delta = Arc::new(move |delta| kept.lock().unwrap().push(delta));
    (performers, rounds, deltas)
}

#[tokio::test]
async fn a_sections_chat_round_gets_the_runs_callback_and_a_nested_infer_round_gets_none() {
    let (recorder, run_id) = begun_log().await;
    let (performers, rounds, deltas) = recording();

    drive_run(
        infers_then_chats(),
        performers,
        recorder,
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(
        *rounds.lock().unwrap(),
        vec![
            ("infer round".to_owned(), false),
            ("chat round".to_owned(), true),
        ],
        "the infer round is handed no callback and the section's chat round is"
    );
    assert_eq!(
        *deltas.lock().unwrap(),
        vec![StreamDelta::Text("re: chat round".to_owned())],
        "the chat round's delta reached the run's callback, and the infer round sent none"
    );
}

#[tokio::test]
async fn each_round_resumes_the_run_with_the_brokers_completion() {
    let (recorder, run_id) = begun_log().await;
    let (performers, _rounds, _deltas) = recording();

    let outcome = drive_run(
        infers_then_chats(),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(
        outcome,
        RunOutcome::Completed {
            final_text: "re: infer round|re: chat round".to_owned()
        },
        "the program saw each round's reply as the broker wrote it"
    );
    let replies: Vec<Value> = recorder
        .records(run_id)
        .iter()
        .filter(|record| record.kind == RecordKind::Answer)
        .map(|record| record.payload["Chat"]["Ok"]["reply"].clone())
        .collect();
    assert_eq!(
        replies,
        vec![json!("re: infer round"), json!("re: chat round")],
        "each round's answer record holds the broker's completion"
    );
}
