//! The discarded decode a new decoder runs before its first real one.

use gateway_stt_engine::{EnginePolicy, TranscribeError};

use super::{WhisperDecoder, transcribe_blocking};

const WARM_UP_SAMPLES: usize = EnginePolicy::SAMPLE_RATE;
/// Peak amplitude of the warm-up noise. Its RMS, about 0.006, clears the
/// silence gate's 0.001, as every buffer a real decode hands whisper does.
const WARM_UP_PEAK: f32 = 0.01;
const WARM_UP_SCALE: f32 = WARM_UP_PEAK / 32_768.0;
const WARM_UP_SEED: u32 = 0x9E37_79B9;

impl WhisperDecoder {
    /// Runs one pass over the warm-up noise with this decoder's profile and
    /// discards its text, so whisper's first-pass setup lands at load
    /// rather than on the first real decode.
    pub(super) fn warm_up(&mut self) -> Result<(), TranscribeError> {
        transcribe_blocking(
            &mut self.state,
            &warm_up_samples(),
            None,
            &self.profile,
            None,
        )?;
        Ok(())
    }
}

/// One second of low-level xorshift noise, the same on every call.
fn warm_up_samples() -> Vec<f32> {
    let mut state = WARM_UP_SEED;
    (0..WARM_UP_SAMPLES)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [high, low, ..] = state.to_be_bytes();
            f32::from(i16::from_be_bytes([high, low])) * WARM_UP_SCALE
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warm_up_buffer_is_one_second_of_low_noise_that_clears_the_silence_gate() {
        let samples = warm_up_samples();
        assert_eq!(samples.len(), EnginePolicy::SAMPLE_RATE);
        assert!(samples.len() >= EnginePolicy::MIN_WINDOW_SAMPLES);
        assert!(
            !EnginePolicy::is_silence(&samples),
            "the backend's silence gate would skip a pass over the warm-up buffer"
        );
        assert!(samples.iter().all(|sample| sample.abs() <= WARM_UP_PEAK));
    }
}
