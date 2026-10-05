//! A conversation's transcript: every event its run has reported, in the
//! order the conversation observed them, numbered from zero.
//!
//! The transcript lives in memory. The conversation appends each event
//! here before it broadcasts it, stamped with the index and reply id the
//! live subscriber receives, so a reconnecting client that reads the
//! transcript past its last seen index sees the same values the live
//! stream carried. A round's content events take their reply id from the
//! round itself, the id the round's deltas carry.

use std::sync::{Mutex, MutexGuard, PoisonError};

use promptforge::event::Event;

use crate::protocol::SessionEvent;

/// The events a conversation has observed, stamped.
pub(crate) struct Transcript {
    events: Mutex<Vec<SessionEvent>>,
}

impl Transcript {
    /// An empty transcript.
    pub(crate) fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    /// Stamps `event` with the next index and its round, appends it, and
    /// returns the stamped entry for the live broadcast.
    pub(crate) fn push(&self, event: &Event) -> SessionEvent {
        let mut events = self.lock();
        // The run serialized this same event for its recorder a moment
        // ago, so this cannot fail; `Null` keeps the index sequence whole
        // if it ever did.
        let stamped = SessionEvent {
            index: events.len() as u64,
            reply: reply_of(event),
            event: serde_json::to_value(event).unwrap_or(serde_json::Value::Null),
        };
        events.push(stamped.clone());
        stamped
    }

    /// The entries with an index of at least `from`, in order.
    pub(crate) fn since(&self, from: u64) -> Vec<SessionEvent> {
        let start = usize::try_from(from).unwrap_or(usize::MAX);
        self.lock()
            .get(start..)
            .map(<[_]>::to_vec)
            .unwrap_or_default()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<SessionEvent>> {
        // `push` leaves the vector whole between its pushes, so a poisoned
        // lock still holds a usable value.
        self.events.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The reply id of a model round's content event: the round that
/// produced it. Every other event settles no deltas.
fn reply_of(event: &Event) -> Option<u64> {
    match event {
        Event::Thinking { round, .. }
        | Event::AssistantReply { round, .. }
        | Event::AssistantToolCalls { round, .. } => Some(round.get()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "transcript-tests.rs"]
mod tests;
