//! The effect loop answers every `Chat` effect through the inference
//! broker: the broker receives each round's `Round`, with origin `Infer`
//! for a nested `models.infer` and `Chat` for a section's own round, the
//! loop hands it no delta callback, and the run resumes with the broker's
//! completion as each round's answer.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use harness_runner::effect_loop::drive_run;
use harness_runner::performers::{BoxFuture, InferenceBroker, OnDelta, Performers};
use harness_runner::recorder::{RecordKind, RunOutcome};
use promptforge::cancel::CancelHandle;
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::ids::RoundId;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, CompletionResult, Message, ModelBinding,
    ModelCatalog, ModelDescriptor, ModelId, ThinkingMode, ToolSchema,
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

/// One round as the broker saw it: the last message's text, the round,
/// and whether the round was handed a delta callback.
type Seen = (String, Round, bool);

/// Replies `re: <last message>` to every round and keeps each round it
/// was handed.
#[derive(Default)]
struct RecordingBroker {
    rounds: Arc<Mutex<Vec<Seen>>>,
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
        round: Round,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let asked = messages
            .last()
            .map(|message| message.content().to_owned())
            .unwrap_or_default();
        self.rounds
            .lock()
            .unwrap()
            .push((asked.clone(), round, on_delta.is_some()));
        Box::pin(async move {
            Completion::from_result(CompletionResult::Text(format!("re: {asked}")), "m")
                .map(Box::new)
        })
    }
}

/// The recording broker in the run's performers, beside what it saw.
fn recording() -> (Performers, Arc<Mutex<Vec<Seen>>>) {
    let broker = RecordingBroker::default();
    let rounds = Arc::clone(&broker.rounds);
    let mut performers = unused();
    performers.broker = Arc::new(broker);
    (performers, rounds)
}

#[tokio::test]
async fn the_broker_receives_each_rounds_round_and_no_delta_callback() {
    let (recorder, run_id) = begun_log().await;
    let (performers, rounds) = recording();

    drive_run(
        infers_then_chats(),
        performers,
        recorder,
        run_id,
        CancelHandle::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        *rounds.lock().unwrap(),
        vec![
            (
                "infer round".to_owned(),
                Round {
                    id: RoundId::new(0),
                    origin: ReplyOrigin::Infer
                },
                false
            ),
            (
                "chat round".to_owned(),
                Round {
                    id: RoundId::new(1),
                    origin: ReplyOrigin::Chat
                },
                false
            ),
        ],
        "the infer round is round 0 from `Infer`, the chat round is round 1 from `Chat`, \
         and neither is handed a callback"
    );
}

#[tokio::test]
async fn each_round_resumes_the_run_with_the_brokers_completion() {
    let (recorder, run_id) = begun_log().await;
    let (performers, _rounds) = recording();

    let outcome = drive_run(
        infers_then_chats(),
        performers,
        recorder.clone(),
        run_id,
        CancelHandle::new(),
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
