//! The workshop's run event log: an append-only in-memory log of the
//! engine's [`Event`] values with live broadcast fan-out.
//!
//! The engine reports every boundary of a run as an [`Event`] value the
//! host receives from its run loop; a session appends the ones its
//! transcript shows here, reads them back by index for a socket's
//! per-client cursor, and wakes attached sockets through the broadcast.
//! The log is memory-only: nothing persists across a server restart. The
//! Turso run log the harness brings takes over durable storage, and this
//! type serves reconnect until then.

use std::fmt;
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use promptforge_api_types::event::Event;
use tokio::sync::broadcast;

/// Capacity of the broadcast channel behind
/// [`WorkshopObserver::subscribe`]. A receiver that falls further behind
/// misses the overwritten entries and recovers them by index through the
/// log itself, which retains every entry.
const BROADCAST_CAPACITY: usize = 256;

/// The workshop's append-only run event log.
///
/// One instance records one session's [`Event`]s. [`append`](Self::append)
/// is the write side, [`len`](Self::len) and [`get`](Self::get) the
/// indexed read side, and [`subscribe`](Self::subscribe) fans every
/// appended entry out live. Entry order and broadcast order agree because
/// both advance under the same write guard, so an index once valid stays
/// valid and its entry never changes.
///
/// A lock poisoned by a panicking peer recovers the value rather than
/// wedging the process (the crate's zone-two posture).
pub struct WorkshopObserver {
    /// The append-only in-memory log; an index once valid stays valid.
    events: RwLock<Vec<Event>>,
    /// The live fan-out; entries are sent under the write guard, so
    /// receivers observe log order.
    sender: broadcast::Sender<Event>,
}

impl WorkshopObserver {
    /// Opens a fresh, empty log.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::event::Event;
    /// use promptforge_api_types::ids::{ChainId, Provenance, TaskId};
    /// use workshop_gateway::WorkshopObserver;
    ///
    /// let log = WorkshopObserver::new();
    /// log.append(Event::UserInput {
    ///     execution: "run".to_owned(),
    ///     section: "chat".to_owned(),
    ///     provenance: Provenance { task: TaskId::from(ChainId::root()), seq: 0 },
    ///     text: "hello".to_owned(),
    /// });
    /// assert_eq!(log.len(), 1);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            events: RwLock::new(Vec::new()),
            sender: broadcast::channel(BROADCAST_CAPACITY).0,
        }
    }

    /// Appends one event to the log and to the broadcast, under the one
    /// write guard so the two orders agree.
    pub fn append(&self, event: Event) {
        let mut events = self.write();
        events.push(event.clone());
        // A send without receivers is the channel's resting state, not a
        // fault; entries stay readable by index regardless.
        let _ = self.sender.send(event);
    }

    /// Returns the number of events recorded so far.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.read().len() as u64
    }

    /// Returns whether no event has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read().is_empty()
    }

    /// Returns the event at `index`, or `None` at or past
    /// [`len`](Self::len). The log is append-only, so every index below a
    /// witnessed `len()` reads.
    #[must_use]
    pub fn get(&self, index: u64) -> Option<Event> {
        let events = self.read();
        usize::try_from(index)
            .ok()
            .and_then(|index| events.get(index).cloned())
    }

    /// Subscribes to every entry appended from this call on.
    ///
    /// Entries arrive in log order, each sent after it is readable
    /// through [`get`](Self::get). Earlier entries never replay here -
    /// read them by index instead - and a receiver that lags past the
    /// channel capacity misses the overwritten entries and recovers them
    /// the same way.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::event::Event;
    /// use promptforge_api_types::ids::{ChainId, Provenance, TaskId};
    /// use workshop_gateway::WorkshopObserver;
    ///
    /// let log = WorkshopObserver::new();
    /// let mut entries = log.subscribe();
    /// log.append(Event::UserInput {
    ///     execution: "run".to_owned(),
    ///     section: "chat".to_owned(),
    ///     provenance: Provenance { task: TaskId::from(ChainId::root()), seq: 0 },
    ///     text: "hello".to_owned(),
    /// });
    /// let Event::UserInput { text, .. } = entries.try_recv()? else {
    ///     panic!("the appended entry broadcasts");
    /// };
    /// assert_eq!(text, "hello");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }

    /// The read guard, recovering a lock poisoned by a panicking peer
    /// rather than wedging the process (the crate's zone-two policy).
    fn read(&self) -> RwLockReadGuard<'_, Vec<Event>> {
        self.events.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// The write guard; the same poison recovery as [`Self::read`].
    fn write(&self) -> RwLockWriteGuard<'_, Vec<Event>> {
        self.events.write().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for WorkshopObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for WorkshopObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkshopObserver")
            .field("len", &self.read().len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[path = "observer-tests.rs"]
mod tests;
