//! The `Chat` performer: one model round through the gateway client, with
//! the round's live deltas streamed to the session as they arrive.
//!
//! The run's `Event` sink receives what the engine reports once it applies
//! the round's answer (the turn, the reply, the tool calls); the deltas
//! are the live view of the reply forming, and they travel on their own
//! channel so a session can render them without a fragment ever reaching
//! the run log.

use harness_runner::performers::{BoxFuture, ChatPerformer};
use promptforge_api_runtime::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, StreamDelta, ToolSchema,
};
use tokio::sync::mpsc;

use crate::GatewayClient;

/// Where a chat round's live deltas go: the send half of an unbounded
/// channel the session drains.
///
/// Unbounded so a slow consumer never stalls a model round; the volume is
/// bounded already by the run's response byte cap. A closed receiver
/// drops the deltas rather than failing the round, since the completed
/// reply travels in the effect's answer regardless.
pub type DeltaSink = mpsc::UnboundedSender<StreamDelta>;

/// Performs the engine's `Chat` effects on a [`GatewayClient`], streaming
/// each delta of a section's own round to a [`DeltaSink`].
///
/// The client arrives configured: the caller applies the run's request
/// limits before constructing the performer, because a `Chat` effect
/// has no limits of its own.
#[derive(Clone, Debug)]
pub struct GatewayChatPerformer {
    client: GatewayClient,
    deltas: DeltaSink,
}

impl GatewayChatPerformer {
    /// A performer over `client` whose live deltas go to `deltas`.
    #[must_use]
    pub fn new(client: GatewayClient, deltas: DeltaSink) -> Self {
        Self { client, deltas }
    }
}

impl ChatPerformer for GatewayChatPerformer {
    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        stream: bool,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let client = self.client.clone();
        let deltas = self.deltas.clone();
        Box::pin(async move {
            // An empty advertisement sends no `tools` field at all, the
            // plain chat-completions shape.
            let tools = (!tools.is_empty()).then_some(tools.as_slice());
            client
                .complete(&messages, tools, &options, |delta| {
                    // Only a section's own round has a live consumer; a
                    // nested infer's fragments drop here. A closed sink is
                    // a session that stopped listening, not a failure.
                    if stream {
                        let _ = deltas.send(delta);
                    }
                })
                .await
                .map(Box::new)
        })
    }
}

#[cfg(test)]
#[path = "performer-tests.rs"]
mod tests;
