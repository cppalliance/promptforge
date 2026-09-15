//! Workbench frames: the `{"type":"workbench",...}` menu snapshots.

use serde::Serialize;

/// One pushed workbench snapshot: the server-owned Model-menu state.
///
/// The server computes `chat_ready` - a chat-capable model available, one
/// selected, no switch in flight, gateway reachable - and the UI never
/// derives it.
///
/// Known gap: a switch to no profile is in flight only as `chat_ready:
/// false`. `switching` is `null` for it, the same as no switch at all, so
/// a UI reading `switching` alone shows no pending mark and leaves the
/// profile rows enabled; a second selection while it runs is refused by
/// the server. The frame keeps its shape deliberately; the server-side
/// refusal is the guard.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkbenchSnapshot {
    /// Every gateway profile name, in gateway order.
    pub profiles: Vec<String>,
    /// The profile the gateway is serving, once known.
    pub active: Option<String>,
    /// The profile a switch is loading, while one is in flight. `None`
    /// both when no switch runs and when the in-flight switch selects no
    /// profile; only `chat_ready` distinguishes the two.
    pub switching: Option<String>,
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
/// state, retained and resent on reconnect, exactly like the catalog
/// frame.
#[derive(Debug, Serialize)]
pub struct WorkbenchFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    profiles: &'a [String],
    active: Option<&'a str>,
    switching: Option<&'a str>,
    selected: Option<&'a str>,
    chat_ready: bool,
}
