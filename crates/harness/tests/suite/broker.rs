//! The inference broker a Host hands `Harness::new`: a section's own
//! round reaches it with origin `Chat`, and a nested `models.infer` round
//! with origin `Infer`, each under its run-wide round id.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};

use harness::plugin::{HostContext, HostServices};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, RunRequest};
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::ids::RoundId;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, CompletionResult, Message, ModelBinding,
    ModelCatalog, ModelDescriptor, ModelId, ThinkingMode, ToolSchema,
};

use crate::support::Clock;

/// The one model the scripted broker lists and the Host selects.
const MODEL: &str = "scripted-model";

/// A program that infers once and then chats once: the nested round
/// first, then the section's own round over its message list.
const INFERS_THEN_CHATS: &str = "---\nname: rounds\ndescription: infers then chats\n\
    promptforge: 0\nmodels:\n  writer: {}\n---\n\n\
    # Rounds\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\n\
    local inferred = models.infer('infer round')\n\
    local msgs = messages.new()\n\
    msgs:user('chat round')\n\
    models.loop(msgs)\n\
    return inferred .. '|' .. msgs[#msgs].content\n\
    ```\n";

/// Lists its catalog, notes each round it serves, and replies
/// `re: <last message>` to every round.
struct ScriptedBroker {
    catalog: ModelCatalog,
    rounds: Mutex<Vec<Round>>,
}

impl InferenceBroker for ScriptedBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { Ok(catalog) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.rounds
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(round);
        let asked = messages
            .last()
            .map(|message| message.content().to_owned())
            .unwrap_or_default();
        Box::pin(async move {
            Completion::from_result(CompletionResult::Text(format!("re: {asked}")), MODEL)
                .map(Box::new)
        })
    }
}

#[tokio::test]
async fn a_sections_round_reaches_the_broker_as_chat_and_a_nested_infer_round_as_infer() {
    let catalog = ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway(MODEL).expect("a literal model name is valid"),
        "the scripted model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Never,
    )])
    .expect("one model is a valid catalog");
    let broker = Arc::new(ScriptedBroker {
        catalog,
        rounds: Mutex::new(Vec::new()),
    });
    let harness = Harness::new(
        Arc::new(MemoryRecorder::new()),
        broker.clone(),
        Arc::new(Clock),
        Arc::new(HostContext::new(HostServices::new())),
        HostServices::new(),
    );
    let report = harness
        .run(RunRequest {
            name: "rounds-1".to_owned(),
            source: INFERS_THEN_CHATS.to_owned(),
            args: String::new(),
            input_text: None,
            vfs: VfsRef::default(),
            host: HostSnapshot {
                selected_model: Some(MODEL.to_owned()),
                ..HostSnapshot::default()
            },
        })
        .await
        .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "re: infer round|re: chat round".to_owned()
        }
    );
    assert_eq!(
        *broker.rounds.lock().expect("the round log is healthy"),
        [
            Round {
                id: RoundId::new(0),
                origin: ReplyOrigin::Infer,
            },
            Round {
                id: RoundId::new(1),
                origin: ReplyOrigin::Chat,
            },
        ],
        "each round reaches the broker numbered in dispatch order, under the path that sent it"
    );
}
