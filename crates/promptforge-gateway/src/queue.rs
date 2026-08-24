//! Dominion admission control: concurrency limits, a bounded waiting queue,
//! and a reject-vs-queue policy for a full queue.
//!
//! `max_depth` is the number of *waiting* requests allowed (not counting
//! in-flight). When nothing caps a dominion's concurrency, the queue is
//! unlimited and [`DominionQueue::admit`] is a no-op pass-through.
//!
//! The full-queue behavior comes from the dominion's `policy`: `Queue` (the
//! default) parks callers up to `max_depth` and rejects beyond that, while
//! `Reject` never parks - it turns the caller away immediately when no
//! concurrency slot is free (TGI-style fail-fast), which the gateway maps to
//! a 429 response.
//!
//! When `fair_scheduling` is on, waiting callers are served in per-client
//! round-robin order keyed by the `X-PromptForge-Client` header. That header
//! is self-asserted: a scheduling hint for trusted-host callers, not an
//! authenticated identity. There is deliberately no discipline abstraction -
//! per-client round-robin is the only discipline, and a future cost-based
//! discipline (DRR, token costs) would be a change contained in this file.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use promptforge_gateway_config::{QueueConfig, QueuePolicy};
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

/// Failure to admit a request onto a dominion queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum AdmitError {
    /// The dominion's waiting queue is already at `max_depth`.
    #[error("queue full")]
    QueueFull,

    /// The `Reject` policy found no free concurrency slot and turned the
    /// request away without enqueueing it (fail-fast).
    #[error("rejected at capacity")]
    Rejected,

    /// The waiting slot's notification channel closed before a slot was granted
    /// (the queue was torn down while this caller waited). Distinct from
    /// [`AdmitError::QueueFull`], which is a live back-pressure signal.
    #[error("dominion queue unavailable")]
    Unavailable,
}

/// A concurrency slot held until dropped.
///
/// Dropping an admitted permit releases the slot (or transfers it to the next
/// waiter). An unlimited queue returns a no-op permit.
#[derive(Debug)]
#[must_use = "dropping the permit releases the concurrency slot"]
pub(crate) struct Permit {
    limited: Option<Arc<LimitedQueue>>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(queue) = self.limited.take() {
            queue.release_slot();
        }
    }
}

/// Per-dominion admission controller.
///
/// Cloning is cheap and every clone shares one underlying limit, which is
/// what lets several endpoints bound to the same dominion compete for a
/// single pool of slots.
#[derive(Debug, Clone)]
pub(crate) struct DominionQueue {
    inner: QueueInner,
}

#[derive(Debug, Clone)]
enum QueueInner {
    Unlimited,
    Limited(Arc<LimitedQueue>),
}

struct LimitedQueue {
    max_inflight: usize,
    max_depth: usize,
    fair: bool,
    policy: QueuePolicy,
    state: Mutex<WaitState>,
}

impl std::fmt::Debug for LimitedQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimitedQueue")
            .field("max_inflight", &self.max_inflight)
            .field("max_depth", &self.max_depth)
            .field("fair", &self.fair)
            .field("policy", &self.policy)
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

impl DominionQueue {
    /// An unlimited queue: every [`admit`](Self::admit) succeeds immediately.
    #[must_use]
    pub(crate) fn unlimited() -> DominionQueue {
        DominionQueue {
            inner: QueueInner::Unlimited,
        }
    }

    /// A queue that admits at most `concurrency` in-flight requests and, under
    /// the `Queue` policy, parks up to `max_depth` waiters before rejecting.
    ///
    /// Total and non-panicking (Q-002): a zero `concurrency` yields an
    /// unlimited queue and a zero `max_depth` is clamped to 1. Configuration
    /// validation already rejects both zeros, so this is purely defensive -
    /// construction can never panic on out-of-range runtime settings.
    #[must_use]
    pub(crate) fn new(
        concurrency: usize,
        max_depth: usize,
        fair_scheduling: bool,
        policy: QueuePolicy,
    ) -> DominionQueue {
        let Some(max_inflight) = std::num::NonZeroUsize::new(concurrency) else {
            return DominionQueue::unlimited();
        };
        DominionQueue {
            inner: QueueInner::Limited(Arc::new(LimitedQueue {
                max_inflight: max_inflight.get(),
                max_depth: max_depth.max(1),
                fair: fair_scheduling,
                policy,
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

    /// The bounded-wait queue implied by the legacy `[queue]` settings, which
    /// carry no policy knob: always the `Queue` policy. Removed together with
    /// `QueueConfig` when the legacy config is deleted.
    #[must_use]
    pub(crate) fn from_queue_config(concurrency: usize, queue: &QueueConfig) -> DominionQueue {
        DominionQueue::new(
            concurrency,
            queue.max_depth(),
            queue.fair_scheduling(),
            QueuePolicy::Queue,
        )
    }

    /// Number of requests currently waiting for a slot on this queue.
    ///
    /// Test-only observation seam so tests can rendezvous on a waiter being
    /// enqueued instead of sleeping.
    #[cfg(test)]
    pub(crate) fn waiter_count(&self) -> usize {
        match &self.inner {
            QueueInner::Unlimited => 0,
            QueueInner::Limited(queue) => {
                queue
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .waiter_count
            }
        }
    }

    /// Number of distinct client buckets currently in the fair round-robin.
    ///
    /// Test-only observation seam for the distinct-client cap (Q-001).
    #[cfg(test)]
    pub(crate) fn distinct_clients(&self) -> usize {
        match &self.inner {
            QueueInner::Unlimited => 0,
            QueueInner::Limited(queue) => queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .client_order
                .len(),
        }
    }

    /// Acquire a concurrency permit for `client_key`.
    ///
    /// When the queue is unlimited, returns a no-op permit immediately. When
    /// limited, the policy decides what a full in-flight set means: `Queue`
    /// waits (fairly or FIFO) until a slot is free or the waiting queue
    /// reaches `max_depth`; `Reject` fails immediately.
    ///
    /// # Errors
    /// Returns [`AdmitError::QueueFull`] when the waiting queue is at
    /// `max_depth` and no in-flight slot is free, [`AdmitError::Rejected`]
    /// when the `Reject` policy finds no free slot, and
    /// [`AdmitError::Unavailable`] when the queue is torn down while this
    /// caller waits.
    pub(crate) async fn admit(&self, client_key: &str) -> Result<Permit, AdmitError> {
        let QueueInner::Limited(queue) = &self.inner else {
            return Ok(Permit { limited: None });
        };

        let outcome = {
            // Short critical section: never held across `.await`.
            let mut state = queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.inflight < queue.max_inflight {
                state.inflight += 1;
                AdmitOutcome::Ready
            } else if queue.policy == QueuePolicy::Reject {
                // Fail-fast: the caller is never parked, so `max_depth` and
                // fairness are irrelevant under this policy.
                AdmitOutcome::Rejected
            } else if state.waiter_count >= queue.max_depth {
                AdmitOutcome::Full
            } else {
                let (tx, rx) = oneshot::channel();
                let id = state.next_id;
                state.next_id = state.next_id.wrapping_add(1);
                state.waiter_count += 1;
                let waiter = Waiter { id, reply: tx };
                // Capture the *effective* bucket key: fair scheduling may fold a
                // fresh label into `default` once the distinct-client cap is hit
                // (Q-001), and cancellation must remove from that same bucket.
                let effective_key = if queue.fair {
                    enqueue_fair(&mut state, client_key, waiter)
                } else {
                    state.fifo.push_back(waiter);
                    client_key.to_string()
                };
                AdmitOutcome::Queued {
                    id,
                    rx,
                    client_key: effective_key,
                }
            }
        };

        match outcome {
            AdmitOutcome::Ready => Ok(Permit {
                limited: Some(Arc::clone(queue)),
            }),
            AdmitOutcome::Full => Err(AdmitError::QueueFull),
            AdmitOutcome::Rejected => Err(AdmitError::Rejected),
            AdmitOutcome::Queued { id, rx, client_key } => {
                let cancel = CancelOnDrop {
                    queue: Arc::clone(queue),
                    client_key,
                    id,
                    armed: true,
                };
                match rx.await {
                    Ok(()) => {
                        cancel.disarm();
                        Ok(Permit {
                            limited: Some(Arc::clone(queue)),
                        })
                    }
                    // The sender was dropped without granting a slot: the queue
                    // is gone, not merely full. Surface that distinctly.
                    Err(_) => Err(AdmitError::Unavailable),
                }
            }
        }
    }
}

enum AdmitOutcome {
    Ready,
    Full,
    Rejected,
    Queued {
        id: u64,
        rx: oneshot::Receiver<()>,
        /// The effective bucket key the waiter was enqueued under.
        client_key: String,
    },
}

/// Removes a still-queued waiter if `admit` is cancelled before admission.
struct CancelOnDrop {
    queue: Arc<LimitedQueue>,
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
                .queue
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if remove_waiter(&mut state, self.queue.fair, &self.client_key, self.id) {
                state.waiter_count = state.waiter_count.saturating_sub(1);
                true
            } else {
                false
            }
        };
        // Slot was already transferred via oneshot before we cancelled; free
        // it (or hand it to the next waiter) so inflight does not leak.
        if !removed {
            self.queue.release_slot();
        }
    }
}

impl LimitedQueue {
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

/// Maximum number of distinct client labels tracked in the fair round-robin.
///
/// Beyond this, a fresh label folds into the shared `default` bucket. Because
/// all callers authenticate with the same shared server key, per-caller identity
/// is indistinguishable, so this global cap is the bound that stops one caller
/// from minting many labels to win a larger share of round-robin turns (Q-001).
const MAX_DISTINCT_CLIENTS: usize = 32;

/// Enqueue a waiter under its fair-scheduling bucket, returning the *effective*
/// bucket key actually used (which may be `default` when the distinct-client cap
/// is reached).
fn enqueue_fair(state: &mut WaitState, client_key: &str, waiter: Waiter) -> String {
    let key = if state.by_client.contains_key(client_key)
        || state.client_order.len() < MAX_DISTINCT_CLIENTS
    {
        client_key.to_string()
    } else {
        ClientId::DEFAULT.to_string()
    };
    let queue = state.by_client.entry(key.clone()).or_default();
    let was_empty = queue.is_empty();
    queue.push_back(waiter);
    if was_empty {
        state.client_order.push_back(key.clone());
    }
    key
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

/// Compile-time assertion that the admission primitives cross thread
/// boundaries, so losing `Send`/`Sync` on any of them fails the build rather
/// than silently breaking spawned request handling.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DominionQueue>();
    assert_send_sync::<Permit>();
    assert_send_sync::<QueueConfig>();
    assert_send_sync::<AdmitError>();
    assert_send_sync::<ClientId>();
};

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
