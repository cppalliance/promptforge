//! Backend-neutral interim and final transcription workers.

use std::sync::Arc;

use crate::startup;
use crate::worker::{FINAL_JOB_CAPACITY, INTERIM_JOB_CAPACITY, Transcriber};
use crate::{DecodeMode, DecodeRequest, EnginePolicy, ModelFactory, TranscribeError};

/// The STT engine: one required interim worker and one optional final worker.
#[derive(Debug)]
pub struct SttEngine {
    transcriber: Transcriber,
    final_pass: Option<Transcriber>,
    policy: EnginePolicy,
}

impl SttEngine {
    /// Builds backend decoders on their owning worker threads.
    ///
    /// # Errors
    /// Returns a backend-translated construction failure,
    /// one or more role-specific startup failures, or
    /// [`TranscribeError::SpawnWorker`]. If joining partially started workers
    /// also fails, [`TranscribeError::StartupCleanup`] preserves both outcomes.
    pub fn new(factory: impl ModelFactory, policy: EnginePolicy) -> Result<Self, TranscribeError> {
        Self::new_with(factory, policy, Transcriber::spawn)
    }

    fn new_with(
        factory: impl ModelFactory,
        policy: EnginePolicy,
        mut spawn: impl FnMut(
            &'static str,
            Arc<dyn ModelFactory>,
            DecodeMode,
            usize,
        ) -> Result<
            (
                Transcriber,
                std::sync::mpsc::Receiver<Result<bool, TranscribeError>>,
            ),
            TranscribeError,
        >,
    ) -> Result<Self, TranscribeError> {
        let factory: Arc<dyn ModelFactory> = Arc::new(factory);
        let startup_deadline = std::time::Instant::now()
            .checked_add(policy.startup_timeout())
            .ok_or_else(|| {
                TranscribeError::InvalidConfig("stt.startup_timeout is too large".to_owned())
            })?;
        let (transcriber, interim_init) = spawn(
            "stt-interim",
            Arc::clone(&factory),
            DecodeMode::Interim,
            INTERIM_JOB_CAPACITY,
        )?;
        let (final_worker, final_init) = match spawn(
            "stt-final",
            Arc::clone(&factory),
            DecodeMode::Final,
            FINAL_JOB_CAPACITY,
        ) {
            Ok(worker) => worker,
            Err(final_spawn) => {
                let interim =
                    startup::outcome(&interim_init, DecodeMode::Interim, startup_deadline);
                let interim_timed_out = startup::timed_out(&interim);
                let Err(startup) = startup::pair(interim, Err(final_spawn)) else {
                    unreachable!("the final spawn failure prevents construction");
                };
                let cleanup = if interim_timed_out {
                    transcriber.abandon_startup();
                    Vec::new()
                } else {
                    vec![transcriber.shutdown()]
                };
                return Err(Transcriber::startup_failure(startup, cleanup));
            }
        };
        let interim = startup::outcome(&interim_init, DecodeMode::Interim, startup_deadline);
        let final_result = startup::outcome(&final_init, DecodeMode::Final, startup_deadline);
        let interim_timed_out = startup::timed_out(&interim);
        let final_timed_out = startup::timed_out(&final_result);
        let (interim_exists, final_exists) = match startup::pair(interim, final_result) {
            Ok(pair) => pair,
            Err(startup) => {
                let mut cleanup = Vec::with_capacity(2);
                if interim_timed_out {
                    transcriber.abandon_startup();
                } else {
                    cleanup.push(transcriber.shutdown());
                }
                if final_timed_out {
                    final_worker.abandon_startup();
                } else {
                    cleanup.push(final_worker.shutdown());
                }
                return Err(Transcriber::startup_failure(startup, cleanup));
            }
        };
        debug_assert!(interim_exists);
        let final_pass = if final_exists {
            Some(final_worker)
        } else {
            final_worker.shutdown()?;
            None
        };

        Ok(Self {
            transcriber,
            final_pass,
            policy,
        })
    }

    /// Whether the final pass is configured.
    #[must_use]
    pub fn has_final_pass(&self) -> bool {
        self.final_pass.is_some()
    }

    /// Whether the backend reports hardware acceleration.
    #[must_use]
    pub fn gpu_transcription_available(&self) -> bool {
        self.policy.gpu_available()
    }

    /// Samples in the sliding interim window.
    #[must_use]
    pub fn window_samples(&self) -> usize {
        self.policy.window_samples()
    }

    /// Cadence of the interim loop.
    #[must_use]
    pub fn interval(&self) -> std::time::Duration {
        self.policy.interval()
    }

    /// Decodes one explicit stateless request on its selected worker.
    ///
    /// # Errors
    /// Returns a decoder failure, [`TranscribeError::WorkerGone`], or an
    /// invalid-configuration error when a final worker was not configured.
    pub async fn decode(&self, request: DecodeRequest) -> Result<String, TranscribeError> {
        match request.mode() {
            DecodeMode::Interim => self.transcriber.transcribe(request).await,
            DecodeMode::Final => match &self.final_pass {
                Some(final_pass) => final_pass.transcribe(request).await,
                None => Err(TranscribeError::InvalidConfig(
                    "the final decoder is not configured".to_owned(),
                )),
            },
        }
    }

    /// Closes both worker queues and joins their threads.
    ///
    /// Calling this method more than once has no additional effect. Native
    /// decoding is non-preemptible, so shutdown waits for a running decode
    /// rather than detaching its worker. This is the blocking,
    /// error-reporting path; `Drop` only signals and detaches.
    /// # Errors
    /// Returns [`TranscribeError::ShutdownPanicked`] for one panicked worker or
    /// [`TranscribeError::ShutdownFailures`] for multiple panicked workers.
    /// Both workers are still joined and every failure remains visible on
    /// repeated calls.
    pub fn shutdown(&self) -> Result<(), TranscribeError> {
        let mut cleanup = Vec::with_capacity(2);
        if let Err(error) = self.transcriber.shutdown() {
            cleanup.push(error);
        }
        if let Some(final_pass) = &self.final_pass
            && let Err(error) = final_pass.shutdown()
        {
            cleanup.push(error);
        }
        if cleanup.len() > 1 {
            return Err(TranscribeError::ShutdownFailures { cleanup });
        }
        match cleanup.pop() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Drop for SttEngine {
    fn drop(&mut self) {
        // `shutdown` is the blocking, error-reporting path; Drop signals
        // both workers and detaches their threads without joining.
        self.transcriber.signal_and_detach();
        if let Some(final_pass) = &self.final_pass {
            final_pass.signal_and_detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier, Condvar, Mutex, PoisonError};
    use std::time::Duration;

    use crate::Decoder;

    use super::*;

    fn policy() -> EnginePolicy {
        EnginePolicy::new(15, 500, false).expect("test policy is valid")
    }

    #[derive(Debug)]
    struct ConcurrentInterimFailure(Arc<Barrier>);

    impl ModelFactory for ConcurrentInterimFailure {
        fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            assert_eq!(mode, DecodeMode::Interim);
            self.0.wait();
            Err(TranscribeError::InvalidConfig(
                "interim startup sentinel".to_owned(),
            ))
        }
    }

    #[test]
    fn final_spawn_failure_preserves_concurrent_interim_startup_failure() {
        let rendezvous = Arc::new(Barrier::new(2));
        let spawn_rendezvous = Arc::clone(&rendezvous);
        let error = SttEngine::new_with(
            ConcurrentInterimFailure(rendezvous),
            policy(),
            move |name, factory, mode, capacity| {
                if mode == DecodeMode::Final {
                    spawn_rendezvous.wait();
                    return Err(TranscribeError::SpawnWorker(std::io::Error::other(
                        "final spawn sentinel",
                    )));
                }
                Transcriber::spawn(name, factory, mode, capacity)
            },
        )
        .expect_err("both concurrent startup failures prevent construction");
        let TranscribeError::StartupFailures { failures, .. } = error else {
            panic!("both observed role failures must be aggregated");
        };
        assert_eq!(failures.len(), 2);
        assert!(matches!(
            &failures[0],
            TranscribeError::InvalidConfig(message) if message == "interim startup sentinel"
        ));
        assert!(matches!(
            &failures[1],
            TranscribeError::SpawnWorker(source)
                if source.to_string() == "final spawn sentinel"
        ));
    }

    /// Shared observation of one decoder parked inside `decode`.
    #[derive(Clone, Debug, Default)]
    struct ParkControl {
        state: Arc<ParkState>,
    }

    #[derive(Debug, Default)]
    struct ParkState {
        phase: Mutex<ParkPhase>,
        changed: Condvar,
        dropped: AtomicBool,
    }

    #[derive(Debug, Default, Eq, PartialEq)]
    enum ParkPhase {
        #[default]
        Armed,
        Entered,
        Released,
    }

    impl ParkControl {
        fn wait_until_entered(&self) {
            let phase = self
                .state
                .phase
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let (_phase, timeout) = self
                .state
                .changed
                .wait_timeout_while(phase, Duration::from_secs(1), |phase| {
                    *phase != ParkPhase::Entered
                })
                .unwrap_or_else(PoisonError::into_inner);
            assert!(!timeout.timed_out(), "the job parks inside the decoder");
        }

        fn release(&self) {
            let mut phase = self
                .state
                .phase
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            *phase = ParkPhase::Released;
            self.state.changed.notify_all();
        }

        fn wait_until_dropped(&self) {
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while !self.state.dropped.load(Ordering::Acquire) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the detached worker drops the decoder after the release"
                );
                std::thread::yield_now();
            }
        }
    }

    #[derive(Debug)]
    struct ParkFactory(ParkControl);

    impl ModelFactory for ParkFactory {
        fn create(&self, _mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            Ok(Some(Box::new(ParkDecoder(Arc::clone(&self.0.state)))))
        }
    }

    struct ParkDecoder(Arc<ParkState>);

    impl Decoder for ParkDecoder {
        fn decode(&mut self, _request: DecodeRequest) -> Result<String, TranscribeError> {
            let mut phase = self.0.phase.lock().unwrap_or_else(PoisonError::into_inner);
            *phase = ParkPhase::Entered;
            self.0.changed.notify_all();
            drop(
                self.0
                    .changed
                    .wait_while(phase, |phase| *phase != ParkPhase::Released)
                    .unwrap_or_else(PoisonError::into_inner),
            );
            Ok("parked".to_owned())
        }
    }

    impl Drop for ParkDecoder {
        fn drop(&mut self) {
            self.0.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn drop_signals_and_detaches_instead_of_joining_a_running_decode() {
        let control = ParkControl::default();
        let engine =
            SttEngine::new(ParkFactory(control.clone()), policy()).expect("the engine builds");
        let request =
            DecodeRequest::new(DecodeMode::Interim, Vec::new(), Vec::new(), String::new());
        let mut decode = Box::pin(engine.decode(request));
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        assert!(
            decode.as_mut().poll(&mut context).is_pending(),
            "the submitted decode pends on the parked worker"
        );
        control.wait_until_entered();
        drop(decode);
        // The delayed releaser turns a blocking join into a failed timing
        // assertion instead of a deadlocked test.
        let releaser = std::thread::spawn({
            let control = control.clone();
            move || {
                std::thread::sleep(Duration::from_millis(500));
                control.release();
            }
        });

        let started = std::time::Instant::now();
        drop(engine);
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "drop signals and detaches instead of joining the running decode"
        );

        releaser.join().expect("the releaser thread joins");
        control.wait_until_dropped();
    }
}
