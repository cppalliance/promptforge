//! Native interim decode timing over successive `jfk.wav` windows, recorded in
//! the `native` sections of the speech-sandbox baseline and metrics.

#![expect(
    clippy::expect_used,
    reason = "native fixture setup fails by panicking with the missing invariant named"
)]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{fixture_dir, jfk_samples};
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::test_fixtures::native::require_fixture;
use gateway_stt_engine::{DecodeMode, DecodeRequest, EnginePolicy, SttEngine};
use serde_json::{Map, Value};

const UPDATE_VARIABLE: &str = "PROMPTFORGE_REPLAY_UPDATE";
const NATIVE_SECTION: &str = "native";
/// The gateway's default interim window and cadence.
const WINDOW_SECONDS: u64 = 15;
const INTERVAL_MS: u64 = 500;

fn replay_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../api/tests/fixtures/replay")
}

#[test]
fn native_timing_writes_the_speech_sandbox_replay_directory() {
    let baseline = replay_dir().join("baseline.json");
    assert!(
        baseline.is_file(),
        "native timing writes beside the committed speech-sandbox baseline, expected {}",
        baseline.display()
    );
}

/// The window an interim tick decodes after every interval of appended
/// audio, skipping windows shorter than the interim minimum.
fn interim_windows(samples: &[f32]) -> Vec<&[f32]> {
    let window =
        usize::try_from(WINDOW_SECONDS).expect("the window fits usize") * EnginePolicy::SAMPLE_RATE;
    let interval = usize::try_from(INTERVAL_MS).expect("the interval fits usize")
        * EnginePolicy::SAMPLE_RATE
        / 1_000;
    (1..=samples.len() / interval)
        .map(|tick| {
            let end = tick * interval;
            &samples[end.saturating_sub(window)..end]
        })
        .filter(|window| window.len() >= EnginePolicy::MIN_WINDOW_SAMPLES)
        .collect()
}

fn interim_request(window: &[f32]) -> DecodeRequest {
    DecodeRequest::new(
        DecodeMode::Interim,
        window.to_vec(),
        Vec::new(),
        String::new(),
    )
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1_000.0
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a clip has far fewer than 2^53 interim windows"
)]
fn timing_section(walls: &[Duration], gpu_available: bool) -> Value {
    let total = walls.iter().sum::<Duration>();
    let max = walls.iter().max().copied().unwrap_or_default();
    serde_json::json!({
        "gpu_available": gpu_available,
        "interim_decodes": walls.len(),
        "interim_decode_mean_ms": millis(total) / walls.len().max(1) as f64,
        "interim_decode_max_ms": millis(max),
        "window_seconds": WINDOW_SECONDS,
    })
}

fn sections(path: &Path) -> Map<String, Value> {
    if !path.exists() {
        return Map::new();
    }
    let bytes =
        std::fs::read(path).unwrap_or_else(|error| panic!("{} reads: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("{} parses: {error}", path.display()))
}

fn write_sections(path: &Path, sections: &Map<String, Value>) {
    let mut text = serde_json::to_string_pretty(sections).expect("sections serialize");
    text.push('\n');
    std::fs::write(path, text).unwrap_or_else(|error| panic!("{} writes: {error}", path.display()));
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn native_interim_decode_timing_is_recorded_in_the_native_sections() {
    let library = require_fixture("PROMPTFORGE_WHISPER_LIBRARY", &fixture_dir(), "whisper.dll");
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &fixture_dir(),
        "ggml-tiny.en.bin",
    );
    let factory = WhisperModelFactory::new(WhisperConfig::new(
        library,
        model,
        None,
        WINDOW_SECONDS,
        None,
    ))
    .expect("packaged runtime loads");
    let gpu_available = factory.gpu_available();
    let policy = EnginePolicy::new(WINDOW_SECONDS, INTERVAL_MS, gpu_available)
        .expect("the gateway default policy is valid");
    let engine = SttEngine::new(factory, policy).expect("the interim model loads");
    let samples = jfk_samples();
    let windows = interim_windows(&samples);
    assert!(!windows.is_empty(), "the clip yields interim windows");

    engine
        .decode(interim_request(windows[0]))
        .await
        .expect("the warm-up interim decode succeeds");
    let mut walls = Vec::with_capacity(windows.len());
    for window in &windows {
        let started = Instant::now();
        engine
            .decode(interim_request(window))
            .await
            .expect("interim decode succeeds");
        walls.push(started.elapsed());
    }
    engine.shutdown().expect("the engine shuts down");
    let section = timing_section(&walls, gpu_available);
    eprintln!("native interim decode timing: {section}");

    let baseline_path = replay_dir().join("baseline.json");
    let metrics_path = replay_dir().join("metrics.json");
    let mut baseline = sections(&baseline_path);
    let mut metrics = sections(&metrics_path);
    if !baseline.contains_key(NATIVE_SECTION) {
        baseline.insert(NATIVE_SECTION.to_owned(), section.clone());
        write_sections(&baseline_path, &baseline);
        metrics.insert(NATIVE_SECTION.to_owned(), section);
        write_sections(&metrics_path, &metrics);
    } else if std::env::var(UPDATE_VARIABLE).is_ok_and(|value| value == "1") {
        metrics.insert(NATIVE_SECTION.to_owned(), section);
        write_sections(&metrics_path, &metrics);
    }
}
