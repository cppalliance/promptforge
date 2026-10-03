//! The agent conversations' inference brokers: [`WorkshopBroker`], which
//! sends every model round and model list through the current Gateway
//! generation, and [`RunBroker`], one run's view of it, which streams the
//! run's own rounds into its conversation.
//!
//! The Workshop broker holds the server's [`Registry`] and reads the
//! gateway through it on every call, as the search provider does, so a
//! replaced gateway serves the next round. It keeps the Gateway broker of
//! the last generation it served under. A model list first waits until
//! the menu's catalog holds a chat-capable model, so a conversation
//! launched before the Gateway's catalog arrives starts its run only once
//! it can bind one.

use std::sync::{Arc, Mutex, PoisonError};

use harness::{BoxFuture, InferenceBroker};
use harness_gateway_client::{CompletionError, CompletionErrorKind, GatewayBroker, StreamDelta};
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use workshop_agents::{Conversation, DeltaKind};
use workshop_menu::{CatalogBus, MenuHandles};
use workshop_registry::Registry;

use super::gateway::usable_gateway;

/// Serves rounds and model lists through the Gateway the server's registry
/// currently holds.
#[derive(Clone)]
pub(crate) struct WorkshopBroker {
    /// The subsystem registry the gateway and menu handles are read
    /// through.
    registry: Registry,
    /// The broker built for the last generation served under, shared with
    /// the model lists still waiting.
    cached: Arc<Mutex<Option<(u64, GatewayBroker)>>>,
}

impl WorkshopBroker {
    /// Builds the broker over the server's subsystem registry.
    pub(crate) fn new(registry: Registry) -> Self {
        Self {
            registry,
            cached: Arc::new(Mutex::new(None)),
        }
    }

    /// The Gateway broker for the current generation, built on the first
    /// call under it. No usable gateway fails as `Unavailable` with the
    /// kind's fixed phrase, so no URL or key reaches the message.
    fn current(&self) -> Result<GatewayBroker, CompletionError> {
        let unavailable = CompletionErrorKind::Unavailable;
        let gateway = usable_gateway(&self.registry)
            .ok_or_else(|| CompletionError::new(unavailable, unavailable.phrase()))?;
        // A poisoned lock holds a pair written whole by one store.
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((generation, broker)) = cached.as_ref()
            && *generation == gateway.generation
        {
            return Ok(broker.clone());
        }
        let broker = GatewayBroker::new(gateway.endpoint, gateway.key);
        *cached = Some((gateway.generation, broker.clone()));
        Ok(broker)
    }

    /// Resolves once the menu's catalog holds a chat-capable model, at
    /// once when no menu is registered.
    pub(crate) async fn chat_model_ready(&self) {
        if let Some(menu) = self.registry.state::<MenuHandles>() {
            chat_model_published(menu.catalog()).await;
        }
    }

    /// Runs one round through the current generation's Gateway broker,
    /// handing `on_piece` each live piece of the reply as it arrives.
    pub(crate) fn chat_streaming(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        on_piece: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        match self.current() {
            Ok(broker) => broker.chat_streaming(binding, messages, tools, options, on_piece),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
}

impl InferenceBroker for WorkshopBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let broker = self.clone();
        Box::pin(async move {
            broker.chat_model_ready().await;
            broker.current()?.models().await
        })
    }

    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        match self.current() {
            Ok(broker) => broker.chat(binding, messages, tools, options, round),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
}

/// One run's broker over the Workshop broker: a section's own round
/// streams its live pieces into the run's conversation, stamped with the
/// round's id, and every other call goes through as it came.
pub(crate) struct RunBroker {
    inner: WorkshopBroker,
    conversation: Conversation,
}

impl RunBroker {
    /// The broker for `conversation`'s run over `inner`.
    pub(crate) fn new(inner: WorkshopBroker, conversation: Conversation) -> Self {
        Self {
            inner,
            conversation,
        }
    }
}

impl InferenceBroker for RunBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        self.inner.models()
    }

    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        // Only a section's own round has a live reader; a nested
        // `models.infer` round is read whole from its answer.
        if round.origin != ReplyOrigin::Chat {
            return self.inner.chat(binding, messages, tools, options, round);
        }
        let conversation = self.conversation.clone();
        let on_piece = Arc::new(move |piece: StreamDelta| {
            let (kind, content) = match piece {
                StreamDelta::Text(text) => (DeltaKind::Text, text),
                StreamDelta::Reasoning(text) => (DeltaKind::Reasoning, text),
                // The enum is non-exhaustive across the crate seam; a
                // future side channel has no delta kind yet and stays
                // unshown.
                _ => return,
            };
            conversation.publish_delta(round.id, kind, content);
        });
        self.inner
            .chat_streaming(binding, messages, tools, options, on_piece)
    }
}

/// Waits until `catalog` holds a chat-capable model. The watch is taken
/// before the first read, so a generation published between the two still
/// wakes the wait.
async fn chat_model_published(catalog: &CatalogBus) {
    let mut generation = catalog.subscribe_chat_generation();
    while catalog.latest_chat().is_none() {
        if generation.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "broker-tests.rs"]
mod tests;
