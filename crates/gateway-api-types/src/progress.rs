//! The progress snapshot: the Gateway's one live activity, as a busy flag
//! and a producer-owned text.
//!
//! The Gateway streams one snapshot per change on `GET /admin/progress` and
//! embeds the current one in `GET /admin/status`. There are no fractions,
//! weights, or hierarchy on the wire: a producer that wants to show a
//! percentage formats it into the text itself.
//!
//! The text is user-visible in the Workshop status bar, the config UI, and
//! the tray, so a producer must never place a bearer key, API key, or other
//! credential in it.

use serde::{Deserialize, Serialize};

/// One snapshot of the Gateway's live activity.
///
/// Future additive fields carry `#[serde(default)]` so a lagging reader
/// survives them; there is no schema version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Whether any activity is live. The UIs show an indeterminate
    /// barberpole while this is set.
    pub busy: bool,
    /// The newest live activity's text, e.g. `"Downloading qwen3-8b.gguf
    /// 45%"`; empty when idle.
    pub text: String,
}

#[cfg(test)]
#[path = "progress-tests.rs"]
mod tests;
