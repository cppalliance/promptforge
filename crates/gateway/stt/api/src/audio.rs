//! Base64 PCM16 audio buffering, resampling, and commit validation for realtime input.

use std::sync::LazyLock;

use base64::Engine as _;

const INPUT_SAMPLE_RATE: u64 = 24_000;
const INPUT_SAMPLE_RATE_USIZE: usize = 24_000;
const BYTES_PER_SAMPLE: usize = size_of::<i16>();
const MAX_BUFFERED_SECONDS: usize = 30;
const MIN_COMMIT_MILLISECONDS: usize = 100;

const MAX_APPEND_AUDIO_BYTES: usize = 15 * 1024 * 1024;
const MAX_BUFFERED_AUDIO_BYTES: usize =
    INPUT_SAMPLE_RATE_USIZE * BYTES_PER_SAMPLE * MAX_BUFFERED_SECONDS;
const MIN_COMMIT_SAMPLES: usize = INPUT_SAMPLE_RATE_USIZE * MIN_COMMIT_MILLISECONDS / 1_000;

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub(super) enum AudioError {
    #[error("audio must be canonical padded Base64")]
    #[non_exhaustive]
    InvalidBase64(#[source] base64::DecodeError),
    #[error("decoded audio exceeds the {max_bytes} byte append limit")]
    AppendTooLarge { max_bytes: usize },
    #[error("audio ended with an incomplete PCM16 sample")]
    IncompletePcm16Sample,
    #[error(
        "audio buffer exceeds {maximum_seconds} seconds: {retained_ms} ms retained, {requested_ms} ms requested"
    )]
    BufferTooLong {
        maximum_seconds: usize,
        retained_ms: usize,
        requested_ms: usize,
    },
    #[error("committed audio must be at least {minimum_ms} milliseconds")]
    CommitTooShort { minimum_ms: usize },
}

#[derive(Debug, PartialEq)]
pub(super) struct CommittedAudio {
    samples: Vec<f32>,
    input_samples: u64,
}

impl CommittedAudio {
    #[cfg(test)]
    fn samples(&self) -> &[f32] {
        &self.samples
    }

    #[cfg(test)]
    const fn input_samples(&self) -> u64 {
        self.input_samples
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "sample counts at audio-buffer scale convert to f64 without meaningful precision loss"
    )]
    pub(super) fn duration_seconds(&self) -> f64 {
        self.input_samples as f64 / INPUT_SAMPLE_RATE as f64
    }

    pub(super) fn into_samples(self) -> Vec<f32> {
        self.samples
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct AudioBuffer {
    input_bytes: usize,
    pub(super) input_samples: u64,
    odd_byte: Option<u8>,
    resampler: Resampler24To16,
}

impl AudioBuffer {
    pub(super) fn append_base64(&mut self, payload: &str) -> Result<(), AudioError> {
        let bytes = decode_base64(payload)?;
        let too_long = || buffer_too_long(self.input_bytes, bytes.len());
        let next_bytes = self
            .input_bytes
            .checked_add(bytes.len())
            .ok_or_else(too_long)?;
        if next_bytes > MAX_BUFFERED_AUDIO_BYTES {
            return Err(too_long());
        }
        let held = usize::from(self.odd_byte.is_some());
        let complete_samples = (bytes.len() + held) / BYTES_PER_SAMPLE;
        let _ = self
            .input_samples
            .checked_add(u64::try_from(complete_samples).map_err(|_| too_long())?)
            .ok_or_else(too_long)?;

        self.input_bytes = next_bytes;
        let mut bytes = bytes.into_iter();
        if let Some(low) = self.odd_byte.take() {
            if let Some(high) = bytes.next() {
                self.push_sample(i16::from_le_bytes([low, high]));
            } else {
                self.odd_byte = Some(low);
                return Ok(());
            }
        }

        while let Some(low) = bytes.next() {
            if let Some(high) = bytes.next() {
                self.push_sample(i16::from_le_bytes([low, high]));
            } else {
                self.odd_byte = Some(low);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn commit(&mut self) -> Result<CommittedAudio, AudioError> {
        self.validate_commit()?;
        Ok(self.commit_validated())
    }

    pub(super) fn commit_validated(&mut self) -> CommittedAudio {
        self.resampler.flush();
        let samples = std::mem::take(&mut self.resampler.output);
        let input_samples = self.input_samples;
        self.clear();
        CommittedAudio {
            samples,
            input_samples,
        }
    }

    pub(super) fn validate_commit(&self) -> Result<(), AudioError> {
        if self.odd_byte.is_some() {
            return Err(AudioError::IncompletePcm16Sample);
        }
        if self.input_samples < MIN_COMMIT_SAMPLES as u64 {
            return Err(AudioError::CommitTooShort {
                minimum_ms: MIN_COMMIT_MILLISECONDS,
            });
        }
        Ok(())
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn take_resampled(&mut self) -> Vec<f32> {
        self.input_bytes = usize::from(self.odd_byte.is_some());
        let mut output = std::mem::take(&mut self.resampler.output);
        output.shrink_to_fit();
        output
    }

    #[cfg(test)]
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample counts at audio-buffer scale convert to f64 without meaningful precision loss"
    )]
    pub(super) fn buffered_duration_seconds(&self) -> f64 {
        self.input_samples as f64 / INPUT_SAMPLE_RATE as f64
    }

    fn push_sample(&mut self, sample: i16) {
        self.resampler.push(f32::from(sample) / 32_768.0);
        self.input_samples += 1;
    }
}

/// The error for an append of `requested_bytes` onto `retained_bytes` of
/// buffered input, in milliseconds so it reads the same as the PCM budget's.
/// A partial millisecond of the request counts as a whole one.
fn buffer_too_long(retained_bytes: usize, requested_bytes: usize) -> AudioError {
    const BYTES_PER_MS: usize = INPUT_SAMPLE_RATE_USIZE / 1_000 * BYTES_PER_SAMPLE;
    AudioError::BufferTooLong {
        maximum_seconds: MAX_BUFFERED_SECONDS,
        retained_ms: retained_bytes / BYTES_PER_MS,
        requested_ms: requested_bytes.div_ceil(BYTES_PER_MS),
    }
}

fn decode_base64(payload: &str) -> Result<Vec<u8>, AudioError> {
    const MAX_BASE64_CHARS: usize = MAX_APPEND_AUDIO_BYTES.div_ceil(3) * 4;
    if payload.len() > MAX_BASE64_CHARS {
        return Err(AudioError::AppendTooLarge {
            max_bytes: MAX_APPEND_AUDIO_BYTES,
        });
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(AudioError::InvalidBase64)?;
    if decoded.len() > MAX_APPEND_AUDIO_BYTES {
        return Err(AudioError::AppendTooLarge {
            max_bytes: MAX_APPEND_AUDIO_BYTES,
        });
    }
    Ok(decoded)
}

/// Taps of the low-pass prototype, which runs at the 48 kHz rate between
/// interpolating 24 kHz by two and decimating by three.
const PROTOTYPE_TAPS: usize = 97;
/// The prototype's half length, the tap offset of its center.
const PROTOTYPE_CENTER: i32 = 48;
const UPSAMPLED_RATE_HZ: f64 = 48_000.0;
/// Below the 8 kHz output Nyquist rate, so the transition band ends near
/// 8.5 kHz and content above it folds back at least 60 dB down.
const CUTOFF_HZ: f64 = 7_600.0;
/// The Kaiser window shape for about 60 dB of stopband attenuation.
const KAISER_BETA: f64 = 5.65;
const EVEN_TAPS: usize = PROTOTYPE_TAPS.div_ceil(2);
const ODD_TAPS: usize = PROTOTYPE_TAPS / 2;
const HISTORY: usize = EVEN_TAPS;

static TAPS: LazyLock<PolyphaseTaps> = LazyLock::new(PolyphaseTaps::design);

/// The prototype split by output phase: output `2p` weighs inputs `3p`,
/// `3p - 1`, and so on with the even prototype taps, and output `2p + 1`
/// weighs inputs `3p + 1`, `3p`, and so on with the odd ones.
#[derive(Debug)]
struct PolyphaseTaps {
    even: [f32; EVEN_TAPS],
    odd: [f32; ODD_TAPS],
}

impl PolyphaseTaps {
    /// A Kaiser-windowed sinc low-pass, each phase scaled to unit sum so a
    /// constant passes unchanged and the two phases agree in gain.
    fn design() -> Self {
        let window_norm = bessel_i0(KAISER_BETA);
        let tap = |offset: i32| {
            let t = f64::from(offset);
            let cutoff = CUTOFF_HZ / UPSAMPLED_RATE_HZ;
            let sinc = if offset == 0 {
                2.0 * cutoff
            } else {
                (2.0 * std::f64::consts::PI * cutoff * t).sin() / (std::f64::consts::PI * t)
            };
            let ratio = t / f64::from(PROTOTYPE_CENTER);
            sinc * bessel_i0(KAISER_BETA * (1.0 - ratio * ratio).sqrt()) / window_norm
        };
        Self {
            even: unit_sum(-PROTOTYPE_CENTER, tap),
            odd: unit_sum(1 - PROTOTYPE_CENTER, tap),
        }
    }
}

/// The `N` taps at offsets `first`, `first + 2`, and so on, scaled to sum
/// to one.
fn unit_sum<const N: usize>(first: i32, tap: impl Fn(i32) -> f64) -> [f32; N] {
    let mut taps = [0.0_f64; N];
    let mut offset = first;
    for slot in &mut taps {
        *slot = tap(offset);
        offset += 2;
    }
    let sum = taps.iter().sum::<f64>();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "filter taps are designed in f64 and stored in the f32 sample type"
    )]
    taps.map(|tap| (tap / sum) as f32)
}

/// The zeroth-order modified Bessel function of the first kind, by its
/// power series.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..=40_u32 {
        term *= half / f64::from(k);
        sum += term * term;
    }
    sum
}

/// Streams 24 kHz input to 16 kHz output through a polyphase low-pass, so
/// input above the 8 kHz output Nyquist rate does not fold into speech
/// frequencies.
///
/// Output `2p` is emitted with input `3p`, and output `2p + 1` with input
/// `3p + 2` or by [`Self::flush`], so `n` flushed inputs yield exactly
/// `ceil(2n / 3)` outputs and the take's timeline, which counts output
/// samples from input samples, needs no lookahead. The price is the
/// prototype's delay: output `n` carries the input at time `1.5 n - 24`, so
/// the audio runs 1 ms behind its timeline and a take's last millisecond is
/// not emitted.
#[derive(Clone, Debug)]
struct Resampler24To16 {
    phase: u8,
    /// The latest [`HISTORY`] inputs, stored twice so that
    /// `history[cursor..cursor + HISTORY]` holds them newest first.
    history: [f32; 2 * HISTORY],
    cursor: usize,
    primed: bool,
    output: Vec<f32>,
}

impl Default for Resampler24To16 {
    fn default() -> Self {
        Self {
            phase: 0,
            history: [0.0; 2 * HISTORY],
            cursor: 0,
            primed: false,
            output: Vec::new(),
        }
    }
}

impl Resampler24To16 {
    fn push(&mut self, sample: f32) {
        // The first input stands in for the audio before it, so a take
        // does not open with a ramp up from zero.
        if !self.primed {
            self.history.fill(sample);
            self.primed = true;
        }
        self.cursor = self.cursor.checked_sub(1).unwrap_or(HISTORY - 1);
        self.history[self.cursor] = sample;
        self.history[self.cursor + HISTORY] = sample;
        match self.phase {
            0 => self.emit(&TAPS.even, 0),
            1 => {}
            2 => self.emit(&TAPS.odd, 1),
            _ => unreachable!("the resampler phase is modulo three"),
        }
        self.phase = if self.phase == 2 { 0 } else { self.phase + 1 };
    }

    /// Emits the odd output whose input group ended after its second input.
    fn flush(&mut self) {
        if self.phase == 2 {
            self.emit(&TAPS.odd, 0);
        }
    }

    /// Emits `taps` weighed over the inputs newest first, after skipping the
    /// newest `skip`.
    fn emit(&mut self, taps: &[f32], skip: usize) {
        let start = self.cursor + skip;
        self.output
            .push(weigh(&self.history[start..start + taps.len()], taps));
    }
}

/// `taps` weighed over `inputs`, both newest first.
fn weigh(inputs: &[f32], taps: &[f32]) -> f32 {
    // An index loop, because unoptimized test builds run iterator adapters
    // several times slower, and their fixtures resample hours of audio.
    let mut sum = 0.0;
    let mut index = 0;
    while index < taps.len() {
        sum += inputs[index] * taps[index];
        index += 1;
    }
    sum
}

/// The newest input that output `position` weighs, and its phase's taps.
#[cfg(any(test, feature = "test-fixtures"))]
fn output_taps(position: u64) -> (u64, &'static [f32]) {
    let group = position / 2 * 3;
    if position.is_multiple_of(2) {
        (group, &TAPS.even)
    } else {
        (group + 1, &TAPS.odd)
    }
}

/// The inputs that output `position` weighs, where an index before 0
/// stands for input 0 as the resampler's first input does.
#[cfg(any(test, feature = "test-fixtures"))]
pub(crate) fn resampled_inputs(position: u64) -> std::ops::RangeInclusive<u64> {
    let (newest, taps) = output_taps(position);
    newest.saturating_sub(taps.len() as u64 - 1)..=newest
}

/// Output `position` of the stream whose input `index` is `input(index)`,
/// bit for bit as [`Resampler24To16`] emits it.
#[cfg(any(test, feature = "test-fixtures"))]
pub(crate) fn resampled_sample(position: u64, input: impl Fn(u64) -> f32) -> f32 {
    let (newest, taps) = output_taps(position);
    let mut inputs = [0.0; HISTORY];
    let mut index = newest;
    for slot in &mut inputs[..taps.len()] {
        *slot = input(index);
        index = index.saturating_sub(1);
    }
    weigh(&inputs, taps)
}

#[cfg(test)]
#[path = "audio-tests.rs"]
mod tests;
