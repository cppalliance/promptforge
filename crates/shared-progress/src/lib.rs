//! The gateway's live-activity hub: one busy flag and one line of text.
//!
//! A producer that starts slow work calls [`ProgressHub::begin`] with a
//! short user-facing text and holds the returned [`Activity`] for the
//! work's lifetime; it updates the text with [`Activity::set_text`] as the
//! work moves ("Downloading qwen 45%") and drops the guard on every exit
//! path. The hub publishes one [`Progress`] snapshot per change through a
//! `watch` channel: `busy` while any activity is live, `text` from the
//! newest live activity. There are no fractions, weights, or trees, and
//! nothing is replayed: a subscriber sees the current snapshot and every
//! later change. The crate never spawns tasks and never logs; producers
//! own their own tracing lines.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use gateway_api_types::Progress;
use tokio::sync::watch;

/// The process-wide activity broker, one per gateway.
///
/// Cheap to clone through an `Arc`; the hub lives in the host's
/// application state for the process lifetime and activities register
/// and remove themselves by their own lifetimes.
///
/// # Examples
///
/// ```
/// use shared_progress::ProgressHub;
///
/// let hub = ProgressHub::new();
/// assert!(!hub.current().busy);
/// let activity = hub.begin("Loading profile");
/// assert_eq!(hub.current().text, "Loading profile");
/// drop(activity);
/// assert!(!hub.current().busy);
/// ```
#[derive(Debug)]
pub struct ProgressHub {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// Live activities in begin order: `(id, text)`. The last entry is the
    /// one the snapshot shows.
    live: Mutex<Vec<(u64, String)>>,
    next_id: AtomicU64,
    tx: watch::Sender<Progress>,
}

impl Default for ProgressHub {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressHub {
    /// Creates an idle hub.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(Progress::default());
        Self {
            inner: Arc::new(Inner {
                live: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(0),
                tx,
            }),
        }
    }

    /// Starts an activity showing `text` and returns its guard. The hub is
    /// busy until every begun activity has dropped.
    pub fn begin(&self, text: impl Into<String>) -> Activity {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner.live().push((id, text.into()));
        self.inner.publish();
        Activity {
            inner: Arc::clone(&self.inner),
            id,
        }
    }

    /// The current snapshot.
    #[must_use]
    pub fn current(&self) -> Progress {
        self.inner.tx.borrow().clone()
    }

    /// Subscribes to every later change. The receiver starts holding the
    /// current snapshot as already seen; `changed()` resolves on the next
    /// publication.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Progress> {
        self.inner.tx.subscribe()
    }
}

impl Inner {
    /// A lock poisoned by a panicking peer recovers the value rather than
    /// wedging the process.
    fn live(&self) -> MutexGuard<'_, Vec<(u64, String)>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Recomputes the snapshot from the live set and publishes it when it
    /// differs from the last one.
    fn publish(&self) {
        let snapshot = {
            let live = self.live();
            Progress {
                busy: !live.is_empty(),
                text: live
                    .last()
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default(),
            }
        };
        self.tx.send_if_modified(|current| {
            if *current == snapshot {
                false
            } else {
                *current = snapshot;
                true
            }
        });
    }
}

/// One live activity: an RAII guard whose drop ends it and republishes.
///
/// `Send + Sync`, so a producer can move it into a blocking task or share
/// it behind an `Arc`; it is not `Clone`, because each guard's drop is the
/// end of exactly one activity.
#[derive(Debug)]
pub struct Activity {
    inner: Arc<Inner>,
    id: u64,
}

impl Activity {
    /// Replaces this activity's text. The snapshot changes only when this
    /// is the newest live activity.
    pub fn set_text(&self, text: impl Into<String>) {
        let text = text.into();
        {
            let mut live = self.inner.live();
            if let Some(entry) = live.iter_mut().find(|(id, _)| *id == self.id) {
                entry.1 = text;
            }
        }
        self.inner.publish();
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        self.inner.live().retain(|(id, _)| *id != self.id);
        self.inner.publish();
    }
}

#[cfg(test)]
#[path = "lib-tests.rs"]
mod tests;
