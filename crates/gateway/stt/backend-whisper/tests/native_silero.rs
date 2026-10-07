//! Native characterization of the Silero speech detector. The tests read
//! `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_SILERO_MODEL`,
//! `PROMPTFORGE_WHISPER_AUDIO` with its scripted line file beside it as
//! `<stem>-lines.json`, and `PROMPTFORGE_NOISE_CLIPS`, a directory of 16 kHz
//! mono noise clips without speech. Run them on both `jfk.wav` and
//! `dictation-01.wav`.

#![expect(
    clippy::expect_used,
    reason = "native fixture setup fails by panicking with the missing invariant named"
)]

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gateway_stt_backend_whisper::SileroDetector;
use gateway_stt_engine::SpeechDetector;
use gateway_whisper_ffi::WhisperLibrary;
use sha2::{Digest, Sha256};

/// SHA-256 of the pinned `ggml-silero-v6.2.0.bin`.
const SILERO_SHA256: &str = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987";
const CHUNK: usize = 512;
const SAMPLES_PER_MS: usize = 16;
/// Line times come from word timestamps, Silero needs a chunk or two of a
/// word before it reaches the start threshold, and the segmenter keeps
/// 100 ms after speech for a trailing consonant, so a line counts as covered
/// when speech starts or ends this close to its edges.
const EDGE_SLACK: usize = 100 * SAMPLES_PER_MS;
/// A gap shorter than the segmenter's 0.6 s sentence-end silence never
/// closes a segment, so it does not break a line's cover.
const BRIDGED_GAP: usize = 600 * SAMPLES_PER_MS;
/// A pause at least this long holds no speech decision past
/// [`PAUSE_SPILL`] from either of its edges.
const LONG_PAUSE: usize = 1_000 * SAMPLES_PER_MS;
const PAUSE_SPILL: usize = 500 * SAMPLES_PER_MS;
const MIN_NOISE_CLIPS: usize = 3;

fn native_fixture(variable: &str) -> PathBuf {
    let Some(path) = std::env::var_os(variable) else {
        panic!("{variable} is set");
    };
    PathBuf::from(path)
}

fn library() -> WhisperLibrary {
    let library = WhisperLibrary::load(&native_fixture("PROMPTFORGE_WHISPER_LIBRARY"))
        .expect("packaged whisper runtime loads");
    library.set_log_callback();
    library
}

/// whisper.cpp can throw a C++ exception across the FFI boundary on a
/// malformed model, so the digest is checked before every load.
fn detector(library: &WhisperLibrary) -> SileroDetector {
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
    SileroDetector::new(library, &path).expect("Silero model loads")
}

fn samples(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path)
        .unwrap_or_else(|error| panic!("{} opens: {error}", path.display()));
    let spec = reader.spec();
    assert_eq!(
        (spec.sample_rate, spec.channels, spec.bits_per_sample),
        (16_000, 1, 16),
        "{} is 16 kHz mono 16-bit PCM",
        path.display()
    );
    reader
        .samples::<i16>()
        .map(|sample| f32::from(sample.expect("clip sample decodes")) / 32_768.0)
        .collect()
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

/// The detector's decision for every whole chunk of `samples`, in order.
fn decisions(detector: &mut SileroDetector, samples: &[f32]) -> Vec<bool> {
    samples
        .as_chunks::<CHUNK>()
        .0
        .iter()
        .map(|chunk| detector.classify(chunk).expect("a whole chunk classifies"))
        .collect()
}

/// The sample ranges of the chunks decided as speech, merged into runs.
fn speech_runs(decisions: &[bool]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (index, _) in decisions.iter().enumerate().filter(|(_, speech)| **speech) {
        let start = index * CHUNK;
        match runs.last_mut() {
            Some(run) if run.end == start => run.end = start + CHUNK,
            _ => runs.push(start..start + CHUNK),
        }
    }
    runs
}

/// `runs` with every gap shorter than [`BRIDGED_GAP`] filled.
fn bridged(runs: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut joined: Vec<Range<usize>> = Vec::new();
    for run in runs {
        match joined.last_mut() {
            Some(last) if run.start - last.end < BRIDGED_GAP => last.end = run.end,
            _ => joined.push(run.clone()),
        }
    }
    joined
}

fn ms(range: &Range<usize>) -> Range<usize> {
    range.start / SAMPLES_PER_MS..range.end / SAMPLES_PER_MS
}

#[test]
#[ignore = "requires packaged whisper, Silero model, and audio fixtures"]
fn speech_decisions_cover_every_line_and_stay_out_of_long_pauses() {
    let library = library();
    let mut detector = detector(&library);
    let audio = native_fixture("PROMPTFORGE_WHISPER_AUDIO");
    let lines = lines(&audio);
    let samples = samples(&audio);
    let runs = speech_runs(&decisions(&mut detector, &samples));
    let heard = runs.iter().map(ms).collect::<Vec<_>>();

    let joined = bridged(&runs);
    for line in &lines {
        let covered = joined
            .iter()
            .any(|run| run.start <= line.start + EDGE_SLACK && line.end <= run.end + EDGE_SLACK);
        assert!(
            covered,
            "speech covers the line at {:?} ms through gaps under {} ms; speech runs {heard:?} ms",
            ms(line),
            BRIDGED_GAP / SAMPLES_PER_MS
        );
    }
    let edges = std::iter::once(0)
        .chain(lines.iter().flat_map(|line| [line.start, line.end]))
        .chain(std::iter::once(samples.len()))
        .collect::<Vec<_>>();
    for pause in edges
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[start, end]| start..end)
    {
        if pause.len() < LONG_PAUSE {
            continue;
        }
        let inside = (pause.start + PAUSE_SPILL)..(pause.end - PAUSE_SPILL);
        let intruding = runs
            .iter()
            .filter(|run| run.start < inside.end && inside.start < run.end)
            .map(ms)
            .collect::<Vec<_>>();
        assert!(
            intruding.is_empty(),
            "no speech run reaches more than {} ms into the pause at {:?} ms, found {intruding:?} ms",
            PAUSE_SPILL / SAMPLES_PER_MS,
            ms(&pause)
        );
    }
}

#[test]
#[ignore = "requires packaged whisper, Silero model, and noise clip fixtures"]
fn noise_clips_hold_no_speech_decision() {
    let directory = native_fixture("PROMPTFORGE_NOISE_CLIPS");
    let mut clips = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{} reads: {error}", directory.display()))
        .map(|entry| entry.expect("a noise clip entry reads").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "wav"))
        .collect::<Vec<_>>();
    clips.sort();
    assert!(
        clips.len() >= MIN_NOISE_CLIPS,
        "{} holds at least {MIN_NOISE_CLIPS} noise clips, found {}",
        directory.display(),
        clips.len()
    );
    let library = library();
    for clip in &clips {
        let mut detector = detector(&library);
        let runs = speech_runs(&decisions(&mut detector, &samples(clip)));
        assert!(
            runs.is_empty(),
            "{} holds no speech, but the detector heard {:?} ms",
            clip.display(),
            runs.iter().map(ms).collect::<Vec<_>>()
        );
    }
}
