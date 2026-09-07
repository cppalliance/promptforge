//! Deterministic decoder fixtures for downstream integration tests.

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
mod tests {
    use super::*;
    use crate::{EnginePolicy, SttEngine};

    fn policy() -> EnginePolicy {
        EnginePolicy::new(15, 500, false).expect("test policy is valid")
    }

    fn request(
        mode: DecodeMode,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: impl Into<String>,
    ) -> DecodeRequest {
        DecodeRequest::new(mode, samples, guidance, finalized.into())
    }

    fn assert_invalid_config(error: TranscribeError, expected: &str) {
        let TranscribeError::InvalidConfig(message) = error else {
            panic!("expected invalid configuration, got {error}");
        };
        assert_eq!(message, expected);
    }

    fn wait_until_waiter_is_registered(decoder: &ScriptedDecoder) {
        let (state, changed) = &*decoder.shared;
        let state = state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, timeout) = changed
            .wait_timeout_while(state, Duration::from_secs(1), |state| state.waiters == 0)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(
            !timeout.timed_out() && state.waiters == 1,
            "request waiter must enter the condition-variable wait"
        );
    }

    #[tokio::test]
    async fn scripted_roles_capture_requests_on_their_creation_threads() {
        let caller = std::thread::current().id();
        let interim = ScriptedDecoder::new();
        interim.push_text("interim");
        let final_decoder = ScriptedDecoder::new();
        final_decoder.push_text("final");
        let engine = SttEngine::new(
            ScriptedModelFactory::new(interim.clone())
                .with_final(final_decoder.clone())
                .with_gpu_available(true),
            EnginePolicy::new(15, 500, true).expect("test policy is valid"),
        )
        .expect("scripted workers start");

        assert_eq!(
            engine
                .decode(request(
                    DecodeMode::Interim,
                    vec![0.25],
                    vec!["term".to_owned()],
                    "",
                ))
                .await
                .expect("interim succeeds"),
            "interim"
        );
        assert_eq!(
            engine
                .decode(request(
                    DecodeMode::Final,
                    vec![0.5],
                    vec!["name".to_owned()],
                    "history",
                ))
                .await
                .expect("final succeeds"),
            "final"
        );
        assert!(engine.gpu_transcription_available());
        let interim_requests = interim.requests();
        assert_eq!(interim_requests.len(), 1);
        assert_eq!(interim_requests[0].mode(), DecodeMode::Interim);
        assert_eq!(interim_requests[0].samples(), &[0.25]);
        assert_eq!(interim_requests[0].guidance(), ["term"]);
        assert_eq!(interim_requests[0].finalized(), "");
        let final_requests = final_decoder.requests();
        assert_eq!(final_requests.len(), 1);
        assert_eq!(final_requests[0].mode(), DecodeMode::Final);
        assert_eq!(final_requests[0].samples(), &[0.5]);
        assert_eq!(final_requests[0].guidance(), ["name"]);
        assert_eq!(final_requests[0].finalized(), "history");
        assert_ne!(interim.creation_thread(), Some(caller));
        assert_eq!(
            interim.decode_threads(),
            vec![interim.creation_thread().expect("interim was constructed")]
        );
        assert_eq!(
            final_decoder.decode_threads(),
            vec![
                final_decoder
                    .creation_thread()
                    .expect("final was constructed")
            ]
        );
        engine.shutdown().expect("workers join");
        assert!(interim.worker_dropped());
        assert!(final_decoder.worker_dropped());
    }

    #[test]
    fn scripted_interim_startup_panic_is_explicit_without_a_decoder() {
        let interim = ScriptedDecoder::new();
        let error = SttEngine::new(
            ScriptedModelFactory::new(interim.clone()).with_interim_panic(),
            policy(),
        )
        .expect_err("startup panic fails construction");
        assert!(matches!(error, TranscribeError::WorkerPanicked));
        assert_eq!(interim.creation_thread(), None);
        assert!(!interim.worker_dropped());
    }

    #[test]
    fn scripted_final_startup_panic_is_explicit_and_cleans_up_interim() {
        let interim = ScriptedDecoder::new();
        let error = SttEngine::new(
            ScriptedModelFactory::new(interim.clone()).with_final_panic(),
            policy(),
        )
        .expect_err("startup panic fails construction");
        assert!(matches!(error, TranscribeError::WorkerPanicked));
        assert!(interim.worker_dropped());
    }

    #[tokio::test]
    async fn scripted_decode_panic_is_explicit_and_closes_the_worker() {
        let interim = ScriptedDecoder::new();
        interim.panic_next();
        let engine = SttEngine::new(ScriptedModelFactory::new(interim), policy())
            .expect("scripted worker starts");
        let first = engine
            .decode(request(DecodeMode::Interim, Vec::new(), Vec::new(), ""))
            .await
            .expect_err("panic is reported");
        assert!(matches!(first, TranscribeError::WorkerPanicked));
        let second = engine
            .decode(request(DecodeMode::Interim, Vec::new(), Vec::new(), ""))
            .await
            .expect_err("panicked worker stays closed");
        assert!(matches!(second, TranscribeError::WorkerGone));
    }

    #[tokio::test]
    async fn request_waiter_started_before_an_unparked_decode_is_notified() {
        let interim = ScriptedDecoder::new();
        let waiter_decoder = interim.clone();
        let waiter =
            std::thread::spawn(move || waiter_decoder.wait_for_requests(1, Duration::from_secs(1)));
        wait_until_waiter_is_registered(&interim);
        let engine = SttEngine::new(ScriptedModelFactory::new(interim.clone()), policy())
            .expect("scripted worker starts");

        engine
            .decode(request(
                DecodeMode::Interim,
                vec![0.25],
                vec!["term".to_owned()],
                "",
            ))
            .await
            .expect("unparked decode succeeds");
        assert!(
            waiter.join().expect("request waiter does not panic"),
            "recording the request wakes the pre-existing waiter"
        );
        engine.shutdown().expect("worker joins");
        assert!(interim.worker_dropped());
    }

    #[tokio::test]
    async fn scripted_decode_error_reaches_the_caller_and_cleanup_drops_the_worker() {
        const SENTINEL: &str = "scripted decode sentinel";

        let interim = ScriptedDecoder::new();
        interim.push_error(SENTINEL);
        let engine = SttEngine::new(ScriptedModelFactory::new(interim.clone()), policy())
            .expect("scripted worker starts");
        let error = engine
            .decode(request(DecodeMode::Interim, vec![0.25], Vec::new(), ""))
            .await
            .expect_err("scripted decode fails");
        let TranscribeError::Inference(source) = error else {
            panic!("expected inference failure, got {error}");
        };
        assert_eq!(source.to_string(), SENTINEL);
        assert!(source.source().is_none());

        engine.shutdown().expect("worker joins");
        assert!(interim.worker_dropped());
    }

    #[test]
    fn scripted_interim_factory_error_reaches_the_constructor_without_a_decoder() {
        const SENTINEL: &str = "scripted interim startup sentinel";

        let interim = ScriptedDecoder::new();
        let error = SttEngine::new(
            ScriptedModelFactory::new(interim.clone()).with_interim_failure(SENTINEL),
            policy(),
        )
        .expect_err("scripted interim construction fails");
        assert_invalid_config(error, SENTINEL);
        assert_eq!(interim.creation_thread(), None);
        assert!(!interim.worker_dropped());
    }

    #[test]
    fn scripted_final_factory_error_reaches_the_constructor_and_cleans_up_interim() {
        const SENTINEL: &str = "scripted final startup sentinel";

        let interim = ScriptedDecoder::new();
        let final_decoder = ScriptedDecoder::new();
        let error = SttEngine::new(
            ScriptedModelFactory::new(interim.clone())
                .with_final(final_decoder.clone())
                .with_final_failure(SENTINEL),
            policy(),
        )
        .expect_err("scripted final construction fails");
        assert_invalid_config(error, SENTINEL);
        assert!(interim.creation_thread().is_some());
        assert!(interim.worker_dropped());
        assert_eq!(final_decoder.creation_thread(), None);
        assert!(!final_decoder.worker_dropped());
    }

    #[test]
    fn parked_interim_construction_has_a_bounded_classified_outcome() {
        let interim = ScriptedDecoder::new();
        interim.park_construction();
        let factory = ScriptedModelFactory::new(interim.clone());
        let timeout = policy().with_startup_timeout(Duration::from_millis(20));
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let constructor = std::thread::spawn(move || {
            let result = SttEngine::new(factory, timeout);
            drop(result_tx.send(result));
        });
        assert!(interim.wait_until_construction_parked(Duration::from_secs(1)));
        let error = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("startup returns by its deadline")
            .expect_err("parked interim construction times out");
        assert!(matches!(error, TranscribeError::InterimStartupTimedOut));
        constructor.join().expect("constructor does not panic");
        interim.release_construction();
        assert!(interim.wait_for(Duration::from_secs(1), |state| state.worker_dropped));
    }

    #[test]
    fn parked_final_construction_cleans_up_the_initialized_interim_worker() {
        let interim = ScriptedDecoder::new();
        let final_decoder = ScriptedDecoder::new();
        final_decoder.park_construction();
        let factory = ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone());
        let timeout = policy().with_startup_timeout(Duration::from_millis(20));
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let constructor = std::thread::spawn(move || {
            let result = SttEngine::new(factory, timeout);
            drop(result_tx.send(result));
        });
        assert!(
            final_decoder.wait_until_construction_parked(Duration::from_secs(1)),
            "final construction reaches its deterministic park"
        );
        let error = result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("startup returns by its deadline")
            .expect_err("parked final construction times out");
        assert!(matches!(error, TranscribeError::FinalStartupTimedOut));
        assert!(
            interim.worker_dropped(),
            "the worker initialized first is joined and cleaned up"
        );
        constructor.join().expect("constructor does not panic");
        final_decoder.release_construction();
        assert!(
            final_decoder.wait_for(Duration::from_secs(1), |state| state.worker_dropped),
            "the abandoned constructor releases its decoder after returning"
        );
    }

    #[test]
    fn shutdown_surfaces_join_panic_and_remains_idempotent() {
        let interim = ScriptedDecoder::new();
        interim.panic_on_drop();
        let engine = SttEngine::new(ScriptedModelFactory::new(interim.clone()), policy())
            .expect("scripted worker starts");

        assert!(matches!(
            engine.shutdown(),
            Err(TranscribeError::ShutdownPanicked)
        ));
        assert!(matches!(
            engine.shutdown(),
            Err(TranscribeError::ShutdownPanicked)
        ));
        assert!(interim.worker_dropped());
    }
}
