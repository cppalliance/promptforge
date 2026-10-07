//! Native capture of `jfk.wav` through a production Realtime session into the
//! `jfk-native` replay fixture.
//!
//! The capture runs one decode at a time on a clock measured in audio
//! milliseconds: appending audio advances it, and so does each decode's wall
//! time. A tick is skipped while an earlier decode is still running at its
//! audio position, as the session skips a tick while its interim task is in
//! flight; a final or the commit is issued at its audio position or when the
//! running decode finishes, whichever is later. Every event's `at_ms` is its
//! issue time plus its measured wall time, so `at_ms` rises in the order the
//! replay applies events.

use std::ops::Range;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{
    RealtimeSessionFixture, RealtimeSessionRegistryFixture, ReplayOutcome, ReplayScript,
    ReplaySnapshot, ReplayTake, load_scripted_initial_with_cancellation,
};
use gateway_stt_backend_whisper::{WhisperConfig, WhisperModelFactory};
use gateway_stt_engine::{
    DecodeMode, DecodeRequest, Decoder, EnginePolicy, ModelFactory, TranscribeError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{NATIVE_FIXTURE, fixture_dir, write_json};
use crate::common;

/// The window of the fixed policy the fixture load publishes, which is the
/// gateway default.
const WINDOW_SECONDS: u64 = 15;
const SAMPLES_PER_MS: u64 = 16;
/// 100 ms of 16 kHz audio, the chunk the client streams.
const CHUNK_SAMPLES: u64 = 1_600;
/// An interim tick follows every 500 ms of appended audio.
const TICK_SAMPLES: u64 = 8_000;
const FRAME_SAMPLES: usize = EnginePolicy::SAMPLE_RATE * 30 / 1_000;
const FRAME: u64 = FRAME_SAMPLES as u64;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(60);
const SETTLE_POLL: Duration = Duration::from_millis(1);
const HYPOTHESIS_UPDATE: &str = r#"{"type":"session.update","session":{"type":"transcription","include":["item.input_audio_transcription.hypothesis"]}}"#;

#[derive(Debug)]
struct Decode {
    mode: DecodeMode,
    samples: u64,
    text: String,
    wall: Duration,
}

type Decodes = Arc<Mutex<Vec<Decode>>>;

/// Wraps the Whisper factory so every decode records its raw output.
#[derive(Debug)]
struct RecordingFactory {
    inner: WhisperModelFactory,
    decodes: Decodes,
}

impl ModelFactory for RecordingFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        let decodes = Arc::clone(&self.decodes);
        Ok(self
            .inner
            .create(mode)?
            .map(|inner| Box::new(RecordingDecoder { inner, decodes }) as Box<dyn Decoder>))
    }
}

struct RecordingDecoder {
    inner: Box<dyn Decoder>,
    decodes: Decodes,
}

impl Decoder for RecordingDecoder {
    fn decode(&mut self, request: DecodeRequest) -> Result<String, TranscribeError> {
        let mode = request.mode();
        let samples = u64::try_from(request.samples().len()).unwrap_or(u64::MAX);
        let started = Instant::now();
        let text = self.inner.decode(request)?;
        let wall = started.elapsed();
        self.decodes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Decode {
                mode,
                samples,
                text: text.clone(),
                wall,
            });
        Ok(text)
    }
}

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
    script: Script,
    snapshots: Vec<ReplaySnapshot>,
}

impl Capture {
    fn new(session: RealtimeSessionFixture, decodes: Decodes, pcm: Vec<i16>) -> Self {
        let speech_samples = speech_runs(&pcm);
        Self {
            session,
            decodes,
            pcm,
            appended: 0,
            clock_ms: 0,
            script: Script {
                window_seconds: WINDOW_SECONDS,
                speech_samples,
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
            let payload = pcm24_payload(&self.pcm, self.appended..end);
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

    fn take_decodes(&self) -> Vec<Decode> {
        std::mem::take(&mut *self.decodes.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn take_one(&self, mode: DecodeMode, what: &str) -> Decode {
        let mut decodes = self.take_decodes();
        assert!(
            decodes.len() == 1 && decodes[0].mode == mode,
            "{what} runs exactly one {mode:?} decode, found {decodes:?}"
        );
        decodes.remove(0)
    }

    fn stamp(&mut self, issued_ms: u64, decode: &Decode) -> u64 {
        let wall_ms = u64::try_from(decode.wall.as_micros().div_ceil(1_000))
            .unwrap_or(u64::MAX)
            .max(1);
        self.clock_ms = issued_ms + wall_ms;
        self.clock_ms
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
        let mut decodes = self.take_decodes();
        if decodes.is_empty() {
            assert!(event.is_none(), "an undecoded window emits nothing");
            return;
        }
        assert!(
            decodes.len() == 1 && decodes[0].mode == DecodeMode::Interim,
            "a tick runs at most one interim decode, found {decodes:?}"
        );
        let decode = decodes.remove(0);
        let at_ms = self.stamp(issued_ms, &decode);
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
        let decode = self.take_one(DecodeMode::Final, "a closed segment");
        let at_ms = self.stamp(issued_ms, &decode);
        let (finalized, unresolved) = self
            .session
            .take_metrics()
            .expect("the take is uncommitted")
            .coverage();
        let sample_end = unresolved.map_or(finalized, |range| range.end);
        self.script.finals.push(Final {
            at_ms,
            sample_start: sample_end - decode.samples,
            sample_end,
            text: decode.text,
        });
    }

    async fn commit(mut self) -> (Script, ReplayOutcome) {
        let issued_ms = self.position_ms().max(self.clock_ms);
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
        let decode = self.take_one(DecodeMode::Final, "the commit");
        let at_ms = self.stamp(issued_ms, &decode);
        self.script.finals.push(Final {
            at_ms,
            sample_start: self.appended - decode.samples,
            sample_end: self.appended,
            text: decode.text,
        });
        let outcome = ReplayOutcome {
            snapshots: self.snapshots,
            completed,
        };
        (self.script, outcome)
    }
}

/// Lists the frame-aligned runs the segmenter reads as speech, so audio
/// synthesized from them segments like the clip.
fn speech_runs(pcm: &[i16]) -> Vec<[u64; 2]> {
    let samples = pcm
        .iter()
        .map(|sample| f32::from(*sample) / 32_768.0)
        .collect::<Vec<_>>();
    let mut runs: Vec<[u64; 2]> = Vec::new();
    let mut start = 0;
    for frame in samples.as_chunks::<FRAME_SAMPLES>().0 {
        let end = start + FRAME;
        if !EnginePolicy::is_silence(frame.as_slice()) {
            match runs.last_mut() {
                Some(run) if run[1] == start => run[1] = end,
                _ => runs.push([start, end]),
            }
        }
        start = end;
    }
    runs
}

/// Encodes the 24 kHz PCM16 input that the production resampler turns back
/// into exactly `pcm[output]`: input `3k` carries sample `2k`, and inputs
/// `3k + 1` and `3k + 2`, whose midpoint the resampler emits, both carry
/// sample `2k + 1`.
fn pcm24_payload(pcm: &[i16], output: Range<u64>) -> String {
    let bytes = (input_samples(output.start)..input_samples(output.end))
        .flat_map(|input| {
            let position = input / 3 * 2 + u64::from(input % 3 != 0);
            let index = usize::try_from(position).expect("the clip index fits usize");
            pcm[index].to_le_bytes()
        })
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

const fn input_samples(output: u64) -> u64 {
    output / 2 * 3 + output % 2
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
    let model = common::require_model();
    let config = WhisperConfig::new(
        common::require_library(),
        model.clone(),
        Some(model),
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
    assert_eq!(
        replayed, native,
        "replaying the capture reproduces the native session's snapshots and transcript"
    );

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
