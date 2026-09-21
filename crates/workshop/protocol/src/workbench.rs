//! Workbench frames: the `{"type":"workbench",...}` menu snapshots.

use serde::Serialize;

/// One pushed workbench snapshot: the server-owned Model-menu state.
///
/// The server computes `chat_ready` - a chat-capable model available, one
/// selected, no switch in flight, gateway reachable - and the UI never
/// derives it.
///
/// `switching` names the target when it is a profile; `switch_in_flight`
/// is true for any in-flight target, including no profile.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkbenchSnapshot {
    /// Every gateway profile name, in gateway order.
    pub profiles: Vec<String>,
    /// The profile the gateway is serving, once known.
    pub active: Option<String>,
    /// The profile a switch is loading, while one is in flight. `None`
    /// when no switch runs and when the in-flight switch selects no
    /// profile.
    pub switching: Option<String>,
    /// Whether a switch is in flight, whatever its target.
    pub switch_in_flight: bool,
    /// The model chat requests go to, once one is selected.
    pub selected_model: Option<String>,
    /// Whether a chat can be submitted right now.
    pub chat_ready: bool,
}

impl WorkbenchSnapshot {
    /// The snapshot as a wire frame: `"type": "workbench"` beside the
    /// fields, with `selected_model` shortened to `selected` on the wire.
    #[must_use]
    pub fn frame(&self) -> WorkbenchFrame<'_> {
        WorkbenchFrame {
            kind: "workbench",
            profiles: &self.profiles,
            active: self.active.as_deref(),
            switching: self.switching.as_deref(),
            switch_in_flight: self.switch_in_flight,
            selected: self.selected_model.as_deref(),
            chat_ready: self.chat_ready,
        }
    }
}

/// The serialized shape of a workbench push on the socket, matching the
/// workshop protocol's frame taxonomy. Absent options serialize as `null`,
/// never as omitted keys: every push is the complete menu state.
///
/// Delivery: ephemeral - every push is a complete snapshot of the menu
/// state, retained and resent on reconnect, like the catalog frame.
#[derive(Debug, Serialize)]
pub struct WorkbenchFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    profiles: &'a [String],
    active: Option<&'a str>,
    switching: Option<&'a str>,
    switch_in_flight: bool,
    selected: Option<&'a str>,
    chat_ready: bool,
}
