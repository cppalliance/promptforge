//! Backend-neutral worker and audio policy.

use std::time::Duration;

use crate::TranscribeError;

const SILENCE_RMS: f64 = 0.001;
const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(120);

/// Checked capture, startup, and backend capability policy.
#[derive(Clone, Copy, Debug)]
pub struct EnginePolicy {
    window_samples: usize,
    interval: Duration,
    startup_timeout: Duration,
    gpu_available: bool,
}

impl EnginePolicy {
    /// PCM sample rate the streaming wire format and decoders require.
    pub const SAMPLE_RATE: usize = 16_000;

    /// Minimum audio the interim loop bothers to transcribe.
    pub const MIN_WINDOW_SAMPLES: usize = Self::SAMPLE_RATE / 2;

    /// Validates host capture policy and applies the bounded startup deadline.
    ///
    /// # Errors
    /// Returns [`TranscribeError::InvalidConfig`] for zero or overflowing
    /// capture policy values.
    pub fn new(
        window_seconds: u64,
        interval_ms: u64,
        gpu_available: bool,
    ) -> Result<Self, TranscribeError> {
        if window_seconds == 0 {
            return Err(TranscribeError::InvalidConfig(
                "stt.window_seconds must be at least 1".to_owned(),
            ));
        }
        if interval_ms == 0 {
            return Err(TranscribeError::InvalidConfig(
                "stt.interval_ms must be at least 1".to_owned(),
            ));
        }
        let seconds = usize::try_from(window_seconds).map_err(|_| {
            TranscribeError::InvalidConfig("stt.window_seconds is too large".to_owned())
        })?;
        let window_samples = seconds.checked_mul(Self::SAMPLE_RATE).ok_or_else(|| {
            TranscribeError::InvalidConfig("stt.window_seconds is too large".to_owned())
        })?;
        Ok(Self {
            window_samples,
            interval: Duration::from_millis(interval_ms),
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
            gpu_available,
        })
    }

    /// Overrides the construction deadline for deterministic hosts and tests.
    #[must_use]
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    /// Samples in the sliding interim window.
    #[must_use]
    pub fn window_samples(self) -> usize {
        self.window_samples
    }

    /// Cadence of the interim loop.
    #[must_use]
    pub fn interval(self) -> Duration {
        self.interval
    }

    /// Maximum shared wait for all worker construction outcomes.
    #[must_use]
    pub fn startup_timeout(self) -> Duration {
        self.startup_timeout
    }

    /// Whether the backend reports hardware acceleration.
    #[must_use]
    pub fn gpu_available(self) -> bool {
        self.gpu_available
    }

    /// Returns true when the buffer is quiet enough that a decoder would
    /// hallucinate rather than transcribe.
    #[must_use]
    pub fn is_silence(samples: &[f32]) -> bool {
        rms(samples) < SILENCE_RMS
    }
}

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
        assert!(EnginePolicy::is_silence(&[0.0; 1600]));
        assert!(EnginePolicy::is_silence(&[0.0005; 1600]));
        assert!(!EnginePolicy::is_silence(&[0.05; 1600]));
    }
}
