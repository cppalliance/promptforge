//! Native long-speech regression: streams long read-speech clips through a
//! production Realtime session at the pace of a live client and fails when
//! the completed transcript drops a run of five or more consecutive words.
//!
//! `PROMPTFORGE_LONG_SPEECH_CLIPS` names a directory holding `clips.json`,
//! an array of `{ "id", "file", "text" }` entries, and the 16 kHz mono 16-bit
//! WAV files it lists. The test returns at once when the variable is unset.
//! It reads the models the native replay capture reads:
//! `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL` for captions,
//! and `PROMPTFORGE_WHISPER_FINAL_MODEL` for finals. The test needs the
//! recommended pair, `base.en` for captions and `small.en` for finals.
//!
//! Each clip streams as 100 ms appends, each followed by a 100 ms sleep on
//! the wall clock, so the final decodes of forced windows overlap the stream
//! as they do for a live client. Reference and transcript are lowercased,
//! stripped of punctuation and split into words, then aligned by word edit
//! distance. A reference word that the alignment leaves without a counterpart
//! is missing; a word the transcriber heard differently is not. The test
//! gates on the yes/no property that no five consecutive reference words are
//! missing, and never on an error rate.

#![expect(
    clippy::expect_used,
    reason = "a native fixture that fails to stream fails the test with the clip named"
)]

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gateway_stt::SpeechService;
use serde::Deserialize;

use crate::common;
use crate::native_session::audio::pcm24_payload;
use crate::native_session::{self, CHUNK_SAMPLES, TICK_SAMPLES};

const CLIPS_VARIABLE: &str = "PROMPTFORGE_LONG_SPEECH_CLIPS";
/// The wall-clock gap after each 100 ms append.
const PACE: Duration = Duration::from_millis(100);
/// A run of this many consecutive missing reference words fails a clip.
const MISSING_RUN: usize = 5;

#[derive(Deserialize)]
struct Clip {
    id: String,
    file: String,
    text: String,
}

fn read_clips(dir: &Path) -> Vec<Clip> {
    let path = dir.join("clips.json");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|error| panic!("{} reads: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("{} parses as clip entries: {error}", path.display()))
}

/// Streams `pcm` through a fresh production session and returns the
/// transcript its commit completes with.
async fn transcribe(service: &SpeechService, id: &str, pcm: &[i16]) -> String {
    let mut session = native_session::register(service);
    let total = u64::try_from(pcm.len()).expect("the clip length fits u64");
    let mut appended = 0;
    while appended < total {
        let end = (appended + CHUNK_SAMPLES).min(total);
        session
            .append_base64(&pcm24_payload(pcm, appended..end))
            .unwrap_or_else(|error| {
                panic!("{id}: the chunk ending at sample {end} appends: {error:?}")
            });
        appended = end;
        tokio::time::sleep(PACE).await;
        if end % TICK_SAMPLES == 0 {
            session.run_interim().await.unwrap_or_else(|error| {
                panic!("{id}: the interim at sample {end} runs: {error:?}")
            });
        }
    }
    let receipt = session
        .commit()
        .unwrap_or_else(|error| panic!("{id}: the clip commits: {error:?}"));
    session
        .finish_finalization(receipt.item_id())
        .await
        .unwrap_or_else(|error| panic!("{id}: the commit finalizes: {error:?}"));
    let results = session.drain_results();
    results
        .iter()
        .find(|result| result["type"] == "completed")
        .and_then(|result| result["transcript"].as_str())
        .map_or_else(
            || panic!("{id}: the commit completes, but its results were {results:?}"),
            str::to_owned,
        )
}

/// Lowercases `text`, drops apostrophes so contractions stay one word, turns
/// every other punctuation mark into a space and splits on whitespace.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .filter(|character| !matches!(character, '\'' | '\u{2019}'))
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// Returns each run of at least `min_run` consecutive `reference` words that
/// the word edit distance alignment against `transcript` deletes.
fn missing_runs(reference: &[String], transcript: &[String], min_run: usize) -> Vec<Range<usize>> {
    let mut distance = vec![vec![0_usize; transcript.len() + 1]; reference.len() + 1];
    for (row, costs) in distance.iter_mut().enumerate() {
        costs[0] = row;
    }
    for (column, cost) in distance[0].iter_mut().enumerate() {
        *cost = column;
    }
    for row in 1..=reference.len() {
        for column in 1..=transcript.len() {
            let substitution = usize::from(reference[row - 1] != transcript[column - 1]);
            distance[row][column] = (distance[row - 1][column - 1] + substitution)
                .min(distance[row - 1][column] + 1)
                .min(distance[row][column - 1] + 1);
        }
    }

    let mut deleted = vec![false; reference.len()];
    let (mut row, mut column) = (reference.len(), transcript.len());
    while row > 0 || column > 0 {
        if row > 0 && column > 0 {
            let substitution = usize::from(reference[row - 1] != transcript[column - 1]);
            if distance[row][column] == distance[row - 1][column - 1] + substitution {
                row -= 1;
                column -= 1;
                continue;
            }
        }
        if row > 0 && distance[row][column] == distance[row - 1][column] + 1 {
            deleted[row - 1] = true;
            row -= 1;
        } else {
            column -= 1;
        }
    }

    let mut runs = Vec::new();
    let mut start = None;
    for (index, &missing) in deleted.iter().chain(&[false]).enumerate() {
        match (missing, start) {
            (true, None) => start = Some(index),
            (false, Some(first)) => {
                if index - first >= min_run {
                    runs.push(first..index);
                }
                start = None;
            }
            _ => {}
        }
    }
    runs
}

#[tokio::test]
#[ignore = "requires packaged whisper, base.en and small.en models, and the long-speech clips"]
async fn realtime_long_speech() {
    let Some(dir) = std::env::var_os(CLIPS_VARIABLE).map(PathBuf::from) else {
        eprintln!("{CLIPS_VARIABLE} is unset, so there are no long-speech clips to stream");
        return;
    };
    let clips = read_clips(&dir);
    assert!(!clips.is_empty(), "{} lists no clips", dir.display());
    let service = native_session::load(native_session::whisper_factory());

    let mut failures = Vec::new();
    for clip in &clips {
        let pcm = common::wav_pcm16(&dir.join(&clip.file));
        let transcript = transcribe(&service, &clip.id, &pcm).await;
        let reference = words(&clip.text);
        let runs = missing_runs(&reference, &words(&transcript), MISSING_RUN);
        eprintln!(
            "{}: {} reference words, {} runs of {MISSING_RUN} or more missing",
            clip.id,
            reference.len(),
            runs.len()
        );
        failures.extend(runs.into_iter().map(|run| {
            let lost = reference[run.clone()].join(" ");
            format!(
                "{} lost reference words {}..{}: \"{lost}\" (transcript: {transcript:?})",
                clip.id, run.start, run.end
            )
        }));
    }
    assert!(
        failures.is_empty(),
        "the transcript dropped {MISSING_RUN} or more consecutive words:\n{}",
        failures.join("\n")
    );
}

#[test]
fn words_lowercase_drop_apostrophes_and_split_on_other_punctuation() {
    assert_eq!(
        words("Don't STOP - it\u{2019}s well-known, isn't it?"),
        ["dont", "stop", "its", "well", "known", "isnt", "it"]
    );
}

fn bounds(runs: &[Range<usize>]) -> Vec<(usize, usize)> {
    runs.iter().map(|run| (run.start, run.end)).collect()
}

#[test]
fn a_dropped_span_of_five_words_is_reported_with_its_position() {
    let reference = words("we hold these truths to be self evident that all men are created equal");
    let transcript = words("we hold these truths all men are created equal");

    assert_eq!(
        bounds(&missing_runs(&reference, &transcript, 5)),
        [(4, 9)],
        "the five words from `to` through `that` are missing"
    );
}

#[test]
fn a_dropped_span_of_four_words_is_not_reported() {
    let reference = words("we hold these truths to be self evident that all men");
    let transcript = words("we hold these truths that all men");

    assert!(missing_runs(&reference, &transcript, 5).is_empty());
}

#[test]
fn words_the_transcriber_heard_differently_are_not_missing() {
    let reference = words("she sells sea shells by the sea shore every single day");
    let transcript = words("she cells see shales buy the see shore every single day");

    assert!(missing_runs(&reference, &transcript, 5).is_empty());
}

#[test]
fn a_dropped_span_at_either_end_is_reported() {
    let reference = words("one two three four five six seven eight nine ten");

    let missing = |transcript: &str| bounds(&missing_runs(&reference, &words(transcript), 5));

    assert_eq!(missing("six seven eight nine ten"), [(0, 5)]);
    assert_eq!(missing("one two three four five"), [(5, 10)]);
    assert_eq!(missing(""), [(0, 10)]);
}
