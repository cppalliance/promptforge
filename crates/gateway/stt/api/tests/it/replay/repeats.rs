//! Replays of repeats the fast pass decodes from trailing silence, which no
//! snapshot shows, and of repeats the speaker says, which stay shown.

use gateway_stt::test_fixtures::{ReplayOutcome, ReplayScript, ReplayTake};

use super::{fixture_dir, read_json, script};

fn shown(outcome: &ReplayOutcome, text: &str) -> bool {
    outcome
        .snapshots
        .iter()
        .any(|snapshot| snapshot.transcript.contains(text))
}

fn shown_at(outcome: &ReplayOutcome, at_ms: u64) -> &str {
    outcome
        .snapshots
        .iter()
        .find(|snapshot| snapshot.at_ms == at_ms)
        .map_or("", |snapshot| snapshot.transcript.as_str())
}

#[tokio::test]
async fn no_snapshot_shows_a_repeat_decoded_from_trailing_silence() {
    for name in ["scripted-trailing-echo", "scripted-trailing-echo-same-pass"] {
        let script: ReplayScript = read_json(&fixture_dir().join(format!("{name}.json")));
        let outcome = ReplayTake::run(&script)
            .await
            .unwrap_or_else(|error| panic!("{name} replays: {error}"));

        assert!(
            !shown(&outcome, "plan. I want"),
            "{name} shows the echo: {:#?}",
            outcome.snapshots
        );
        assert!(
            shown(&outcome, "create a plan."),
            "{name} shows the sentence"
        );
    }
}

#[tokio::test]
async fn a_phrase_the_speaker_repeats_stays_shown_as_it_is_spoken() {
    let outcome = ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, 64_320]],
        "ticks": [
            {"at_ms": 1_150, "audio_start_ms": 0, "audio_end_ms": 1_000, "transcript": "I want you to"},
            {"at_ms": 1_650, "audio_start_ms": 0, "audio_end_ms": 1_500, "transcript": "I want you to create"},
            {"at_ms": 2_150, "audio_start_ms": 0, "audio_end_ms": 2_000, "transcript": "I want you to create a plan."},
            {"at_ms": 2_650, "audio_start_ms": 0, "audio_end_ms": 2_500, "transcript": "I want you to create a plan. I want"},
            {"at_ms": 3_150, "audio_start_ms": 0, "audio_end_ms": 3_000, "transcript": "I want you to create a plan. I want you to"},
            {"at_ms": 3_650, "audio_start_ms": 0, "audio_end_ms": 3_500, "transcript": "I want you to create a plan. I want you to create"}
        ],
        "finals": [
            {"at_ms": 6_200, "sample_start": 0, "sample_end": 65_920, "text": "I want you to create a plan. I want you to create."},
            {"at_ms": 7_000, "sample_start": 65_920, "sample_end": 112_000, "text": ""}
        ]
    })))
    .await
    .expect("a phrase said twice replays");

    assert!(shown_at(&outcome, 2_650).ends_with("plan. I want"));
    assert!(shown_at(&outcome, 3_650).ends_with("plan. I want you to create"));
}

async fn repeated_word(speech_end: u64, final_text: &str) -> ReplayOutcome {
    let final_end = speech_end + 1_600;
    ReplayTake::run(&script(serde_json::json!({
        "speech_samples": [[0, speech_end]],
        "ticks": [
            {"at_ms": 1_150, "audio_start_ms": 0, "audio_end_ms": 1_000, "transcript": "create a"},
            {"at_ms": 1_650, "audio_start_ms": 0, "audio_end_ms": 1_500, "transcript": "create a plan."},
            {"at_ms": 2_050, "audio_start_ms": 0, "audio_end_ms": 2_000, "transcript": "create a plan. Plan."}
        ],
        "finals": [
            {"at_ms": 3_200, "sample_start": 0, "sample_end": final_end, "text": final_text},
            {"at_ms": 4_000, "sample_start": final_end, "sample_end": 64_000, "text": ""}
        ]
    })))
    .await
    .expect("a repeated word replays")
}

#[tokio::test]
async fn a_repeated_word_is_trimmed_after_silence_and_kept_when_spoken() {
    let silent = repeated_word(24_000, "Create a plan.").await;
    assert!(
        !shown(&silent, "plan. Plan"),
        "speech ends with the first word: {:#?}",
        silent.snapshots
    );

    let spoken = repeated_word(32_160, "Create a plan. Plan.").await;
    assert!(shown_at(&spoken, 2_050).ends_with("plan. Plan."));
}
