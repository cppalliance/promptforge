//! The Gateway's inference broker: one model round per `Chat` effect on
//! the Gateway client, and the Gateway's model list.
//!
//! The Harness takes only a round's finished reply, so the broker's
//! `InferenceBroker` round returns it whole. A Host that shows the reply
//! as it forms runs the round through [`GatewayBroker::chat_streaming`]
//! instead, which hands each live piece to the Host's callback as it
//! arrives, so no fragment ever reaches the Host's recorder. Either way
//! the completion names the model the round's options sent it to, so a
//! Host that routes a round to another model sees that model on its reply.

use std::fmt;
use std::sync::Arc;

use harness::{BoxFuture, InferenceBroker};
use promptforge::RunLimits;
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};

use crate::catalog::fetch_model_catalog;
use crate::config::{GatewayEndpoint, SecretString};
use crate::transport::GatewayChat;
use crate::wire::delta::StreamDelta;

/// A model broker that sends the Harness's model rounds to the PromptForge
/// Gateway and lists the Gateway's models.
///
/// A Host passes this broker to the Harness as its [`InferenceBroker`].
/// The broker performs each `Chat` effect from the Engine as one model
/// round on a [`GatewayChat`]. It lists the Gateway's models through
/// [`fetch_model_catalog`]. Both kinds of request go to the same API root
/// and authenticate with the same bearer key.
///
/// Every round uses the Engine's default run limits from
/// [`RunLimits::new`]: the wait limit for each part of the response and
/// the cap on response size in bytes.
///
/// The broker's futures, [`models`](InferenceBroker::models) included, need
/// a tokio runtime with its reactor and timer. A Host that uses this broker
/// must therefore await `Harness::run` inside such a runtime. The Harness
/// itself needs no runtime.
#[derive(Clone)]
pub struct GatewayBroker {
    client: GatewayChat,
    endpoint: GatewayEndpoint,
    key: SecretString,
}

impl fmt::Debug for GatewayBroker {
    /// The bearer key is never written to logs or `Debug` output.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayBroker")
            .field("client", &self.client)
            .field("api_root", &self.endpoint.url())
            .field("key", &"<redacted>")
            .finish()
    }
}

impl GatewayBroker {
    /// Creates a broker that reaches the Gateway at `endpoint` with the
    /// bearer key `key`.
    ///
    /// `endpoint` is the Gateway's OpenAI-compatible API root, the `/v1`
    /// path. Model rounds and model-list requests both go to it.
    #[must_use]
    pub fn new(endpoint: GatewayEndpoint, key: SecretString) -> GatewayBroker {
        let limits = RunLimits::new();
        let client = GatewayChat::new(endpoint.clone(), key.clone())
            .with_request_limits(limits.timeout(), limits.response_bytes());
        GatewayBroker {
            client,
            endpoint,
            key,
        }
    }

    /// Runs one model round and passes each piece of the reply to
    /// `on_piece` as it arrives.
    ///
    /// The round is the same one `InferenceBroker::chat` runs. `on_piece`
    /// receives the pieces in the order the stream delivers them. The
    /// returned completion holds the whole reply.
    ///
    /// `on_piece` is called inline as each piece is read, so it must not
    /// block.
    pub fn chat_streaming(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        on_piece: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.round(messages, tools, options, move |piece| on_piece(piece))
    }

    /// One round on the client, handing each live piece to `on_piece`.
    fn round(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        on_piece: impl Fn(StreamDelta) + Send + Sync + 'static,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let client = self.client.clone();
        Box::pin(async move {
            // An empty advertisement sends no `tools` field at all, the
            // plain chat-completions shape.
            let tools = (!tools.is_empty()).then_some(tools.as_slice());
            client
                .complete(&messages, tools, &options, on_piece)
                .await
                .map(Box::new)
        })
    }
}

impl InferenceBroker for GatewayBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let api_root = self.endpoint.url().to_owned();
        let key = self.key.clone();
        Box::pin(async move { fetch_model_catalog(&api_root, key.expose()).await })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        _round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.round(messages, tools, options, |_| {})
    }
}

#[cfg(test)]
#[path = "broker-tests.rs"]
mod tests;
