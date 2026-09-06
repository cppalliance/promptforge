//! Backend-neutral interim and final transcription workers.

use std::sync::Arc;
use std::time::Duration;

use crate::worker::{FINAL_JOB_CAPACITY, INTERIM_JOB_CAPACITY, Transcriber};
use crate::{ModelFactory, SAMPLE_RATE, TranscribeError};

/// The STT engine: one required interim worker and one optional final worker.
#[derive(Debug)]
pub struct SttEngine {
    transcriber: Transcriber,
    final_pass: Option<Transcriber>,
    gpu_available: bool,
    window_samples: usize,
    interval: Duration,
}

impl SttEngine {
    /// Builds backend decoders on their owning worker threads.
    ///
    /// `window_seconds` and `interval_ms` are backend-neutral capture policy.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] for zero or overflowing
    /// policy values, a backend-translated construction failure, or
    /// [`TranscribeError::SpawnWorker`] when a worker cannot start.
    pub fn new(
        factory: impl ModelFactory,
        window_seconds: u64,
        interval_ms: u64,
    ) -> Result<Self, TranscribeError> {
        if window_seconds == 0 {
            return Err(TranscribeError::InvalidConfig(
                "stt.window_seconds must be at least 1".to_owned(),
            ));
        }
        if interval_ms == 0 {
            return Err(TranscribeError::InvalidConfig(
                "stt.interval_ms must be at least 1".to_owned(),
            ));
        }
        let seconds = usize::try_from(window_seconds).map_err(|_| {
            TranscribeError::InvalidConfig("stt.window_seconds is too large".to_owned())
        })?;
        let window_samples = seconds.checked_mul(SAMPLE_RATE).ok_or_else(|| {
            TranscribeError::InvalidConfig("stt.window_seconds is too large".to_owned())
        })?;

        let gpu_available = factory.gpu_available();
        let factory: Arc<dyn ModelFactory> = Arc::new(factory);
        let (transcriber, interim_init) = Transcriber::spawn(
            "stt-interim",
            Arc::clone(&factory),
            false,
            INTERIM_JOB_CAPACITY,
        )?;
        let interim_exists = interim_init
            .recv()
            .map_err(|_| TranscribeError::WorkerGone)??;
        if !interim_exists {
            return Err(TranscribeError::InvalidConfig(
                "the interim decoder is required".to_owned(),
            ));
        }
        let (final_worker, final_init) =
            Transcriber::spawn("stt-final", Arc::clone(&factory), true, FINAL_JOB_CAPACITY)?;
        let final_pass = if final_init
            .recv()
            .map_err(|_| TranscribeError::WorkerGone)??
        {
            Some(final_worker)
        } else {
            None
        };

        Ok(Self {
            transcriber,
            final_pass,
            gpu_available,
            window_samples,
            interval: Duration::from_millis(interval_ms),
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
        self.gpu_available
    }

    /// Samples in the sliding interim window.
    #[must_use]
    pub fn window_samples(&self) -> usize {
        self.window_samples
    }

    /// Cadence of the interim loop.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Transcribes one interim audio buffer.
    ///
    /// # Errors
    /// Returns a decoder failure or [`TranscribeError::WorkerGone`].
    pub async fn transcribe(
        &self,
        samples: Vec<f32>,
        guidance: Vec<String>,
    ) -> Result<String, TranscribeError> {
        self.transcriber
            .transcribe(samples, guidance, String::new())
            .await
    }

    /// Transcribes one independent buffer with the optional final decoder.
    ///
    /// # Errors
    /// Returns a decoder failure or [`TranscribeError::WorkerGone`].
    pub async fn transcribe_final(
        &self,
        samples: Vec<f32>,
        guidance: Vec<String>,
        finalized: String,
    ) -> Option<Result<String, TranscribeError>> {
        match &self.final_pass {
            Some(final_pass) => Some(final_pass.transcribe(samples, guidance, finalized).await),
            None => None,
        }
    }

    /// Closes both worker queues and joins their threads.
    ///
    /// Calling this method more than once has no additional effect. Native
    /// decoding is non-preemptible, so shutdown waits for a running decode
    /// rather than detaching its worker.
    pub fn shutdown(&mut self) {
        self.transcriber.shutdown();
        if let Some(final_pass) = &mut self.final_pass {
            final_pass.shutdown();
        }
        self.final_pass = None;
    }
}

impl Drop for SttEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread::ThreadId;

    use crate::{Decoder, ModelFactory};

    use super::*;

    #[derive(Debug)]
    struct NeverFactory;

    impl ModelFactory for NeverFactory {
        fn create_interim(&self) -> Result<Box<dyn Decoder>, TranscribeError> {
            panic!("invalid policy must fail before backend construction");
        }

        fn create_final(&self) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            panic!("invalid policy must fail before backend construction");
        }

        fn gpu_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn zero_window_is_rejected_before_backend_construction() {
        let error = SttEngine::new(NeverFactory, 0, 500).expect_err("zero window must fail");
        assert!(matches!(error, TranscribeError::InvalidConfig(_)));
    }

    #[test]
    fn zero_interval_is_rejected_before_backend_construction() {
        let error = SttEngine::new(NeverFactory, 15, 0).expect_err("zero interval must fail");
        assert!(matches!(error, TranscribeError::InvalidConfig(_)));
    }

    #[derive(Debug)]
    struct FailingModelFactory {
        created: mpsc::Sender<ThreadId>,
    }

    impl ModelFactory for FailingModelFactory {
        fn create_interim(&self) -> Result<Box<dyn Decoder>, TranscribeError> {
            assert!(
                self.created.send(std::thread::current().id()).is_ok(),
                "the test must receive the worker identity"
            );
            Err(TranscribeError::load_model(
                PathBuf::from("failing-model.bin"),
                std::io::Error::other("fake model construction failure"),
            ))
        }

        fn create_final(&self) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            Ok(None)
        }

        fn gpu_available(&self) -> bool {
            false
        }
    }

    #[test]
    fn model_initialization_failure_reaches_the_constructor_from_the_worker() {
        let caller = std::thread::current().id();
        let (created_tx, created_rx) = mpsc::channel();
        let error = SttEngine::new(
            FailingModelFactory {
                created: created_tx,
            },
            15,
            500,
        )
        .expect_err("model construction must fail");
        assert!(matches!(error, TranscribeError::LoadModel { .. }));
        let created = created_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the factory records its owning thread");
        assert_ne!(
            created, caller,
            "model construction belongs on the dedicated worker"
        );
    }

    const FINAL_INIT_SENTINEL: &str = "sentinel final initialization failure";

    #[derive(Debug)]
    struct FinalFailingModelFactory {
        interim_dropped: mpsc::Sender<()>,
    }

    impl ModelFactory for FinalFailingModelFactory {
        fn create_interim(&self) -> Result<Box<dyn Decoder>, TranscribeError> {
            Ok(Box::new(InterimDropProbe {
                dropped: self.interim_dropped.clone(),
            }))
        }

        fn create_final(&self) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            Err(TranscribeError::InvalidConfig(
                FINAL_INIT_SENTINEL.to_owned(),
            ))
        }

        fn gpu_available(&self) -> bool {
            false
        }
    }

    struct InterimDropProbe {
        dropped: mpsc::Sender<()>,
    }

    impl Decoder for InterimDropProbe {
        fn transcribe(
            &mut self,
            _samples: &[f32],
            _guidance: &[String],
            _finalized: &str,
        ) -> Result<String, TranscribeError> {
            Ok(String::new())
        }
    }

    impl Drop for InterimDropProbe {
        fn drop(&mut self) {
            let _ignored = self.dropped.send(());
        }
    }

    #[test]
    fn final_initialization_failure_propagates_and_cleans_up_the_interim_worker() {
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let error = SttEngine::new(
            FinalFailingModelFactory {
                interim_dropped: dropped_tx,
            },
            15,
            500,
        )
        .expect_err("final model construction must fail");
        let TranscribeError::InvalidConfig(message) = error else {
            panic!("the final worker's exact failure must reach the constructor");
        };
        assert_eq!(message, FINAL_INIT_SENTINEL);
        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("constructor failure releases the initialized interim decoder");
    }

    #[derive(Debug)]
    enum WorkerEvent {
        Created(ThreadId),
        Decoded { owner: ThreadId, current: ThreadId },
    }

    #[derive(Debug)]
    struct FailingDecoderFactory {
        events: mpsc::Sender<WorkerEvent>,
    }

    impl ModelFactory for FailingDecoderFactory {
        fn create_interim(&self) -> Result<Box<dyn Decoder>, TranscribeError> {
            let owner = std::thread::current().id();
            assert!(
                self.events.send(WorkerEvent::Created(owner)).is_ok(),
                "the test must receive decoder creation"
            );
            Ok(Box::new(FailingDecoder {
                owner,
                events: self.events.clone(),
            }))
        }

        fn create_final(&self) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
            Ok(None)
        }

        fn gpu_available(&self) -> bool {
            false
        }
    }

    struct FailingDecoder {
        owner: ThreadId,
        events: mpsc::Sender<WorkerEvent>,
    }

    impl Decoder for FailingDecoder {
        fn transcribe(
            &mut self,
            _samples: &[f32],
            _guidance: &[String],
            _finalized: &str,
        ) -> Result<String, TranscribeError> {
            assert!(
                self.events
                    .send(WorkerEvent::Decoded {
                        owner: self.owner,
                        current: std::thread::current().id(),
                    })
                    .is_ok(),
                "the test must receive decoder execution"
            );
            Err(TranscribeError::inference(std::io::Error::other(
                "fake decode failure",
            )))
        }
    }

    #[tokio::test]
    async fn decode_failure_reaches_the_caller_on_the_decoder_owner_thread() {
        let caller = std::thread::current().id();
        let (event_tx, event_rx) = mpsc::channel();
        let engine = SttEngine::new(FailingDecoderFactory { events: event_tx }, 15, 500)
            .expect("fake decoder loads");
        let WorkerEvent::Created(created) = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the factory records decoder creation")
        else {
            panic!("decoder creation must be the first event");
        };
        let error = engine
            .transcribe(vec![0.25; SAMPLE_RATE], Vec::new())
            .await
            .expect_err("fake decode must fail");
        assert!(matches!(error, TranscribeError::Inference(_)));
        let WorkerEvent::Decoded { owner, current } = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the decoder records execution")
        else {
            panic!("decoder execution must follow creation");
        };
        assert_ne!(created, caller, "decoder creation uses a worker thread");
        assert_eq!(owner, created, "the decoder retains its creating worker");
        assert_eq!(
            current, created,
            "decode execution stays on the decoder's owning worker"
        );
    }
}
