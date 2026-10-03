//! The Gateway's inference broker: one model round per `Chat` effect on
//! the Gateway client, with a section's live deltas handed to the round's
//! callback as they arrive, and the Gateway's model list.
//!
//! The run's `Event` sink receives what the Engine reports once it applies
//! the round's answer (the turn, the reply, the tool calls); the deltas
//! are the live view of the reply forming, and they travel through their
//! own callback so a session can render them without a fragment ever
//! reaching the Host's recorder.

use std::fmt;

use harness::{BoxFuture, InferenceBroker, OnDelta};
use promptforge::RunLimits;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};

use crate::catalog::fetch_model_catalog;
use crate::config::{GatewayEndpoint, SecretString};
use crate::transport::GatewayChat;

/// The [`InferenceBroker`] a Host hands the Harness to reach the Gateway:
/// it performs the Engine's `Chat` effects on a [`GatewayChat`] and lists
/// the Gateway's models through [`fetch_model_catalog`], both at one API
/// root under one bearer key.
///
/// Every round runs under the Engine's default run limits: the per-receive
/// timeout and the response byte cap of [`RunLimits::new`].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
///
/// use harness::InferenceBroker;
/// use harness_gateway_client::{GatewayBroker, GatewayEndpoint, SecretString};
///
/// let broker = GatewayBroker::new(
///     GatewayEndpoint::new("http://127.0.0.1:8081/v1")?,
///     SecretString::new("bearer-token")?,
/// );
/// assert!(!format!("{broker:?}").contains("bearer-token"));
/// // What a Host passes to `harness::Harness::new`.
/// let broker: Arc<dyn InferenceBroker> = Arc::new(broker);
/// # let _ = broker;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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
    /// A broker whose rounds and model list reach the Gateway API root
    /// `endpoint` (the OpenAI-compatible `/v1` root) under `key`.
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
#[path = "broker-tests.rs"]
mod tests;
