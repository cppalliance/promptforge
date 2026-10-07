//! Scripted decoder and model factory fixtures with parking controls.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use crate::{DecodeOutput, DecodeRequest, Decoder, TranscribeError};

#[derive(Debug)]
enum ScriptedOutcome {
    Text(String),
    Error(String),
    Overloaded,
    Panic,
}

#[derive(Debug, Default, Eq, PartialEq)]
enum ParkState {
    #[default]
    Ready,
    Armed,
    Parked,
    Released,
}

#[derive(Debug, Default, Eq, PartialEq)]
enum ConstructionState {
    #[default]
    Ready,
    Armed,
    Parked,
    Released,
}

#[derive(Debug, Default)]
struct DecoderState {
    outcomes: VecDeque<ScriptedOutcome>,
    construction_errors: VecDeque<String>,
    requests: Vec<DecodeRequest>,
    completed: usize,
    creation_thread: Option<ThreadId>,
    decode_threads: Vec<ThreadId>,
    waiters: usize,
    park: ParkState,
    cancellation_park: Option<Duration>,
    observed_cancellation: bool,
    construction: ConstructionState,
    worker_dropped: bool,
    panic_on_drop: bool,
}

/// A cloneable controller for one deterministic decoder.
#[derive(Clone, Debug, Default)]
pub struct ScriptedDecoder {
    shared: Arc<(Mutex<DecoderState>, Condvar)>,
}

struct DecodeBlock(ScriptedDecoder);

impl Drop for DecodeBlock {
    fn drop(&mut self) {
        self.0.release_decode();
    }
}

impl ScriptedDecoder {
    /// Creates a decoder whose unscripted calls return an empty transcript.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one successful decode result.
    pub fn push_text(&self, text: impl Into<String>) {
        self.state()
            .outcomes
            .push_back(ScriptedOutcome::Text(text.into()));
    }

    /// Appends one backend-neutral decode failure.
    pub fn push_error(&self, message: impl Into<String>) {
        self.state()
            .outcomes
            .push_back(ScriptedOutcome::Error(message.into()));
    }

    /// Appends one [`TranscribeError::Overloaded`] result, as a full worker
    /// queue reports it.
    pub fn push_overloaded(&self) {
        self.state().outcomes.push_back(ScriptedOutcome::Overloaded);
    }

    /// Makes the next decode panic on its owning worker.
    pub fn panic_next(&self) {
        self.state().outcomes.push_back(ScriptedOutcome::Panic);
    }

    /// Makes the next decode poll its request's cancellation flag until the
    /// flag reads true or `bound` passes, and then fail.
    ///
    /// A request without a flag waits out the whole bound.
    pub fn park_next_until_cancelled(&self, bound: Duration) {
        self.state().cancellation_park = Some(bound);
    }

    /// Whether the latest decode parked by [`Self::park_next_until_cancelled`]
    /// saw its request's cancellation flag set.
    #[must_use]
    pub fn observed_cancellation(&self) -> bool {
        self.state().observed_cancellation
    }

    /// Makes the next construction attempt return the supplied failure.
    pub fn fail_next_construction(&self, message: impl Into<String>) {
        self.state().construction_errors.push_back(message.into());
    }

    /// Makes dropping the worker-owned decoder panic.
    pub fn panic_on_drop(&self) {
        self.state().panic_on_drop = true;
    }

    /// Waits until at least `count` requests have entered the decoder.
    #[must_use]
    pub fn wait_for_requests(&self, count: usize, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| state.requests.len() >= count)
    }

    /// Waits until at least `count` scripted decodes have returned.
    #[must_use]
    pub fn wait_for_completed(&self, count: usize, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| state.completed >= count)
    }

    /// Returns all captured stateless requests.
    #[must_use]
    pub fn requests(&self) -> Vec<DecodeRequest> {
        self.state().requests.clone()
    }

    /// Returns the worker that constructed the decoder, if construction ran.
    #[must_use]
    pub fn creation_thread(&self) -> Option<ThreadId> {
        self.state().creation_thread
    }

    /// Returns the worker thread observed by every decode.
    #[must_use]
    pub fn decode_threads(&self) -> Vec<ThreadId> {
        self.state().decode_threads.clone()
    }

    /// Whether engine cleanup dropped the worker-owned decoder.
    #[must_use]
    pub fn worker_dropped(&self) -> bool {
        self.state().worker_dropped
    }

    /// Waits until engine cleanup drops the worker-owned decoder.
    #[must_use]
    pub fn wait_until_worker_dropped(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| state.worker_dropped)
    }

    /// Runs an asynchronous scenario while the next decode is blocked.
    ///
    /// `start` must initiate the decode without awaiting its result. After the
    /// decode enters the fixture, `while_blocked` runs and the decoder is
    /// released when that future returns, is canceled, times out, or unwinds.
    #[must_use]
    pub async fn with_next_decode_blocked<Start, Started, Context, Scenario, Running, Output>(
        &self,
        timeout: Duration,
        start: Start,
        while_blocked: Scenario,
    ) -> Option<Output>
    where
        Start: FnOnce() -> Started,
        Started: Future<Output = Context>,
        Scenario: FnOnce(Context) -> Running,
        Running: Future<Output = Output>,
    {
        self.state().park = ParkState::Armed;
        let block = DecodeBlock(self.clone());
        let context = start().await;
        let observer = self.clone();
        let parked = tokio::task::spawn_blocking(move || {
            observer.wait_for(timeout, |state| state.park == ParkState::Parked)
        })
        .await
        .ok()?;
        if !parked {
            return None;
        }
        let output = while_blocked(context).await;
        drop(block);
        Some(output)
    }

    pub(super) fn arm_construction(&self) {
        self.state().construction = ConstructionState::Armed;
    }

    pub(super) fn release_construction(&self) {
        let (_, changed) = &*self.shared;
        self.state().construction = ConstructionState::Released;
        changed.notify_all();
    }

    pub(super) fn wait_until_construction_parked(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| {
            state.construction == ConstructionState::Parked
        })
    }

    #[cfg(test)]
    pub(super) fn wait_until_waiter_registered(&self, timeout: Duration) -> bool {
        let (state, changed) = &*self.shared;
        let state = state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, result) = changed
            .wait_timeout_while(state, timeout, |state| state.waiters == 0)
            .unwrap_or_else(PoisonError::into_inner);
        !result.timed_out() && state.waiters == 1
    }

    pub(super) fn take_construction_error(&self) -> Option<String> {
        self.state().construction_errors.pop_front()
    }

    pub(super) fn mark_created(&self) {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.construction == ConstructionState::Armed {
            state.construction = ConstructionState::Parked;
            changed.notify_all();
            state = changed
                .wait_while(state, |state| {
                    state.construction != ConstructionState::Released
                })
                .unwrap_or_else(PoisonError::into_inner);
            state.construction = ConstructionState::Ready;
        }
        state.creation_thread = Some(std::thread::current().id());
    }

    pub(super) fn worker(&self) -> Box<dyn Decoder> {
        Box::new(WorkerDecoder(self.clone()))
    }

    fn release_decode(&self) {
        let (_, changed) = &*self.shared;
        self.state().park = ParkState::Released;
        changed.notify_all();
    }

    fn state(&self) -> std::sync::MutexGuard<'_, DecoderState> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_for(&self, timeout: Duration, predicate: impl Fn(&DecoderState) -> bool) -> bool {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        if predicate(&state) {
            return true;
        }
        state.waiters += 1;
        changed.notify_all();
        let (mut state, result) = changed
            .wait_timeout_while(state, timeout, |state| !predicate(state))
            .unwrap_or_else(PoisonError::into_inner);
        state.waiters -= 1;
        !result.timed_out() && predicate(&state)
    }
}

struct WorkerDecoder(ScriptedDecoder);

impl Decoder for WorkerDecoder {
    fn decode(&mut self, request: DecodeRequest) -> Result<DecodeOutput, TranscribeError> {
        let (state, changed) = &*self.0.shared;
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        state.requests.push(request.clone());
        state.decode_threads.push(std::thread::current().id());
        changed.notify_all();
        if let Some(bound) = state.cancellation_park.take() {
            drop(state);
            let observed = wait_for_cancellation(request.cancellation(), bound);
            let mut state = self.0.state();
            state.observed_cancellation = observed;
            state.completed += 1;
            changed.notify_all();
            let message = if observed {
                "scripted decode observed its cancellation flag".to_owned()
            } else {
                format!("scripted decode waited out its {bound:?} cancellation bound")
            };
            return Err(TranscribeError::inference(std::io::Error::other(message)));
        }
        if state.park == ParkState::Armed {
            state.park = ParkState::Parked;
            changed.notify_all();
            state = changed
                .wait_while(state, |state| state.park != ParkState::Released)
                .unwrap_or_else(PoisonError::into_inner);
            state.park = ParkState::Ready;
        }
        let outcome = match state.outcomes.pop_front() {
            Some(ScriptedOutcome::Text(text)) => Ok(DecodeOutput::new(text)),
            Some(ScriptedOutcome::Error(message)) => {
                Err(TranscribeError::inference(std::io::Error::other(message)))
            }
            Some(ScriptedOutcome::Overloaded) => Err(TranscribeError::Overloaded),
            Some(ScriptedOutcome::Panic) => panic!("scripted decoder panic"),
            None => Ok(DecodeOutput::default()),
        };
        state.completed += 1;
        changed.notify_all();
        outcome
    }
}

fn wait_for_cancellation(flag: Option<&Arc<AtomicBool>>, bound: Duration) -> bool {
    const POLL: Duration = Duration::from_millis(1);

    let deadline = Instant::now() + bound;
    loop {
        if flag.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(POLL));
    }
}

impl Drop for WorkerDecoder {
    fn drop(&mut self) {
        let (_, changed) = &*self.0.shared;
        let panic_on_drop = {
            let mut state = self.0.state();
            state.worker_dropped = true;
            state.panic_on_drop
        };
        changed.notify_all();
        assert!(!panic_on_drop, "scripted decoder drop panic");
    }
}
