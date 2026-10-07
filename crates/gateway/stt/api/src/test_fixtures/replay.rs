//! Deterministic replay of one scripted take through a hypothesis Realtime session.

#[path = "replay-script.rs"]
mod script;

use std::ops::Range;
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;

use super::{
    FixtureError, RealtimeSessionFixture, ScriptedDecoder, ScriptedModelFactory, boxed,
    scripted_service,
};
use crate::realtime::{ItemResult, Session, SessionRegistry};
use crate::segment::FRAME_SAMPLES;
pub use script::{
    ReplayError, ReplayFinal, ReplayOutcome, ReplayScript, ReplaySnapshot, ReplayTick,
};
use script::{SAMPLES_PER_MS, Step, millis_to_samples, speech_ranges, timeline};

const FRAME: u64 = FRAME_SAMPLES as u64;
const SPEECH_SAMPLE: i16 = 16_384;
const INTERVAL_MS: u64 = 500;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
const SETTLE_POLL: Duration = Duration::from_millis(1);
const HYPOTHESIS_UPDATE: &str = r#"{"type":"session.update","session":{"type":"transcription","include":["item.input_audio_transcription.hypothesis"]}}"#;

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

/// Drives one scripted take through a production Realtime session.
///
/// Audio is synthesized from the speech layout and appended only as far as
/// each event needs, so the take's segmenter closes a natural final while its
/// event runs. Each tick calls the session's interim scheduling directly and
/// stamps its snapshot with the tick's `at_ms`.
#[derive(Debug)]
pub struct ReplayTake {
    session: RealtimeSessionFixture,
    interim: ScriptedDecoder,
    final_decoder: ScriptedDecoder,
    speech: Vec<Range<u64>>,
    appended: u64,
    interim_decodes: usize,
    final_decodes: usize,
    snapshots: Vec<ReplaySnapshot>,
}

impl ReplayTake {
    /// Replays `script` and returns its emitted snapshots and completed transcript.
    ///
    /// # Errors
    /// Returns [`ReplayError::InvalidScript`] for a script that cannot describe
    /// one take, [`ReplayError::Diverged`] when the production take disagrees
    /// with the script, and [`ReplayError::Fixture`] when a session operation
    /// fails.
    pub async fn run(script: &ReplayScript) -> Result<ReplayOutcome, ReplayError> {
        let (steps, commit) = timeline(script)?;
        let speech = speech_ranges(script, commit.sample_end)?;
        let interim = ScriptedDecoder::new();
        let final_decoder = ScriptedDecoder::new();
        let factory = ScriptedModelFactory::new(interim.clone()).with_final(final_decoder.clone());
        let service = scripted_service(factory, script.window_seconds, INTERVAL_MS)
            .map_err(FixtureError::ScriptedService)?;
        let engine = service
            .state
            .active()
            .ok_or(FixtureError::ScriptedGenerationNotPublished)?;
        let registration = SessionRegistry::default()
            .register()
            .map_err(|error| FixtureError::Register(boxed(error)))?;
        let mut session = RealtimeSessionFixture {
            session: Session::new(registration, Some(engine)),
        };
        session.update_text(HYPOTHESIS_UPDATE)?;
        let mut take = Self {
            session,
            interim,
            final_decoder,
            speech,
            appended: 0,
            interim_decodes: 0,
            final_decodes: 0,
            snapshots: Vec::new(),
        };
        for step in steps {
            match step {
                Step::Tick(tick) => take.tick(tick).await?,
                Step::Final(natural) => take.natural_final(natural, commit.sample_end).await?,
            }
        }
        let completed = take.commit_final(commit).await?;
        Ok(ReplayOutcome {
            snapshots: take.snapshots,
            completed,
        })
    }

    async fn tick(&mut self, tick: &ReplayTick) -> Result<(), ReplayError> {
        let at_ms = tick.at_ms;
        let end = millis_to_samples(tick.audio_end_ms);
        self.append_to(at_ms, end)?;
        if self.segment_closed() {
            return Err(diverged(
                at_ms,
                format!(
                    "audio through sample {end} closed a final segment before its final ran; \
                     end this tick's audio before that segment's closing silence"
                ),
            ));
        }
        self.interim.push_text(tick.transcript.clone());
        let event = self.session.run_interim().await?;
        let requests = self.interim.requests();
        let Some([request]) = requests.get(self.interim_decodes..) else {
            return Err(diverged(
                at_ms,
                "the take did not decode the tick's window; it skips a window that is silent, \
                 shorter than 0.5 s, or identical to the previous one",
            ));
        };
        self.interim_decodes += 1;
        let decoded = sample_count(request.samples());
        if decoded != millis_to_samples(tick.audio_end_ms.saturating_sub(tick.audio_start_ms)) {
            return Err(diverged(
                at_ms,
                format!(
                    "the tick expects a window starting at {} ms, but the take decoded one \
                     starting at {} ms",
                    tick.audio_start_ms,
                    end.saturating_sub(decoded) / SAMPLES_PER_MS
                ),
            ));
        }
        if let Some(event) = event {
            self.snapshots.push(snapshot(at_ms, event)?);
        }
        Ok(())
    }

    async fn natural_final(&mut self, step: &ReplayFinal, limit: u64) -> Result<(), ReplayError> {
        let at_ms = step.at_ms;
        self.final_decoder.push_text(step.text.clone());
        while !self.segment_closed() {
            if self.appended >= limit {
                return Err(diverged(
                    at_ms,
                    format!(
                        "the take closed no segment through the commit at sample {limit}; samples \
                         {}..{} need trailing silence long enough to close them",
                        step.sample_start, step.sample_end
                    ),
                ));
            }
            let next = (self.appended / FRAME + 1) * FRAME;
            self.append_to(at_ms, next.min(limit))?;
        }
        self.settle(at_ms).await?;
        self.verify_final(step)?;
        let covered = self.session.take_metrics().map_or(0, |metrics| {
            let (finalized, unresolved) = metrics.coverage();
            unresolved.map_or(finalized, |range| range.end)
        });
        if covered != step.sample_end {
            return Err(diverged(
                at_ms,
                format!(
                    "the final expects audio through sample {} finalized or awaiting the next \
                     final's overlap, but the take covers through sample {covered}",
                    step.sample_end
                ),
            ));
        }
        Ok(())
    }

    async fn commit_final(&mut self, step: &ReplayFinal) -> Result<String, ReplayError> {
        let at_ms = step.at_ms;
        self.append_to(at_ms, step.sample_end)?;
        if self.segment_closed() {
            return Err(diverged(
                at_ms,
                format!(
                    "audio before the commit at sample {} closed a segment that no final covers",
                    step.sample_end
                ),
            ));
        }
        self.final_decoder.push_text(step.text.clone());
        let receipt = self.session.commit()?;
        self.session.finish_finalization(receipt.item_id()).await?;
        let results = self.session.session.drain_results();
        let result = results.into_iter().find_map(|result| match result {
            ItemResult::Completed { transcript, .. } => Some(Ok(transcript)),
            ItemResult::Failed { failure, .. } => Some(Err(failure.diagnostic().to_owned())),
            _ => None,
        });
        match result {
            Some(Ok(completed)) => {
                self.verify_final(step)?;
                Ok(completed)
            }
            Some(Err(failure)) => Err(diverged(
                at_ms,
                format!("the committed take failed: {failure}"),
            )),
            None => Err(diverged(
                at_ms,
                "the commit produced no completed or failed result",
            )),
        }
    }

    fn append_to(&mut self, at_ms: u64, target: u64) -> Result<(), ReplayError> {
        if target < self.appended {
            return Err(diverged(
                at_ms,
                format!(
                    "the event needs audio through sample {target}, but audio through sample {} \
                     is already appended",
                    self.appended
                ),
            ));
        }
        if target > self.appended {
            let payload = pcm_payload(&self.speech, self.appended..target);
            self.session.append_base64(&payload)?;
            self.appended = target;
        }
        Ok(())
    }

    fn segment_closed(&self) -> bool {
        self.session
            .pending_final_segments()
            .is_some_and(|pending| pending > 0)
            || self
                .final_decoder
                .wait_for_requests(self.final_decodes + 1, Duration::ZERO)
    }

    async fn settle(&self, at_ms: u64) -> Result<(), ReplayError> {
        tokio::time::timeout(SETTLE_TIMEOUT, async {
            while self
                .session
                .pending_final_segments()
                .is_some_and(|pending| pending > 0)
            {
                tokio::time::sleep(SETTLE_POLL).await;
            }
        })
        .await
        .map_err(|_| {
            diverged(
                at_ms,
                format!("the final decode did not settle within {SETTLE_TIMEOUT:?}"),
            )
        })
    }

    fn verify_final(&mut self, step: &ReplayFinal) -> Result<(), ReplayError> {
        let requests = self.final_decoder.requests();
        let decodes = requests.get(self.final_decodes..).unwrap_or_default();
        let expected = step.sample_end.saturating_sub(step.sample_start);
        match decodes {
            [request] if sample_count(request.samples()) == expected => {
                self.final_decodes += 1;
                Ok(())
            }
            [request] => Err(diverged(
                step.at_ms,
                format!(
                    "the final expects the take to decode samples {}..{} ({expected} samples), \
                     but it decoded {} samples",
                    step.sample_start,
                    step.sample_end,
                    sample_count(request.samples())
                ),
            )),
            _ => Err(diverged(
                step.at_ms,
                format!(
                    "the final expects one decode of samples {}..{}, but the take ran {} final \
                     decodes",
                    step.sample_start,
                    step.sample_end,
                    decodes.len()
                ),
            )),
        }
    }
}

/// Encodes the 24 kHz PCM16 input that the production resampler turns into
/// exactly the 16 kHz samples in `output`.
fn pcm_payload(speech: &[Range<u64>], output: Range<u64>) -> String {
    let bytes = (input_samples(output.start)..input_samples(output.end))
        .flat_map(|input| {
            let position = input / 3 * 2 + u64::from(input % 3 != 0);
            let sample = if speech.iter().any(|run| run.contains(&position)) {
                SPEECH_SAMPLE
            } else {
                0
            };
            sample.to_le_bytes()
        })
        .collect::<Vec<_>>();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Input samples whose resampling emits the first `output` 16 kHz samples:
/// output `2k` comes from input `3k`, and output `2k + 1` from inputs `3k + 1`
/// and `3k + 2`.
const fn input_samples(output: u64) -> u64 {
    output / 2 * 3 + output % 2
}

fn snapshot(at_ms: u64, event: serde_json::Value) -> Result<ReplaySnapshot, ReplayError> {
    let fields: HypothesisFields =
        serde_json::from_value(event).map_err(FixtureError::Serialize)?;
    Ok(ReplaySnapshot {
        at_ms,
        revision: fields.revision,
        transcript: fields.transcript,
        finalized: fields.finalized,
        agreed: fields.agreed,
        tentative: fields.tentative,
        audio_start_ms: fields.audio_start_ms,
        audio_end_ms: fields.audio_end_ms,
    })
}

fn sample_count(samples: &[f32]) -> u64 {
    u64::try_from(samples.len()).unwrap_or(u64::MAX)
}

fn diverged(at_ms: u64, detail: impl Into<String>) -> ReplayError {
    ReplayError::Diverged {
        at_ms,
        detail: detail.into(),
    }
}
