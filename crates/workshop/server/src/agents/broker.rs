//! The agent sessions' inference broker: every model round and model list
//! goes through the current Gateway generation.
//!
//! The broker holds the server's [`Registry`] and reads the gateway
//! through it on every call, as the search provider does, so a replaced
//! gateway serves the next round. It keeps the Gateway broker of the last
//! generation it served under.

use std::sync::{Mutex, PoisonError};

use harness::{BoxFuture, InferenceBroker, OnDelta};
use harness_gateway_client::{CompletionError, CompletionErrorKind, GatewayBroker};
use promptforge::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use workshop_registry::Registry;

use super::gateway::usable_gateway;

/// Serves rounds and model lists through the Gateway the server's registry
/// currently holds.
pub(crate) struct WorkshopBroker {
    /// The subsystem registry the gateway handles are read through.
    registry: Registry,
    /// The broker built for the last generation served under.
    cached: Mutex<Option<(u64, GatewayBroker)>>,
}

impl WorkshopBroker {
    /// Builds the broker over the server's subsystem registry.
    pub(crate) fn new(registry: Registry) -> Self {
        Self {
            registry,
            cached: Mutex::new(None),
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
}

impl InferenceBroker for WorkshopBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        match self.current() {
            Ok(broker) => broker.models(),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        match self.current() {
            Ok(broker) => broker.chat(binding, messages, tools, options, on_delta),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
}

#[cfg(test)]
#[path = "broker-tests.rs"]
mod tests;
