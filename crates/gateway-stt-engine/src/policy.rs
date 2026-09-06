//! Backend-neutral audio policy shared by the engine and its host.

/// PCM sample rate the streaming wire format and decoders require.
pub const SAMPLE_RATE: usize = 16_000;

/// Minimum audio the interim loop bothers to transcribe.
pub const MIN_WINDOW_SAMPLES: usize = SAMPLE_RATE / 2;

/// Windows below this RMS are treated as silence.
const SILENCE_RMS: f64 = 0.001;

#[expect(
    clippy::cast_precision_loss,
    reason = "audio buffers are far below 2^53 samples"
)]
fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let energy: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (energy / samples.len() as f64).sqrt()
}

/// Returns true when the buffer is quiet enough that a speech decoder would
/// hallucinate rather than transcribe.
#[must_use]
pub fn is_silence(samples: &[f32]) -> bool {
    rms(samples) < SILENCE_RMS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[]).to_bits(), 0.0f64.to_bits());
        assert_eq!(rms(&[0.0; 1600]).to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn rms_of_a_constant_signal_is_its_amplitude() {
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn silence_gate_separates_quiet_from_speech() {
        assert!(is_silence(&[0.0; 1600]));
        assert!(is_silence(&[0.0005; 1600]));
        assert!(!is_silence(&[0.05; 1600]));
    }
}
