//! One-time initial speech publication integration tests.

#![expect(
    clippy::expect_used,
    reason = "integration tests panic with the failed publication invariant"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::http::StatusCode;
use gateway_config::{Config, ProfileName};
use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{
    ScriptedDecoder, ScriptedModelFactory, generation_ownership, load_scripted_initial,
    load_scripted_initial_with_cancellation, scripted_loaded_service,
};
use gateway_stt_engine::{DecodeMode, DecodeRequest, Decoder, ModelFactory, TranscribeError};
use tokio_util::sync::CancellationToken;

use crate::common::transcribe_batch;

const WAIT: Duration = Duration::from_secs(2);

fn speech_config(models: &str, selected: &str) -> Config {
    Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"k\"\n\
         {models}\
         [[profile]]\nname = \"speech\"\nmodels = {selected}\n"
    ))
    .expect("speech catalog parses")
    .select_profile(&ProfileName::parse("speech").expect("profile name"))
    .expect("speech profile selects")
}

#[test]
fn an_inactive_facade_reports_unready_and_discovers_no_models() {
    let service = SpeechService::new();

    let status = service.status();
    assert!(!status.configured());
    assert!(!status.ready());
    assert!(!status.gpu());
    assert_eq!(status.generation(), None);
    assert!(service.models().is_empty());
}

#[tokio::test]
async fn the_initial_load_publishes_one_complete_runtime() {
    let interim = ScriptedDecoder::new();
    interim.push_text("boot transcript");
    let final_decoder = ScriptedDecoder::new();
    let service = scripted_loaded_service(
        ScriptedModelFactory::new(interim.clone())
            .with_final(final_decoder.clone())
            .with_gpu_available(true),
        15,
        500,
    )
    .expect("the initial load publishes");

    let status = service.status();
    assert!(status.configured());
    assert!(status.ready());
    assert!(status.gpu());
    assert_eq!(status.generation(), Some(1));
    assert_eq!(
        service
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim", "scripted-final", "realtime-transcribe"]
    );

    let (status_code, response) =
        transcribe_batch(service.clone(), "scripted-interim", &[0.25; 16]).await;
    assert_eq!(status_code, StatusCode::OK);
    assert_eq!(response["text"].as_str(), Some("boot transcript"));

    service.shutdown();
    assert!(interim.worker_dropped());
    assert!(final_decoder.worker_dropped());
}

#[test]
fn a_second_initial_load_is_rejected_without_disturbing_the_runtime() {
    let first = ScriptedDecoder::new();
    let service = scripted_loaded_service(ScriptedModelFactory::new(first.clone()), 15, 500)
        .expect("the initial load publishes");
    let second = ScriptedDecoder::new();

    let error = load_scripted_initial(&service, ScriptedModelFactory::new(second.clone()), 15, 500)
        .expect_err("the one initial load is already spent");
    assert!(error.to_string().contains("already attempted"), "{error}");
    assert!(
        second.creation_thread().is_none(),
        "a rejected load never constructs workers"
    );

    let status = service.status();
    assert!(status.ready());
    assert_eq!(status.generation(), Some(1));
    assert_eq!(
        service
            .models()
            .iter()
            .map(gateway_stt::SpeechModelInfo::name)
            .collect::<Vec<_>>(),
        ["scripted-interim"]
    );

    service.shutdown();
    assert!(first.worker_dropped());
}

#[test]
fn a_failed_initial_load_joins_started_workers_and_stays_unavailable() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    let service = SpeechService::new();

    let error = load_scripted_initial(
        &service,
        ScriptedModelFactory::new(interim.clone())
            .with_final(final_decoder.clone())
            .with_final_failure("final startup sentinel"),
        15,
        500,
    )
    .expect_err("the final worker failure fails the load");
    let debug = format!("{error:?}");
    assert!(debug.contains("final startup sentinel"), "{debug}");

    assert!(!service.status().ready());
    assert!(service.models().is_empty());
    assert!(
        interim.worker_dropped(),
        "the started interim worker is joined during cleanup"
    );

    let again = load_scripted_initial(
        &service,
        ScriptedModelFactory::new(ScriptedDecoder::new()),
        15,
        500,
    )
    .expect_err("a failed attempt is never retried");
    assert!(again.to_string().contains("already attempted"), "{again}");
}

#[test]
fn a_cancelled_initial_load_publishes_nothing_and_is_not_retried() {
    let config = speech_config(
        "[[stt_model]]\nname = \"speech\"\nrole = \"interim\"\n\
         source = \"/missing-interim.bin\"\nvram_gb = 1.0\n",
        "[\"speech\"]",
    );
    let service = SpeechService::new();
    let token = CancellationToken::new();
    token.cancel();

    let error = service
        .load_initial(&config, None, &token)
        .expect_err("cancellation before work fails the attempt");
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(!service.status().ready());
    assert!(service.models().is_empty());

    let error = service
        .load_initial(&config, None, &CancellationToken::new())
        .expect_err("the cancelled attempt is never retried");
    assert!(error.to_string().contains("already attempted"), "{error}");
}

#[derive(Debug)]
struct CancelDuringBuild {
    token: CancellationToken,
    dropped: Arc<AtomicBool>,
}

impl ModelFactory for CancelDuringBuild {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        self.token.cancel();
        Ok(match mode {
            DecodeMode::Interim => Some(Box::new(TrackedDecoder(Arc::clone(&self.dropped)))),
            DecodeMode::Final => None,
        })
    }
}

struct TrackedDecoder(Arc<AtomicBool>);

impl Decoder for TrackedDecoder {
    fn decode(&mut self, _request: DecodeRequest) -> Result<String, TranscribeError> {
        Ok(String::new())
    }
}

impl Drop for TrackedDecoder {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[test]
fn cancellation_after_construction_joins_the_built_workers() {
    let dropped = Arc::new(AtomicBool::new(false));
    let token = CancellationToken::new();
    let service = SpeechService::new();

    let error = load_scripted_initial_with_cancellation(
        &service,
        CancelDuringBuild {
            token: token.clone(),
            dropped: Arc::clone(&dropped),
        },
        &token,
    )
    .expect_err("cancellation before publication fails the load");
    assert!(error.to_string().contains("cancelled"), "{error}");
    assert!(
        dropped.load(Ordering::Acquire),
        "the built worker is joined and its decoder dropped"
    );
    assert!(!service.status().ready());
    assert!(service.models().is_empty());
}

#[test]
fn a_panicking_load_leaves_speech_unavailable_without_poisoning_state() {
    let interim = ScriptedDecoder::new();
    let service = SpeechService::new();

    let error = load_scripted_initial(
        &service,
        ScriptedModelFactory::new(interim.clone()).with_interim_panic(),
        15,
        500,
    )
    .expect_err("the worker panic surfaces as a load failure");
    let debug = format!("{error:?}");
    assert!(debug.contains("WorkerPanicked"), "{debug}");

    assert!(!service.status().ready(), "speech stays unavailable");
    assert!(service.models().is_empty());
    let again = load_scripted_initial(
        &service,
        ScriptedModelFactory::new(ScriptedDecoder::new()),
        15,
        500,
    )
    .expect_err("the panicked attempt is never retried");
    assert!(again.to_string().contains("already attempted"), "{again}");
}

#[test]
fn an_initial_load_without_speech_models_leaves_the_facade_inactive() {
    let config = speech_config("", "[]");
    let service = SpeechService::new();

    service
        .load_initial(&config, None, &CancellationToken::new())
        .expect("an empty speech selection loads nothing");

    let status = service.status();
    assert!(!status.configured());
    assert!(!status.ready());
    assert!(service.models().is_empty());
}

#[test]
fn request_and_worker_job_ownership_are_counted_independently() {
    let decoder = ScriptedDecoder::new();
    let service = scripted_loaded_service(ScriptedModelFactory::new(decoder.clone()), 15, 500)
        .expect("the initial load publishes");
    let request = generation_ownership(&service).expect("the published runtime admits a request");

    let job = request
        .own_worker_job()
        .expect("the admitted request owns a worker job");
    drop(request);
    drop(job);

    service.shutdown();
    assert!(decoder.worker_dropped());
}

#[test]
fn shutdown_stops_admission_and_joins_workers() {
    let interim = ScriptedDecoder::new();
    let service = scripted_loaded_service(ScriptedModelFactory::new(interim.clone()), 15, 500)
        .expect("the initial load publishes");

    service.shutdown();

    assert!(!service.status().ready());
    assert!(service.models().is_empty());
    assert!(generation_ownership(&service).is_none());
    assert!(interim.worker_dropped());
}

#[test]
fn dropping_the_final_owner_joins_the_workers() {
    let interim = ScriptedDecoder::new();
    let service = scripted_loaded_service(ScriptedModelFactory::new(interim.clone()), 15, 500)
        .expect("the initial load publishes");
    let clone = service.clone();

    drop(service);
    assert!(
        !interim.worker_dropped(),
        "a surviving clone keeps the runtime alive"
    );
    drop(clone);
    assert!(
        interim.wait_until_worker_dropped(WAIT),
        "the final owner drop joins the workers"
    );
}

#[test]
fn a_replacement_is_refused_after_the_initial_load() {
    let interim = ScriptedDecoder::new();
    let service = scripted_loaded_service(ScriptedModelFactory::new(interim.clone()), 15, 500)
        .expect("the initial load publishes");
    let replacement = ScriptedDecoder::new();

    let error = gateway_stt::test_fixtures::begin_scripted_replacement(
        &service,
        ScriptedModelFactory::new(replacement.clone()),
        false,
        WAIT,
    )
    .expect_err("the compatibility replacement path is closed after the initial load");
    assert!(error.to_string().contains("already attempted"), "{error}");
    assert!(
        replacement.creation_thread().is_none(),
        "a refused replacement never constructs workers"
    );
    assert!(service.status().ready());

    service.shutdown();
    assert!(interim.worker_dropped());
}
