//! Bounded deterministic decoders for arbitrary-duration integration tests.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use gateway_stt_engine::{
    DecodeMode, DecodeRequest, Decoder, EnginePolicy, ModelFactory, TranscribeError,
};

use crate::realtime::UncommittedInput;
use crate::{SpeechError, SpeechService};

const FINAL_STRIDE_SECONDS: usize = 10;
const FINAL_OVERLAP_SECONDS: usize = 8;
const MARKER_START: u16 = 0x6a5a;
const MARKER_END: u16 = 0xa5a6;
const SPEECH_SAMPLE: i16 = 8_192;

/// A bounded snapshot of one production take's retained ownership.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RealtimeTakeMetricsFixture {
    input_samples: u64,
    retained_samples: usize,
    finalized_samples: u64,
    unresolved_final: Option<std::ops::Range<u64>>,
    pending_final_segments: usize,
    pending_final_outcomes: usize,
    retained_hypotheses: usize,
}

impl RealtimeTakeMetricsFixture {
    /// Returns exact lifetime 24 kHz input samples.
    #[must_use]
    pub const fn input_samples(&self) -> u64 {
        self.input_samples
    }

    /// Returns all allocated retained 16 kHz sample capacity.
    #[must_use]
    pub const fn retained_samples(&self) -> usize {
        self.retained_samples
    }

    /// Returns finalized coverage and the sole unresolved forced range.
    #[must_use]
    pub fn coverage(&self) -> (u64, Option<std::ops::Range<u64>>) {
        (self.finalized_samples, self.unresolved_final.clone())
    }

    /// Returns queued finals, retained outcomes, and accepted hypotheses.
    #[must_use]
    pub const fn work_counts(&self) -> (usize, usize, usize) {
        (
            self.pending_final_segments,
            self.pending_final_outcomes,
            self.retained_hypotheses,
        )
    }
}

pub(super) fn take_metrics(input: &UncommittedInput) -> RealtimeTakeMetricsFixture {
    let metrics = input.take().metrics();
    RealtimeTakeMetricsFixture {
        input_samples: input.input_samples(),
        retained_samples: metrics.retained_samples,
        finalized_samples: metrics.finalized_samples,
        unresolved_final: metrics.unresolved_final,
        pending_final_segments: metrics.pending_final_segments,
        pending_final_outcomes: metrics.pending_final_outcomes,
        retained_hypotheses: metrics.retained_hypotheses,
    }
}

#[derive(Debug, Default)]
struct SimulationState {
    final_decodes: usize,
    interim_decodes: usize,
    max_final_samples: usize,
    gap_free_coverage_samples: u64,
    final_shape_valid: bool,
}

/// Produces deterministic 24 kHz PCM16 whose resampled output encodes absolute time.
#[must_use]
pub fn hour_marker_input(start: u64, samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|offset| {
            let offset = u64::try_from(offset).map_or(u64::MAX, |value| value);
            let input = start.saturating_add(offset);
            let output = input / 3 * 2 + u64::from(input % 3 != 0);
            marker_sample(output)
        })
        .collect()
}

/// Constant-memory observations from one deterministic hour-equivalent engine.
#[derive(Clone, Debug)]
pub struct HourSimulationProbe {
    shared: Arc<(Mutex<SimulationState>, Condvar)>,
}

impl Default for HourSimulationProbe {
    fn default() -> Self {
        Self {
            shared: Arc::new((
                Mutex::new(SimulationState {
                    final_shape_valid: true,
                    ..SimulationState::default()
                }),
                Condvar::new(),
            )),
        }
    }
}

impl HourSimulationProbe {
    /// Creates an empty simulation probe.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Waits until the requested number of final decodes complete.
    #[must_use]
    pub fn wait_for_final_decodes(&self, count: usize, timeout: Duration) -> bool {
        let (state, changed) = &*self.shared;
        let state = state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, result) = changed
            .wait_timeout_while(state, timeout, |state| state.final_decodes < count)
            .unwrap_or_else(PoisonError::into_inner);
        !result.timed_out() && state.final_decodes >= count
    }

    /// Returns completed accurate decode count.
    #[must_use]
    pub fn final_decode_count(&self) -> usize {
        self.state().final_decodes
    }

    /// Returns completed interim decode count.
    #[must_use]
    pub fn interim_decode_count(&self) -> usize {
        self.state().interim_decodes
    }

    /// Returns the largest accurate decode allocation in samples.
    #[must_use]
    pub fn max_final_samples(&self) -> usize {
        self.state().max_final_samples
    }

    /// Returns exact newly covered samples, or zero after any window-shape gap.
    #[must_use]
    pub fn gap_free_coverage_samples(&self) -> u64 {
        let state = self.state();
        if state.final_shape_valid {
            state.gap_free_coverage_samples
        } else {
            0
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SimulationState> {
        self.shared.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug)]
pub(super) struct HourSimulationFactory {
    probe: HourSimulationProbe,
}

impl HourSimulationFactory {
    pub(super) fn new(probe: HourSimulationProbe) -> Self {
        Self { probe }
    }
}

impl ModelFactory for HourSimulationFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        Ok(Some(Box::new(HourSimulationDecoder {
            mode,
            probe: self.probe.clone(),
        })))
    }
}

#[derive(Debug)]
struct HourSimulationDecoder {
    mode: DecodeMode,
    probe: HourSimulationProbe,
}

impl Decoder for HourSimulationDecoder {
    fn decode(&mut self, request: DecodeRequest) -> Result<String, TranscribeError> {
        let (start, end) = marked_window(request.samples())?;
        match self.mode {
            DecodeMode::Interim => {
                let mut state = self.probe.state();
                state.interim_decodes += 1;
                let end_second = end / EnginePolicy::SAMPLE_RATE as u64;
                let index = end_second.saturating_sub(4) / FINAL_STRIDE_SECONDS as u64;
                Ok(format!("live region {index:04}"))
            }
            DecodeMode::Final => {
                let mut state = self.probe.state();
                let stride = (EnginePolicy::SAMPLE_RATE * FINAL_STRIDE_SECONDS) as u64;
                let overlap = (EnginePolicy::SAMPLE_RATE * FINAL_OVERLAP_SECONDS) as u64;
                let expected_start = if state.gap_free_coverage_samples == 0 {
                    0
                } else {
                    state.gap_free_coverage_samples.saturating_sub(overlap)
                };
                let expected_end = state.gap_free_coverage_samples.saturating_add(stride);
                if start != expected_start || end != expected_end {
                    state.final_shape_valid = false;
                    return Err(marker_error(
                        "final marker coverage is stale, gapped, or reordered",
                    ));
                }
                state.max_final_samples = state.max_final_samples.max(request.samples().len());
                state.gap_free_coverage_samples = end;
                state.final_decodes += 1;
                self.probe.shared.1.notify_all();
                drop(state);

                let start_second = usize::try_from(start / EnginePolicy::SAMPLE_RATE as u64)
                    .map_err(|_| marker_error("marker start does not fit fixture text"))?;
                let end_second = usize::try_from(end / EnginePolicy::SAMPLE_RATE as u64)
                    .map_err(|_| marker_error("marker end does not fit fixture text"))?;
                Ok(timeline_text(start_second, end_second))
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "each shifted marker word is explicitly masked to sixteen bits"
)]
fn marker_sample(output: u64) -> i16 {
    let second = output / EnginePolicy::SAMPLE_RATE as u64;
    let offset = output % EnginePolicy::SAMPLE_RATE as u64;
    let word = match offset {
        0 => MARKER_START,
        1 => (second & u64::from(u16::MAX)) as u16,
        2 => ((second >> 16) & u64::from(u16::MAX)) as u16,
        3 => ((second >> 32) & u64::from(u16::MAX)) as u16,
        4 => ((second >> 48) & u64::from(u16::MAX)) as u16,
        5 => MARKER_END,
        _ => return SPEECH_SAMPLE,
    };
    i16::from_le_bytes(word.to_le_bytes())
}

fn marked_window(samples: &[f32]) -> Result<(u64, u64), TranscribeError> {
    let Some((marker_offset, second)) = samples
        .windows(6)
        .enumerate()
        .find_map(|(offset, marker)| decode_marker(marker).map(|second| (offset, second)))
    else {
        return Err(marker_error("absolute PCM marker is missing"));
    };
    let marker_position = second
        .checked_mul(EnginePolicy::SAMPLE_RATE as u64)
        .ok_or_else(|| marker_error("absolute PCM marker overflowed"))?;
    let start = marker_position
        .checked_sub(
            u64::try_from(marker_offset).map_err(|_| marker_error("marker offset overflowed"))?,
        )
        .ok_or_else(|| marker_error("absolute PCM marker precedes the window"))?;
    for (offset, actual) in samples.iter().enumerate() {
        let position = start
            .checked_add(
                u64::try_from(offset).map_err(|_| marker_error("sample offset overflowed"))?,
            )
            .ok_or_else(|| marker_error("absolute sample position overflowed"))?;
        let expected = f32::from(marker_sample(position)) / 32_768.0;
        if actual.to_bits() != expected.to_bits() {
            return Err(marker_error("absolute PCM marker sequence is corrupt"));
        }
    }
    let end = start
        .checked_add(
            u64::try_from(samples.len()).map_err(|_| marker_error("window length overflowed"))?,
        )
        .ok_or_else(|| marker_error("absolute PCM window overflowed"))?;
    Ok((start, end))
}

fn decode_marker(samples: &[f32]) -> Option<u64> {
    if sample_word(samples[0])? != MARKER_START || sample_word(samples[5])? != MARKER_END {
        return None;
    }
    let mut second = 0_u64;
    for (index, sample) in samples[1..5].iter().enumerate() {
        second |= u64::from(sample_word(*sample)?) << [0_u32, 16, 32, 48][index];
    }
    Some(second)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "validated normalized PCM16 fixture samples are converted back to their exact words"
)]
fn sample_word(sample: f32) -> Option<u16> {
    let scaled = sample * 32_768.0;
    if !scaled.is_finite() || scaled.fract() != 0.0 {
        return None;
    }
    let signed = i16::try_from(scaled as i32).ok()?;
    Some(u16::from_le_bytes(signed.to_le_bytes()))
}

fn marker_error(_message: &'static str) -> TranscribeError {
    match EnginePolicy::new(0, 1, false) {
        Err(error) => error,
        Ok(_) => unreachable!("a zero-second fixture policy is invalid"),
    }
}

fn timeline_text(start_second: usize, end_second: usize) -> String {
    (start_second..end_second)
        .map(|second| format!("word{second:04}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Creates a speech service whose decoders validate absolute PCM markers.
///
/// # Errors
/// Returns the deterministic policy or worker startup failure.
pub fn hour_simulation_service(probe: HourSimulationProbe) -> Result<SpeechService, SpeechError> {
    let service = SpeechService::new();
    let policy = EnginePolicy::new(15, 500, false).map_err(SpeechError::Engine)?;
    let replacement = service
        .state
        .stage_scripted_with_policy(HourSimulationFactory::new(probe), policy)?;
    service.commit_replacement(replacement)?;
    Ok(service)
}

#[cfg(test)]
mod tests {
    use gateway_stt_engine::{DecodeMode, DecodeRequest, Decoder, EnginePolicy};

    use super::{HourSimulationDecoder, HourSimulationProbe, hour_marker_input, marker_sample};

    fn output(start: u64, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|offset| {
                let position = start + u64::try_from(offset).expect("fixture offset fits");
                f32::from(marker_sample(position)) / 32_768.0
            })
            .collect()
    }

    fn decoder() -> HourSimulationDecoder {
        HourSimulationDecoder {
            mode: DecodeMode::Final,
            probe: HourSimulationProbe::new(),
        }
    }

    fn request(samples: Vec<f32>) -> DecodeRequest {
        DecodeRequest::new(DecodeMode::Final, samples, Vec::new(), String::new())
    }

    #[test]
    fn input_markers_survive_the_production_resampler_mapping() {
        let input = hour_marker_input(0, 24_000);
        let actual = input
            .chunks_exact(3)
            .flat_map(|group| {
                [
                    f32::from(group[0]) / 32_768.0,
                    f32::midpoint(
                        f32::from(group[1]) / 32_768.0,
                        f32::from(group[2]) / 32_768.0,
                    ),
                ]
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, output(0, EnginePolicy::SAMPLE_RATE));
    }

    #[test]
    fn hour_decoder_rejects_wrong_stale_duplicated_reordered_and_compacted_pcm() {
        let stride = EnginePolicy::SAMPLE_RATE * 10;
        let mut stale = decoder();
        let valid = output(0, stride);
        assert!(stale.decode(request(valid.clone())).is_ok());
        assert!(stale.decode(request(valid)).is_err(), "stale PCM fails");

        let mut wrong = output(0, stride);
        wrong[123] = 0.0;
        assert!(decoder().decode(request(wrong)).is_err(), "wrong PCM fails");

        let mut duplicated = output(0, stride);
        duplicated.insert(321, duplicated[321]);
        assert!(
            decoder().decode(request(duplicated)).is_err(),
            "duplicated PCM fails"
        );

        let mut reordered = output(0, stride);
        reordered.swap(1, 5);
        assert!(
            decoder().decode(request(reordered)).is_err(),
            "reordered PCM fails"
        );

        let mut compacted = output(0, stride);
        compacted.remove(777);
        assert!(
            decoder().decode(request(compacted)).is_err(),
            "incorrectly compacted PCM fails"
        );
    }
}
