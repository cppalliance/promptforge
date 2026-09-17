//! Status-bar frames: the `{"type":"status",...}` updates.

use serde::Serialize;

/// One status bar update: what the bar should show right now.
///
/// Every update is a complete snapshot, so a lagging receiver loses nothing
/// by skipping intermediates. `label` is the short text rendered in the
/// status bar; `description` is the longer tooltip shown on hover.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusBarUpdate {
    /// Short text rendered in the status bar.
    pub label: String,
    /// Longer text shown as the bar's tooltip.
    pub description: String,
    /// Determinate progress, when the activity can report it.
    pub progress: Option<Progress>,
    /// How loudly the update speaks; the UI ignores `Debug` updates.
    pub severity: Severity,
    /// Which subsystem is active, driving the bar's activity indicator.
    pub activity: Activity,
}

impl StatusBarUpdate {
    /// The update as a wire frame: its own fields plus `"type": "status"`.
    #[must_use]
    pub fn frame(&self) -> StatusFrame<'_> {
        StatusFrame {
            kind: "status",
            update: self,
        }
    }
}

/// A determinate progress report for the status bar's progress slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Units completed so far.
    pub current: u64,
    /// Units expected in total.
    pub total: u64,
}

/// How loudly a status update speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
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
pub enum Activity {
    /// No specific subsystem; the activity LED stays dark.
    General,
    /// A model turn in flight: amber on the activity LED.
    Thinking,
    /// Output tokens arriving: green on the activity LED.
    Generating,
}

/// The serialized shape of one update on the socket: the update's fields
/// flattened beside `"type": "status"`, matching the workshop protocol's frame
/// taxonomy.
///
/// Delivery: ephemeral - every update is a complete snapshot, so a
/// lagging client skips intermediates and the current status is resent
/// on reconnect.
#[derive(Debug, Serialize)]
pub struct StatusFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    update: &'a StatusBarUpdate,
}
