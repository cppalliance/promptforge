//! Pure state transitions for one agent-session supervisor: the reducer
//! whose matches stay wildcard-free, so a new variant is a compile error.

/// One run's terminal result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunCompletion {
    /// Cancellation stopped the run without ending the session.
    Interrupted,
    /// The program returned normally.
    Completed,
    /// The program failed.
    Failed,
}

/// Identity assigned to one launched run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunId(u64);

/// An input to the pure supervisor transition model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorEvent {
    /// The session launched; its first run starts.
    Start,
    /// A run produced its terminal result.
    RunCompleted {
        /// The run that completed.
        run: RunId,
        /// How it completed.
        result: RunCompletion,
    },
    /// The operator cancelled the current turn.
    OperatorCancellation,
    /// The owning session closed.
    Close,
}

/// Why the current ownership remains unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreserveReason {
    /// The current run remains authoritative.
    CurrentRun,
    /// Cancellation already owns run retirement.
    CancellationPending,
    /// A duplicate or stale event has already been accounted for.
    AlreadyHandled,
    /// The session is already closed.
    Closed,
}

/// Event-log handling for a launched replacement run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryEffect {
    /// Reuses the session's retained event log.
    Preserve,
}

/// The complete immutable inputs for one replacement run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelaunchEffect {
    /// Identity assigned to the replacement run.
    pub run: RunId,
    /// Event-log treatment across replacement.
    pub history: HistoryEffect,
}

/// Why supervision ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The owning session requested close.
    Requested,
    /// The agent prompt returned normally.
    RunCompleted,
    /// The agent prompt failed.
    RunFailed,
}

/// One typed action selected by the transition model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorEffect {
    /// Cancels the current run.
    Cancel,
    /// Keeps the named ownership unchanged.
    Preserve(PreserveReason),
    /// Launches a replacement over retained history.
    Relaunch(RelaunchEffect),
    /// Ends supervision.
    Close(CloseReason),
}

/// Whether the session is starting, running, retiring, or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Starting,
    Running,
    Cancelling,
    Closed,
}

/// Pure state owned by one agent-session supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupervisorState {
    phase: Phase,
    active_run: Option<RunId>,
    next_run: u64,
}

impl SupervisorState {
    /// Starts supervision before the first run launches.
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Phase::Starting,
            active_run: None,
            next_run: 1,
        }
    }
}

impl Default for SupervisorState {
    fn default() -> Self {
        Self::new()
    }
}

/// The next immutable state and its one typed effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupervisorTransition {
    /// The state after the event.
    pub state: SupervisorState,
    /// The one typed action the event selected.
    pub effect: SupervisorEffect,
}

/// Reduces one explicit event without performing asynchronous work.
#[must_use]
pub fn transition(state: SupervisorState, event: SupervisorEvent) -> SupervisorTransition {
    if state.phase == Phase::Closed {
        return changed(state, SupervisorEffect::Preserve(PreserveReason::Closed));
    }
    match event {
        SupervisorEvent::Close => close(state, CloseReason::Requested),
        SupervisorEvent::Start => started(state),
        SupervisorEvent::OperatorCancellation => operator_cancelled(state),
        SupervisorEvent::RunCompleted { run, result } => run_completed(state, run, result),
    }
}

fn started(state: SupervisorState) -> SupervisorTransition {
    match state.phase {
        Phase::Starting => relaunch(state),
        Phase::Running => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CurrentRun),
        ),
        Phase::Cancelling => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CancellationPending),
        ),
        Phase::Closed => changed(state, SupervisorEffect::Preserve(PreserveReason::Closed)),
    }
}

fn operator_cancelled(mut state: SupervisorState) -> SupervisorTransition {
    match state.phase {
        Phase::Starting => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        ),
        Phase::Running => {
            state.phase = Phase::Cancelling;
            changed(state, SupervisorEffect::Cancel)
        }
        Phase::Cancelling => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CancellationPending),
        ),
        Phase::Closed => changed(state, SupervisorEffect::Preserve(PreserveReason::Closed)),
    }
}

fn run_completed(
    mut state: SupervisorState,
    run: RunId,
    result: RunCompletion,
) -> SupervisorTransition {
    if state.active_run != Some(run) {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        );
    }
    state.active_run = None;
    match result {
        RunCompletion::Interrupted => relaunch(state),
        RunCompletion::Completed => close(state, CloseReason::RunCompleted),
        RunCompletion::Failed => close(state, CloseReason::RunFailed),
    }
}

fn relaunch(mut state: SupervisorState) -> SupervisorTransition {
    let run = RunId(state.next_run);
    state.next_run = state.next_run.saturating_add(1);
    state.active_run = Some(run);
    state.phase = Phase::Running;
    let effect = RelaunchEffect {
        run,
        history: HistoryEffect::Preserve,
    };
    changed(state, SupervisorEffect::Relaunch(effect))
}

fn close(mut state: SupervisorState, reason: CloseReason) -> SupervisorTransition {
    state.phase = Phase::Closed;
    state.active_run = None;
    changed(state, SupervisorEffect::Close(reason))
}

fn changed(state: SupervisorState, effect: SupervisorEffect) -> SupervisorTransition {
    SupervisorTransition { state, effect }
}

#[path = "transition-interrupt.rs"]
mod interrupt;
pub use interrupt::{
    EffectiveInterrupt, Interrupt, SessionState, SyntheticTerminal, effective_interrupt,
};

#[cfg(test)]
#[path = "transition-tests.rs"]
mod tests;
