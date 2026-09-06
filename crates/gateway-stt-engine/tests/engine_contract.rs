//! Public engine construction and decode regressions.

use gateway_stt_engine::{
    DecodeMode, DecodeRequest, Decoder, EnginePolicy, ModelFactory, SttEngine, TranscribeError,
};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::ThreadId;
use std::time::Duration;
use thiserror as _;
use tokio as _;

fn policy() -> EnginePolicy {
    let Ok(policy) = EnginePolicy::new(15, 500, false) else {
        panic!("test policy must be valid");
    };
    policy
}

#[test]
fn zero_window_is_rejected_before_backend_construction() {
    let error = EnginePolicy::new(0, 500, false).expect_err("zero window must fail");
    assert_eq!(
        error.to_string(),
        "invalid STT configuration: stt.window_seconds must be at least 1"
    );
}

#[test]
fn zero_interval_is_rejected_before_backend_construction() {
    let error = EnginePolicy::new(15, 0, false).expect_err("zero interval must fail");
    assert_eq!(
        error.to_string(),
        "invalid STT configuration: stt.interval_ms must be at least 1"
    );
}

#[test]
fn oversized_startup_timeout_is_the_exact_typed_configuration_error() {
    let (created, _created_rx) = mpsc::channel();
    let Err(error) = SttEngine::new(
        FailingModelFactory { created },
        policy().with_startup_timeout(Duration::MAX),
    ) else {
        panic!("an unrepresentable absolute deadline must fail");
    };
    assert_eq!(
        error.to_string(),
        "invalid STT configuration: stt.startup_timeout is too large"
    );
}

#[derive(Debug)]
struct FailingModelFactory {
    created: mpsc::Sender<ThreadId>,
}

impl ModelFactory for FailingModelFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        if mode == DecodeMode::Final {
            return Ok(None);
        }
        assert!(
            self.created.send(std::thread::current().id()).is_ok(),
            "the test must receive the worker identity"
        );
        Err(TranscribeError::load_model(
            PathBuf::from("failing-model.bin"),
            std::io::Error::other("fake model construction failure"),
        ))
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
        policy(),
    )
    .expect_err("model construction must fail");
    assert_eq!(
        error.to_string(),
        "load transcription model failing-model.bin"
    );
    let created = created_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the factory records its owning thread");
    assert_ne!(
        created, caller,
        "model construction belongs on the dedicated worker"
    );
}

const FINAL_INIT_SENTINEL: &str = "sentinel-final-initialization-failure";

#[derive(Debug)]
struct FinalFailingModelFactory {
    interim_dropped: mpsc::Sender<()>,
}

impl ModelFactory for FinalFailingModelFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        match mode {
            DecodeMode::Interim => Ok(Some(Box::new(InterimDropProbe {
                dropped: self.interim_dropped.clone(),
            }))),
            DecodeMode::Final => Err(TranscribeError::load_model(
                PathBuf::from(FINAL_INIT_SENTINEL),
                std::io::Error::other(FINAL_INIT_SENTINEL),
            )),
        }
    }
}

struct InterimDropProbe {
    dropped: mpsc::Sender<()>,
}

impl Decoder for InterimDropProbe {
    fn decode(&mut self, _request: DecodeRequest) -> Result<String, TranscribeError> {
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
        policy(),
    )
    .expect_err("final model construction must fail");
    assert_eq!(
        error.to_string(),
        format!("load transcription model {FINAL_INIT_SENTINEL}")
    );
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
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        if mode == DecodeMode::Final {
            return Ok(None);
        }
        let owner = std::thread::current().id();
        assert!(
            self.events.send(WorkerEvent::Created(owner)).is_ok(),
            "the test must receive decoder creation"
        );
        Ok(Some(Box::new(FailingDecoder {
            owner,
            events: self.events.clone(),
        })))
    }
}

struct FailingDecoder {
    owner: ThreadId,
    events: mpsc::Sender<WorkerEvent>,
}

impl Decoder for FailingDecoder {
    fn decode(&mut self, _request: DecodeRequest) -> Result<String, TranscribeError> {
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
    let engine = SttEngine::new(FailingDecoderFactory { events: event_tx }, policy())
        .expect("fake decoder loads");
    let WorkerEvent::Created(created) = event_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the factory records decoder creation")
    else {
        panic!("decoder creation must be the first event");
    };
    let error = engine
        .decode(DecodeRequest::new(
            DecodeMode::Interim,
            vec![0.25; EnginePolicy::SAMPLE_RATE],
            Vec::new(),
            String::new(),
        ))
        .await
        .expect_err("fake decode must fail");
    assert_eq!(error.to_string(), "transcribe audio window");
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
