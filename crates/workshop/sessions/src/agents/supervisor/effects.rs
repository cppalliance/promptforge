//! Execution of reducer-selected supervisor effects.

use std::sync::Arc;

use harness_api::bridge::input::SessionInputBroker;
use harness_api::bridge::{CapabilityRegistry, GatewayClient as ModelClient};
use promptforge_api_types::wire::StreamDelta;

use workshop_gateway::GatewaySnapshot;
use workshop_menu::ChatCatalog;
use workshop_protocol::Activity;

use crate::agents::environment::{current_model, session_registry};
use crate::agents::run::{RunParts, now_timestamp, run_markdown_agent};
use crate::agents::{
    AgentSession, AgentSource, SessionHost, SessionSink, agent_client, delta_stamp, ui_provider,
};

use super::events::{CollectedEvent, EventCollector, RunFuture};
use super::transition::{
    CancelOrigin, CatalogDisposition, CloseReason, HistoryEffect, RelaunchEffect, RunCompletion,
    RunId, SupervisorEffect, SupervisorEvent,
};

/// One agent run's terminal outcome, session-local: cancellation maps to
/// the interrupted stop reason and every other failure to an
/// operator-facing message. Replaces the retired agent runtime's
/// `AgentError`, which named Lua-specific failure shapes no session run
/// can produce.
#[derive(Debug, thiserror::Error)]
pub(in crate::agents) enum AgentRunError {
    /// The run was cancelled: a stop reason, never a failure.
    #[error("the agent run was interrupted")]
    Interrupted,
    /// The run failed; the message is operator-facing.
    #[error("{message}")]
    Failed {
        /// What failed, operator-facing.
        message: String,
        /// The underlying error, when one exists.
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

/// The result of executing one reducer-selected effect.
pub(super) enum EffectOutcome {
    Continue,
    Event(SupervisorEvent),
    Close,
}

/// Immutable resources reused by each reducer-selected relaunch.
struct RunFactory {
    session: Arc<AgentSession>,
    on_delta: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    ui: Arc<dyn Fn() -> serde_json::Value + Send + Sync>,
    host: SessionHost,
}

impl RunFactory {
    /// Builds reusable run resources for one session.
    fn new(session: Arc<AgentSession>, host: &SessionHost) -> Self {
        Self {
            on_delta: delta_stamp(&session, &host.push()),
            ui: ui_provider(host.menu(), host.registry()),
            session,
            host: host.clone(),
        }
    }

    /// The session's event sink for one run: the memory log every
    /// transcript event lands in and the side effects the session wires
    /// to the run's reports.
    fn sink(&self) -> SessionSink {
        SessionSink {
            log: Arc::clone(&self.session.log),
            rounds: Arc::clone(&self.session.rounds),
            push: self.host.push(),
            backoff: self.host.backoff().clone(),
            errors: self.session.errors.clone(),
            lifecycle: Arc::clone(&self.session.lifecycle),
        }
    }

    /// Builds one run over retained history and frozen bindings.
    fn launch(
        &self,
        run: RunId,
        client: ModelClient,
        registry: Arc<CapabilityRegistry>,
        gateway: Arc<GatewaySnapshot>,
    ) -> RunFuture {
        let AgentSource::Markdown(source) = self.session.source.clone();
        self.launch_markdown(run, source, client, registry, gateway)
    }

    /// Builds one unified-runtime run of a Markdown prompt document. The
    /// run's host-drawn inputs are taken here, at launch: the `ui()`
    /// snapshot (so a menu or workspace change takes effect on the next
    /// run), a fresh seed, and the start instant. The dropdown's current
    /// model is resolved at launch too, so a selection change takes
    /// effect on the next run; the session's wait registry stands behind
    /// the generic input broker, and deltas go to the session's channel.
    fn launch_markdown(
        &self,
        run: RunId,
        source: String,
        client: ModelClient,
        registry: Arc<CapabilityRegistry>,
        gateway: Arc<GatewaySnapshot>,
    ) -> RunFuture {
        let session = Arc::clone(&self.session);
        let host = self.host.clone();
        let sink = self.sink();
        let ui = (self.ui)();
        let on_delta = Arc::clone(&self.on_delta);
        Box::pin(async move {
            let result = async {
                let model = current_model(&host, gateway.base_url(), gateway.api_key())
                    .await
                    .map_err(|cause| AgentRunError::Failed {
                        message: format!("the chat cannot launch: {cause}"),
                        source: Some(Box::new(cause)),
                    })?;
                let parts = RunParts {
                    sink,
                    broker: Arc::new(SessionInputBroker::new(
                        Arc::clone(&session.waits),
                        session.input_frames.clone(),
                    )),
                    ui,
                    seed: rand::random(),
                    started_at: now_timestamp(),
                    on_delta,
                    model,
                    execution: session.id.clone(),
                    cancel: session.arm_cancel(run),
                };
                run_markdown_agent(&source, parts, client, registry).await
            }
            .await;
            (run, result)
        })
    }
}

/// Mutable runtime bindings and the currently executing run.
pub(super) struct EffectExecutor {
    session: Arc<AgentSession>,
    host: SessionHost,
    factory: RunFactory,
    /// The shared registry of first-party capabilities every run activates
    /// against, rebuilt when the gateway generation changes so a
    /// replacement gateway's root and key reach the contributed tools.
    registry: Option<Arc<CapabilityRegistry>>,
    /// The gateway generation `registry` was built from.
    registry_generation: u64,
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
        initial_catalog: Option<ChatCatalog>,
        initial_gateway: Arc<GatewaySnapshot>,
    ) -> Self {
        let registry =
            session_registry(initial_gateway.base_url(), initial_gateway.api_key()).map(Arc::new);
        Self {
            factory: RunFactory::new(Arc::clone(&session), &host),
            registry_generation: initial_gateway.generation(),
            registry,
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
        let Some(client) = agent_client(gateway.base_url(), gateway.api_key()) else {
            report_failure(
                &self.session,
                &self.host,
                "the replacement Gateway credentials cannot make a model client",
            );
            return failed_relaunch(relaunch.run);
        };
        if gateway.generation() != self.registry_generation {
            self.registry = session_registry(gateway.base_url(), gateway.api_key()).map(Arc::new);
            self.registry_generation = gateway.generation();
        }
        let Some(registry) = self.registry.clone() else {
            report_failure(
                &self.session,
                &self.host,
                "the Gateway settings cannot build the promptforge/web capability",
            );
            return failed_relaunch(relaunch.run);
        };
        match relaunch.history {
            HistoryEffect::Preserve => {}
        }
        self.active_catalog = Some(catalog);
        self.active_gateway = Some(Arc::clone(&gateway));
        self.active_run = Some(self.factory.launch(relaunch.run, client, registry, gateway));
        EffectOutcome::Continue
    }
}

/// Converts one run result into its typed reducer event.
fn run_completion_event(
    run: RunId,
    result: Result<(), AgentRunError>,
    session: &AgentSession,
    host: &SessionHost,
) -> SupervisorEvent {
    let result = match result {
        Err(AgentRunError::Interrupted) => RunCompletion::Interrupted,
        Ok(()) => RunCompletion::Completed,
        Err(error) => {
            tracing::warn!(
                %error,
                session = %session.id,
                agent = %session.agent,
                "agent run failed"
            );
            let _ = session.errors.send(error.to_string());
            host.push()
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
    host.push()
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
