//! Silero speech detection on whisper.cpp's streaming VAD.

use std::path::Path;

use gateway_stt_engine::{DetectorError, SpeechDetector};
use gateway_whisper_ffi::{VadContext, WhisperLibrary};

/// A chunk at or above this probability starts speech.
const SPEECH_START_PROBABILITY: f32 = 0.5;
/// Speech holds until a chunk falls below this probability.
const SPEECH_END_PROBABILITY: f32 = 0.35;

/// Classifies 512-sample chunks of 16 kHz PCM with the Silero model, whose
/// streaming state carries from each chunk to the next.
#[derive(Debug)]
pub struct SileroDetector {
    context: VadContext,
    hysteresis: Hysteresis,
}

impl SileroDetector {
    /// Loads the Silero model at `model` through `library`.
    ///
    /// whisper.cpp reads the file without validating it, so `model` must
    /// already match the pinned digest.
    ///
    /// # Errors
    /// Returns [`DetectorError::Load`] when whisper cannot load the model.
    pub fn new(library: &WhisperLibrary, model: &Path) -> Result<Self, DetectorError> {
        let context = VadContext::new(library, model)
            .map_err(|error| DetectorError::load(error.to_string()))?;
        Ok(Self {
            context,
            hysteresis: Hysteresis::default(),
        })
    }
}

impl SpeechDetector for SileroDetector {
    fn classify(&mut self, chunk: &[f32]) -> Result<bool, DetectorError> {
        let probability = self
            .context
            .detect_chunk(chunk)
            .map_err(|error| DetectorError::inference(error.to_string()))?;
        Ok(self.hysteresis.decide(validated(probability)?))
    }
}

/// Speech starts at [`SPEECH_START_PROBABILITY`] and holds until a chunk
/// falls below [`SPEECH_END_PROBABILITY`], so probabilities between the two
/// keep the previous decision.
#[derive(Clone, Copy, Debug, Default)]
struct Hysteresis {
    speaking: bool,
}

impl Hysteresis {
    fn decide(&mut self, probability: f32) -> bool {
        let threshold = if self.speaking {
            SPEECH_END_PROBABILITY
        } else {
            SPEECH_START_PROBABILITY
        };
        self.speaking = probability >= threshold;
        self.speaking
    }
}

/// `probability` when it lies in 0 to 1. A failed window compute still
/// reports success, so any other value is an inference error.
fn validated(probability: f32) -> Result<f32, DetectorError> {
    if (0.0..=1.0).contains(&probability) {
        Ok(probability)
    } else {
        Err(DetectorError::inference(format!(
            "Silero probability {probability} is outside 0 to 1"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decisions(probabilities: &[f32]) -> Vec<bool> {
        let mut hysteresis = Hysteresis::default();
        probabilities
            .iter()
            .map(|&probability| hysteresis.decide(probability))
            .collect()
    }

    #[test]
    fn hysteresis_starts_speech_at_the_start_probability() {
        assert_eq!(
            decisions(&[0.0, 0.49, 0.5]),
            [false, false, true],
            "speech starts at 0.5 and not below"
        );
    }

    #[test]
    fn hysteresis_holds_speech_down_to_the_end_probability_and_ends_it_below() {
        assert_eq!(
            decisions(&[0.9, 0.49, 0.36, 0.35, 0.34, 0.49]),
            [true, true, true, true, false, false],
            "speech holds through 0.35 and ends below it, and 0.49 does not restart it"
        );
    }

    #[test]
    fn hysteresis_does_not_flicker_between_the_two_probabilities() {
        let between = [0.4, 0.45, 0.36, 0.49, 0.4, 0.45];
        assert!(
            decisions(&between).iter().all(|speech| !speech),
            "silence stays silence"
        );
        let mut speaking = vec![0.6];
        speaking.extend(between);
        assert!(
            decisions(&speaking).iter().all(|speech| *speech),
            "speech stays speech"
        );
    }

    #[test]
    fn probabilities_outside_zero_to_one_are_inference_errors() {
        for probability in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            let error = validated(probability).expect_err("only 0 to 1 is a probability");
            assert!(
                matches!(error, DetectorError::Inference { .. }),
                "{probability}: {error}"
            );
        }
        for probability in [0.0, 0.35, 1.0] {
            assert_eq!(validated(probability).ok(), Some(probability));
        }
    }
}
