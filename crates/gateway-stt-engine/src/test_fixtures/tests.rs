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
