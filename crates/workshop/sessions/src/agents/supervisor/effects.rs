//! Execution of reducer-selected supervisor effects.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use harness_api::bridge::{CapabilityRegistry, GatewayClient as ModelClient};
use promptforge_api_runtime::{Environment, Prompt, RunContext, RunHost, RunResult};
use promptforge_api_types::cancel::sync::CancelHandle as CancelFlag;
use promptforge_api_types::observe::Observer;
use promptforge_api_types::timestamp::Timestamp;
use promptforge_api_types::wire::StreamDelta;

use workshop_gateway::GatewaySnapshot;
use workshop_menu::ChatCatalog;
use workshop_protocol::Activity;

use crate::agents::environment::{current_model, session_registry};
use crate::agents::{
    AgentSession, AgentSource, SessionHost, SessionObserver, agent_client, delta_stamp, ui_provider,
};
use crate::input::SessionInputBroker;

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
pub(super) enum AgentRunError {
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
    observer: Arc<dyn Observer>,
    on_delta: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    ui: Arc<dyn Fn() -> serde_json::Value + Send + Sync>,
    host: SessionHost,
}

impl RunFactory {
    /// Builds reusable run resources for one session.
    fn new(session: Arc<AgentSession>, host: &SessionHost) -> Self {
        let observer: Arc<dyn Observer> = Arc::new(SessionObserver {
            log: Arc::clone(&session.log),
            rounds: Arc::clone(&session.rounds),
            push: host.push(),
            backoff: host.backoff().clone(),
            errors: session.errors.clone(),
            lifecycle: Arc::clone(&session.lifecycle),
        });
        Self {
            on_delta: delta_stamp(&session, &host.push()),
            ui: ui_provider(host.menu(), host.registry()),
            session,
            observer,
            host: host.clone(),
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
    /// run), a fresh seed, and the start instant.
    fn launch_markdown(
        &self,
        run: RunId,
        source: String,
        client: ModelClient,
        registry: Arc<CapabilityRegistry>,
        gateway: Arc<GatewaySnapshot>,
    ) -> RunFuture {
        let parts = MarkdownRunParts {
            session: Arc::clone(&self.session),
            observer: Arc::clone(&self.observer),
            ui: (self.ui)(),
            seed: rand::random(),
            started_at: now_timestamp(),
            on_delta: Arc::clone(&self.on_delta),
            host: self.host.clone(),
        };
        Box::pin(async move {
            let result = run_markdown_agent(&source, parts, run, client, registry, &gateway).await;
            (run, result)
        })
    }
}

/// The owned pieces one unified-runtime run needs beyond its source and
/// client, cloned out of the factory per relaunch.
struct MarkdownRunParts {
    session: Arc<AgentSession>,
    observer: Arc<dyn Observer>,
    /// The `ui()` snapshot taken at launch.
    ui: serde_json::Value,
    /// The run's seed, drawn at launch from the OS CSPRNG.
    seed: u64,
    /// The launch instant, every section's `sys.when`.
    started_at: Timestamp,
    on_delta: Arc<dyn Fn(StreamDelta) + Send + Sync>,
    host: SessionHost,
}

/// Runs one Markdown agent prompt on the unified runtime: the session's
/// wait registry behind the generic input broker, the launch-time menu
/// selection behind `ui().selected_model`, deltas forwarded to the
/// session's channel. The host carries the session's shared registry, and
/// the engine's loop activates the first-party capabilities the prompt's
/// frontmatter declares against it - the catalog goes to the engine, the
/// implementations stay on the host's side - while the context carries
/// the dropdown's current model resolved at launch, so a selection change
/// takes effect on the next run.
async fn run_markdown_agent(
    source: &str,
    parts: MarkdownRunParts,
    run: RunId,
    client: ModelClient,
    registry: Arc<CapabilityRegistry>,
    gateway: &GatewaySnapshot,
) -> Result<(), AgentRunError> {
    let MarkdownRunParts {
        session,
        observer,
        ui,
        seed,
        started_at,
        on_delta,
        host,
    } = parts;
    let prompt = Prompt::parse(source, &session.id, observer.as_ref()).map_err(|error| {
        AgentRunError::Failed {
            message: format!("the embedded Markdown agent failed to parse: {error}"),
            source: Some(Box::new(error)),
        }
    })?;
    let broker = Arc::new(SessionInputBroker::new(
        Arc::clone(&session.waits),
        session.input_frames.clone(),
    ));
    let model = match current_model(&host, gateway.base_url(), gateway.api_key()).await {
        Ok(model) => model,
        Err(cause) => {
            return Err(AgentRunError::Failed {
                message: format!("the chat cannot launch: {cause}"),
                source: Some(Box::new(cause)),
            });
        }
    };
    let (cancel, bridge) = bridge_cancel(session.arm_cancel(run));
    let mut ctx = RunContext::new(session.id.clone(), seed, started_at)
        .cancel(cancel)
        .ui(ui);
    if let Some(model) = model {
        ctx = ctx.model(model);
    }
    // The engine's context carries the run's inputs; everything the loop
    // performs with - the client, the registry it activates, the broker,
    // the delta hook, the observer - rides the host.
    let host = RunHost::new()
        .observer(observer)
        .client(client)
        .registry(registry)
        .input_broker(broker)
        .on_delta(on_delta);
    let outcome = Environment::new().run(&prompt, "", ctx, host).await;
    // The run is over, so nothing reads the flag: the bridge ends with it.
    bridge.abort();
    match outcome {
        RunResult::Ok(_output) => Ok(()),
        RunResult::Cancelled => Err(AgentRunError::Interrupted),
        RunResult::Failure(error) => Err(AgentRunError::Failed {
            message: error.to_string(),
            source: Some(Box::new(error)),
        }),
    }
}

/// The system clock now as the engine's `Timestamp`: the host's stamp for
/// a run's `started_at`, since the engine reads no clock of its own. A
/// clock before the epoch or beyond `i64` milliseconds (neither reachable
/// on a real host) saturates to the epoch rather than refusing the launch.
fn now_timestamp() -> Timestamp {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .map_or(Timestamp::UNIX_EPOCH, Timestamp::from_unix_millis)
}

/// Bridges the session's awaitable cancel token to the synchronous flag
/// the engine polls: the flag is set the moment the token fires. The
/// caller aborts the returned bridge task once the run is over, so a run
/// that finishes uncancelled leaves no task waiting on a token nobody
/// will fire.
fn bridge_cancel(
    token: promptforge_api_types::cancel::CancelHandle,
) -> (CancelFlag, tokio::task::JoinHandle<()>) {
    let flag = CancelFlag::new();
    let bridged = flag.clone();
    let bridge = tokio::spawn(async move {
        token.cancelled().await;
        bridged.cancel();
    });
    (flag, bridge)
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
