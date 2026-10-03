//! The inference broker a Host hands `Harness::new`: a section's
//! streaming round reaches the session's live deltas, and a `models.infer`
//! round publishes none.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::MemoryRecorder;
use harness::{
    BoxFuture, DeltaKind, Harness, HarnessConfig, HostSnapshot, InferenceBroker, LaunchRequest,
    OnDelta, SessionState,
};
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, CompletionResult, Message, ModelBinding,
    ModelCatalog, ModelDescriptor, ModelId, StreamDelta, ThinkingMode, ToolSchema,
};

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

/// Lists its catalog and replies `re: <last message>` to every round,
/// sending the reply as one text delta first when the round has a
/// callback.
struct ScriptedBroker {
    catalog: ModelCatalog,
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
        _round: Round,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let asked = messages
            .last()
            .map(|message| message.content().to_owned())
            .unwrap_or_default();
        Box::pin(async move {
            let reply = format!("re: {asked}");
            if let Some(on_delta) = on_delta {
                on_delta(StreamDelta::Text(reply.clone()));
            }
            Completion::from_result(CompletionResult::Text(reply), MODEL).map(Box::new)
        })
    }
}

#[tokio::test]
async fn a_sections_streaming_round_reaches_the_sessions_deltas_and_an_infer_round_publishes_none()
{
    let dir = tempfile::tempdir().expect("a temporary directory");
    let agents = dir.path().join("agents");
    std::fs::create_dir_all(&agents).expect("the agents directory creates");
    std::fs::write(agents.join("rounds.md"), INFERS_THEN_CHATS).expect("the agent writes");
    let catalog = ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway(MODEL).expect("a literal model name is valid"),
        "the scripted model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Never,
    )])
    .expect("one model is a valid catalog");
    let harness = Harness::new(
        HarnessConfig {
            agents_path: agents,
        },
        Arc::new(MemoryRecorder::new()),
        Arc::new(ScriptedBroker { catalog }),
        CapabilityRegistry::new(),
        HostServices::new(),
    );
    harness.set_host(HostSnapshot {
        selected_model: Some(MODEL.to_owned()),
        ..HostSnapshot::default()
    });

    let session = harness
        .launch(LaunchRequest {
            agent: "rounds".to_owned(),
            args: String::new(),
            input_text: None,
        })
        .await
        .expect("the discovered agent launches");
    // The session is running, but this single-threaded runtime has not
    // polled it yet, so this receiver sees every delta of the run.
    let mut deltas = session.subscribe_deltas();
    let mut state = session.subscribe_state();
    tokio::time::timeout(
        Duration::from_secs(10),
        state.wait_for(|current| *current == SessionState::Closed),
    )
    .await
    .expect("the program returns in time")
    .expect("the session's state watch stays open");

    let mut seen = Vec::new();
    while let Ok(delta) = deltas.try_recv() {
        seen.push((delta.kind, delta.content));
    }
    assert_eq!(
        seen,
        [(DeltaKind::Text, "re: chat round".to_owned())],
        "the section's chat round streams its reply, and the infer round streams nothing"
    );
}
