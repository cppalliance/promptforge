use std::time::Duration;

use promptforge_api_types::ids::{ChainId, Provenance, TaskId};
use workshop_protocol::Severity;
use workshop_registry::Registry;
use workshop_status::StatusBus;

use super::*;

/// A push facade wired to a real status bus through the registry, with
/// the bus's receiver and the registration guards a test reads through.
fn wired_push() -> (
    Push,
    broadcast::Receiver<workshop_protocol::StatusBarUpdate>,
    impl std::fmt::Debug + Send + Sync + 'static + use<>,
) {
    let status = StatusBus::new();
    let status_rx = status.subscribe();
    let registry = Registry::new();
    let guards = workshop_status::register(&registry, &status);
    (registry.push(), status_rx, guards)
}

/// A session event carrying one engine event under the fixed test
/// coordinates, in the persisted shape the relay reads.
fn session_event(event: &Event) -> SessionEvent {
    SessionEvent {
        index: 0,
        reply: None,
        event: serde_json::to_value(event).expect("an engine event serializes"),
    }
}

fn reply_event() -> Event {
    Event::AssistantReply {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: Provenance {
            task: TaskId::from(ChainId::root()),
            seq: 0,
        },
        turn: 1,
        text: "hello".to_owned(),
        finish_reason: None,
        model: "m".to_owned(),
        metrics: None,
    }
}

/// The three report shapes the session sends on its error channel: a
/// failed model turn and a failed tool call the program survived, and a
/// run that ended in error. Each must release the turn-dispatch Thinking
/// push with a terminal, non-thinking status; the survived turns keep
/// the boundary as their label because the agent is still running, and
/// only a run that ended reads `Agent failed`.
#[test]
fn every_error_report_pushes_a_terminal_failure_status() {
    let (push, mut status_rx, _guards) = wired_push();
    let reports = [
        ("Model turn failed in agent `chat`", "Model turn failed"),
        ("Tool call failed in agent `chat`", "Tool call failed"),
        ("agent run failed: kaboom", "Agent failed"),
        ("run cancelled", "Agent failed"),
    ];

    for (report, label) in reports {
        on_error(report, &push);
        let update = status_rx
            .try_recv()
            .expect("the report pushes a terminal status");
        assert_eq!(update.severity, Severity::Error, "report: {report}");
        assert_eq!(
            update.activity,
            Activity::General,
            "a non-thinking activity releases the status bar's sustained amber LED"
        );
        assert_eq!(
            update.label, label,
            "a survived turn is labeled by its boundary, a dead run by the agent: {report}"
        );
        assert_eq!(
            update.description, report,
            "the status carries the same message the socket's error frame does"
        );
    }
}

#[test]
fn a_completed_reply_pushes_idle_and_resets_the_backoff() {
    let (push, mut status_rx, _guards) = wired_push();
    let backoff = ReconnectBackoff::with_schedule(
        Duration::from_millis(1),
        Duration::from_millis(8),
        Duration::from_secs(1),
    );
    let _ = backoff.next_delay();
    assert!(
        backoff.is_escalated_for_test(),
        "a handed-out delay escalates the schedule"
    );

    on_event(&session_event(&reply_event()), &push, &backoff);

    let update = status_rx
        .try_recv()
        .expect("the reply pushes the idle status");
    assert_eq!(update.severity, Severity::Info);
    assert_eq!(update.activity, Activity::General);
    assert_eq!(update.label, "Ready");
    assert!(
        !backoff.is_escalated_for_test(),
        "an agent reply is useful gateway work and resets the backoff"
    );
}

#[test]
fn a_lifecycle_event_pushes_nothing() {
    let (push, mut status_rx, _guards) = wired_push();
    let backoff = ReconnectBackoff::new();
    let started = Event::SectionStarted {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: Provenance {
            task: TaskId::from(ChainId::root()),
            seq: 0,
        },
    };

    on_event(&session_event(&started), &push, &backoff);

    assert!(
        status_rx.try_recv().is_err(),
        "the status bar has nothing to say about a section starting"
    );
}
