//! Side-channel proxies that forward a fanout arm's observation and debug
//! traffic to the parent task over bounded channels.

use tokio::sync::mpsc;

use crate::debug::{DebugCapture, DebugEvent};
use crate::observe::{Observation, Observer};

/// Bound on each fanout side channel (observation and debug).
///
/// Sized to absorb normal bursts while capping worst-case queued memory. On
/// overload the proxies drop events instead of blocking, so this bound never
/// changes execution results - only best-effort report completeness.
pub(crate) const SIDE_CHANNEL_CAPACITY: usize = 256;

pub(crate) struct ProxyObserver {
    pub(crate) tx: mpsc::Sender<(String, Observation)>,
}

impl Observer for ProxyObserver {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        // Report-only: never block an arm on a slow/full/closed consumer. A full
        // channel drops this event; the parent may also have returned already
        // after a fail-fast drain/drop. Neither can alter execution results.
        let _ = self.tx.try_send((section.to_owned(), event));
    }
}

pub(crate) struct DebugMsg {
    pub(crate) section: String,
    pub(crate) turn_index: u32,
    pub(crate) event: DebugEvent,
}

pub(crate) struct ProxyDebugCapture {
    pub(crate) tx: mpsc::Sender<DebugMsg>,
}

impl DebugCapture for ProxyDebugCapture {
    fn on_event(&self, _execution: &str, section: &str, turn_index: u32, event: DebugEvent) {
        // Report-only: a full or closed channel drops this event rather than
        // blocking the arm, so debug back-pressure cannot alter execution.
        let _ = self.tx.try_send(DebugMsg {
            section: section.to_owned(),
            turn_index,
            event,
        });
    }
}
