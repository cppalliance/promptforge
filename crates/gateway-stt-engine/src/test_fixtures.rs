//! Deterministic decoder fixtures for downstream integration tests.

/// Native asset resolution for ignored integration tests.
pub mod native;

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::ThreadId;
use std::time::Duration;

use crate::{DecodeMode, DecodeRequest, Decoder, ModelFactory, TranscribeError};

#[derive(Debug)]
enum ScriptedOutcome {
    Text(String),
    Error(String),
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
    construction: ConstructionState,
    worker_dropped: bool,
    panic_on_drop: bool,
}

/// A cloneable controller for one deterministic decoder.
#[derive(Clone, Debug, Default)]
pub struct ScriptedDecoder {
    shared: Arc<(Mutex<DecoderState>, Condvar)>,
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

    /// Makes the next decode panic on its owning worker.
    pub fn panic_next(&self) {
        self.state().outcomes.push_back(ScriptedOutcome::Panic);
    }

    /// Parks the next decode until [`Self::release`] is called.
    pub fn park_next(&self) {
        self.state().park = ParkState::Armed;
    }

    /// Parks decoder construction until [`Self::release_construction`] runs.
    pub fn park_construction(&self) {
        self.state().construction = ConstructionState::Armed;
    }

    /// Makes the next construction attempt return the supplied failure.
    pub fn fail_next_construction(&self, message: impl Into<String>) {
        self.state().construction_errors.push_back(message.into());
    }

    /// Releases a decode parked by [`Self::park_next`].
    pub fn release(&self) {
        let (_, changed) = &*self.shared;
        self.state().park = ParkState::Released;
        changed.notify_all();
    }

    /// Releases construction parked by [`Self::park_construction`].
    pub fn release_construction(&self) {
        let (_, changed) = &*self.shared;
        self.state().construction = ConstructionState::Released;
        changed.notify_all();
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

    /// Waits until a parked decode has entered its rendezvous.
    #[must_use]
    pub fn wait_until_parked(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| state.park == ParkState::Parked)
    }

    /// Waits until decoder construction has entered its rendezvous.
    #[must_use]
    pub fn wait_until_construction_parked(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |state| {
            state.construction == ConstructionState::Parked
        })
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

    fn mark_created(&self) {
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

    fn take_construction_error(&self) -> Option<String> {
        self.state().construction_errors.pop_front()
    }
}

struct WorkerDecoder(ScriptedDecoder);

impl Decoder for WorkerDecoder {
    fn decode(&mut self, request: DecodeRequest) -> Result<String, TranscribeError> {
        let (state, changed) = &*self.0.shared;
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        state.requests.push(request);
        state.decode_threads.push(std::thread::current().id());
        changed.notify_all();
        if state.park == ParkState::Armed {
            state.park = ParkState::Parked;
            changed.notify_all();
            state = changed
                .wait_while(state, |state| state.park != ParkState::Released)
                .unwrap_or_else(PoisonError::into_inner);
            state.park = ParkState::Ready;
        }
        let outcome = match state.outcomes.pop_front() {
            Some(ScriptedOutcome::Text(text)) => Ok(text),
            Some(ScriptedOutcome::Error(message)) => {
                Err(TranscribeError::inference(std::io::Error::other(message)))
            }
            Some(ScriptedOutcome::Panic) => panic!("scripted decoder panic"),
            None => Ok(String::new()),
        };
        state.completed += 1;
        changed.notify_all();
        outcome
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

/// A role-specific scripted [`ModelFactory`] for test engines.
#[derive(Debug)]
pub struct ScriptedModelFactory {
    interim: ScriptedDecoder,
    final_decoder: Option<ScriptedDecoder>,
    interim_failure: Option<String>,
    final_failure: Option<String>,
    panic_interim: bool,
    panic_final: bool,
    gpu_available: bool,
}

impl ScriptedModelFactory {
    /// Creates an interim-only scripted factory.
    #[must_use]
    pub fn new(interim: ScriptedDecoder) -> Self {
        Self {
            interim,
            final_decoder: None,
            interim_failure: None,
            final_failure: None,
            panic_interim: false,
            panic_final: false,
            gpu_available: false,
        }
    }

    /// Installs the optional final-role decoder.
    #[must_use]
    pub fn with_final(mut self, decoder: ScriptedDecoder) -> Self {
        self.final_decoder = Some(decoder);
        self
    }

    /// Makes interim construction fail with the supplied message.
    #[must_use]
    pub fn with_interim_failure(mut self, message: impl Into<String>) -> Self {
        self.interim_failure = Some(message.into());
        self
    }

    /// Makes final construction fail with the supplied message.
    #[must_use]
    pub fn with_final_failure(mut self, message: impl Into<String>) -> Self {
        self.final_failure = Some(message.into());
        self
    }

    /// Makes interim construction panic.
    #[must_use]
    pub fn with_interim_panic(mut self) -> Self {
        self.panic_interim = true;
        self
    }

    /// Makes final construction panic.
    #[must_use]
    pub fn with_final_panic(mut self) -> Self {
        self.panic_final = true;
        self
    }

    /// Sets the hardware-acceleration fact reported by the fixture.
    #[must_use]
    pub fn with_gpu_available(mut self, available: bool) -> Self {
        self.gpu_available = available;
        self
    }

    /// Returns the fixture hardware-acceleration fact.
    #[must_use]
    pub fn gpu_available(&self) -> bool {
        self.gpu_available
    }
}

impl ModelFactory for ScriptedModelFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        let (decoder, failure, panic) = match mode {
            DecodeMode::Interim => (
                Some(&self.interim),
                &self.interim_failure,
                self.panic_interim,
            ),
            DecodeMode::Final => (
                self.final_decoder.as_ref(),
                &self.final_failure,
                self.panic_final,
            ),
        };
        assert!(!panic, "scripted {mode:?} factory panic");
        if let Some(message) = failure {
            return Err(TranscribeError::InvalidConfig(message.clone()));
        }
        let Some(decoder) = decoder else {
            return Ok(None);
        };
        if let Some(message) = decoder.take_construction_error() {
            return Err(TranscribeError::InvalidConfig(message));
        }
        decoder.mark_created();
        Ok(Some(Box::new(WorkerDecoder(decoder.clone()))))
    }
}

#[cfg(test)]
mod tests;
