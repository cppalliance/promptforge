//! The server's status reporter for one agent session: the status-bar frames
//! and the backoff reset, derived in the server from the session's live
//! events, deltas, and error reports.
//!
//! One reporter task per session, spawned at launch. It holds only the
//! session's broadcast receivers, never the session handle, so it ends by
//! itself when the harness lets the session go and the last socket
//! detaches: the channels close, and the loop returns.

use harness_api::{Delta, DeltaKind, FailureKind, SessionEvent, SessionFailure};
use promptforge::event::Event;
use tokio::sync::broadcast;
use workshop_protocol::Activity;
use workshop_registry::Push;
use workshop_support::ReconnectBackoff;

/// Spawns the reporter for `session`, reporting through `push` and resetting
/// `backoff` on completed replies.
pub(super) fn spawn_reporter(
    session: &harness_api::Session,
    push: Push,
    backoff: ReconnectBackoff,
) {
    let events = session.subscribe_events();
    let deltas = session.subscribe_deltas();
    let errors = session.subscribe_errors();
    tokio::spawn(report(events, deltas, errors, push, backoff));
}

/// Reports until the session's channels close. Deltas are drained ahead of
/// events, so a round's activity pulses precede the idle its reply
/// pushes when both sit queued.
async fn report(
    mut events: broadcast::Receiver<SessionEvent>,
    mut deltas: broadcast::Receiver<Delta>,
    mut errors: broadcast::Receiver<SessionFailure>,
    push: Push,
    backoff: ReconnectBackoff,
) {
    loop {
        tokio::select! {
            biased;
            received = deltas.recv() => match received {
                Ok(delta) => on_delta(&delta, &push),
                // Pulses are ephemeral: a lost chunk's LED state is
                // repaired by the next one or by the reply's idle.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            received = events.recv() => match received {
                Ok(event) => on_event(&event, &push, &backoff),
                // A lagged receiver missed at most a status transition the
                // next event restates; the transcript itself is the log's.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
            received = errors.recv() => match received {
                Ok(failure) => on_error(&failure, &push),
                // Reports are ephemeral like the deltas; a lagged receiver
                // missed a failure the error frame already reported.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            },
        }
    }
}

/// The activity pulse that lights the status LED: Generating for answer
/// content, Thinking for the reasoning side channel.
fn on_delta(delta: &Delta, push: &Push) {
    let activity = match delta.kind {
        DeltaKind::Reasoning => Activity::Thinking,
        // `Text`, or a side channel `harness-sessions` adds behind its
        // `#[non_exhaustive]` `DeltaKind`: the agent is producing output.
        _ => Activity::Generating,
    };
    push.push_activity("Streaming response...", "an agent response chunk", activity);
}

/// The side effects the server wires to a completed reply: the backoff
/// reset (an agent reply is useful gateway work) and the idle status
/// that releases the turn-dispatch Thinking push.
fn on_event(event: &SessionEvent, push: &Push, backoff: &ReconnectBackoff) {
    let Ok(event) = serde_json::from_value::<Event>(event.event.clone()) else {
        // A stored payload this build cannot read is the log's concern;
        // the status bar has nothing to say about it.
        return;
    };
    if let Event::AssistantReply { .. } = event {
        backoff.record_useful_work();
        push.push_idle();
    }
}

/// The label for a run that ended in error or the synthetic terminal of
/// an interrupt: the agent itself is gone.
const RUN_FAILED_LABEL: &str = "Agent failed";

/// The operator-facing failure status for one of the session's failure
/// reports. The session reports the kind - a failed model turn or tool
/// call the program survived, a run that ended in error, or the synthetic
/// terminal of an interrupt - and the server labels it; the report's
/// message passes through as the description, the same text the socket's
/// error frame reports. Each kind is terminal for its turn and never
/// reaches a reply, so this status is the one frame that releases the
/// turn-dispatch Thinking push; without it the status bar's sustained
/// amber LED never returns to idle.
fn on_error(failure: &SessionFailure, push: &Push) {
    push.push_failure(
        failure_label(failure.kind),
        &failure.message,
        Activity::General,
    );
}

/// The status label for one failure kind: a survived turn is labelled by
/// its boundary, so the status bar tells a still-running agent from one
/// whose run ended. The match is exhaustive on purpose: a new kind fails
/// this build until it is labelled here.
fn failure_label(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::ModelTurnFailed => "Model turn failed",
        FailureKind::ToolCallFailed => "Tool call failed",
        FailureKind::RunFailed | FailureKind::Interrupted => RUN_FAILED_LABEL,
    }
}

#[cfg(test)]
#[path = "status-tests.rs"]
mod tests;
