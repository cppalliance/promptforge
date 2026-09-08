//! Execution of reducer-selected supervisor effects.

use std::sync::Arc;

use promptforge_agent::{AgentConfig, AgentError, AgentLimits, run_agent_with_client};
use promptforge_core_support::observe::Observer;
use promptforge_model_client::client::{GatewayClient as ModelClient, StreamDelta};
use promptforge_store::StoreRef;
use promptforge_tools::ToolCatalog;

use crate::catalog::ChatCatalog;
use crate::gateway_binding::GatewaySnapshot;
use crate::protocol::Activity;

use super::events::{CollectedEvent, EventCollector, RunFuture};
use super::transition::{
    CancelOrigin, CatalogDisposition, CloseReason, HistoryEffect, RelaunchEffect, RunCompletion,
    RunId, SupervisorEffect, SupervisorEvent,
};
use crate::session_agents::{
    AgentSession, SessionHost, SessionObserver, build_model_catalog, delta_stamp, ui_provider,
};

/// The result of executing one reducer-selected effect.
pub(super) enum EffectOutcome {
    Continue,
    Event(SupervisorEvent),
    Close,
}

/// Immutable resources reused by each reducer-selected relaunch.
struct RunFactory {
    session: Arc<AgentSession>,
    tools: ToolCatalog,
    store: StoreRef,
    observer: Arc<dyn Observer>,
    on_delta: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    ui: Arc<dyn Fn() -> serde_json::Value + Send + Sync>,
}

impl RunFactory {
    /// Builds reusable run resources for one session.
    fn new(session: Arc<AgentSession>, tools: ToolCatalog, host: &SessionHost) -> Self {
        let observer: Arc<dyn Observer> = Arc::new(SessionObserver {
            log: Arc::clone(&session.log),
            rounds: Arc::clone(&session.rounds),
            push: host.push.clone(),
            backoff: host.backoff.clone(),
            errors: session.errors.clone(),
            lifecycle: Arc::clone(&session.lifecycle),
        });
        Self {
            on_delta: delta_stamp(&session, &host.push),
            ui: ui_provider(&host.menu, &host.workspace),
            session,
            tools,
            store: StoreRef::memory(),
            observer,
        }
    }

    /// Builds one run over retained history and frozen bindings.
    fn launch(&self, run: RunId, models: Vec<serde_json::Value>, client: ModelClient) -> RunFuture {
        let source = self.session.source.clone();
        let tools = self.tools.clone();
        let models = build_model_catalog(Some(models));
        let store = self.store.clone();
        let config = AgentConfig {
            name: self.session.agent.clone(),
            execution: self.session.id.clone(),
            observer: Arc::clone(&self.observer),
            cancel: self.session.arm_cancel(run),
            event_log: Some(Arc::clone(&self.session.log) as _),
            on_delta: Some(Arc::clone(&self.on_delta)),
            ui: Some(Arc::clone(&self.ui)),
            limits: AgentLimits::default(),
        };
        Box::pin(async move {
            let result =
                run_agent_with_client(&source, &tools, &models, &store, config, Some(client)).await;
            (run, result)
        })
    }
}

/// Mutable runtime bindings and the currently executing run.
pub(super) struct EffectExecutor {
    session: Arc<AgentSession>,
    host: SessionHost,
    factory: RunFactory,
    latest_catalog: Option<ChatCatalog>,
    active_catalog: Option<ChatCatalog>,
    latest_gateway: Arc<GatewaySnapshot>,
    active_gateway: Option<Arc<GatewaySnapshot>>,
    active_run: Option<RunFuture>,
}

impl EffectExecutor {
    /// Creates the executor from snapshots collected after subscriptions.
    pub(super) fn new(
        session: Arc<AgentSession>,
        host: SessionHost,
        tools: ToolCatalog,
        initial_catalog: Option<ChatCatalog>,
        initial_gateway: Arc<GatewaySnapshot>,
    ) -> Self {
        Self {
            factory: RunFactory::new(Arc::clone(&session), tools, &host),
            session,
            host,
            latest_catalog: initial_catalog,
            active_catalog: None,
            latest_gateway: initial_gateway,
            active_gateway: None,
            active_run: None,
        }
    }

    /// Collects the next event using the currently frozen run bindings.
    pub(super) async fn next_event(&mut self, collector: &mut EventCollector) -> CollectedEvent {
        let active_models = self
            .active_catalog
            .as_ref()
            .map(|catalog| catalog.models.as_slice());
        collector
            .next(active_models, self.active_run.as_mut())
            .await
    }

    /// Applies collected runtime data and returns only the pure event.
    pub(super) fn event_from(&mut self, collected: CollectedEvent) -> SupervisorEvent {
        match collected {
            CollectedEvent::Supervisor(event) => event,
            CollectedEvent::Catalog(catalog) => {
                let event = catalog.event;
                if matches!(
                    event,
                    SupervisorEvent::CatalogGeneration {
                        disposition: CatalogDisposition::Retained,
                        ..
                    }
                ) && self.active_catalog.is_some()
                {
                    self.active_catalog.clone_from(&catalog.snapshot);
                }
                self.latest_catalog = catalog.snapshot;
                event
            }
            CollectedEvent::Gateway { event, snapshot } => {
                self.latest_gateway = snapshot;
                event
            }
            CollectedEvent::Run { run, result } => {
                self.active_run.take();
                self.session.finish_run(run);
                run_completion_event(run, result, &self.session, &self.host)
            }
        }
    }

    /// Executes one typed effect without making transition decisions.
    pub(super) fn execute(&mut self, effect: SupervisorEffect) -> EffectOutcome {
        match effect {
            SupervisorEffect::Wait(_) | SupervisorEffect::Preserve(_) => EffectOutcome::Continue,
            SupervisorEffect::Cancel(origin) => {
                report_cancel_origin(&self.session, origin);
                self.session.cancel_current_run();
                EffectOutcome::Continue
            }
            SupervisorEffect::Relaunch(relaunch) => self.relaunch(relaunch),
            SupervisorEffect::Close(reason) => {
                if reason == CloseReason::Requested {
                    self.session.cancel_current_run();
                }
                self.active_run.take();
                EffectOutcome::Close
            }
        }
    }

    /// Resolves and launches one reducer-selected binding generation.
    fn relaunch(&mut self, relaunch: RelaunchEffect) -> EffectOutcome {
        let catalog = binding_for_catalog(relaunch, self.latest_catalog.as_ref()).cloned();
        let gateway =
            binding_for_gateway(relaunch, &self.latest_gateway, self.active_gateway.as_ref())
                .cloned();
        let (Some(catalog), Some(gateway)) = (catalog, gateway) else {
            report_failure(
                &self.session,
                &self.host,
                "agent supervisor lost a reducer-selected binding",
            );
            return failed_relaunch(relaunch.run);
        };
        let Some(client) = gateway.model_client() else {
            report_failure(
                &self.session,
                &self.host,
                "the replacement Gateway credentials cannot make a model client",
            );
            return failed_relaunch(relaunch.run);
        };
        match relaunch.history {
            HistoryEffect::Preserve => {}
        }
        self.active_catalog = Some(catalog.clone());
        self.active_gateway = Some(gateway);
        self.active_run = Some(self.factory.launch(relaunch.run, catalog.models, client));
        EffectOutcome::Continue
    }
}

/// Converts one run result into its typed reducer event.
fn run_completion_event(
    run: RunId,
    result: Result<(), AgentError>,
    session: &AgentSession,
    host: &SessionHost,
) -> SupervisorEvent {
    let result = match result {
        Err(AgentError::Interrupted) => RunCompletion::Interrupted,
        Ok(()) => RunCompletion::Completed,
        Err(error) => {
            tracing::warn!(
                %error,
                session = %session.id,
                agent = %session.agent,
                "agent run failed"
            );
            let _ = session.errors.send(error.to_string());
            host.push
                .push_failure("Agent failed", error.to_string(), Activity::General);
            RunCompletion::Failed
        }
    };
    SupervisorEvent::RunCompleted { run, result }
}

/// Records reducer-selected retirement separately from operator cancellation.
fn report_cancel_origin(session: &AgentSession, origin: CancelOrigin) {
    match origin {
        CancelOrigin::Operator => {}
        CancelOrigin::Catalog => tracing::debug!(
            session = %session.id,
            "agent run retired for a new catalog generation"
        ),
        CancelOrigin::Gateway => tracing::debug!(
            session = %session.id,
            "agent run retired for a new gateway generation"
        ),
    }
}

/// Reports a failure shared by relaunch validation paths.
fn report_failure(session: &AgentSession, host: &SessionHost, message: &str) {
    let _ = session.errors.send(message.to_owned());
    host.push
        .push_failure("Agent failed", message, Activity::General);
}

/// Converts a failed relaunch into the reducer's terminal event.
fn failed_relaunch(run: RunId) -> EffectOutcome {
    EffectOutcome::Event(SupervisorEvent::RunCompleted {
        run,
        result: RunCompletion::Failed,
    })
}

/// Resolves a reducer-selected catalog generation from retained bindings.
fn binding_for_catalog(
    effect: RelaunchEffect,
    latest: Option<&ChatCatalog>,
) -> Option<&ChatCatalog> {
    latest.filter(|catalog| catalog.generation == effect.catalog_generation)
}

/// Resolves a reducer-selected Gateway generation from retained bindings.
fn binding_for_gateway<'a>(
    effect: RelaunchEffect,
    latest: &'a Arc<GatewaySnapshot>,
    active: Option<&'a Arc<GatewaySnapshot>>,
) -> Option<&'a Arc<GatewaySnapshot>> {
    (latest.generation() == effect.gateway_generation)
        .then_some(latest)
        .or_else(|| active.filter(|gateway| gateway.generation() == effect.gateway_generation))
}
