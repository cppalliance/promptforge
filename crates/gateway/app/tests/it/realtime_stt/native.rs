//! Native whisper fixture: the workspace-local fixture root, the clip at
//! 24 kHz, the packaged native speech service, and its incremental-span
//! checks.

use std::path::Path;
use std::path::PathBuf;

use gateway::Config;
use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::native::{
    fixture_final_model, fixture_whisper_backend, require_fixture,
};
use tempfile::TempDir;

/// The `[stt]` tuning the incremental-span checks expect: a four-second
/// window, so the JFK clip slides within its eleven seconds.
const INCREMENTAL_TUNING: &str = "window_seconds = 4\ninterval_ms = 500\n";

pub(super) fn native_fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../local/stt-fixtures")
}

#[test]
fn gateway_native_realtime_keeps_its_workspace_local_fixture_root() {
    assert_eq!(
        native_fixture_root(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../local/stt-fixtures")
    );
}

/// The 16 kHz mono 16-bit clip `PROMPTFORGE_WHISPER_AUDIO` names, or
/// `jfk.wav`, upsampled to the 24 kHz wire rate.
pub(super) fn native_clip_24khz() -> Vec<i16> {
    clip_24khz(&require_fixture(
        "PROMPTFORGE_WHISPER_AUDIO",
        &native_fixture_root(),
        "jfk.wav",
    ))
}

/// The 16 kHz mono 16-bit clip at `path`, upsampled to the 24 kHz wire rate.
pub(super) fn clip_24khz(path: &Path) -> Vec<i16> {
    let mut reader = hound::WavReader::open(path)
        .unwrap_or_else(|error| panic!("{} opens: {error}", path.display()));
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "native clip must be 16 kHz");
    assert_eq!(spec.channels, 1, "native clip must be mono");
    assert_eq!(spec.bits_per_sample, 16, "native clip must be 16-bit PCM");
    let source = reader
        .samples::<i16>()
        .map(|sample| sample.expect("native clip sample decodes"))
        .collect::<Vec<_>>();
    let mut resampled = Vec::with_capacity(source.len() * 3 / 2);
    for pair in source.chunks(2) {
        let first = pair[0];
        let second = pair.get(1).copied().unwrap_or(first);
        let midpoint = i16::try_from(i32::midpoint(i32::from(first), i32::from(second)))
            .expect("the midpoint of two i16 samples remains i16");
        resampled.extend([first, midpoint, second]);
    }
    resampled
}

pub(super) fn native_speech_service() -> (SpeechService, TempDir) {
    native_speech_service_with(INCREMENTAL_TUNING)
}

/// The packaged speech service under a real interim and final
/// `[[stt_model]]` pair, with `tuning` as the `[stt]` keys beside the
/// whisper backend; empty tuning keeps the gateway defaults. Also returns
/// the temporary `[local] cache_dir` the service provisions into, which the
/// caller holds until the service shuts down: each take opens the Silero
/// model from it.
pub(super) fn native_speech_service_with(tuning: &str) -> (SpeechService, TempDir) {
    let model = require_fixture(
        "PROMPTFORGE_WHISPER_MODEL",
        &native_fixture_root(),
        "ggml-tiny.en.bin",
    );
    let final_model = fixture_final_model(&model);
    let [model, final_model] =
        [model, final_model].map(|path| path.display().to_string().replace('\\', "/"));
    let whisper_backend = fixture_whisper_backend();
    let tuning = tuning.to_owned();
    std::thread::spawn(move || {
        let cache = tempfile::tempdir().expect("native test cache creates");
        let cache_dir = cache.path().display().to_string().replace('\\', "/");
        let catalog = Config::from_toml_str(&format!(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [local]\ncache_dir = {cache_dir:?}\n\
             [stt]\n{tuning}whisper_backend = {whisper_backend:?}\n\
             [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = {model:?}\nvram_gb = 1.0\n\
             [[stt_model]]\nname = \"speech-final\"\nrole = \"final\"\nsource = {final_model:?}\nvram_gb = 1.0\n\
             [[profile]]\nname = \"native\"\nmodels = [\"speech\", \"speech-final\"]\n"
        ))
        .expect("native fixture catalog parses");
        let config = catalog
            .select_profile(Some(
                &gateway_config::ProfileName::parse("native").expect("profile name"),
            ))
            .expect("native fixture profile selects");
        let service = SpeechService::new();
        service
            .load_initial(&config, None, &tokio_util::sync::CancellationToken::new())
            .expect("the native speech engine loads");
        (service, cache)
    })
    .join()
    .expect("native startup thread joins")
}

pub(super) fn assert_native_incremental_spans(spans: &[(u64, u64, String)]) {
    assert!(
        spans.windows(2).all(|pair| pair[0].1 < pair[1].1),
        "incremental snapshots advance their accepted audio end"
    );
    assert_eq!(spans[0].0, 0);
    let normalized = spans
        .iter()
        .map(|(_, _, transcript)| normalized_words(transcript))
        .collect::<Vec<_>>();
    assert_eq!(
        spans.iter().map(|span| span.0).collect::<Vec<_>>(),
        [0, 0, 0, 1_000],
        "three growing windows precede the first one-second slide"
    );
    assert!(
        normalized[1].len() < normalized[2].len() && normalized[2].starts_with(&normalized[1]),
        "the fixed-origin JFK hypothesis grows before sliding: {normalized:?}"
    );
    assert!(
        spans.iter().skip(1).any(|(start, _, _)| *start > 0),
        "the packaged native route eventually slides its window origin"
    );
    assert!(
        normalized[3].starts_with(&normalized[2]),
        "sliding snapshots retain prior speech exactly once: {normalized:?}"
    );
    assert_eq!(
        normalized.last().expect("a final native snapshot exists"),
        &["and", "so", "my", "fellow", "americans", "ask", "not"],
        "the known JFK overlap is rebased without duplication"
    );
    assert!(
        spans
            .iter()
            .all(|(_, _, transcript)| !transcript.is_empty()),
        "every emitted native hypothesis includes replacement text"
    );
}

fn normalized_words(transcript: &str) -> Vec<String> {
    transcript
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}
