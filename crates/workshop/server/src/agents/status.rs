//! The shell's status relay for one agent session: the status-bar frames
//! and the backoff reset the session's run used to push from inside the
//! sessions crate, now derived on this side of the harness door from the
//! session's live events, deltas, and error reports.
//!
//! One relay task per session, spawned at launch. It holds only the
//! session's broadcast receivers, never the session handle, so it ends by
//! itself when the harness lets the session go and the last socket
//! detaches: the channels close, and the loop returns.

use harness_api::{Delta, DeltaKind, SessionEvent};
use promptforge_api_types::event::Event;
use tokio::sync::broadcast;
use workshop_protocol::Activity;
use workshop_registry::Push;
use workshop_support::ReconnectBackoff;

/// Spawns the relay for `session`, reporting through `push` and resetting
/// `backoff` on completed replies.
pub(super) fn spawn_relay(session: &harness_api::Session, push: Push, backoff: ReconnectBackoff) {
    let events = session.subscribe_events();
    let deltas = session.subscribe_deltas();
    let errors = session.subscribe_errors();
    tokio::spawn(relay(events, deltas, errors, push, backoff));
}

/// Relays until the session's channels close. Deltas are drained ahead of
/// events, so a round's activity pulses precede the idle its reply
/// pushes when both sit queued.
async fn relay(
    mut events: broadcast::Receiver<SessionEvent>,
    mut deltas: broadcast::Receiver<Delta>,
    mut errors: broadcast::Receiver<String>,
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
                Ok(message) => on_error(&message, &push),
                // Reports are ephemeral like the deltas; a lagged receiver
                // missed a failure the error frame already carried.
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
        DeltaKind::Text => Activity::Generating,
        DeltaKind::Reasoning => Activity::Thinking,
    };
    push.push_activity("Streaming response...", "an agent response chunk", activity);
}

/// The side effects the shell wires to a completed reply: the backoff
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

/// The status labels for the two turn failures the program survives. The
/// session's report for a survived turn opens with the boundary that
/// failed (`Model turn failed in agent ...`), the same text the socket's
/// error frame carries, and the label repeats that boundary so the status
/// bar tells a still-running agent from one whose run ended.
const SURVIVED_TURN_LABELS: [&str; 2] = ["Model turn failed", "Tool call failed"];

/// The label for a run that ended in error or the synthetic terminal of
/// an interrupt: the agent itself is gone.
const RUN_FAILED_LABEL: &str = "Agent failed";

/// The operator-facing failure status for one of the session's error
/// reports: a failed model turn or tool call the program survived, a run
/// that ended in error, or the synthetic terminal of an interrupt. Each
/// is terminal for its turn and never reaches a reply, so this status is
/// the one frame that releases the turn-dispatch Thinking push; without
/// it the status bar's sustained amber LED never returns to idle. The
/// session reports every failure here, engine-side or harness-side, so
/// the status bar and the socket's error frame always agree.
fn on_error(message: &str, push: &Push) {
    push.push_failure(failure_label(message), message, Activity::General);
}

/// The boundary a survived-turn report names, else the run-failed label.
fn failure_label(message: &str) -> &'static str {
    SURVIVED_TURN_LABELS
        .into_iter()
        .find(|label| message.starts_with(label))
        .unwrap_or(RUN_FAILED_LABEL)
}

#[cfg(test)]
#[path = "status-tests.rs"]
mod tests;
