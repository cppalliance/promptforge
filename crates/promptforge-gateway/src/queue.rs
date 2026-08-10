//! Per-endpoint admission control: concurrency limits and a fair waiting queue.
//!
//! `max_depth` is the number of *waiting* requests allowed (not counting
//! in-flight). When an endpoint omits `concurrency`, the lane is unlimited and
//! [`EndpointLane::admit`] is a no-op pass-through.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tokio::sync::oneshot;

/// A bounded scheduling identity parsed from the client header.
///
/// Callers name themselves via `X-PromptForge-Client` for fair queueing. The
/// value is parsed at the boundary into a bounded id (max length, restricted
/// charset); anything missing, empty, oversized, or containing other characters
/// maps to the single documented `default` bucket so an authenticated caller
/// cannot mint unbounded, attacker-chosen scheduler identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientId(String);

impl ClientId {
    /// Maximum accepted client-id length, in bytes.
    pub(crate) const MAX_LEN: usize = 64;
    /// The fallback bucket for absent or invalid ids.
    pub(crate) const DEFAULT: &'static str = "default";

    /// Parse an optional header string into a bounded [`ClientId`].
    pub(crate) fn from_header(value: Option<&str>) -> ClientId {
        value.map_or_else(|| ClientId(Self::DEFAULT.to_owned()), Self::parse)
    }

    /// Parse a raw string into a bounded [`ClientId`], falling back to `default`.
    pub(crate) fn parse(raw: &str) -> ClientId {
        let trimmed = raw.trim();
        let valid = !trimmed.is_empty()
            && trimmed.len() <= Self::MAX_LEN
            && trimmed
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
        if valid {
            ClientId(trimmed.to_owned())
        } else {
            ClientId(Self::DEFAULT.to_owned())
        }
    }

    /// The validated id as a string slice.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Waiting-queue settings shared by every limited endpoint lane.
///
/// `max_depth` counts only requests waiting for a concurrency slot, not
/// requests already admitted (in-flight).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub(crate) struct QueueConfig {
    /// Maximum number of waiting requests before new admits return
    /// [`AdmitError::QueueFull`]. Defaults to 100.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,
    /// When true, waiting callers are served round-robin by client key.
    /// Defaults to true.
    #[serde(default = "default_fair_scheduling")]
    pub fair_scheduling: bool,
}

fn default_max_depth() -> usize {
    100
}

fn default_fair_scheduling() -> bool {
    true
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            max_depth: default_max_depth(),
            fair_scheduling: default_fair_scheduling(),
        }
    }
}

/// Failure to admit a request onto an endpoint lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum AdmitError {
    /// The endpoint's waiting queue is already at `max_depth`.
    #[error("queue full")]
    QueueFull,
}

/// A concurrency slot held until dropped.
///
/// Dropping an admitted permit releases the slot (or transfers it to the next
/// waiter). An unlimited lane returns a no-op permit.
#[derive(Debug)]
#[must_use = "dropping the permit releases the concurrency slot"]
pub(crate) struct Permit {
    limited: Option<Arc<LimitedLane>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(lane) = self.limited.take() {
            lane.release_slot();
        }
    }
}

/// Per-endpoint admission controller.
#[derive(Debug, Clone)]
pub(crate) struct EndpointLane {
    inner: LaneInner,
}

#[derive(Debug, Clone)]
enum LaneInner {
    Unlimited,
    Limited(Arc<LimitedLane>),
}

struct LimitedLane {
    max_inflight: usize,
    max_depth: usize,
    fair: bool,
    state: Mutex<WaitState>,
}

impl std::fmt::Debug for LimitedLane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimitedLane")
            .field("max_inflight", &self.max_inflight)
            .field("max_depth", &self.max_depth)
            .field("fair", &self.fair)
            .finish_non_exhaustive()
    }
}

struct WaitState {
    inflight: usize,
    waiter_count: usize,
    next_id: u64,
    /// FIFO waiters when `fair_scheduling` is false.
    fifo: VecDeque<Waiter>,
    /// Per-client queues and round-robin order when fair scheduling is on.
    by_client: HashMap<String, VecDeque<Waiter>>,
    client_order: VecDeque<String>,
}

struct Waiter {
    id: u64,
    reply: oneshot::Sender<()>,
}

impl EndpointLane {
    /// An unlimited lane: every [`admit`](Self::admit) succeeds immediately.
    #[must_use]
    pub(crate) fn unlimited() -> EndpointLane {
        EndpointLane {
            inner: LaneInner::Unlimited,
        }
    }

    /// A lane that admits at most `concurrency` in-flight requests and queues
    /// up to `queue.max_depth` waiters.
    ///
    /// # Panics
    /// Panics if `concurrency` is zero. Callers must validate configuration
    /// first.
    #[must_use]
    pub(crate) fn new(concurrency: usize, queue: &QueueConfig) -> EndpointLane {
        assert!(concurrency >= 1, "endpoint concurrency must be at least 1");
        assert!(queue.max_depth >= 1, "queue.max_depth must be at least 1");
        EndpointLane {
            inner: LaneInner::Limited(Arc::new(LimitedLane {
                max_inflight: concurrency,
                max_depth: queue.max_depth,
                fair: queue.fair_scheduling,
                state: Mutex::new(WaitState {
                    inflight: 0,
                    waiter_count: 0,
                    next_id: 1,
                    fifo: VecDeque::new(),
                    by_client: HashMap::new(),
                    client_order: VecDeque::new(),
                }),
            })),
        }
    }

    /// Acquire a concurrency permit for `client_key`.
    ///
    /// When the lane is unlimited, returns a no-op permit immediately. When
    /// limited, waits (fairly or FIFO) until a slot is free, or fails if the
    /// waiting queue is already at `max_depth`.
    ///
    /// Number of requests currently waiting for a slot on this lane.
    ///
    /// Test-only observation seam so tests can rendezvous on a waiter being
    /// enqueued instead of sleeping.
    #[cfg(test)]
    pub(crate) fn waiter_count(&self) -> usize {
        match &self.inner {
            LaneInner::Unlimited => 0,
            LaneInner::Limited(lane) => {
                lane.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .waiter_count
            }
        }
    }

    /// # Errors
    /// Returns [`AdmitError::QueueFull`] when the waiting queue is at
    /// `max_depth` and no in-flight slot is free.
    pub(crate) async fn admit(&self, client_key: &str) -> Result<Permit, AdmitError> {
        let LaneInner::Limited(lane) = &self.inner else {
            return Ok(Permit { limited: None });
        };

        let outcome = {
            // Short critical section: never held across `.await`.
            let mut state = lane
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.inflight < lane.max_inflight {
                state.inflight += 1;
                AdmitOutcome::Ready
            } else if state.waiter_count >= lane.max_depth {
                AdmitOutcome::Full
            } else {
                let (tx, rx) = oneshot::channel();
                let id = state.next_id;
                state.next_id = state.next_id.wrapping_add(1);
                state.waiter_count += 1;
                let waiter = Waiter { id, reply: tx };
                if lane.fair {
                    enqueue_fair(&mut state, client_key, waiter);
                } else {
                    state.fifo.push_back(waiter);
                }
                AdmitOutcome::Queued { id, rx }
            }
        };

        match outcome {
            AdmitOutcome::Ready => Ok(Permit {
                limited: Some(Arc::clone(lane)),
            }),
            AdmitOutcome::Full => Err(AdmitError::QueueFull),
            AdmitOutcome::Queued { id, rx } => {
                let cancel = CancelOnDrop {
                    lane: Arc::clone(lane),
                    client_key: client_key.to_string(),
                    id,
                    armed: true,
                };
                match rx.await {
                    Ok(()) => {
                        cancel.disarm();
                        Ok(Permit {
                            limited: Some(Arc::clone(lane)),
                        })
                    }
                    Err(_) => Err(AdmitError::QueueFull),
                }
            }
        }
    }
}

enum AdmitOutcome {
    Ready,
    Full,
    Queued { id: u64, rx: oneshot::Receiver<()> },
}

/// Removes a still-queued waiter if `admit` is cancelled before admission.
struct CancelOnDrop {
    lane: Arc<LimitedLane>,
    client_key: String,
    id: u64,
    armed: bool,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let removed = {
            let mut state = self
                .lane
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if remove_waiter(&mut state, self.lane.fair, &self.client_key, self.id) {
                state.waiter_count = state.waiter_count.saturating_sub(1);
                true
            } else {
                false
            }
        };
        // Slot was already transferred via oneshot before we cancelled; free
        // it (or hand it to the next waiter) so inflight does not leak.
        if !removed {
            self.lane.release_slot();
        }
    }
}

impl LimitedLane {
    fn release_slot(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            let next = if self.fair {
                dequeue_fair(&mut state)
            } else {
                state.fifo.pop_front()
            };
            let Some(waiter) = next else {
                state.inflight = state.inflight.saturating_sub(1);
                return;
            };
            state.waiter_count = state.waiter_count.saturating_sub(1);
            // Transfer the in-flight slot to the waiter. If they cancelled,
            // try the next waiter (or free the slot).
            if waiter.reply.send(()).is_ok() {
                return;
            }
        }
    }
}

fn enqueue_fair(state: &mut WaitState, client_key: &str, waiter: Waiter) {
    let queue = state.by_client.entry(client_key.to_string()).or_default();
    let was_empty = queue.is_empty();
    queue.push_back(waiter);
    if was_empty {
        state.client_order.push_back(client_key.to_string());
    }
}

fn dequeue_fair(state: &mut WaitState) -> Option<Waiter> {
    let client = state.client_order.pop_front()?;
    let queue = state.by_client.get_mut(&client)?;
    let waiter = queue.pop_front()?;
    if queue.is_empty() {
        state.by_client.remove(&client);
    } else {
        state.client_order.push_back(client);
    }
    Some(waiter)
}

#[cfg(test)]
mod tests;

fn remove_waiter(state: &mut WaitState, fair: bool, client_key: &str, id: u64) -> bool {
    if fair {
        let Some(queue) = state.by_client.get_mut(client_key) else {
            return false;
        };
        let Some(pos) = queue.iter().position(|w| w.id == id) else {
            return false;
        };
        queue.remove(pos);
        if queue.is_empty() {
            state.by_client.remove(client_key);
            state.client_order.retain(|c| c != client_key);
        }
        true
    } else {
        let Some(pos) = state.fifo.iter().position(|w| w.id == id) else {
            return false;
        };
        state.fifo.remove(pos);
        true
    }
}
