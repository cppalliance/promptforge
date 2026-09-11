//! Typed asynchronous event collection for one supervisor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use promptforge_agent::AgentError;
use tokio::sync::{mpsc, watch};

use crate::catalog::CatalogBus;
use crate::gateway_binding::{GatewayBinding, GatewaySnapshot};

use super::catalog::{CatalogEvent, current_catalog_event, next_catalog_event};
use super::transition::{RunId, SupervisorEvent};

/// One owned run future paired with its reducer identity.
pub(super) type RunFuture = Pin<Box<dyn Future<Output = (RunId, Result<(), AgentError>)> + Send>>;

/// Runtime data collected alongside one pure supervisor event.
pub(super) enum CollectedEvent {
    Supervisor(SupervisorEvent),
    Catalog(CatalogEvent),
    Gateway {
        event: SupervisorEvent,
        snapshot: Arc<GatewaySnapshot>,
    },
    Run {
        run: RunId,
        result: Result<(), AgentError>,
    },
}

/// External event sources owned by one supervisor.
pub(super) struct EventCollector {
    lifecycle: mpsc::UnboundedReceiver<SupervisorEvent>,
    cancellations: mpsc::Receiver<SupervisorEvent>,
    catalog: CatalogBus,
    catalog_generation: watch::Receiver<u64>,
    gateway: GatewayBinding,
    gateway_generation: watch::Receiver<u64>,
}

impl EventCollector {
    /// Subscribes before loading initial snapshots so replacements cannot
    /// disappear between those operations.
    pub(super) fn new(
        lifecycle: mpsc::UnboundedReceiver<SupervisorEvent>,
        cancellations: mpsc::Receiver<SupervisorEvent>,
        catalog: CatalogBus,
        gateway: GatewayBinding,
    ) -> (Self, CatalogEvent, Arc<GatewaySnapshot>) {
        let mut catalog_generation = catalog.subscribe_chat_generation();
        let gateway_generation = gateway.subscribe();
        let initial_catalog = current_catalog_event(&catalog, &mut catalog_generation);
        let initial_gateway = gateway.snapshot();
        (
            Self {
                lifecycle,
                cancellations,
                catalog,
                catalog_generation,
                gateway,
                gateway_generation,
            },
            initial_catalog,
            initial_gateway,
        )
    }

    /// Waits for the next typed event, prioritizing synchronous lifecycle
    /// events that causally precede a run wake or watched replacement.
    pub(super) async fn next(
        &mut self,
        active_models: Option<&[serde_json::Value]>,
        active_run: Option<&mut RunFuture>,
    ) -> CollectedEvent {
        if let Some(run) = active_run {
            tokio::select! {
                biased;
                event = next_lifecycle_event(
                    &mut self.lifecycle,
                    &mut self.cancellations,
                ) => {
                    CollectedEvent::Supervisor(event)
                }
                catalog = next_catalog_event(
                    &self.catalog,
                    &mut self.catalog_generation,
                    active_models,
                ) => CollectedEvent::Catalog(catalog),
                gateway = next_gateway_event(
                    &self.gateway,
                    &mut self.gateway_generation,
                ) => gateway,
                result = run.as_mut() => {
                    let (run, result) = result;
                    CollectedEvent::Run { run, result }
                }
            }
        } else {
            tokio::select! {
                biased;
                event = next_lifecycle_event(
                    &mut self.lifecycle,
                    &mut self.cancellations,
                ) => {
                    CollectedEvent::Supervisor(event)
                }
                catalog = next_catalog_event(
                    &self.catalog,
                    &mut self.catalog_generation,
                    active_models,
                ) => CollectedEvent::Catalog(catalog),
                gateway = next_gateway_event(
                    &self.gateway,
                    &mut self.gateway_generation,
                ) => gateway,
            }
        }
    }
}

/// Waits for the host's next complete Gateway snapshot.
async fn next_gateway_event(
    gateway: &GatewayBinding,
    generation: &mut watch::Receiver<u64>,
) -> CollectedEvent {
    if generation.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
    let snapshot = gateway.snapshot();
    CollectedEvent::Gateway {
        event: SupervisorEvent::GatewayGeneration(snapshot.generation()),
        snapshot,
    }
}

/// Waits for the next synchronous lifecycle event, polling the guaranteed
/// queue before the bounded cancellation queue. Cross-channel ordering is
/// not load-bearing: a cancellation is valid in any reducer phase, and a
/// close or settlement processed late lands on a phase that ignores it.
async fn next_lifecycle_event(
    lifecycle: &mut mpsc::UnboundedReceiver<SupervisorEvent>,
    cancellations: &mut mpsc::Receiver<SupervisorEvent>,
) -> SupervisorEvent {
    tokio::select! {
        biased;
        event = lifecycle.recv() => match event {
            Some(event) => event,
            None => std::future::pending().await,
        },
        event = cancellations.recv() => match event {
            Some(event) => event,
            None => std::future::pending().await,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_collector_drains_the_guaranteed_and_bounded_lifecycle_queues() {
        let (events, guaranteed) = mpsc::unbounded_channel();
        let (cancellations, bounded) = mpsc::channel(1);
        let (mut collector, _initial_catalog, _initial_gateway) = EventCollector::new(
            guaranteed,
            bounded,
            CatalogBus::default(),
            GatewayBinding::new("http://127.0.0.1:1", "").expect("the test binding builds"),
        );

        events
            .send(SupervisorEvent::Close)
            .expect("guaranteed send");
        cancellations
            .try_send(SupervisorEvent::OperatorCancellation)
            .expect("bounded send");

        assert!(
            matches!(
                collector.next(None, None).await,
                CollectedEvent::Supervisor(SupervisorEvent::Close)
            ),
            "the guaranteed queue is polled first"
        );
        assert!(
            matches!(
                collector.next(None, None).await,
                CollectedEvent::Supervisor(SupervisorEvent::OperatorCancellation)
            ),
            "the bounded cancellation queue drains through the same collector"
        );
    }
}
