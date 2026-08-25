//! The observer: a broadcast bus carrying status bar updates from every
//! subsystem to every connected `/ws` session.
//!
//! Anything with user-visible latency - startup phases, gateway round
//! trips, voice capture and transcription, model downloads - reports what
//! it is doing as a [`StatusBarUpdate`]. The bus is a tokio broadcast
//! channel: updates fan out to all current subscribers, a send with no
//! subscribers is a no-op, and a subscriber that falls more than
//! [`STATUS_CHANNEL_CAPACITY`] updates behind is told it lagged and resumes
//! at the oldest retained update. Sending never blocks, so instrumenting a
//! hot path cannot stall the subsystem it observes.
//!
//! On the wire each update rides the main chat socket as an unsolicited
//! `{"type":"status",...}` frame (see [`StatusBarUpdate::frame`]),
//! interleaving freely with a chat's `delta`/`done`/`error` replies.

use serde::Serialize;
use tokio::sync::broadcast;

/// Ring capacity of the status bus. Covers a startup burst plus a chat's
/// phase transitions with headroom; a receiver lagging past it skips ahead
/// rather than slowing the senders.
const STATUS_CHANNEL_CAPACITY: usize = 64;

/// One status bar update: what the bar should show right now.
///
/// Every update is a complete snapshot, so a lagging receiver loses nothing
/// by skipping intermediates. `label` is the short text rendered in the
/// status bar; `description` is the longer tooltip shown on hover.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct StatusBarUpdate {
    /// Short text rendered in the status bar.
    pub(crate) label: String,
    /// Longer text shown as the bar's tooltip.
    pub(crate) description: String,
    /// Determinate progress, when the activity can report it.
    pub(crate) progress: Option<Progress>,
    /// How loudly the update speaks; the UI ignores `Debug` updates.
    pub(crate) severity: Severity,
    /// Which subsystem is active, driving the bar's activity indicator.
    pub(crate) activity: Activity,
}

impl StatusBarUpdate {
    /// The update as a wire frame: its own fields plus `"type": "status"`.
    pub(crate) fn frame(&self) -> StatusFrame<'_> {
        StatusFrame {
            kind: "status",
            update: self,
        }
    }
}

/// A determinate progress report for the status bar's progress slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Progress {
    /// Units completed so far.
    pub(crate) current: u64,
    /// Units expected in total.
    pub(crate) total: u64,
}

/// How loudly a status update speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    /// User-visible status text.
    Info,
    /// Internal instrumentation; the UI ignores it for display.
    Debug,
    /// A failure the user should see.
    Error,
}

/// The subsystem an update belongs to, driving the activity indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Activity {
    /// No specific subsystem; the activity LED stays dark.
    General,
    /// A model turn in flight: amber on the activity LED.
    Thinking,
    /// Output tokens arriving: green on the activity LED.
    Generating,
}

/// The serialized shape of one update on the socket: the update's fields
/// flattened beside `"type": "status"`, matching the chat protocol's frame
/// taxonomy.
#[derive(Debug, Serialize)]
pub(crate) struct StatusFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    update: &'a StatusBarUpdate,
}

/// The shared status bus: a cloneable handle onto the broadcast channel.
///
/// Clones are cheap (an `Arc` bump) and all of them send into the same
/// channel, so subsystems take their own copy rather than a reference.
#[derive(Debug, Clone)]
pub(crate) struct StatusBus {
    sender: broadcast::Sender<StatusBarUpdate>,
}

impl StatusBus {
    /// Creates a bus with no subscribers and an empty ring.
    pub(crate) fn new() -> Self {
        Self {
            sender: broadcast::channel(STATUS_CHANNEL_CAPACITY).0,
        }
    }

    /// Subscribes to every update sent from this call onward.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<StatusBarUpdate> {
        self.sender.subscribe()
    }

    /// Broadcasts one update. With no subscribers this is a no-op; a slow
    /// subscriber skips ahead rather than applying backpressure.
    pub(crate) fn emit(&self, update: StatusBarUpdate) {
        // A send only fails when there are no receivers, which is the bus's
        // resting state before the first client connects.
        let _ = self.sender.send(update);
    }

    /// Broadcasts one progress-free update at the given severity.
    pub(crate) fn report(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        severity: Severity,
        activity: Activity,
    ) {
        self.emit(StatusBarUpdate {
            label: label.into(),
            description: description.into(),
            progress: None,
            severity,
            activity,
        });
    }

    /// Broadcasts a user-visible status text.
    pub(crate) fn info(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.report(label, description, Severity::Info, activity);
    }

    /// Broadcasts a user-visible update carrying determinate progress,
    /// which the status bar renders as its progress bar.
    pub(crate) fn progress(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        progress: Progress,
        activity: Activity,
    ) {
        self.emit(StatusBarUpdate {
            label: label.into(),
            description: description.into(),
            progress: Some(progress),
            severity: Severity::Info,
            activity,
        });
    }

    /// Broadcasts an internal instrumentation pulse the UI does not
    /// display.
    pub(crate) fn debug(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.report(label, description, Severity::Debug, activity);
    }

    /// Broadcasts a failure the user should see.
    pub(crate) fn error(
        &self,
        label: impl Into<String>,
        description: impl Into<String>,
        activity: Activity,
    ) {
        self.report(label, description, Severity::Error, activity);
    }

    /// Returns the bar to its resting state.
    pub(crate) fn idle(&self) {
        self.info("Ready", "idle", Activity::General);
    }
}

impl Default for StatusBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal update with the given label.
    fn stub(label: impl Into<String>) -> StatusBarUpdate {
        StatusBarUpdate {
            label: label.into(),
            description: String::new(),
            progress: None,
            severity: Severity::Info,
            activity: Activity::General,
        }
    }

    #[test]
    fn a_status_update_serializes_as_a_status_frame() {
        let frame = serde_json::to_value(stub("Ready").frame()).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "status",
                "label": "Ready",
                "description": "",
                "progress": null,
                "severity": "info",
                "activity": "general",
            }),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn progress_and_the_remaining_variants_serialize() {
        let update = StatusBarUpdate {
            progress: Some(Progress {
                current: 1,
                total: 2,
            }),
            severity: Severity::Error,
            activity: Activity::Thinking,
            ..stub("Working")
        };
        let frame = serde_json::to_value(update.frame()).expect("the frame serializes");
        assert_eq!(
            frame["progress"],
            serde_json::json!({"current": 1, "total": 2})
        );
        assert_eq!(frame["severity"], "error");
        assert_eq!(frame["activity"], "thinking");
        // Debug serializes too; the UI, not the bus, ignores it.
        let debug = serde_json::to_value(
            StatusBarUpdate {
                severity: Severity::Debug,
                activity: Activity::Generating,
                ..stub("x")
            }
            .frame(),
        )
        .expect("the frame serializes");
        assert_eq!(debug["severity"], "debug");
        assert_eq!(debug["activity"], "generating");
    }

    #[tokio::test]
    async fn emitting_with_no_subscribers_is_a_no_op() {
        let bus = StatusBus::new();
        bus.info("Ready", "idle", Activity::General);
    }

    #[tokio::test]
    async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
        let bus = StatusBus::new();
        let mut receiver = bus.subscribe();
        let sent = STATUS_CHANNEL_CAPACITY + 10;
        for index in 0..sent {
            // Sends never block, however far behind the receiver is.
            bus.debug(format!("update {index}"), "", Activity::General);
        }
        let lag = match receiver.recv().await {
            Err(broadcast::error::RecvError::Lagged(skipped)) => skipped,
            Ok(got) => panic!("expected a lag report, got {got:?}"),
            Err(broadcast::error::RecvError::Closed) => panic!("the bus is still open"),
        };
        assert_eq!(lag, 10, "the ring retained only its capacity");
        let resumed = receiver.recv().await.expect("the ring still holds updates");
        assert_eq!(
            resumed.label, "update 10",
            "receiving resumes at the oldest retained update"
        );
    }
}
