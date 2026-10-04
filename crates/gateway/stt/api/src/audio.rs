//! Base64 PCM16 audio buffering, resampling, and commit validation for realtime input.

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
    #[error("audio buffer exceeds {maximum_seconds} seconds")]
    BufferTooLong { maximum_seconds: usize },
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
        let next_bytes =
            self.input_bytes
                .checked_add(bytes.len())
                .ok_or(AudioError::BufferTooLong {
                    maximum_seconds: MAX_BUFFERED_SECONDS,
                })?;
        if next_bytes > MAX_BUFFERED_AUDIO_BYTES {
            return Err(AudioError::BufferTooLong {
                maximum_seconds: MAX_BUFFERED_SECONDS,
            });
        }
        let held = usize::from(self.odd_byte.is_some());
        let complete_samples = (bytes.len() + held) / BYTES_PER_SAMPLE;
        let _ = self
            .input_samples
            .checked_add(u64::try_from(complete_samples).map_err(|_| {
                AudioError::BufferTooLong {
                    maximum_seconds: MAX_BUFFERED_SECONDS,
                }
            })?)
            .ok_or(AudioError::BufferTooLong {
                maximum_seconds: MAX_BUFFERED_SECONDS,
            })?;

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

#[derive(Clone, Debug, Default)]
struct Resampler24To16 {
    phase: u8,
    previous: Option<f32>,
    output: Vec<f32>,
}

impl Resampler24To16 {
    fn push(&mut self, sample: f32) {
        match self.phase {
            0 => self.emit(sample),
            1 => {}
            2 => self.emit(self.previous.unwrap_or(sample).midpoint(sample)),
            _ => unreachable!("the resampler phase is modulo three"),
        }
        self.previous = Some(sample);
        self.phase = if self.phase == 2 { 0 } else { self.phase + 1 };
    }

    fn flush(&mut self) {
        if self.phase == 2
            && let Some(sample) = self.previous
        {
            self.emit(sample);
        }
    }

    fn emit(&mut self, sample: f32) {
        self.output.push(sample);
    }
}

#[cfg(test)]
#[path = "audio-tests.rs"]
mod tests;
