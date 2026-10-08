//! Bounded deterministic decoders for arbitrary-duration integration tests.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use gateway_stt_engine::{
    DecodeMode, DecodeOutput, DecodeRequest, Decoder, EnginePolicy, ModelFactory, TranscribeError,
};

use crate::audio::{resampled_inputs, resampled_sample};
use crate::realtime::UncommittedInput;
use crate::{SpeechError, SpeechService};

/// One forced stride of 16 kHz output: 313 frames of 512 samples.
const FINAL_STRIDE_SAMPLES: u64 = 160_256;
const FINAL_OVERLAP_SAMPLES: u64 = (EnginePolicy::SAMPLE_RATE * 8) as u64;
/// The simulated speaker says this many words per forced stride.
const WORDS_PER_STRIDE: u64 = 10;
/// 16 kHz output samples per second of fixture audio, each second one level.
const SECOND: u64 = EnginePolicy::SAMPLE_RATE as u64;
/// Second `s` holds the PCM16 level `LEVEL_BASE + s`, positive in even
/// seconds and negative in odd ones, so every second opens with a step of
/// half full scale that the resampler's low-pass keeps sharp to the sample.
const LEVEL_BASE: i16 = 8_192;

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
            let offset = u64::try_from(offset).unwrap_or(u64::MAX);
            marker_sample(input_position(start.saturating_add(offset)))
        })
        .collect()
}

/// The 16 kHz position whose level input `input` holds: input `3k` holds
/// position `2k`, and inputs `3k + 1` and `3k + 2` hold position `2k + 1`.
fn input_position(input: u64) -> u64 {
    input / 3 * 2 + u64::from(!input.is_multiple_of(3))
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
struct HourSimulationFactory {
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
    fn decode(&mut self, request: DecodeRequest) -> Result<DecodeOutput, TranscribeError> {
        let (start, end) = marked_window(request.samples())?;
        match self.mode {
            DecodeMode::Interim => {
                let mut state = self.probe.state();
                state.interim_decodes += 1;
                let live = end.saturating_sub((EnginePolicy::SAMPLE_RATE * 4) as u64);
                let index = live / FINAL_STRIDE_SAMPLES;
                Ok(DecodeOutput::new(format!("live region {index:04}")))
            }
            DecodeMode::Final => {
                let mut state = self.probe.state();
                let expected_start = if state.gap_free_coverage_samples == 0 {
                    0
                } else {
                    state
                        .gap_free_coverage_samples
                        .saturating_sub(FINAL_OVERLAP_SAMPLES)
                };
                let expected_end = state
                    .gap_free_coverage_samples
                    .saturating_add(FINAL_STRIDE_SAMPLES);
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

                let word = |sample: u64| {
                    usize::try_from(sample * WORDS_PER_STRIDE / FINAL_STRIDE_SAMPLES)
                        .map_err(|_| marker_error("marker position does not fit fixture text"))
                };
                Ok(DecodeOutput::new(timeline_text(word(start)?, word(end)?)))
            }
            _ => unreachable!("the hour simulation scripts only interim and final decodes"),
        }
    }
}

/// The PCM16 level of output position `output`; levels stay distinct for
/// the first 24 575 seconds.
fn marker_sample(output: u64) -> i16 {
    let second = output / SECOND;
    let level = i16::try_from(second)
        .ok()
        .and_then(|second| LEVEL_BASE.checked_add(second))
        .unwrap_or(i16::MAX);
    if second.is_multiple_of(2) {
        level
    } else {
        -level
    }
}

/// The 16 kHz samples the production resampler emits at `start..start +
/// len` from [`hour_marker_input`] audio, computing each position once per
/// second and phase where it weighs only that second's level.
fn expected_window(start: u64, len: u64) -> Vec<f32> {
    let sample = |position| {
        resampled_sample(position, |input| {
            f32::from(marker_sample(input_position(input))) / 32_768.0
        })
    };
    let mut plateau_second = u64::MAX;
    let mut plateau = [None; 2];
    (start..start + len)
        .map(|position| {
            let second = position / SECOND;
            if input_position(*resampled_inputs(position).start()) / SECOND != second {
                return sample(position);
            }
            if plateau_second != second {
                plateau_second = second;
                plateau = [None; 2];
            }
            *plateau[usize::from(position % 2 == 1)].get_or_insert_with(|| sample(position))
        })
        .collect()
}

fn marked_window(samples: &[f32]) -> Result<(u64, u64), TranscribeError> {
    let start =
        window_start(samples).ok_or_else(|| marker_error("absolute PCM marker is missing"))?;
    let len = u64::try_from(samples.len()).map_err(|_| marker_error("window length overflowed"))?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| marker_error("absolute PCM window overflowed"))?;
    let expected = expected_window(start, len);
    if samples
        .iter()
        .zip(&expected)
        .any(|(actual, expected)| actual.to_bits() != expected.to_bits())
    {
        return Err(marker_error("absolute PCM marker sequence is corrupt"));
    }
    Ok((start, end))
}

/// The absolute position of `samples[0]`. A plateau, where both phases
/// repeat, names its second by its level, and the first sample after it
/// that differs from the sample two before is the next second's first.
fn window_start(samples: &[f32]) -> Option<u64> {
    let same = |left: usize, right: usize| samples[left].to_bits() == samples[right].to_bits();
    let flat = (4..samples.len()).find(|&index| {
        same(index, index - 2) && same(index - 2, index - 4) && same(index - 1, index - 3)
    })?;
    let second = level_second(samples[flat])?;
    let boundary = (flat + 1..samples.len()).find(|&index| !same(index, index - 2))?;
    (second + 1)
        .checked_mul(SECOND)?
        .checked_sub(u64::try_from(boundary).ok()?)
}

/// The second whose level a plateau sample holds, within the rounding of
/// the resampler's unit-sum taps.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a plateau sample is rounded to the PCM16 level it holds"
)]
fn level_second(sample: f32) -> Option<u64> {
    let scaled = f64::from(sample) * 32_768.0;
    let rounded = scaled.round();
    if !rounded.is_finite() || (scaled - rounded).abs() >= 0.25 {
        return None;
    }
    let level = i16::try_from(rounded as i32).ok()?;
    let second = u64::from(
        level
            .unsigned_abs()
            .checked_sub(LEVEL_BASE.unsigned_abs())?,
    );
    (u64::from(level < 0) == second % 2).then_some(second)
}

fn marker_error(_message: &'static str) -> TranscribeError {
    match EnginePolicy::new(0, 1, false) {
        Err(error) => error,
        Ok(_) => unreachable!("a zero-second fixture policy is invalid"),
    }
}

fn timeline_text(start_word: usize, end_word: usize) -> String {
    (start_word..end_word)
        .map(|word| format!("word{word:04}"))
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
    service.state.load_scripted(
        HourSimulationFactory::new(probe),
        policy,
        &tokio_util::sync::CancellationToken::new(),
    )?;
    Ok(service)
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use gateway_stt_engine::{DecodeMode, DecodeRequest, Decoder};

    use super::{
        FINAL_STRIDE_SAMPLES, HourSimulationDecoder, HourSimulationProbe, SECOND, expected_window,
        hour_marker_input, window_start,
    };
    use crate::audio::AudioBuffer;

    fn output(start: u64, samples: usize) -> Vec<f32> {
        expected_window(
            start,
            u64::try_from(samples).expect("fixture length fits u64"),
        )
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
    fn marker_audio_resamples_to_the_expected_window_bit_for_bit() {
        let bytes = hour_marker_input(0, 60_000)
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        let mut audio = AudioBuffer::default();
        audio
            .append_base64(&base64::engine::general_purpose::STANDARD.encode(bytes))
            .expect("marker audio appends");
        let actual = audio.commit_validated().into_samples();
        let expected = output(0, 40_000);
        assert_eq!(actual.len(), expected.len());
        assert!(
            actual
                .iter()
                .zip(&expected)
                .all(|(actual, expected)| actual.to_bits() == expected.to_bits()),
            "the production resampler emits the fixture's expected samples"
        );
    }

    #[test]
    fn a_window_names_its_own_start_from_its_first_second_boundary() {
        for start in [0, 1, 12_345, 3 * SECOND - 7, 3_599 * SECOND] {
            assert_eq!(window_start(&output(start, 40_000)), Some(start), "{start}");
        }
    }

    #[test]
    fn hour_decoder_rejects_wrong_stale_duplicated_reordered_and_compacted_pcm() {
        let stride = usize::try_from(FINAL_STRIDE_SAMPLES).expect("a stride fits usize");
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
        let (step, later) = (16_002, 16_010);
        assert_ne!(reordered[step], reordered[later], "both lie in a step");
        reordered.swap(step, later);
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
