//! Native Silero tests. They read `PROMPTFORGE_WHISPER_LIBRARY`,
//! `PROMPTFORGE_WHISPER_AUDIO`, and `PROMPTFORGE_SILERO_MODEL`, and the
//! audio's scripted line file beside it as `<stem>-lines.json`; run them on
//! both `jfk.wav` and `dictation-01.wav`.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::VadContext;
use crate::tests::{CapturedLog, native_fixture};
use crate::{WhisperError, WhisperLibrary};

/// SHA-256 of the pinned `ggml-silero-v6.2.0.bin`.
const SILERO_SHA256: &str = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987";
const CHUNK: usize = VadContext::CHUNK_SAMPLES;
const SAMPLES_PER_MS: usize = 16;

/// Speech starts at 0.5 and ends below 0.35 in the speech detector.
const SPEECH_THRESHOLD: f32 = 0.5;
const SILENCE_THRESHOLD: f32 = 0.35;

/// Silero holds a probability up briefly after speech ends, and line times
/// come from word timestamps, so pause checks skip this much at both edges.
const PAUSE_MARGIN: usize = 250 * SAMPLES_PER_MS;

/// The audio fixture followed by at least 1 s of digital silence, padded to
/// whole chunks.
struct Fixture {
    samples: Vec<f32>,
    silence: Range<usize>,
    lines: Vec<Range<usize>>,
}

fn library() -> WhisperLibrary {
    WhisperLibrary::load(&native_fixture("PROMPTFORGE_WHISPER_LIBRARY"))
        .expect("packaged whisper runtime loads")
}

/// whisper.cpp can throw a C++ exception across the FFI boundary on a
/// malformed model, so the digest is checked before init.
fn silero(library: &WhisperLibrary) -> VadContext {
    let path = native_fixture("PROMPTFORGE_SILERO_MODEL");
    let bytes = std::fs::read(&path).expect("Silero model reads");
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(&bytes) {
        write!(&mut digest, "{byte:02x}").expect("writing to String is infallible");
    }
    assert_eq!(
        digest,
        SILERO_SHA256,
        "{} is the pinned Silero v6.2.0 model",
        path.display()
    );
    VadContext::new(library, &path).expect("Silero model loads")
}

fn fixture() -> Fixture {
    let path = native_fixture("PROMPTFORGE_WHISPER_AUDIO");
    let mut reader = hound::WavReader::open(&path).expect("audio fixture opens");
    let spec = reader.spec();
    assert_eq!(
        (spec.sample_rate, spec.channels, spec.bits_per_sample),
        (16_000, 1, 16),
        "{} is 16 kHz mono 16-bit PCM",
        path.display()
    );
    let mut samples = reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("fixture sample decodes")) / 32_768.0)
        .collect::<Vec<_>>();
    let audio_end = samples.len();
    samples.resize(
        (audio_end + 1_000 * SAMPLES_PER_MS).next_multiple_of(CHUNK),
        0.0,
    );
    Fixture {
        silence: audio_end..samples.len(),
        lines: lines(&path),
        samples,
    }
}

/// Reads each scripted line's speech span, in samples.
fn lines(audio: &Path) -> Vec<Range<usize>> {
    let stem = audio
        .file_stem()
        .and_then(|stem| stem.to_str())
        .expect("audio fixture has a UTF-8 stem");
    let path = audio.with_file_name(format!("{stem}-lines.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} reads: {error}", path.display()));
    let value: serde_json::Value = serde_json::from_str(&text).expect("line file is JSON");
    let sample = |line: &serde_json::Value, key: &str| {
        let ms = line[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{key} is a whole millisecond"));
        usize::try_from(ms).expect("millisecond fits usize") * SAMPLES_PER_MS
    };
    let lines = value
        .as_array()
        .expect("line file is an array")
        .iter()
        .map(|line| sample(line, "start_ms")..sample(line, "end_ms"))
        .collect::<Vec<_>>();
    assert!(!lines.is_empty(), "{} names lines", path.display());
    lines
}

fn stream(vad: &mut VadContext, samples: &[f32]) -> Vec<f32> {
    samples
        .as_chunks::<CHUNK>()
        .0
        .iter()
        .map(|chunk| vad.detect_chunk(chunk).expect("a whole chunk classifies"))
        .collect()
}

/// Probabilities of the chunks lying wholly inside `span`.
fn within<'a>(probabilities: &'a [f32], span: &Range<usize>) -> &'a [f32] {
    let first = span.start.div_ceil(CHUNK).min(probabilities.len());
    let end = (span.end / CHUNK).clamp(first, probabilities.len());
    &probabilities[first..end]
}

/// The nearest-rank `percent` percentile of ascending `sorted`.
fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    sorted[(sorted.len() * percent).div_ceil(100).saturating_sub(1)]
}

#[test]
#[ignore = "requires packaged whisper and Silero model fixtures"]
fn pinned_model_loads_and_a_missing_one_is_rejected() {
    let library = library();
    let _vad = silero(&library);
    let missing = native_fixture("PROMPTFORGE_SILERO_MODEL").with_file_name("missing-silero.bin");
    assert!(
        matches!(
            VadContext::new(&library, &missing),
            Err(WhisperError::NullVadContext { path }) if path == missing
        ),
        "a model whisper cannot open yields no context"
    );
}

#[test]
#[ignore = "requires packaged whisper, Silero model, and audio fixtures"]
fn probabilities_are_high_in_speech_and_low_in_pauses_and_silence() {
    let library = library();
    let mut vad = silero(&library);
    let fixture = fixture();
    let probabilities = stream(&mut vad, &fixture.samples);

    for line in &fixture.lines {
        let speech = within(&probabilities, line);
        let voiced = speech.iter().filter(|p| **p >= SPEECH_THRESHOLD).count();
        assert!(
            !speech.is_empty() && voiced * 2 >= speech.len(),
            "most of line {line:?} is speech: {speech:.2?}"
        );
    }
    for pair in fixture.lines.windows(2) {
        let pause = (pair[0].end + PAUSE_MARGIN)..(pair[1].start.saturating_sub(PAUSE_MARGIN));
        let pause = within(&probabilities, &pause);
        assert!(
            pause.iter().all(|p| *p < SILENCE_THRESHOLD),
            "the pause between {:?} and {:?} is not speech: {pause:.2?}",
            pair[0],
            pair[1]
        );
    }
    let silence = (fixture.silence.start + PAUSE_MARGIN)..fixture.silence.end;
    let silence = within(&probabilities, &silence);
    assert!(
        !silence.is_empty() && silence.iter().all(|p| *p < SILENCE_THRESHOLD),
        "appended digital silence is not speech: {silence:.2?}"
    );
}

#[test]
#[ignore = "requires packaged whisper, Silero model, and audio fixtures"]
fn streaming_chunks_agree_with_one_whole_buffer_call() {
    // Both paths run the same graph over the same windows and carry the same
    // LSTM state between them, so only float rounding may differ.
    const TOLERANCE: f32 = 1e-5;
    let library = library();
    let mut vad = silero(&library);
    let samples = fixture().samples;
    let streamed = stream(&mut vad, &samples);
    vad.reset();
    let whole = vad
        .probabilities(&samples)
        .expect("the whole buffer classifies");
    assert_eq!(whole.len(), streamed.len(), "one probability per chunk");
    let (index, worst) = streamed
        .iter()
        .zip(&whole)
        .map(|(streamed, whole)| (streamed - whole).abs())
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("the fixture has chunks");
    assert!(
        worst <= TOLERANCE,
        "chunk {index} differs by {worst}: streamed {} whole {}",
        streamed[index],
        whole[index]
    );
}

#[test]
#[ignore = "requires packaged whisper and Silero model fixtures"]
fn a_chunk_of_any_other_length_is_rejected() {
    let library = library();
    let mut vad = silero(&library);
    for samples in [0, CHUNK - 1, CHUNK + 1, 2 * CHUNK] {
        let error = vad
            .detect_chunk(&vec![0.0; samples])
            .expect_err("only one whole chunk classifies");
        assert!(
            matches!(error, WhisperError::VadChunkLength { samples: got } if got == samples),
            "{samples} samples: {error:?}"
        );
    }
    vad.detect_chunk(&[0.0; CHUNK])
        .expect("the context still classifies a whole chunk");
}

#[test]
#[ignore = "requires packaged whisper and Silero model fixtures"]
fn detection_logs_reach_tracing_only_at_trace() {
    let library = library();
    library.set_log_callback();
    let mut vad = silero(&library);
    let log = CapturedLog::default();
    tracing::subscriber::with_default(log.clone(), || {
        vad.detect_chunk(&[0.0; CHUNK])
            .expect("a whole chunk classifies");
        // A partial final window adds the `chunk_len` line.
        vad.probabilities(&[0.0; CHUNK + 100])
            .expect("two windows classify");
    });
    let events = log.take_leveled();
    let loud = events
        .iter()
        .filter(|(level, _)| *level <= tracing::Level::INFO)
        .collect::<Vec<_>>();
    assert!(
        loud.is_empty(),
        "detection logs nothing at INFO or above: {loud:?}"
    );
    let traced = events
        .iter()
        .filter(|(level, _)| *level == tracing::Level::TRACE)
        .count();
    assert_eq!(
        traced, 9,
        "four lines per call plus `chunk_len`: {events:?}"
    );
}

/// Run alone on `dictation-01.wav`. Above 1 ms at p99, detection no longer
/// fits inline on the session task.
#[test]
#[ignore = "requires packaged whisper, Silero model, and audio fixtures"]
fn per_chunk_detection_stays_within_one_millisecond_at_p99() {
    const WARM_UP_CHUNKS: usize = 64;
    const BUDGET: Duration = Duration::from_millis(1);
    let library = library();
    // Production routes the per-call log lines through the tracing bridge;
    // whisper's default handler writes them to stderr instead.
    library.set_log_callback();
    let mut vad = silero(&library);
    let samples = fixture().samples;
    let chunks = samples.as_chunks::<CHUNK>().0;
    for chunk in chunks.iter().take(WARM_UP_CHUNKS) {
        vad.detect_chunk(chunk).expect("a whole chunk classifies");
    }
    vad.reset();
    let mut costs = chunks
        .iter()
        .map(|chunk| {
            let start = Instant::now();
            vad.detect_chunk(chunk).expect("a whole chunk classifies");
            start.elapsed()
        })
        .collect::<Vec<_>>();
    costs.sort_unstable();
    let p50 = percentile(&costs, 50);
    let p99 = percentile(&costs, 99);
    println!(
        "Silero cost per chunk over {} chunks on one CPU thread: p50 {p50:?}, p99 {p99:?}, max {:?}; {}",
        costs.len(),
        costs.last().expect("the fixture has chunks"),
        library
            .system_info()
            .expect("system information is exported"),
    );
    assert!(
        p99 <= BUDGET,
        "p99 {p99:?} exceeds {BUDGET:?} (p50 {p50:?})"
    );
}
