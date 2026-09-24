//! Agent status tests: which session events push a status frame and reset the backoff.

use std::time::Duration;

use promptforge::ids::{ChainId, Provenance, TaskId};
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

/// A session event holding one engine event under the fixed test
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
        origin: promptforge::event::ReplyOrigin::Chat,
    }
}

/// The four failure kinds the session reports on its error channel: a
/// failed model turn and a failed tool call the program survived, a run
/// that ended in error, and the synthetic terminal of an interrupt. Each
/// must release the turn-dispatch Thinking push with a terminal,
/// non-thinking status; the survived turns keep the boundary as their
/// label because the agent is still running, and only a run that ended
/// reads `Agent failed`. The label comes from the kind alone: the message
/// is deliberately unlike the label, so a relay that read the sentence
/// would mislabel every row.
#[test]
fn every_error_report_pushes_a_terminal_failure_status() {
    let (push, mut status_rx, _guards) = wired_push();
    let reports = [
        (
            FailureKind::ModelTurnFailed,
            "round 3 in agent `chat`",
            "Model turn failed",
        ),
        (
            FailureKind::ToolCallFailed,
            "call 7 in agent `chat`",
            "Tool call failed",
        ),
        (
            FailureKind::RunFailed,
            "agent run failed: kaboom",
            "Agent failed",
        ),
        (FailureKind::Interrupted, "run cancelled", "Agent failed"),
    ];

    for (kind, message, label) in reports {
        let failure = SessionFailure {
            kind,
            message: message.to_owned(),
        };
        on_error(&failure, &push);
        let update = status_rx
            .try_recv()
            .expect("the report pushes a terminal status");
        assert_eq!(update.severity, Severity::Error, "kind: {kind:?}");
        assert_eq!(
            update.activity,
            Activity::General,
            "a non-thinking activity releases the status bar's sustained amber LED"
        );
        assert_eq!(
            update.label, label,
            "the label is chosen by the kind, never by the sentence: {kind:?}"
        );
        assert_eq!(
            update.description, message,
            "the message passes through unchanged as the status description"
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
