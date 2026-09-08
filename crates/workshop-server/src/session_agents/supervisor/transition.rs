//! Pure state transitions for one agent-session supervisor.

/// Why the current run's cancellation handle fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum CancelOrigin {
    /// The operator explicitly cancelled the current turn.
    Operator,
    /// A usable catalog generation replaced the run's frozen bindings.
    Catalog,
    /// The desktop host published a new Gateway generation.
    Gateway,
}

/// One run's terminal result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum RunCompletion {
    /// Cancellation stopped the run without ending the session.
    Interrupted,
    /// The program returned normally.
    Completed,
    /// The program failed.
    Failed,
}

/// How a published catalog generation relates to the frozen run catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum CatalogDisposition {
    /// No chat-capable catalog is currently available.
    Unavailable,
    /// The generation is usable without changing frozen model bindings.
    Retained,
    /// The generation is usable and changes frozen model bindings.
    Replacement,
}

/// Identity assigned to one launched run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) struct RunId(u64);

/// An input to the pure supervisor transition model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum SupervisorEvent {
    /// A run produced its terminal result.
    RunCompleted {
        /// The run that completed.
        run: RunId,
        /// How it completed.
        result: RunCompletion,
    },
    /// The host published a catalog generation.
    CatalogGeneration {
        /// The catalog bus generation.
        generation: u64,
        /// Whether the frozen run can retain its bindings.
        disposition: CatalogDisposition,
    },
    /// The atomically published Gateway generation changed.
    GatewayGeneration(u64),
    /// The operator cancelled the current turn.
    OperatorCancellation,
    /// A durable input event resumed this run.
    AcceptedInput(RunId),
    /// The accepted turn reached a durable terminal event.
    TerminalSettlement(RunId),
    /// The owning session closed.
    Close,
}

/// The condition the supervisor must await.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum WaitFor {
    /// A usable chat catalog.
    Catalog,
    /// The accepted turn's durable terminal event.
    TerminalSettlement,
}

/// Why the current ownership remains unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum PreserveReason {
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
pub(in crate::session_agents) enum HistoryEffect {
    /// Reuse the session's retained event log.
    Preserve,
}

/// The complete immutable inputs for one replacement run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) struct RelaunchEffect {
    /// Identity assigned to the replacement run.
    pub(in crate::session_agents) run: RunId,
    /// Catalog generation frozen by the replacement.
    pub(in crate::session_agents) catalog_generation: u64,
    /// Gateway generation frozen by the replacement.
    pub(in crate::session_agents) gateway_generation: u64,
    /// Event-log treatment across replacement.
    pub(in crate::session_agents) history: HistoryEffect,
}

/// Why supervision ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum CloseReason {
    /// The owning session requested close.
    Requested,
    /// The agent program returned normally.
    RunCompleted,
    /// The agent program failed.
    RunFailed,
}

/// One typed action selected by the transition model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) enum SupervisorEffect {
    /// Await a named condition.
    Wait(WaitFor),
    /// Cancel the current run with provenance.
    Cancel(CancelOrigin),
    /// Keep the named ownership unchanged.
    Preserve(PreserveReason),
    /// Launch a replacement over retained history.
    Relaunch(RelaunchEffect),
    /// End supervision.
    Close(CloseReason),
}

/// Whether the session is waiting, running, retiring, or closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    WaitingForCatalog,
    Running,
    Cancelling,
    Closed,
}

/// Pure state owned by one agent-session supervisor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) struct SupervisorState {
    phase: Phase,
    active_run: Option<RunId>,
    next_run: u64,
    catalog_generation: Option<u64>,
    observed_catalog_generation: Option<u64>,
    catalog_retirement_pending: bool,
    gateway_generation: u64,
    accepted_run: Option<RunId>,
}

impl SupervisorState {
    /// Starts supervision before a usable chat catalog exists.
    pub(in crate::session_agents) fn new(gateway_generation: u64) -> Self {
        Self {
            phase: Phase::WaitingForCatalog,
            active_run: None,
            next_run: 1,
            catalog_generation: None,
            observed_catalog_generation: None,
            catalog_retirement_pending: false,
            gateway_generation,
            accepted_run: None,
        }
    }
}

/// The next immutable state and its one typed effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::session_agents) struct SupervisorTransition {
    pub(in crate::session_agents) state: SupervisorState,
    pub(in crate::session_agents) effect: SupervisorEffect,
}

/// Reduces one explicit event without performing asynchronous work.
pub(in crate::session_agents) fn transition(
    state: SupervisorState,
    event: SupervisorEvent,
) -> SupervisorTransition {
    if state.phase == Phase::Closed {
        return changed(state, SupervisorEffect::Preserve(PreserveReason::Closed));
    }
    match event {
        SupervisorEvent::Close => close(state, CloseReason::Requested),
        SupervisorEvent::CatalogGeneration {
            generation,
            disposition,
        } => catalog_changed(state, generation, disposition),
        SupervisorEvent::GatewayGeneration(generation) => gateway_changed(state, generation),
        SupervisorEvent::OperatorCancellation => operator_cancelled(state),
        SupervisorEvent::AcceptedInput(run) => input_accepted(state, run),
        SupervisorEvent::TerminalSettlement(run) => turn_settled(state, run),
        SupervisorEvent::RunCompleted { run, result } => run_completed(state, run, result),
    }
}

fn catalog_changed(
    mut state: SupervisorState,
    generation: u64,
    disposition: CatalogDisposition,
) -> SupervisorTransition {
    if state
        .observed_catalog_generation
        .is_some_and(|observed| generation <= observed)
    {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        );
    }
    state.observed_catalog_generation = Some(generation);
    state.catalog_generation =
        (disposition != CatalogDisposition::Unavailable).then_some(generation);

    match state.phase {
        Phase::WaitingForCatalog => {
            if disposition == CatalogDisposition::Unavailable {
                return changed(state, SupervisorEffect::Wait(WaitFor::Catalog));
            }
            relaunch(state)
        }
        Phase::Running => match disposition {
            CatalogDisposition::Unavailable | CatalogDisposition::Retained => {
                let effect =
                    if state.catalog_retirement_pending && state.accepted_run == state.active_run {
                        SupervisorEffect::Wait(WaitFor::TerminalSettlement)
                    } else {
                        SupervisorEffect::Preserve(PreserveReason::CurrentRun)
                    };
                changed(state, effect)
            }
            CatalogDisposition::Replacement => {
                state.catalog_retirement_pending = true;
                if state.accepted_run == state.active_run {
                    changed(state, SupervisorEffect::Wait(WaitFor::TerminalSettlement))
                } else {
                    state.phase = Phase::Cancelling;
                    changed(state, SupervisorEffect::Cancel(CancelOrigin::Catalog))
                }
            }
        },
        Phase::Cancelling => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CancellationPending),
        ),
        Phase::Closed => changed(state, SupervisorEffect::Preserve(PreserveReason::Closed)),
    }
}

fn gateway_changed(mut state: SupervisorState, generation: u64) -> SupervisorTransition {
    if generation <= state.gateway_generation {
        let reason = if state.phase == Phase::Cancelling {
            PreserveReason::CancellationPending
        } else {
            PreserveReason::CurrentRun
        };
        return changed(state, SupervisorEffect::Preserve(reason));
    }
    state.gateway_generation = generation;
    match state.phase {
        Phase::WaitingForCatalog => changed(state, SupervisorEffect::Wait(WaitFor::Catalog)),
        Phase::Running => {
            state.accepted_run = None;
            state.phase = Phase::Cancelling;
            changed(state, SupervisorEffect::Cancel(CancelOrigin::Gateway))
        }
        Phase::Cancelling => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CancellationPending),
        ),
        Phase::Closed => changed(state, SupervisorEffect::Preserve(PreserveReason::Closed)),
    }
}

fn operator_cancelled(mut state: SupervisorState) -> SupervisorTransition {
    match state.phase {
        Phase::WaitingForCatalog => changed(state, SupervisorEffect::Wait(WaitFor::Catalog)),
        Phase::Running => {
            state.accepted_run = None;
            state.phase = Phase::Cancelling;
            changed(state, SupervisorEffect::Cancel(CancelOrigin::Operator))
        }
        Phase::Cancelling => changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CancellationPending),
        ),
        Phase::Closed => changed(state, SupervisorEffect::Preserve(PreserveReason::Closed)),
    }
}

fn input_accepted(mut state: SupervisorState, run: RunId) -> SupervisorTransition {
    if state.phase != Phase::Running || state.active_run != Some(run) {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        );
    }
    if state.accepted_run == Some(run) {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        );
    }
    state.accepted_run = Some(run);
    changed(
        state,
        SupervisorEffect::Preserve(PreserveReason::CurrentRun),
    )
}

fn turn_settled(mut state: SupervisorState, run: RunId) -> SupervisorTransition {
    if state.active_run != Some(run) {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        );
    }
    if state.phase == Phase::Cancelling {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CancellationPending),
        );
    }
    if state.accepted_run != Some(run) {
        return changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        );
    }
    state.accepted_run = None;
    if state.catalog_retirement_pending {
        state.phase = Phase::Cancelling;
        changed(state, SupervisorEffect::Cancel(CancelOrigin::Catalog))
    } else {
        changed(
            state,
            SupervisorEffect::Preserve(PreserveReason::CurrentRun),
        )
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
    state.accepted_run = None;
    match result {
        RunCompletion::Interrupted => relaunch(state),
        RunCompletion::Completed => close(state, CloseReason::RunCompleted),
        RunCompletion::Failed => close(state, CloseReason::RunFailed),
    }
}

fn relaunch(mut state: SupervisorState) -> SupervisorTransition {
    let Some(catalog_generation) = state.catalog_generation else {
        state.phase = Phase::WaitingForCatalog;
        state.catalog_retirement_pending = false;
        return changed(state, SupervisorEffect::Wait(WaitFor::Catalog));
    };
    let run = RunId(state.next_run);
    state.next_run = state.next_run.saturating_add(1);
    state.catalog_generation = Some(catalog_generation);
    state.active_run = Some(run);
    state.accepted_run = None;
    state.phase = Phase::Running;
    state.catalog_retirement_pending = false;
    let effect = RelaunchEffect {
        run,
        catalog_generation,
        gateway_generation: state.gateway_generation,
        history: HistoryEffect::Preserve,
    };
    changed(state, SupervisorEffect::Relaunch(effect))
}

fn close(mut state: SupervisorState, reason: CloseReason) -> SupervisorTransition {
    state.phase = Phase::Closed;
    state.active_run = None;
    state.accepted_run = None;
    state.catalog_generation = None;
    state.catalog_retirement_pending = false;
    changed(state, SupervisorEffect::Close(reason))
}

fn changed(state: SupervisorState, effect: SupervisorEffect) -> SupervisorTransition {
    SupervisorTransition { state, effect }
}

#[cfg(test)]
mod tests;
