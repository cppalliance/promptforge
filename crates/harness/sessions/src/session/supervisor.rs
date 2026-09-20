//! Agent-run supervision across cancellation and binding generations:
//! the one task per session that collects events, feeds the pure reducer
//! ([`transition`]), and executes the effect it selects.
//!
//! Each run freezes one gateway generation and one catalog generation;
//! cancellation or a genuinely new usable generation relaunches over the
//! retained transcript. A requested close cancels the run and then drains
//! it: the effect loop answers every outstanding effect `Dropped` and
//! steps the run to `Done` before the session reports `Closed`, so
//! nothing is left in flight when the session leaves its harness. The
//! synthetic terminal frame for that interrupt is decided by
//! [`effective_interrupt`] and rendered in exactly one place, after the
//! drain.
//!
//! The raw deltas the chat performers stream are drained here too,
//! stamped with the session's current round, ahead of the run future in
//! the select order so a round's chunks are broadcast before the event
//! that supersedes them is applied.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use harness_log::RunOutcome;
use promptforge_api_types::wire::StreamDelta;
use tokio::sync::{mpsc, watch};

use crate::environment::{Bindings, CatalogBinding, GatewayResources};
use crate::transition::{
    CancelOrigin, CatalogDisposition, CloseReason, EffectiveInterrupt, HistoryEffect, Interrupt,
    RelaunchEffect, RunCompletion, RunId, SupervisorEffect, SupervisorEvent, SupervisorState,
    SyntheticTerminal, effective_interrupt, transition,
};

use crate::runtime::SessionTable;

use super::SessionCore;
use super::run::{RunFailure, RunInputs, run_once};

/// One owned run future paired with its reducer identity.
type RunFuture = Pin<Box<dyn Future<Output = (RunId, Result<RunOutcome, RunFailure>)> + Send>>;

/// Runtime data collected alongside one pure supervisor event.
enum Collected {
    Supervisor(SupervisorEvent),
    Catalog {
        event: SupervisorEvent,
        snapshot: Option<CatalogBinding>,
    },
    Gateway {
        event: SupervisorEvent,
        resources: Arc<GatewayResources>,
    },
    Run {
        run: RunId,
        result: Result<RunOutcome, RunFailure>,
    },
}

/// The result of executing one reducer-selected effect.
enum Outcome {
    Continue,
    Event(SupervisorEvent),
    Close,
}

/// One session's supervisor: its event sources and frozen bindings.
pub(crate) struct Supervisor {
    core: Arc<SessionCore>,
    bindings: Arc<Bindings>,
    table: Arc<SessionTable>,
    lifecycle: mpsc::UnboundedReceiver<SupervisorEvent>,
    cancellations: mpsc::Receiver<SupervisorEvent>,
    gateway_watch: watch::Receiver<Option<u64>>,
    catalog_watch: watch::Receiver<Option<u64>>,
    /// The raw delta stream; `None` once it closes, which cannot happen
    /// while the core holds its sender.
    raw_deltas: Option<mpsc::UnboundedReceiver<StreamDelta>>,
    latest_gateway: Arc<GatewayResources>,
    active_gateway: Option<Arc<GatewayResources>>,
    latest_catalog: Option<CatalogBinding>,
    active_catalog: Option<CatalogBinding>,
    active_run: Option<RunFuture>,
    /// Whether a genuine terminal outcome has been observed for the
    /// current run; the input to [`effective_interrupt`].
    saw_terminal: bool,
    /// The interrupt frame a requested close renders after the drain.
    interrupt: Option<SyntheticTerminal>,
}

/// What a launch hands the supervisor.
pub(crate) struct SupervisorParts {
    pub(crate) core: Arc<SessionCore>,
    pub(crate) bindings: Arc<Bindings>,
    pub(crate) table: Arc<SessionTable>,
    pub(crate) lifecycle: mpsc::UnboundedReceiver<SupervisorEvent>,
    pub(crate) cancellations: mpsc::Receiver<SupervisorEvent>,
    pub(crate) raw_deltas: mpsc::UnboundedReceiver<StreamDelta>,
    /// The gateway snapshot the launch checked for usability.
    pub(crate) gateway: Arc<GatewayResources>,
    /// The gateway watch, subscribed by the launch before it read
    /// `gateway`, so a replacement landing after the read wakes the
    /// supervisor.
    pub(crate) gateway_watch: watch::Receiver<Option<u64>>,
}

impl Supervisor {
    /// Subscribes to the catalog watch before reading its snapshot, so a
    /// replacement cannot disappear between the two; the gateway pair
    /// arrives already ordered the same way by the launch.
    pub(crate) fn new(parts: SupervisorParts) -> Self {
        let SupervisorParts {
            core,
            bindings,
            table,
            lifecycle,
            cancellations,
            raw_deltas,
            gateway,
            gateway_watch,
        } = parts;
        let catalog_watch = bindings.subscribe_catalog();
        let latest_catalog = bindings.catalog();
        Self {
            core,
            bindings,
            table,
            lifecycle,
            cancellations,
            gateway_watch,
            catalog_watch,
            raw_deltas: Some(raw_deltas),
            latest_gateway: gateway,
            active_gateway: None,
            latest_catalog,
            active_catalog: None,
            active_run: None,
            saw_terminal: false,
            interrupt: None,
        }
    }

    /// Supervises the session until it closes, then drains its last run
    /// and removes the session from its harness.
    pub(crate) async fn run(mut self) {
        let mut state = SupervisorState::new(self.latest_gateway.generation());
        let mut pending = Some(self.initial_catalog_event());
        loop {
            let collected = match pending.take() {
                Some(event) => Collected::Supervisor(event),
                None => self.next().await,
            };
            let event = self.event_from(collected);
            let next = transition(state, event);
            state = next.state;
            match self.execute(next.effect) {
                Outcome::Continue => {}
                Outcome::Event(event) => pending = Some(event),
                Outcome::Close => break,
            }
        }
        self.drain().await;
        self.table.forget(&self.core.id);
    }

    /// The catalog event the reducer starts from: the retained catalog,
    /// or `Unavailable` at generation zero when none was pushed yet.
    fn initial_catalog_event(&mut self) -> SupervisorEvent {
        let observed = self.catalog_watch.borrow_and_update().unwrap_or(0);
        classify(self.latest_catalog.as_ref(), observed, None)
    }

    /// Waits for the next typed event, prioritizing synchronous lifecycle
    /// events that causally precede a run wake or watched replacement,
    /// and broadcasting deltas as they arrive without leaving the wait.
    async fn next(&mut self) -> Collected {
        loop {
            tokio::select! {
                biased;
                event = next_lifecycle_event(&mut self.lifecycle, &mut self.cancellations) => {
                    return Collected::Supervisor(event);
                }
                () = changed(&mut self.catalog_watch) => return self.catalog_event(),
                () = changed(&mut self.gateway_watch) => {
                    if let Some(collected) = self.gateway_event() {
                        return collected;
                    }
                }
                received = recv_or_pending(&mut self.raw_deltas) => match received {
                    Some(delta) => self.core.publish_delta(delta),
                    None => self.raw_deltas = None,
                },
                (run, result) = finished(&mut self.active_run) => {
                    return Collected::Run { run, result };
                }
            }
        }
    }

    /// Classifies the catalog behind the watch that just changed.
    fn catalog_event(&mut self) -> Collected {
        let observed = self.catalog_watch.borrow_and_update().unwrap_or(0);
        let snapshot = self.bindings.catalog();
        let active = self.active_catalog.as_ref().map(|c| c.models.as_slice());
        let event = classify(snapshot.as_ref(), observed, active);
        Collected::Catalog { event, snapshot }
    }

    /// The gateway resources behind the watch that just changed.
    fn gateway_event(&mut self) -> Option<Collected> {
        self.gateway_watch.borrow_and_update();
        let resources = self.bindings.gateway()?;
        Some(Collected::Gateway {
            event: SupervisorEvent::GatewayGeneration(resources.generation()),
            resources,
        })
    }

    /// Applies collected runtime data and returns only the pure event.
    fn event_from(&mut self, collected: Collected) -> SupervisorEvent {
        match collected {
            Collected::Supervisor(event) => event,
            Collected::Catalog { event, snapshot } => {
                if matches!(
                    event,
                    SupervisorEvent::CatalogGeneration {
                        disposition: CatalogDisposition::Retained,
                        ..
                    }
                ) && self.active_catalog.is_some()
                {
                    self.active_catalog.clone_from(&snapshot);
                }
                self.latest_catalog = snapshot;
                event
            }
            Collected::Gateway { event, resources } => {
                self.latest_gateway = resources;
                event
            }
            Collected::Run { run, result } => {
                self.active_run = None;
                self.core.finish_run(run);
                self.core.done();
                let result = self.completion(result);
                SupervisorEvent::RunCompleted { run, result }
            }
        }
    }

    /// Converts one run's report into its typed completion, reporting a
    /// failure to the client.
    fn completion(&mut self, result: Result<RunOutcome, RunFailure>) -> RunCompletion {
        let completion = match result {
            Ok(RunOutcome::Completed { .. }) => RunCompletion::Completed,
            Ok(RunOutcome::Cancelled) => RunCompletion::Interrupted,
            Ok(RunOutcome::Failed { message, .. }) => {
                self.report_failure(&message);
                RunCompletion::Failed
            }
            Err(failure) => {
                self.report_failure(&failure.to_string());
                RunCompletion::Failed
            }
        };
        self.saw_terminal |= completion.is_genuine();
        completion
    }

    fn report_failure(&self, message: &str) {
        tracing::warn!(
            session = %self.core.id,
            agent = %self.core.agent,
            %message,
            "agent run failed"
        );
        self.core.report(message.to_owned());
    }

    /// Executes one typed effect without making transition decisions.
    fn execute(&mut self, effect: SupervisorEffect) -> Outcome {
        match effect {
            SupervisorEffect::Wait(_) | SupervisorEffect::Preserve(_) => Outcome::Continue,
            SupervisorEffect::Cancel(origin) => {
                self.report_cancel_origin(origin);
                self.core.interrupted();
                self.core.cancel_current_run();
                Outcome::Continue
            }
            SupervisorEffect::Relaunch(relaunch) => self.relaunch(relaunch),
            SupervisorEffect::Close(reason) => {
                if reason == CloseReason::Requested && self.active_run.is_some() {
                    match effective_interrupt(Interrupt::Cancel, self.saw_terminal) {
                        EffectiveInterrupt::Terminal(frame) => self.interrupt = Some(frame),
                        EffectiveInterrupt::Superseded => {}
                    }
                    self.core.interrupted();
                    self.core.cancel_current_run();
                }
                Outcome::Close
            }
        }
    }

    /// Resolves and launches one reducer-selected binding generation.
    fn relaunch(&mut self, relaunch: RelaunchEffect) -> Outcome {
        let catalog = self
            .latest_catalog
            .as_ref()
            .filter(|catalog| catalog.generation == relaunch.catalog_generation)
            .cloned();
        let gateway = (self.latest_gateway.generation() == relaunch.gateway_generation)
            .then(|| Arc::clone(&self.latest_gateway))
            .or_else(|| {
                self.active_gateway
                    .clone()
                    .filter(|gateway| gateway.generation() == relaunch.gateway_generation)
            });
        let (Some(catalog), Some(gateway)) = (catalog, gateway) else {
            return self.failed_relaunch(
                relaunch.run,
                "agent supervisor lost a reducer-selected binding",
            );
        };
        let Some(client) = gateway.client().cloned() else {
            return self.failed_relaunch(
                relaunch.run,
                "the replacement Gateway credentials cannot make a model client",
            );
        };
        let Some(registry) = gateway.registry().cloned() else {
            return self.failed_relaunch(
                relaunch.run,
                "the Gateway settings cannot build the promptforge/web capability",
            );
        };
        match relaunch.history {
            HistoryEffect::Preserve => {}
        }
        self.active_catalog = Some(catalog.clone());
        self.active_gateway = Some(Arc::clone(&gateway));
        let inputs = RunInputs {
            run: relaunch.run,
            gateway,
            client,
            registry: Some(registry),
            catalog: Some(catalog),
            host: self.bindings.host(),
        };
        let core = Arc::clone(&self.core);
        let run = relaunch.run;
        self.core.alive();
        self.active_run = Some(Box::pin(async move {
            let result = run_once(core, inputs).await;
            (run, result)
        }));
        Outcome::Continue
    }

    /// Reports a relaunch that could not start and converts it into the
    /// reducer's terminal event.
    fn failed_relaunch(&mut self, run: RunId, message: &str) -> Outcome {
        self.report_failure(message);
        self.saw_terminal = true;
        Outcome::Event(SupervisorEvent::RunCompleted {
            run,
            result: RunCompletion::Failed,
        })
    }

    /// Records reducer-selected retirement separately from operator
    /// cancellation.
    fn report_cancel_origin(&self, origin: CancelOrigin) {
        match origin {
            CancelOrigin::Operator => {}
            CancelOrigin::Catalog => tracing::debug!(
                session = %self.core.id,
                "agent run retired for a new catalog generation"
            ),
            CancelOrigin::Gateway => tracing::debug!(
                session = %self.core.id,
                "agent run retired for a new gateway generation"
            ),
        }
    }

    /// Drains the last run after a close: the cancelled run answers its
    /// outstanding effects `Dropped` and steps to `Done`, closing its row;
    /// only then is the session `Closed`, and the interrupt's one frame
    /// rendered.
    async fn drain(&mut self) {
        if let Some(run) = self.active_run.take() {
            let (run, result) = run.await;
            self.core.finish_run(run);
            if let Err(failure) = result {
                tracing::warn!(
                    session = %self.core.id,
                    error = %failure,
                    "the closing run ended without an outcome"
                );
            }
        }
        self.core.done();
        if let Some(frame) = self.interrupt.take() {
            self.core.report(frame.message().to_owned());
        }
    }
}

/// Classifies one retained catalog against the run's frozen bindings. No
/// catalog pushed yet, or a catalog with no chat-capable entry, is
/// unavailable: nothing a run could bind a model against.
fn classify(
    snapshot: Option<&CatalogBinding>,
    observed_generation: u64,
    active_models: Option<&[serde_json::Value]>,
) -> SupervisorEvent {
    let generation = snapshot.map_or(observed_generation, |catalog| catalog.generation);
    let disposition = match (snapshot, active_models) {
        (None, _) => CatalogDisposition::Unavailable,
        (Some(catalog), _) if catalog.models.is_empty() => CatalogDisposition::Unavailable,
        (Some(catalog), Some(active)) if catalog.models != active => {
            CatalogDisposition::Replacement
        }
        (Some(_), _) => CatalogDisposition::Retained,
    };
    SupervisorEvent::CatalogGeneration {
        generation,
        disposition,
    }
}

/// Waits for a watch to change; a dropped sender (the harness is gone)
/// pends forever, so the session ends through its own lifecycle.
async fn changed(watch: &mut watch::Receiver<Option<u64>>) {
    if watch.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Receives from an optional channel, pending forever when absent.
async fn recv_or_pending<T>(receiver: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// Awaits the active run, pending forever when there is none.
async fn finished(run: &mut Option<RunFuture>) -> (RunId, Result<RunOutcome, RunFailure>) {
    match run {
        Some(run) => run.as_mut().await,
        None => std::future::pending().await,
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
