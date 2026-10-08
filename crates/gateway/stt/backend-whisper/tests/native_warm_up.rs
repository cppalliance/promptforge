//! Native first interim decode timing after decoder creation, with and
//! without the warm-up decode.

#![cfg(feature = "test-fixtures")]
#![expect(
    clippy::expect_used,
    reason = "native fixture setup fails by panicking with the missing invariant named"
)]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{fixture_dir, jfk_samples};
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::test_fixtures::native::require_fixture;
use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy, SttEngine};

/// The gateway's default interim window and cadence.
const WINDOW_SECONDS: u64 = 15;
const INTERVAL_MS: u64 = 500;
/// Every timed decode transcribes the clip's first five seconds, so window
/// length cannot favor the first decode over the steady state.
const TIMED_WINDOW_SAMPLES: usize = 5 * EnginePolicy::SAMPLE_RATE;
const STEADY_DECODES: u32 = 10;
static NATIVE_TEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct FirstDecodeTiming {
    gpu_available: bool,
    load: Duration,
    first: Duration,
    steady_mean: Duration,
}

impl FirstDecodeTiming {
    fn report(&self, label: &str) {
        eprintln!(
            "{label}: gpu_available={} load={:?} first={:?} steady_mean={:?}",
            self.gpu_available, self.load, self.first, self.steady_mean
        );
    }
}

/// Builds an engine, warmed or not, and times its first interim decode and
/// the mean of the next [`STEADY_DECODES`] over the same window.
async fn first_decode_timing(warm_up: bool) -> FirstDecodeTiming {
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let final_model = std::env::var_os("PROMPTFORGE_WHISPER_FINAL_MODEL").map(PathBuf::from);
    let factory = WhisperModelFactory::new(WhisperConfig::new(
        library,
        model,
        final_model,
        WINDOW_SECONDS,
        None,
    ))
    .expect("packaged runtime loads");
    let factory = if warm_up {
        factory
    } else {
        factory.without_warm_up()
    };
    let gpu_available = factory.gpu_available();
    let policy = EnginePolicy::new(WINDOW_SECONDS, INTERVAL_MS, gpu_available)
        .expect("the gateway default policy is valid");
    let samples = jfk_samples();
    let window = samples
        .get(..TIMED_WINDOW_SAMPLES)
        .expect("the clip covers the timed window");

    let started = Instant::now();
    let engine = SttEngine::new(factory, policy).expect("the models load");
    let load = started.elapsed();
    let mut walls = Vec::new();
    for _ in 0..=STEADY_DECODES {
        let request = DecodeRequest::new(
            DecodeMode::Interim,
            window.to_vec(),
            Vec::new(),
            String::new(),
        );
        let started = Instant::now();
        engine
            .decode(request)
            .await
            .expect("interim decode succeeds");
        walls.push(started.elapsed());
    }
    engine.shutdown().expect("the engine shuts down");
    let (first, steady) = walls.split_first().expect("the first decode ran");
    FirstDecodeTiming {
        gpu_available,
        load,
        first: *first,
        steady_mean: steady.iter().sum::<Duration>() / STEADY_DECODES,
    }
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn a_warmed_decoder_runs_its_first_interim_decode_within_twice_the_steady_mean() {
    let _guard = NATIVE_TEST.lock().await;
    let timing = first_decode_timing(true).await;
    timing.report("warmed first interim decode");
    assert!(
        timing.first <= timing.steady_mean * 2,
        "the first interim decode took {:?}, more than twice the steady mean {:?}",
        timing.first,
        timing.steady_mean
    );
}

/// Records what the warm-up saves; run in its own process, as nextest does,
/// so no earlier decode in the process warms the runtime.
#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn a_cold_decoder_first_interim_decode_is_reported() {
    let _guard = NATIVE_TEST.lock().await;
    first_decode_timing(false)
        .await
        .report("cold first interim decode");
}
