//! The agent socket's framing helpers: the pure render functions that
//! map the harness's wait and delta vocabulary onto Workshop's wire
//! shapes, and the durable-event framing that drains a session's
//! transcript past the per-client cursor. Split out of the `socket`
//! module so each stays under the 500-line ceiling.

use axum::extract::ws::WebSocket;
use harness_api::{Delta, DeltaKind, SessionEvent, WaitFrame};
use promptforge::event::Event;
use workshop_protocol::{AgentDeltaFrame, AgentDeltaKind, AgentEventFrame, InputFrame};

use super::socket::Attached;
use crate::websocket::send_frame;

/// Renders a harness wait frame as the protocol's input frame: the one
/// place the harness's wait vocabulary meets Workshop's wire shape.
pub(crate) fn input_frame(frame: WaitFrame) -> InputFrame {
    match frame {
        WaitFrame::Required { token } => InputFrame::Required { token },
        WaitFrame::Cancelled { token } => InputFrame::Cancelled { token },
    }
}

/// Renders a harness delta as the protocol's delta frame, the reply stamp
/// passed through; `None` for a side channel the wire has no label for,
/// dropped like a lagged delta because the completed-reply event repairs
/// the transcript.
pub(crate) fn delta_frame(delta: Delta) -> Option<AgentDeltaFrame> {
    let channel = match delta.kind {
        DeltaKind::Text => AgentDeltaKind::Text,
        DeltaKind::Reasoning => AgentDeltaKind::Reasoning,
        // `DeltaKind` is `#[non_exhaustive]` in `harness-sessions`.
        _ => return None,
    };
    Some(AgentDeltaFrame::new(channel, delta.content, delta.reply))
}

/// Sends every transcript entry past the client's cursor as a durable
/// `agent_event` frame stamped with its wire index and, on the
/// model-round content kinds, the reply stamp its deltas had. A `false`
/// return means the client is gone.
pub(crate) async fn drain_events(attached: &mut Attached, socket: &mut WebSocket) -> bool {
    let transcript = match attached.session.transcript(attached.cursor).await {
        Ok(transcript) => transcript,
        Err(error) => {
            // The run log refused the read; the next wakeup retries from
            // the same cursor, so nothing is skipped.
            tracing::warn!(session = %attached.session.id(), %error, "transcript read failed");
            return true;
        }
    };
    for entry in &transcript {
        if !frame_entry(attached, entry, socket).await {
            return false;
        }
    }
    true
}

/// Frames one transcript entry at or past the cursor and advances the
/// cursor over it. An entry with no wire shape (lifecycle, task, and
/// debug events) advances the cursor without a frame or a wire index. A
/// `false` return means the client is gone.
pub(crate) async fn frame_entry(
    attached: &mut Attached,
    entry: &SessionEvent,
    socket: &mut WebSocket,
) -> bool {
    if entry.index < attached.cursor {
        return true;
    }
    attached.cursor = entry.index + 1;
    let Ok(event) = serde_json::from_value::<Event>(entry.event.clone()) else {
        // A stored payload this build cannot read has no wire shape
        // either; the transcript's index sequence stays whole.
        return true;
    };
    let Some(frame) = AgentEventFrame::new(attached.framed, entry.reply, &event) else {
        return true;
    };
    attached.framed += 1;
    send_frame(socket, &frame).await
}

#[cfg(test)]
#[path = "socket_frames-tests.rs"]
mod tests;
