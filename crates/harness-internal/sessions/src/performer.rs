//! The inference broker over the gateway client: one model round per
//! `Chat` effect, with a section's live deltas handed to the round's
//! callback as they arrive, and the gateway's model list.
//!
//! The run's `Event` sink receives what the Engine reports once it applies
//! the round's answer (the turn, the reply, the tool calls); the deltas
//! are the live view of the reply forming, and they travel through their
//! own callback so a session can render them without a fragment ever
//! reaching the Host's recorder.

use std::fmt;

use harness_gateway_client::{GatewayClient, fetch_model_catalog};
use harness_runner::performers::{BoxFuture, InferenceBroker, OnDelta};
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};

/// Performs the Engine's `Chat` effects on a [`GatewayClient`] and lists
/// the models of the gateway at `api_root`.
///
/// The client arrives configured: the caller applies the run's request
/// limits before constructing the performer, because a `Chat` effect
/// has no limits of its own.
#[derive(Clone)]
pub struct GatewayChatPerformer {
    client: GatewayClient,
    api_root: String,
    key: String,
}

impl fmt::Debug for GatewayChatPerformer {
    /// The bearer key is never written to logs or `Debug` output.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayChatPerformer")
            .field("client", &self.client)
            .field("api_root", &self.api_root)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl GatewayChatPerformer {
    /// A performer whose rounds run on `client` and whose model list is
    /// fetched from `api_root` under `key`.
    #[must_use]
    pub fn new(client: GatewayClient, api_root: String, key: String) -> Self {
        Self {
            client,
            api_root,
            key,
        }
    }
}

impl InferenceBroker for GatewayChatPerformer {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let api_root = self.api_root.clone();
        let key = self.key.clone();
        Box::pin(async move { fetch_model_catalog(&api_root, &key).await })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let client = self.client.clone();
        Box::pin(async move {
            // An empty advertisement sends no `tools` field at all, the
            // plain chat-completions shape.
            let tools = (!tools.is_empty()).then_some(tools.as_slice());
            client
                .complete(&messages, tools, &options, |delta| {
                    // Only a section's own round has a live consumer; a
                    // nested infer's round arrives with no callback.
                    if let Some(on_delta) = &on_delta {
                        on_delta(delta);
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
