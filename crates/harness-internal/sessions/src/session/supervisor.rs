//! Agent-run supervision across cancellation: the one task per session
//! that collects events, feeds the pure reducer ([`transition`]), and
//! executes the effect it selects.
//!
//! The session's first run starts at launch, and a turn-cancel relaunches
//! the program over the retained transcript. A requested close cancels
//! the run and then drains it: the effect loop answers every outstanding
//! effect `Dropped` and steps the run to `Done` before the session reports
//! `Closed`, so nothing is left in flight when the session leaves its
//! Harness. The synthetic terminal frame for that interrupt is decided by
//! [`effective_interrupt`] and rendered in one place, after the drain.
//!
//! Each run is polled inside this task, so the run's events and live
//! deltas reach the session from here as the run produces them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use harness_runner::HarnessError;
use harness_runner::recorder::RunOutcome;
use tokio::sync::mpsc;

use crate::environment::Bindings;
use crate::transition::{
    CloseReason, EffectiveInterrupt, HistoryEffect, Interrupt, RelaunchEffect, RunCompletion,
    RunId, SupervisorEffect, SupervisorEvent, SupervisorState, SyntheticTerminal,
    effective_interrupt, transition,
};

use crate::runtime::SessionTable;

use super::run::run_once;
use super::{FailureKind, SessionCore};

/// One owned run future paired with its reducer identity.
type RunFuture = Pin<Box<dyn Future<Output = (RunId, Result<RunOutcome, HarnessError>)> + Send>>;

/// Runtime data collected alongside one pure supervisor event.
enum Collected {
    Supervisor(SupervisorEvent),
    Run {
        run: RunId,
        result: Result<RunOutcome, HarnessError>,
    },
}

/// The result of executing one reducer-selected effect.
enum Outcome {
    Continue,
    Close,
}

/// One session's supervisor: its event sources and the bindings each run
/// reads.
pub(crate) struct Supervisor {
    core: Arc<SessionCore>,
    bindings: Arc<Bindings>,
    table: Arc<SessionTable>,
    lifecycle: mpsc::UnboundedReceiver<SupervisorEvent>,
    cancellations: mpsc::Receiver<SupervisorEvent>,
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
}

impl Supervisor {
    /// The supervisor over a launch's parts.
    pub(crate) fn new(parts: SupervisorParts) -> Self {
        let SupervisorParts {
            core,
            bindings,
            table,
            lifecycle,
            cancellations,
        } = parts;
        Self {
            core,
            bindings,
            table,
            lifecycle,
            cancellations,
            active_run: None,
            saw_terminal: false,
            interrupt: None,
        }
    }

    /// Supervises the session from its start until it closes, then drains
    /// its last run and removes the session from its Harness.
    pub(crate) async fn run(mut self) {
        let mut state = SupervisorState::new();
        let mut collected = Collected::Supervisor(SupervisorEvent::Start);
        loop {
            let event = self.event_from(collected);
            let next = transition(state, event);
            state = next.state;
            match self.execute(next.effect) {
                Outcome::Continue => {}
                Outcome::Close => break,
            }
            collected = self.next().await;
        }
        self.drain().await;
        self.table.forget(&self.core.id);
    }

    /// Waits for the next typed event, prioritizing synchronous lifecycle
    /// events that causally precede a run wake.
    async fn next(&mut self) -> Collected {
        tokio::select! {
            biased;
            event = next_lifecycle_event(&mut self.lifecycle, &mut self.cancellations) => {
                Collected::Supervisor(event)
            }
            (run, result) = finished(&mut self.active_run) => Collected::Run { run, result },
        }
    }

    /// Applies collected runtime data and returns only the pure event.
    fn event_from(&mut self, collected: Collected) -> SupervisorEvent {
        match collected {
            Collected::Supervisor(event) => event,
            Collected::Run { run, result } => {
                self.active_run = None;
                self.core.done();
                let result = self.completion(result);
                SupervisorEvent::RunCompleted { run, result }
            }
        }
    }

    /// Converts one run's report into its typed completion, reporting a
    /// failure to the client.
    fn completion(&mut self, result: Result<RunOutcome, HarnessError>) -> RunCompletion {
        let completion = match result {
            Ok(RunOutcome::Completed { .. }) => RunCompletion::Completed,
            Ok(RunOutcome::Cancelled) => RunCompletion::Interrupted,
            Ok(RunOutcome::Failed { message, .. }) => {
                self.report_failure(&message);
                RunCompletion::Failed
            }
            Err(failure) => {
                self.report_failure(&harness_runner::display_chain(&failure));
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
        self.core.report(FailureKind::RunFailed, message.to_owned());
    }

    /// Executes one typed effect without making transition decisions.
    fn execute(&mut self, effect: SupervisorEffect) -> Outcome {
        match effect {
            SupervisorEffect::Preserve(_) => Outcome::Continue,
            SupervisorEffect::Cancel => {
                self.core.interrupted();
                self.core.cancel_current_run();
                Outcome::Continue
            }
            SupervisorEffect::Relaunch(relaunch) => {
                self.relaunch(relaunch);
                Outcome::Continue
            }
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

    /// Launches one reducer-selected run over the session's bindings.
    fn relaunch(&mut self, relaunch: RelaunchEffect) {
        match relaunch.history {
            HistoryEffect::Preserve => {}
        }
        let bindings = Arc::clone(&self.bindings);
        let core = Arc::clone(&self.core);
        let run = relaunch.run;
        self.core.alive();
        self.active_run = Some(Box::pin(async move {
            let result = run_once(core, bindings).await;
            (run, result)
        }));
    }

    /// Drains the last run after a close: the cancelled run answers its
    /// outstanding effects `Dropped` and steps to `Done`, closing its row;
    /// only then is the session `Closed`, and the interrupt's one frame
    /// rendered.
    async fn drain(&mut self) {
        if let Some(run) = self.active_run.take() {
            let (_, result) = run.await;
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
            self.core
                .report(FailureKind::Interrupted, frame.message().to_owned());
        }
    }
}

/// Awaits the active run, pending forever when there is none.
async fn finished(run: &mut Option<RunFuture>) -> (RunId, Result<RunOutcome, HarnessError>) {
    match run {
        Some(run) => run.as_mut().await,
        None => std::future::pending().await,
    }
}

/// Waits for the next synchronous lifecycle event, polling the guaranteed
/// queue before the bounded cancellation queue. Cross-channel ordering is
/// not load-bearing: a cancellation is valid in any reducer phase, and a
/// close processed late lands on a phase that ignores it.
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
