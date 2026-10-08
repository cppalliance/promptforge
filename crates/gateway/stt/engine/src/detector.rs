//! Per-chunk speech decisions behind a backend-neutral seam.

use std::fmt;

use crate::DetectorError;

/// Classifies streaming audio as speech one chunk at a time.
///
/// Callers pass consecutive [`EnginePolicy::DETECTOR_CHUNK_SAMPLES`]-sample
/// chunks in stream order, so implementations may carry state across calls.
///
/// [`EnginePolicy::DETECTOR_CHUNK_SAMPLES`]: crate::EnginePolicy::DETECTOR_CHUNK_SAMPLES
pub trait SpeechDetector: fmt::Debug + Send {
    /// Returns whether `chunk` contains speech.
    ///
    /// # Errors
    /// Returns [`DetectorError::Inference`] when the chunk cannot be classified.
    fn classify(&mut self, chunk: &[f32]) -> Result<bool, DetectorError>;
}

/// Reads speech wherever [`EnginePolicy::is_silence`] does not; never fails.
///
/// [`EnginePolicy::is_silence`]: crate::EnginePolicy::is_silence
#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct EnergyDetector;

#[cfg(any(test, feature = "test-fixtures"))]
impl SpeechDetector for EnergyDetector {
    fn classify(&mut self, chunk: &[f32]) -> Result<bool, DetectorError> {
        Ok(!crate::EnginePolicy::is_silence(chunk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EnginePolicy;

    const CHUNK: usize = EnginePolicy::DETECTOR_CHUNK_SAMPLES;
    const SILENT: [f32; CHUNK] = [0.0; CHUNK];
    const QUIET: [f32; CHUNK] = [0.0005; CHUNK];
    const LOUD: [f32; CHUNK] = [0.05; CHUNK];

    #[test]
    fn miri_energy_detector_matches_the_silence_gate() {
        let mut detector = EnergyDetector;
        for (chunk, speech) in [(&SILENT, false), (&QUIET, false), (&LOUD, true)] {
            assert_eq!(speech, !EnginePolicy::is_silence(chunk));
            assert_eq!(detector.classify(chunk).ok(), Some(speech));
        }
    }

    #[test]
    fn miri_detector_errors_carry_their_source_message() {
        assert_eq!(
            DetectorError::load("missing model").to_string(),
            "load speech detector: missing model"
        );
        assert_eq!(
            DetectorError::inference("probability is NaN").to_string(),
            "classify speech chunk: probability is NaN"
        );
    }
}
