use super::*;

const RUN_1: RunId = RunId(1);
const RUN_2: RunId = RunId(2);

fn catalog(generation: u64, disposition: CatalogDisposition) -> SupervisorEvent {
    SupervisorEvent::CatalogGeneration {
        generation,
        disposition,
    }
}

fn completed(run: RunId, result: RunCompletion) -> SupervisorEvent {
    SupervisorEvent::RunCompleted { run, result }
}

fn relaunch(run: RunId, catalog: u64, gateway: u64) -> SupervisorEffect {
    SupervisorEffect::Relaunch(RelaunchEffect {
        run,
        catalog_generation: catalog,
        gateway_generation: gateway,
        history: HistoryEffect::Preserve,
    })
}

fn apply(gateway: u64, events: &[SupervisorEvent]) -> (SupervisorState, Vec<SupervisorEffect>) {
    let mut state = SupervisorState::new(gateway);
    let effects = events
        .iter()
        .map(|event| {
            let next = transition(state, *event);
            state = next.state;
            next.effect
        })
        .collect();
    (state, effects)
}

struct Scenario {
    name: &'static str,
    events: Vec<SupervisorEvent>,
    effects: Vec<SupervisorEffect>,
    phase: Phase,
}

fn assert_scenarios(scenarios: Vec<Scenario>) {
    for scenario in scenarios {
        let (state, effects) = apply(7, &scenario.events);
        assert_eq!(effects, scenario.effects, "{}", scenario.name);
        assert_eq!(state.phase, scenario.phase, "{}", scenario.name);
    }
}

#[test]
fn transition_table_covers_wait_cancel_and_relaunch_effects() {
    assert_scenarios(vec![
        Scenario {
            name: "delayed catalog follows the latest gateway",
            events: vec![
                catalog(1, CatalogDisposition::Unavailable),
                SupervisorEvent::GatewayGeneration(8),
                catalog(2, CatalogDisposition::Retained),
            ],
            effects: vec![
                SupervisorEffect::Wait(WaitFor::Catalog),
                SupervisorEffect::Wait(WaitFor::Catalog),
                relaunch(RUN_1, 2, 8),
            ],
            phase: Phase::Running,
        },
        Scenario {
            name: "overlapping catalog retirement relaunches the newest applicable generation",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                catalog(2, CatalogDisposition::Replacement),
                catalog(3, CatalogDisposition::Retained),
                completed(RUN_1, RunCompletion::Interrupted),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Cancel(CancelOrigin::Catalog),
                SupervisorEffect::Preserve(PreserveReason::CancellationPending),
                relaunch(RUN_2, 3, 7),
            ],
            phase: Phase::Running,
        },
        Scenario {
            name: "accepted input defers catalog retirement until settlement",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                SupervisorEvent::AcceptedInput(RUN_1),
                catalog(2, CatalogDisposition::Replacement),
                SupervisorEvent::TerminalSettlement(RUN_1),
                completed(RUN_1, RunCompletion::Interrupted),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Preserve(PreserveReason::CurrentRun),
                SupervisorEffect::Wait(WaitFor::TerminalSettlement),
                SupervisorEffect::Cancel(CancelOrigin::Catalog),
                relaunch(RUN_2, 2, 7),
            ],
            phase: Phase::Running,
        },
    ]);
}

#[test]
fn transition_table_covers_preservation_and_immediate_retirement() {
    assert_scenarios(vec![
        Scenario {
            name: "unavailable and retained catalogs preserve a running generation",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                catalog(2, CatalogDisposition::Unavailable),
                catalog(3, CatalogDisposition::Retained),
                SupervisorEvent::TerminalSettlement(RUN_1),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Preserve(PreserveReason::CurrentRun),
                SupervisorEffect::Preserve(PreserveReason::CurrentRun),
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
            ],
            phase: Phase::Running,
        },
        Scenario {
            name: "gateway replacement interrupts accepted input immediately",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                SupervisorEvent::AcceptedInput(RUN_1),
                SupervisorEvent::GatewayGeneration(8),
                SupervisorEvent::TerminalSettlement(RUN_1),
                completed(RUN_1, RunCompletion::Interrupted),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Preserve(PreserveReason::CurrentRun),
                SupervisorEffect::Cancel(CancelOrigin::Gateway),
                SupervisorEffect::Preserve(PreserveReason::CancellationPending),
                relaunch(RUN_2, 1, 8),
            ],
            phase: Phase::Running,
        },
        Scenario {
            name: "operator cancellation interrupts and relaunches",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                SupervisorEvent::OperatorCancellation,
                completed(RUN_1, RunCompletion::Interrupted),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Cancel(CancelOrigin::Operator),
                relaunch(RUN_2, 1, 7),
            ],
            phase: Phase::Running,
        },
    ]);
}

#[test]
fn transition_table_covers_terminal_close_and_stale_events() {
    assert_scenarios(vec![
        Scenario {
            name: "normal run completion closes supervision",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                completed(RUN_1, RunCompletion::Completed),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Close(CloseReason::RunCompleted),
            ],
            phase: Phase::Closed,
        },
        Scenario {
            name: "failed run completion closes supervision",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                completed(RUN_1, RunCompletion::Failed),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Close(CloseReason::RunFailed),
            ],
            phase: Phase::Closed,
        },
        Scenario {
            name: "close settles a delayed supervisor",
            events: vec![
                catalog(1, CatalogDisposition::Unavailable),
                SupervisorEvent::Close,
                catalog(2, CatalogDisposition::Retained),
            ],
            effects: vec![
                SupervisorEffect::Wait(WaitFor::Catalog),
                SupervisorEffect::Close(CloseReason::Requested),
                SupervisorEffect::Preserve(PreserveReason::Closed),
            ],
            phase: Phase::Closed,
        },
        Scenario {
            name: "stale run events and current gateway preserve ownership",
            events: vec![
                catalog(1, CatalogDisposition::Retained),
                SupervisorEvent::AcceptedInput(RunId(99)),
                SupervisorEvent::TerminalSettlement(RunId(99)),
                completed(RunId(99), RunCompletion::Interrupted),
                SupervisorEvent::GatewayGeneration(7),
            ],
            effects: vec![
                relaunch(RUN_1, 1, 7),
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
                SupervisorEffect::Preserve(PreserveReason::CurrentRun),
            ],
            phase: Phase::Running,
        },
    ]);
}

#[test]
fn deferred_catalog_settlement_cancels_and_relaunches_exactly_once() {
    let events = [
        catalog(1, CatalogDisposition::Retained),
        SupervisorEvent::AcceptedInput(RUN_1),
        catalog(2, CatalogDisposition::Replacement),
        catalog(2, CatalogDisposition::Replacement),
        SupervisorEvent::TerminalSettlement(RUN_1),
        SupervisorEvent::TerminalSettlement(RUN_1),
        completed(RUN_1, RunCompletion::Interrupted),
        completed(RUN_1, RunCompletion::Interrupted),
    ];
    let (state, effects) = apply(7, &events);
    assert_eq!(
        effects
            .iter()
            .filter(|effect| **effect == SupervisorEffect::Cancel(CancelOrigin::Catalog))
            .count(),
        1,
        "duplicate generations and terminal events cannot cancel twice"
    );
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, SupervisorEffect::Relaunch(_)))
            .count(),
        2,
        "one initial run and one replacement run launch"
    );
    assert_eq!(state.active_run, Some(RUN_2));
    assert_eq!(state.catalog_generation, Some(2));
}

#[test]
fn overlapping_retirement_causes_cancel_only_the_owned_run() {
    let events = [
        catalog(1, CatalogDisposition::Retained),
        SupervisorEvent::GatewayGeneration(8),
        SupervisorEvent::GatewayGeneration(9),
        SupervisorEvent::OperatorCancellation,
        catalog(2, CatalogDisposition::Replacement),
        completed(RUN_1, RunCompletion::Interrupted),
    ];
    let (state, effects) = apply(7, &events);
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, SupervisorEffect::Cancel(_)))
            .count(),
        1,
        "the first retirement owns cancellation through run completion"
    );
    assert_eq!(
        effects.last(),
        Some(&relaunch(RUN_2, 2, 9)),
        "the one replacement consumes the latest catalog and Gateway generations"
    );
    assert_eq!(state.active_run, Some(RUN_2));
}

#[test]
fn terminal_settlement_is_scoped_to_the_run_that_accepted_input() {
    let events = [
        catalog(1, CatalogDisposition::Retained),
        SupervisorEvent::OperatorCancellation,
        completed(RUN_1, RunCompletion::Interrupted),
        SupervisorEvent::AcceptedInput(RUN_2),
        catalog(2, CatalogDisposition::Replacement),
        SupervisorEvent::TerminalSettlement(RUN_1),
        SupervisorEvent::TerminalSettlement(RUN_2),
        SupervisorEvent::TerminalSettlement(RUN_2),
    ];
    let (state, effects) = apply(7, &events);

    assert_eq!(
        effects[5],
        SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
        "a stale terminal event cannot settle the current run"
    );
    assert_eq!(
        effects
            .iter()
            .filter(|effect| **effect == SupervisorEffect::Cancel(CancelOrigin::Catalog))
            .count(),
        1,
        "the accepted run's terminal event retires it once"
    );
    assert_eq!(state.phase, Phase::Cancelling);
    assert_eq!(state.accepted_run, None);
}

#[test]
fn close_effect_is_emitted_exactly_once() {
    let events = [
        catalog(1, CatalogDisposition::Retained),
        SupervisorEvent::Close,
        SupervisorEvent::Close,
        completed(RUN_1, RunCompletion::Interrupted),
        completed(RUN_1, RunCompletion::Completed),
    ];
    let (state, effects) = apply(7, &events);

    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, SupervisorEffect::Close(_)))
            .count(),
        1,
        "close owns terminal settlement despite later run notifications"
    );
    assert_eq!(state.phase, Phase::Closed);
    assert_eq!(state.active_run, None);
}
