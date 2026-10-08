//! Native capture of `jfk.wav`, or of the 16 kHz mono clip that
//! `PROMPTFORGE_WHISPER_AUDIO` names, through a production Realtime session
//! into a replay script.
//!
//! The capture runs one decode at a time on a clock measured in audio
//! milliseconds: appending audio advances it, and so does each decode's wall
//! time. A tick is skipped while an earlier decode is still running at its
//! audio position, as the session skips a tick while its interim task is in
//! flight; a final or the commit is issued at its audio position or when the
//! running decode finishes, whichever is later. Every event's `at_ms` is its
//! issue time plus its measured wall time, so `at_ms` rises in the order the
//! replay applies events. A segment or commit tail the take skips without a
//! final decode is recorded as a final with empty text, and each natural
//! final records the update the session sends for it.
//!
//! The final role uses the interim model unless
//! `PROMPTFORGE_WHISPER_FINAL_MODEL` names another one. The capture builds
//! its Whisper factory from `PROMPTFORGE_WHISPER_LIBRARY` rather than
//! loading a gateway config: a config load builds its factory internally,
//! leaving no place for the recording wrapper, and provisions the pinned
//! whisper build from the artifact store instead of the named library.
//!
//! With `PROMPTFORGE_REPLAY_CAPTURE` naming a scratch `<name>.json` outside
//! the fixture directory, the capture writes its script there and its
//! snapshots to `<name>.snapshots.json`, prints its metrics, and leaves the
//! fixture alone, so decode experiments compare captures without replacing it.
//! A clip other than `jfk.wav` is captured only to a scratch path.
//!
//! A native fixture is never hand-edited. To replace one, capture to a scratch
//! path, copy the script and snapshots unchanged under a new fixture name, and
//! point `NATIVE_FIXTURE` at it.

mod audio;
mod recording;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::native::fixture_final_model;
use gateway_stt::test_fixtures::{
    RealtimeSessionFixture, RealtimeSessionRegistryFixture, ReplayOutcome, ReplayScript,
    ReplaySnapshot, ReplayTake, load_scripted_initial_with_cancellation,
};
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::DecodeMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{NATIVE_FIXTURE, fixture_dir, metrics, write_json};
use crate::common;
use recording::{Decode, Decodes, RecordingFactory, take_decodes};

const CAPTURE_VARIABLE: &str = "PROMPTFORGE_REPLAY_CAPTURE";
const AUDIO_VARIABLE: &str = "PROMPTFORGE_WHISPER_AUDIO";
const FIXTURE_AUDIO: &str = "jfk.wav";

/// The window of the fixed policy the fixture load publishes, which is the
/// gateway default.
const WINDOW_SECONDS: u64 = 15;
const SAMPLES_PER_MS: u64 = 16;
/// 100 ms of 16 kHz audio, the chunk the client streams.
const CHUNK_SAMPLES: u64 = 1_600;
/// An interim tick follows every 500 ms of appended audio.
const TICK_SAMPLES: u64 = 8_000;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);
const SETTLE_POLL: Duration = Duration::from_millis(1);
const HYPOTHESIS_UPDATE: &str = r#"{"type":"session.update","session":{"type":"transcription","include":["item.input_audio_transcription.hypothesis"]}}"#;

#[derive(Debug, Serialize)]
struct Script {
    window_seconds: u64,
    speech_samples: Vec<[u64; 2]>,
    ticks: Vec<Tick>,
    finals: Vec<Final>,
}

#[derive(Debug, Serialize)]
struct Tick {
    at_ms: u64,
    audio_start_ms: u64,
    audio_end_ms: u64,
    transcript: String,
}

#[derive(Debug, Serialize)]
struct Final {
    at_ms: u64,
    sample_start: u64,
    sample_end: u64,
    text: String,
}

#[derive(Deserialize)]
struct HypothesisFields {
    revision: u64,
    transcript: String,
    finalized: String,
    agreed: String,
    tentative: String,
    audio_start_ms: u64,
    audio_end_ms: u64,
}

struct Capture {
    session: RealtimeSessionFixture,
    decodes: Decodes,
    pcm: Vec<i16>,
    appended: u64,
    clock_ms: u64,
    /// End of the audio finalized or awaiting a forced overlap.
    covered: u64,
    script: Script,
    snapshots: Vec<ReplaySnapshot>,
}

impl Capture {
    fn new(session: RealtimeSessionFixture, decodes: Decodes, pcm: Vec<i16>) -> Self {
        Self {
            session,
            decodes,
            pcm,
            appended: 0,
            clock_ms: 0,
            covered: 0,
            script: Script {
                window_seconds: WINDOW_SECONDS,
                speech_samples: Vec::new(),
                ticks: Vec::new(),
                finals: Vec::new(),
            },
            snapshots: Vec::new(),
        }
    }

    async fn stream(&mut self) {
        let total = u64::try_from(self.pcm.len()).expect("the clip length fits u64");
        while self.appended < total {
            let end = (self.appended + CHUNK_SAMPLES).min(total);
            let payload = audio::pcm24_payload(&self.pcm, self.appended..end);
            self.session
                .append_base64(&payload)
                .expect("a native chunk appends");
            self.appended = end;
            self.settle_natural_final().await;
            if end % TICK_SAMPLES == 0 {
                self.tick().await;
            }
        }
    }

    fn position_ms(&self) -> u64 {
        self.appended / SAMPLES_PER_MS
    }

    /// The one decode in `mode` that `what` ran, or `None` when the take
    /// skipped it without a decode.
    fn take_at_most_one(&self, mode: DecodeMode, what: &str) -> Option<Decode> {
        let mut decodes = take_decodes(&self.decodes);
        assert!(
            decodes.len() <= 1 && decodes.iter().all(|decode| decode.mode == mode),
            "{what} runs at most one {mode:?} decode, found {decodes:?}"
        );
        decodes.pop()
    }

    fn stamp(&mut self, issued_ms: u64, wall: Duration) -> u64 {
        let wall_ms = u64::try_from(wall.as_micros().div_ceil(1_000))
            .unwrap_or(u64::MAX)
            .max(1);
        self.clock_ms = issued_ms + wall_ms;
        self.clock_ms
    }

    /// Records the final `decode` ran over the audio ending at `sample_end`,
    /// or a range skipped without a decode since the last covered sample.
    fn record_final(&mut self, issued_ms: u64, decode: Option<Decode>, sample_end: u64) -> u64 {
        let (wall, sample_start, text) = decode
            .map_or((Duration::ZERO, self.covered, String::new()), |decode| {
                (decode.wall, sample_end - decode.samples, decode.text)
            });
        let at_ms = self.stamp(issued_ms, wall);
        self.covered = sample_end;
        self.script.finals.push(Final {
            at_ms,
            sample_start,
            sample_end,
            text,
        });
        at_ms
    }

    async fn tick(&mut self) {
        let issued_ms = self.position_ms();
        if issued_ms < self.clock_ms {
            return;
        }
        let event = self
            .session
            .run_interim()
            .await
            .expect("the native interim runs");
        let mut decodes = take_decodes(&self.decodes);
        if decodes.is_empty() {
            assert!(event.is_none(), "an undecoded window emits nothing");
            return;
        }
        assert!(
            decodes.len() == 1 && decodes[0].mode == DecodeMode::Interim,
            "a tick runs at most one interim decode, found {decodes:?}"
        );
        let decode = decodes.remove(0);
        let at_ms = self.stamp(issued_ms, decode.wall);
        let start = self.appended - decode.samples;
        assert_eq!(
            start % SAMPLES_PER_MS,
            0,
            "the interim window starts on a whole millisecond"
        );
        self.script.ticks.push(Tick {
            at_ms,
            audio_start_ms: start / SAMPLES_PER_MS,
            audio_end_ms: issued_ms,
            transcript: decode.text,
        });
        if let Some(event) = event {
            self.snapshots.push(snapshot(at_ms, event));
        }
    }

    async fn settle_natural_final(&mut self) {
        let session = &self.session;
        if session.pending_final_segments().unwrap_or(0) == 0 {
            return;
        }
        tokio::time::timeout(SETTLE_TIMEOUT, async {
            while session
                .pending_final_segments()
                .is_some_and(|pending| pending > 0)
            {
                tokio::time::sleep(SETTLE_POLL).await;
            }
        })
        .await
        .expect("the native final settles within its bound");
        let issued_ms = self.position_ms().max(self.clock_ms);
        let decode = self.take_at_most_one(DecodeMode::Final, "a closed segment");
        let (finalized, unresolved) = self
            .session
            .take_metrics()
            .expect("the take is uncommitted")
            .coverage();
        let sample_end = unresolved.map_or(finalized, |range| range.end);
        let at_ms = self.record_final(issued_ms, decode, sample_end);
        if let Some(event) = self
            .session
            .finalized_update()
            .expect("the update for the landed final is composed")
        {
            self.snapshots.push(snapshot(at_ms, event));
        }
    }

    async fn commit(mut self) -> (Script, ReplayOutcome) {
        let issued_ms = self.position_ms().max(self.clock_ms);
        // The stream leaves no sample in the resampler, so the take has
        // classified every whole frame before the commit hands it over.
        self.script.speech_samples = self
            .session
            .speech_runs()
            .expect("the take is uncommitted")
            .into_iter()
            .map(|run| [run.start, run.end])
            .collect();
        let receipt = self.session.commit().expect("the native take commits");
        self.session
            .finish_finalization(receipt.item_id())
            .await
            .expect("the native commit finalizes");
        let results = self.session.drain_results();
        let completed = results
            .iter()
            .find(|result| result["type"] == "completed")
            .and_then(|result| result["transcript"].as_str())
            .unwrap_or_else(|| panic!("the native commit completes: {results:?}"))
            .to_owned();
        let decode = self.take_at_most_one(DecodeMode::Final, "the commit");
        self.record_final(issued_ms, decode, self.appended);
        let outcome = ReplayOutcome {
            snapshots: self.snapshots,
            completed,
        };
        (self.script, outcome)
    }
}

/// Refuses a scratch capture whose directory resolves inside the fixture
/// directory, where it could replace a fixed input or plant a script the
/// replay test runs and an update adds to the baseline.
fn check_scratch(scratch: &Path) -> Result<(), String> {
    let parent = match scratch.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    let resolved = std::fs::canonicalize(parent).map_err(|error| {
        format!(
            "{CAPTURE_VARIABLE} requires an existing directory, but {} does not resolve: {error}",
            parent.display()
        )
    })?;
    let fixtures = std::fs::canonicalize(fixture_dir())
        .map_err(|error| format!("the replay fixture directory resolves: {error}"))?;
    if resolved.starts_with(&fixtures) {
        return Err(format!(
            "{CAPTURE_VARIABLE} requires a path outside {}, but names {}",
            fixtures.display(),
            scratch.display()
        ));
    }
    Ok(())
}

/// Refuses to record the fixture from a clip other than `jfk.wav`, whose
/// transcript must not enter the repository.
fn check_fixture_audio(audio: Option<&Path>) -> Result<(), String> {
    match audio {
        Some(audio) if audio.file_name().is_none_or(|name| name != FIXTURE_AUDIO) => Err(format!(
            "{AUDIO_VARIABLE} names {}, so {CAPTURE_VARIABLE} must name a scratch path",
            audio.display()
        )),
        _ => Ok(()),
    }
}

fn snapshot(at_ms: u64, event: Value) -> ReplaySnapshot {
    let fields: HypothesisFields =
        serde_json::from_value(event).expect("a hypothesis event carries its snapshot fields");
    ReplaySnapshot {
        at_ms,
        revision: fields.revision,
        transcript: fields.transcript,
        finalized: fields.finalized,
        agreed: fields.agreed,
        tentative: fields.tentative,
        audio_start_ms: fields.audio_start_ms,
        audio_end_ms: fields.audio_end_ms,
    }
}

#[tokio::test]
#[ignore = "requires packaged whisper, model, and audio fixtures"]
async fn native_jfk_capture_replays_exactly_and_records_the_native_fixture_once() {
    let scratch = std::env::var_os(CAPTURE_VARIABLE).map(PathBuf::from);
    if let Some(scratch) = &scratch {
        check_scratch(scratch).unwrap_or_else(|reason| panic!("{reason}"));
    } else {
        let audio = std::env::var_os(AUDIO_VARIABLE).map(PathBuf::from);
        check_fixture_audio(audio.as_deref()).unwrap_or_else(|reason| panic!("{reason}"));
    }
    let model = common::require_model();
    let config = WhisperConfig::new(
        common::require_library(),
        model.clone(),
        Some(fixture_final_model(&model)),
        WINDOW_SECONDS,
        None,
    );
    let decodes = Decodes::default();
    let factory = RecordingFactory {
        inner: WhisperModelFactory::new(config).expect("the packaged runtime loads"),
        decodes: Arc::clone(&decodes),
    };
    let service = SpeechService::new();
    load_scripted_initial_with_cancellation(&service, factory, &CancellationToken::new())
        .expect("the native interim and final models load");
    let mut session = RealtimeSessionRegistryFixture::default()
        .register_with_service(&service)
        .expect("a native session registers");
    session
        .update_text(HYPOTHESIS_UPDATE)
        .expect("the hypothesis include applies");

    let mut capture = Capture::new(session, decodes, common::jfk_pcm16());
    capture.stream().await;
    let (script, native) = capture.commit().await;

    let replay: ReplayScript =
        serde_json::from_value(serde_json::to_value(&script).expect("the capture serializes"))
            .expect("the capture is a replay script");
    let replayed = ReplayTake::run(&replay)
        .await
        .unwrap_or_else(|error| panic!("the native capture replays: {error}"));
    if let Some(scratch) = &scratch {
        write_json(scratch, &script);
        write_json(&scratch.with_extension("snapshots.json"), &native);
    }
    assert_eq!(
        replayed, native,
        "replaying the capture reproduces the native session's snapshots and transcript"
    );

    if let Some(scratch) = scratch {
        let metrics = metrics::compute(&replayed.snapshots, &replayed.completed, &replay.finals);
        eprintln!("{} metrics: {metrics:?}", scratch.display());
        return;
    }
    let path = fixture_dir().join(format!("{NATIVE_FIXTURE}.json"));
    if path.exists() {
        eprintln!(
            "{} is a fixed input and stays as committed; this capture was {script:?}",
            path.display()
        );
    } else {
        write_json(&path, &script);
    }
}

#[test]
fn scratch_capture_resolving_inside_the_fixture_directory_is_refused() {
    let fixtures = fixture_dir();
    for scratch in [
        fixtures.join(format!("{NATIVE_FIXTURE}.json")),
        fixtures.join("baseline.json"),
        fixtures.join("..").join("replay").join("seeded.json"),
    ] {
        assert!(
            check_scratch(&scratch).is_err(),
            "{} resolves inside the fixture directory and is refused",
            scratch.display()
        );
    }
}

#[test]
fn scratch_capture_outside_the_fixture_directory_is_accepted() {
    let scratch = std::env::temp_dir().join("seeded.json");
    assert_eq!(check_scratch(&scratch), Ok(()));
}

#[test]
fn only_the_fixture_clip_records_the_fixture() {
    assert_eq!(check_fixture_audio(None), Ok(()));
    assert_eq!(
        check_fixture_audio(Some(Path::new("fixtures/jfk.wav"))),
        Ok(())
    );
    assert!(check_fixture_audio(Some(Path::new("local/stt-fixtures/dictation-01.wav"))).is_err());
}
