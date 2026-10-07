//! Speech-sandbox replay of scripted and natively captured takes against
//! golden snapshots, the current metrics, and the baseline thresholds.

#![expect(
    clippy::expect_used,
    reason = "replay fixture helpers fail with the fixture operation named"
)]

mod metrics;
mod native_capture;
mod repeats;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gateway_stt::test_fixtures::{
    ReplayError, ReplayOutcome, ReplayScript, ReplayTake, ReplayTick,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use metrics::Metrics;

const UPDATE_VARIABLE: &str = "PROMPTFORGE_REPLAY_UPDATE";
const SCRIPTED_PREFIX: &str = "scripted-";
const NATIVE_FIXTURE: &str = "jfk-native";
/// Sections no replay produces, kept as written: the native interim decode
/// timing and the Workshop take reducer's rendered-text metrics, which
/// `crates/workshop/ui/test/stt-replay.mjs` owns.
const FOREIGN_SECTIONS: [&str; 2] = ["native", "ui"];
const SUMMARY_FILES: [&str; 2] = ["baseline", "metrics"];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("replay")
}

fn updating() -> bool {
    std::env::var(UPDATE_VARIABLE).is_ok_and(|value| value == "1")
}

fn read_json<T: DeserializeOwned>(path: &Path) -> T {
    let bytes = std::fs::read(path).unwrap_or_else(|error| {
        panic!(
            "{} reads ({error}); run with {UPDATE_VARIABLE}=1 to generate it",
            path.display()
        )
    });
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("{} parses: {error}", path.display()))
}

fn write_json(path: &Path, value: &impl Serialize) {
    let mut text = serde_json::to_string_pretty(value).expect("replay output serializes");
    text.push('\n');
    std::fs::write(path, text).unwrap_or_else(|error| panic!("{} writes: {error}", path.display()));
}

fn scripts() -> Vec<(String, ReplayScript)> {
    let mut scripts = std::fs::read_dir(fixture_dir())
        .expect("the replay fixture directory reads")
        .map(|entry| entry.expect("a replay fixture entry reads").path())
        .filter_map(|path| {
            let name = path
                .file_name()?
                .to_str()?
                .strip_suffix(".json")?
                .to_owned();
            (!name.ends_with(".snapshots") && !SUMMARY_FILES.contains(&name.as_str()))
                .then(|| (name, read_json(&path)))
        })
        .collect::<Vec<_>>();
    scripts.sort_by(|left, right| left.0.cmp(&right.0));
    scripts
}

fn sections(path: &Path) -> Map<String, Value> {
    if path.exists() {
        read_json(path)
    } else {
        Map::new()
    }
}

fn replay_sections(sections: &Map<String, Value>) -> BTreeMap<String, Metrics> {
    sections
        .iter()
        .filter(|(name, _)| !FOREIGN_SECTIONS.contains(&name.as_str()))
        .map(|(name, value)| {
            let metrics = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("the {name} metrics section parses: {error}"));
            (name.clone(), metrics)
        })
        .collect()
}

fn section(metrics: &Metrics) -> Value {
    serde_json::to_value(metrics).expect("metrics serialize")
}

#[tokio::test]
async fn every_replay_fixture_matches_golden_snapshots_metrics_and_baseline_thresholds() {
    let update = updating();
    let scripts = scripts();
    assert!(
        scripts
            .iter()
            .filter(|(name, _)| name.starts_with(SCRIPTED_PREFIX))
            .count()
            >= 4,
        "every scripted fixture is discovered"
    );
    assert!(
        scripts.iter().any(|(name, _)| name == NATIVE_FIXTURE),
        "{NATIVE_FIXTURE}.json is replayed; record it once with the ignored native capture test"
    );
    let mut current = BTreeMap::new();
    for (name, script) in &scripts {
        let outcome = ReplayTake::run(script)
            .await
            .unwrap_or_else(|error| panic!("{name} replays: {error}"));
        let golden = fixture_dir().join(format!("{name}.snapshots.json"));
        if update {
            write_json(&golden, &outcome);
        } else {
            let expected: ReplayOutcome = read_json(&golden);
            assert_eq!(
                outcome, expected,
                "{name} snapshots drifted; run with {UPDATE_VARIABLE}=1 to regenerate them"
            );
        }
        current.insert(
            name.clone(),
            metrics::compute(&outcome.snapshots, &outcome.completed, &script.finals),
        );
    }

    let metrics_path = fixture_dir().join("metrics.json");
    let mut recorded = sections(&metrics_path);
    if update {
        recorded.retain(|name, _| FOREIGN_SECTIONS.contains(&name.as_str()));
        for (name, metrics) in &current {
            recorded.insert(name.clone(), section(metrics));
        }
        write_json(&metrics_path, &recorded);
    } else {
        assert_eq!(
            replay_sections(&recorded),
            current,
            "metrics.json drifted; run with {UPDATE_VARIABLE}=1 to regenerate it"
        );
    }

    let baseline_path = fixture_dir().join("baseline.json");
    let mut baseline = sections(&baseline_path);
    let unrecorded = current
        .iter()
        .filter(|(name, _)| !baseline.contains_key(name.as_str()))
        .map(|(name, metrics)| (name.clone(), section(metrics)))
        .collect::<Vec<_>>();
    if update && !unrecorded.is_empty() {
        baseline.extend(unrecorded);
        write_json(&baseline_path, &baseline);
    }
    let violations = metrics::threshold_violations(&current, &replay_sections(&baseline));
    assert!(
        violations.is_empty(),
        "speech-sandbox thresholds are broken:\n{}",
        violations.join("\n")
    );
}

fn script(value: Value) -> ReplayScript {
    serde_json::from_value(value).expect("the inline replay script parses")
}

#[tokio::test]
async fn replay_rejects_a_script_that_does_not_end_with_its_commit_final() {
    let error = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 48_000]],
        "ticks": [
            {"at_ms": 4_000, "audio_start_ms": 0, "audio_end_ms": 3_000, "transcript": "late"}
        ],
        "finals": [
            {"at_ms": 3_500, "sample_start": 0, "sample_end": 52_800, "text": "early"}
        ]
    })))
    .await
    .expect_err("a tick after the commit final cannot replay");

    assert!(
        matches!(error, ReplayError::InvalidScript(_)),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn replay_rejects_finals_listed_out_of_at_ms_order() {
    let error = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 38_400], [81_600, 120_000]],
        "ticks": [
            {"at_ms": 1_150, "audio_start_ms": 0, "audio_end_ms": 1_000, "transcript": "We choose"}
        ],
        "finals": [
            {"at_ms": 8_200, "sample_start": 38_400, "sample_end": 124_800, "text": "and do"},
            {"at_ms": 4_800, "sample_start": 0, "sample_end": 38_400, "text": "We chose"}
        ]
    })))
    .await
    .expect_err("metrics read finals in file order, so they must match replay order");

    assert!(
        matches!(error, ReplayError::InvalidScript(_)),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn replay_accepts_a_forced_final_whose_range_awaits_the_next_final() {
    let outcome = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 176_000]],
        "ticks": [
            {"at_ms": 5_100, "audio_start_ms": 0, "audio_end_ms": 5_000, "transcript": "ask not"}
        ],
        "finals": [
            {"at_ms": 10_200, "sample_start": 0, "sample_end": 160_000, "text": "ask not what your country"},
            {"at_ms": 11_200, "sample_start": 32_000, "sample_end": 176_000, "text": "what your country can do"}
        ]
    })))
    .await
    .expect("ten seconds of continuous speech force a final the commit reconciles");

    assert!(
        outcome.completed.ends_with("can do"),
        "unexpected completed transcript: {}",
        outcome.completed
    );
}

fn sentence_then_silence(finals: &Value) -> ReplayScript {
    script(serde_json::json!({
        "speech_samples": [[0, 48_000], [128_160, 135_360]],
        "ticks": [
            {"at_ms": 2_650, "audio_start_ms": 0, "audio_end_ms": 2_500, "transcript": "ask not"},
            {"at_ms": 3_150, "audio_start_ms": 0, "audio_end_ms": 3_000, "transcript": "ask not what you can do."},
            {"at_ms": 3_550, "audio_start_ms": 0, "audio_end_ms": 3_500, "transcript": "ask not what you can do."}
        ],
        "finals": finals
    }))
}

#[tokio::test]
async fn replay_completes_a_sentence_once_when_a_silent_commit_follows_its_final() {
    let mut script = sentence_then_silence(&serde_json::json!([
        {"at_ms": 4_200, "sample_start": 0, "sample_end": 49_600, "text": "Ask not what you can do."},
        {"at_ms": 6_000, "sample_start": 49_600, "sample_end": 96_000, "text": ""}
    ]));
    script.speech_samples.truncate(1);
    let outcome = ReplayTake::run(&script)
        .await
        .expect("a final and a silent commit tail replay");

    assert_eq!(
        outcome.completed, "Ask not what you can do.",
        "the last hypothesis reaches into the silent tail, but the final already holds its words"
    );
}

#[tokio::test]
async fn replay_completes_a_short_word_once_when_a_silent_commit_follows_its_final() {
    let mut script = sentence_then_silence(&serde_json::json!([
        {"at_ms": 4_200, "sample_start": 0, "sample_end": 49_600, "text": "Ask not what you can do."},
        {"at_ms": 9_600, "sample_start": 120_160, "sample_end": 136_960, "text": "Hey."},
        {"at_ms": 10_500, "sample_start": 136_960, "sample_end": 168_000, "text": ""}
    ]));
    script.ticks.extend(
        [(8_350, 8_300), (8_850, 8_800)].map(|(at_ms, audio_end_ms)| ReplayTick {
            at_ms,
            audio_start_ms: 3_100,
            audio_end_ms,
            transcript: "Hey.".to_owned(),
        }),
    );
    let outcome = ReplayTake::run(&script)
        .await
        .expect("a 0.45 s word that the final pass decodes replays");

    assert_eq!(
        outcome.completed, "Ask not what you can do. Hey.",
        "the accepted word reaches into the silent tail, but the final already holds it"
    );
}

#[tokio::test]
async fn replay_finalizes_a_short_word_with_the_final_pass_punctuation() {
    let script: ReplayScript = read_json(&fixture_dir().join("scripted-short-word-final.json"));
    let outcome = ReplayTake::run(&script)
        .await
        .expect("a short word between two sentences replays");

    assert_eq!(
        outcome.completed,
        "Okay, listen up. Hey. The commit rule needs two passes."
    );
    assert!(
        outcome
            .snapshots
            .iter()
            .any(|snapshot| snapshot.finalized.ends_with("up. Hey.")),
        "the word's final lands with its period: {:#?}",
        outcome.snapshots
    );
    assert!(
        outcome
            .snapshots
            .iter()
            .all(|snapshot| snapshot.transcript.matches("Hey").count() <= 1),
        "the word never shows twice: {:#?}",
        outcome.snapshots
    );
}

#[tokio::test]
async fn replay_keeps_a_click_with_accepted_text_when_decoded_speech_follows() {
    let script: ReplayScript =
        read_json(&fixture_dir().join("scripted-skipped-word-then-speech.json"));
    let outcome = ReplayTake::run(&script)
        .await
        .expect("a click skipped between two sentences replays");

    assert_eq!(
        outcome.completed,
        "Okay, listen up. Hey. The commit rule needs two passes."
    );
    assert!(
        outcome
            .snapshots
            .iter()
            .skip_while(|snapshot| !snapshot.transcript.contains("Hey."))
            .all(|snapshot| snapshot.transcript.contains("Hey.")),
        "once shown, the word stays shown: {:#?}",
        outcome.snapshots
    );
}

#[tokio::test]
async fn replay_reports_a_tick_window_that_disagrees_with_the_take() {
    let error = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 48_000]],
        "ticks": [
            {"at_ms": 1_150, "audio_start_ms": 500, "audio_end_ms": 1_000, "transcript": "ask"}
        ],
        "finals": [
            {"at_ms": 3_700, "sample_start": 0, "sample_end": 52_800, "text": "ask not"}
        ]
    })))
    .await
    .expect_err("the take's window starts at the segment, not at 500 ms");

    assert!(
        matches!(error, ReplayError::Diverged { at_ms: 1_150, .. }),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn replay_reports_a_final_range_the_take_does_not_finalize() {
    let error = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 48_000]],
        "ticks": [
            {"at_ms": 1_150, "audio_start_ms": 0, "audio_end_ms": 1_000, "transcript": "ask"}
        ],
        "finals": [
            {"at_ms": 3_700, "sample_start": 16_000, "sample_end": 52_800, "text": "ask not"}
        ]
    })))
    .await
    .expect_err("the commit decodes the take from sample zero");

    assert!(
        matches!(error, ReplayError::Diverged { at_ms: 3_700, .. }),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn replay_reports_a_natural_final_ending_where_the_take_does_not_close() {
    let error = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 38_400], [81_600, 120_000]],
        "ticks": [
            {"at_ms": 1_150, "audio_start_ms": 0, "audio_end_ms": 1_000, "transcript": "We choose"}
        ],
        "finals": [
            {"at_ms": 4_800, "sample_start": 1_600, "sample_end": 41_600, "text": "We chose"},
            {"at_ms": 8_200, "sample_start": 40_000, "sample_end": 124_800, "text": "and do"}
        ]
    })))
    .await
    .expect_err("the take decodes 40,000 samples but finalizes through 40,000, not 41,600");

    assert!(
        matches!(error, ReplayError::Diverged { at_ms: 4_800, .. }),
        "unexpected error: {error}"
    );
}
