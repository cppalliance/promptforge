//! Shared fixtures: an offline broker, and a per-run Harness built from
//! what a conversation hands it.

// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking with the invariant
// named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::sync::Arc;

use harness::capability::{CapabilityRegistry, HostServices, UserInput};
use harness::record::MemoryRecorder;
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, OnDelta, RunRequest};
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding,
    ModelCatalog, ToolSchema,
};
use workshop_agents::{Conversation, TokioTimer};

/// Lists no model and refuses every round as `Unavailable`.
pub(crate) struct Offline;

impl InferenceBroker for Offline {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
        _on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// A prompt whose one section runs `lua`, declaring the operator-input
/// capability.
pub(crate) fn prompt(lua: &str) -> String {
    format!(
        "---\nname: fixture\ndescription: a conversation fixture\npromptforge: 0\n\
         capabilities:\n  - promptforge/user-input\n---\n\n\
         # Fixture\n\n## Conversation\n\n```lua\n{lua}\n```\n"
    )
}

/// The run's Harness, recording into `recorder` through the
/// conversation's tee, and its request over `source`.
pub(crate) fn harness_for(
    conversation: &Conversation,
    recorder: Arc<MemoryRecorder>,
    source: String,
) -> (Harness, RunRequest) {
    let mut capabilities = CapabilityRegistry::new();
    capabilities
        .register(Arc::new(UserInput::new()))
        .expect("an empty registry takes the capability");
    let harness = Harness::new(
        conversation.recorder(recorder),
        Arc::new(Offline),
        Arc::new(TokioTimer),
        capabilities,
        conversation.services(&HostServices::new()),
    );
    let request = RunRequest {
        name: conversation.id().to_string(),
        source,
        args: String::new(),
        input_text: None,
        vfs: VfsRef::default(),
        host: HostSnapshot::default(),
    };
    (harness, request)
}
