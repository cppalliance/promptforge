//! Tests for the supervisor transition table across start, cancel, completion, and close events.

use super::*;

const RUN_1: RunId = RunId(1);
const RUN_2: RunId = RunId(2);

fn completed(run: RunId, result: RunCompletion) -> SupervisorEvent {
    SupervisorEvent::RunCompleted { run, result }
}

fn relaunch(run: RunId) -> SupervisorEffect {
    SupervisorEffect::Relaunch(RelaunchEffect {
        run,
        history: HistoryEffect::Preserve,
    })
}

fn apply(events: &[SupervisorEvent]) -> (SupervisorState, Vec<SupervisorEffect>) {
    let mut state = SupervisorState::new();
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
        let (state, effects) = apply(&scenario.events);
        assert_eq!(effects, scenario.effects, "{}", scenario.name);
        assert_eq!(state.phase, scenario.phase, "{}", scenario.name);
    }
}

#[test]
fn transition_table_covers_start_cancel_and_relaunch_effects() {
    assert_scenarios(vec![
        Scenario {
            name: "start launches the first run",
            events: vec![SupervisorEvent::Start],
            effects: vec![relaunch(RUN_1)],
            phase: Phase::Running,
        },
        Scenario {
            name: "a repeated start keeps the current run",
            events: vec![SupervisorEvent::Start, SupervisorEvent::Start],
            effects: vec![
                relaunch(RUN_1),
                SupervisorEffect::Preserve(PreserveReason::CurrentRun),
            ],
            phase: Phase::Running,
        },
        Scenario {
            name: "a cancellation before start has no run to retire",
            events: vec![
                SupervisorEvent::OperatorCancellation,
                SupervisorEvent::Start,
            ],
            effects: vec![
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
                relaunch(RUN_1),
            ],
            phase: Phase::Running,
        },
        Scenario {
            name: "operator cancellation interrupts and relaunches",
            events: vec![
                SupervisorEvent::Start,
                SupervisorEvent::OperatorCancellation,
                completed(RUN_1, RunCompletion::Interrupted),
            ],
            effects: vec![relaunch(RUN_1), SupervisorEffect::Cancel, relaunch(RUN_2)],
            phase: Phase::Running,
        },
        Scenario {
            name: "a pending cancellation owns retirement through run completion",
            events: vec![
                SupervisorEvent::Start,
                SupervisorEvent::OperatorCancellation,
                SupervisorEvent::OperatorCancellation,
                SupervisorEvent::Start,
                completed(RUN_1, RunCompletion::Interrupted),
            ],
            effects: vec![
                relaunch(RUN_1),
                SupervisorEffect::Cancel,
                SupervisorEffect::Preserve(PreserveReason::CancellationPending),
                SupervisorEffect::Preserve(PreserveReason::CancellationPending),
                relaunch(RUN_2),
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
                SupervisorEvent::Start,
                completed(RUN_1, RunCompletion::Completed),
            ],
            effects: vec![
                relaunch(RUN_1),
                SupervisorEffect::Close(CloseReason::RunCompleted),
            ],
            phase: Phase::Closed,
        },
        Scenario {
            name: "failed run completion closes supervision",
            events: vec![
                SupervisorEvent::Start,
                completed(RUN_1, RunCompletion::Failed),
            ],
            effects: vec![
                relaunch(RUN_1),
                SupervisorEffect::Close(CloseReason::RunFailed),
            ],
            phase: Phase::Closed,
        },
        Scenario {
            name: "stale run events preserve ownership",
            events: vec![
                SupervisorEvent::Start,
                completed(RunId(99), RunCompletion::Interrupted),
                completed(RunId(99), RunCompletion::Completed),
            ],
            effects: vec![
                relaunch(RUN_1),
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
                SupervisorEffect::Preserve(PreserveReason::AlreadyHandled),
            ],
            phase: Phase::Running,
        },
    ]);
}

#[test]
fn close_effect_is_emitted_exactly_once() {
    let events = [
        SupervisorEvent::Start,
        SupervisorEvent::Close,
        SupervisorEvent::Close,
        SupervisorEvent::Start,
        completed(RUN_1, RunCompletion::Interrupted),
        completed(RUN_1, RunCompletion::Completed),
    ];
    let (state, effects) = apply(&events);

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
