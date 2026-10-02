//! A session's transcript: every event of every run the session has made,
//! in the order the session observed them, numbered from zero across
//! relaunches.
//!
//! The transcript lives in memory. The session appends each event here
//! before it broadcasts it, stamped with the index and reply id the live
//! subscriber receives, so a reconnecting client that reads the transcript
//! past its last seen index sees the same values the live stream carried.
//!
//! Reply ids coalesce deltas: every live delta is stamped with the id of
//! the durable event that will supersede it. The id is the count of
//! settled model rounds, which [`Transcript::push`] advances as the reply
//! or tool-call event lands, through the one rule [`reply_stamp`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use promptforge::event::Event;

use crate::protocol::SessionEvent;

/// The events a session has observed, stamped, and the count of settled
/// model rounds the deltas are stamped with.
pub(crate) struct Transcript {
    events: Mutex<Vec<SessionEvent>>,
    /// Settled model rounds: the reply id deltas are stamped with.
    rounds: AtomicU64,
}

impl Transcript {
    /// An empty transcript with no settled round.
    pub(crate) fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            rounds: AtomicU64::new(0),
        }
    }

    /// Stamps `event` with the next index and its reply id, appends it,
    /// and returns the stamped entry for the live broadcast. The round
    /// count is settled before this returns, so a client woken by the
    /// broadcast reads the count the event left behind.
    pub(crate) fn push(&self, event: &Event) -> SessionEvent {
        let mut events = self.lock();
        let mut rounds = self.rounds.load(Ordering::SeqCst);
        let reply = reply_stamp(event, &mut rounds);
        self.rounds.store(rounds, Ordering::SeqCst);
        // The run loop serialized this same event for its recorder a
        // moment ago, so this cannot fail; `Null` keeps the index
        // sequence whole if it ever did.
        let stamped = SessionEvent {
            index: events.len() as u64,
            reply,
            event: serde_json::to_value(event).unwrap_or(serde_json::Value::Null),
        };
        events.push(stamped.clone());
        stamped
    }

    /// The count of settled model rounds: the reply id a live delta takes.
    pub(crate) fn rounds(&self) -> u64 {
        self.rounds.load(Ordering::SeqCst)
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

/// The reply-id rule: the model-round content kinds are stamped with the
/// current round count, and a reply or tool-call batch advances it.
#[must_use]
pub fn reply_stamp(event: &Event, rounds_seen: &mut u64) -> Option<u64> {
    match event {
        Event::Thinking { .. } => Some(*rounds_seen),
        Event::AssistantReply { .. } | Event::AssistantToolCalls { .. } => {
            let round = *rounds_seen;
            *rounds_seen += 1;
            Some(round)
        }
        _ => None,
    }
}
