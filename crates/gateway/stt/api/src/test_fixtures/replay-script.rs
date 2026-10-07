//! Replay script and outcome data, and the ordered event timeline of a script.

use std::ops::Range;

use gateway_stt_engine::test_fixtures::ScriptedDetector;
use gateway_stt_engine::{EnginePolicy, FallbackDetector};
use serde::{Deserialize, Serialize};

use crate::test_fixtures::FixtureError;

pub(super) const SAMPLES_PER_MS: u64 = (EnginePolicy::SAMPLE_RATE / 1_000) as u64;
const DEFAULT_WINDOW_SECONDS: u64 = 15;

/// One scripted take: its audio layout, interim decoder outputs, and finals.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayScript {
    /// Interim window length in seconds; the gateway default when omitted.
    #[serde(default = "default_window_seconds")]
    pub window_seconds: u64,
    /// Half-open 16 kHz sample ranges that carry speech; all other audio is silent.
    pub speech_samples: Vec<[u64; 2]>,
    /// Interim decoder outputs, one per interim tick.
    pub ticks: Vec<ReplayTick>,
    /// Final decoder outputs in take order; the latest event is the final the commit decodes.
    pub finals: Vec<ReplayFinal>,
}

/// One interim tick: the interim decoder's raw output for one window.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayTick {
    /// Replay clock time at which the decode result is accepted.
    pub at_ms: u64,
    /// Start of the window the take must choose.
    pub audio_start_ms: u64,
    /// Audio appended before the tick, which is where its window ends. A
    /// window ends no later than the speech tail after the last speech frame.
    pub audio_end_ms: u64,
    /// The interim decoder's raw output for the window.
    pub transcript: String,
}

/// One final decode over a sample range the take finalizes.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayFinal {
    /// Replay clock time at which the final is applied.
    pub at_ms: u64,
    /// First 16 kHz sample the take decodes for this final.
    pub sample_start: u64,
    /// End of the decoded 16 kHz sample range, exclusive.
    pub sample_end: u64,
    /// The final decoder's output, or empty for a range the take skips
    /// without a final decode: silence, a click, or a commit tail shorter
    /// than the final window.
    pub text: String,
}

/// One emitted hypothesis event reduced to its deterministic fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySnapshot {
    /// The `at_ms` of the tick whose interim decode emitted the event, or of
    /// the natural final whose outcome emitted it.
    pub at_ms: u64,
    /// The event's hypothesis revision.
    pub revision: u64,
    /// The whole displayed transcript.
    pub transcript: String,
    /// The finalized prefix of the transcript.
    pub finalized: String,
    /// The agreed text after the finalized prefix.
    pub agreed: String,
    /// The tentative tail of the transcript.
    pub tentative: String,
    /// Start of the decoded window in milliseconds.
    pub audio_start_ms: u64,
    /// End of the decoded window in milliseconds.
    pub audio_end_ms: u64,
}

/// The snapshots a replayed take emitted and its completed transcript.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayOutcome {
    /// Every hypothesis event the session emitted, in order.
    pub snapshots: Vec<ReplaySnapshot>,
    /// The transcript of the take's `completed` event.
    pub completed: String,
}

/// One replay failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// The script cannot describe one take.
    #[error("replay script is invalid: {0}")]
    InvalidScript(String),
    /// The production take disagreed with the script.
    #[error("replay diverged from its script at {at_ms} ms: {detail}")]
    Diverged {
        /// The `at_ms` of the event that diverged.
        at_ms: u64,
        /// What the script required and what the take did.
        detail: String,
    },
    /// A session operation failed.
    #[error(transparent)]
    Fixture(#[from] FixtureError),
}

pub(super) enum Step<'a> {
    Tick(&'a ReplayTick),
    Final(&'a ReplayFinal),
}

impl Step<'_> {
    const fn at_ms(&self) -> u64 {
        match self {
            Self::Tick(tick) => tick.at_ms,
            Self::Final(step) => step.at_ms,
        }
    }
}

/// Orders every event by `at_ms` and splits off the final the commit decodes.
pub(super) fn timeline(
    script: &ReplayScript,
) -> Result<(Vec<Step<'_>>, &ReplayFinal), ReplayError> {
    if let Some(pair) = script
        .finals
        .windows(2)
        .find(|pair| pair[0].at_ms > pair[1].at_ms)
    {
        return Err(invalid(format!(
            "finals must be listed in ascending at_ms order, since metrics interpolate audio \
             ends over them in file order; found the final at {} ms before the one at {} ms",
            pair[0].at_ms, pair[1].at_ms
        )));
    }
    let mut steps = script
        .ticks
        .iter()
        .map(Step::Tick)
        .chain(script.finals.iter().map(Step::Final))
        .collect::<Vec<_>>();
    steps.sort_by_key(Step::at_ms);
    if let Some(pair) = steps
        .windows(2)
        .find(|pair| pair[0].at_ms() == pair[1].at_ms())
    {
        return Err(invalid(format!(
            "two events share at_ms {}; every tick and final needs its own time",
            pair[0].at_ms()
        )));
    }
    let commit = match steps.pop() {
        Some(Step::Final(commit)) => commit,
        Some(Step::Tick(tick)) => {
            return Err(invalid(format!(
                "the latest event is the tick at {} ms; a take ends with the final its commit \
                 decodes",
                tick.at_ms
            )));
        }
        None => {
            return Err(invalid(
                "the script has no events; a take ends with the final its commit decodes",
            ));
        }
    };
    for step in &steps {
        if let Step::Tick(tick) = step
            && (tick.audio_start_ms > tick.audio_end_ms
                || millis_to_samples(tick.audio_end_ms) > commit.sample_end)
        {
            return Err(invalid(format!(
                "the tick at {} ms needs audio_start_ms <= audio_end_ms <= the commit at sample \
                 {}, found {}..{} ms",
                tick.at_ms, commit.sample_end, tick.audio_start_ms, tick.audio_end_ms
            )));
        }
    }
    Ok((steps, commit))
}

pub(super) fn speech_ranges(
    script: &ReplayScript,
    commit_end: u64,
) -> Result<Vec<Range<u64>>, ReplayError> {
    script
        .speech_samples
        .iter()
        .map(|&[start, end]| {
            if start < end && end <= commit_end {
                Ok(start..end)
            } else {
                Err(invalid(format!(
                    "speech run {start}..{end} must be nonempty and end by the commit at sample \
                     {commit_end}"
                )))
            }
        })
        .collect()
}

/// A detector that hears speech exactly in `speech`, so the take segments
/// the script's layout whatever the loudness of the synthesized audio.
pub(super) fn speech_detector(speech: &[Range<u64>]) -> FallbackDetector {
    let index = |sample: u64| usize::try_from(sample).unwrap_or(usize::MAX);
    let runs = speech.iter().map(|run| (index(run.start), index(run.end)));
    FallbackDetector::new(Box::new(ScriptedDetector::new(runs)))
}

pub(super) const fn millis_to_samples(millis: u64) -> u64 {
    millis.saturating_mul(SAMPLES_PER_MS)
}

const fn default_window_seconds() -> u64 {
    DEFAULT_WINDOW_SECONDS
}

fn invalid(detail: impl Into<String>) -> ReplayError {
    ReplayError::InvalidScript(detail.into())
}
