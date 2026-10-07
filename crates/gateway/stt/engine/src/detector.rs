//! Per-chunk speech decisions behind a backend-neutral seam.

use std::fmt;

use crate::{DetectorError, EnginePolicy};

/// Classifies streaming audio as speech one chunk at a time.
///
/// Callers pass consecutive [`EnginePolicy::DETECTOR_CHUNK_SAMPLES`]-sample
/// chunks in stream order, so implementations may carry state across calls.
pub trait SpeechDetector: Send {
    /// Returns whether `chunk` contains speech.
    ///
    /// # Errors
    /// Returns [`DetectorError::Inference`] when the chunk cannot be classified.
    fn classify(&mut self, chunk: &[f32]) -> Result<bool, DetectorError>;
}

/// Reads speech wherever [`EnginePolicy::is_silence`] does not; never fails.
#[derive(Clone, Copy, Debug, Default)]
pub struct EnergyDetector;

impl EnergyDetector {
    fn is_speech(chunk: &[f32]) -> bool {
        !EnginePolicy::is_silence(chunk)
    }
}

impl SpeechDetector for EnergyDetector {
    fn classify(&mut self, chunk: &[f32]) -> Result<bool, DetectorError> {
        Ok(Self::is_speech(chunk))
    }
}

/// An infallible detector that answers with loudness once its primary errs.
pub struct FallbackDetector {
    primary: Option<Box<dyn SpeechDetector>>,
    fault: Option<DetectorError>,
}

impl FallbackDetector {
    /// Classifies with `primary` until its first error.
    #[must_use]
    pub fn new(primary: Box<dyn SpeechDetector>) -> Self {
        Self {
            primary: Some(primary),
            fault: None,
        }
    }

    /// Classifies with loudness alone.
    #[must_use]
    pub fn energy() -> Self {
        Self {
            primary: None,
            fault: None,
        }
    }

    /// Returns the primary's decision, or loudness once the primary has erred.
    ///
    /// The chunk the primary fails on is answered with loudness, and the
    /// primary is dropped so it is never called again.
    #[must_use]
    pub fn classify(&mut self, chunk: &[f32]) -> bool {
        if let Some(primary) = self.primary.as_mut() {
            match primary.classify(chunk) {
                Ok(speech) => return speech,
                Err(error) => {
                    self.primary = None;
                    self.fault = Some(error);
                }
            }
        }
        EnergyDetector::is_speech(chunk)
    }

    /// Hands out the primary's first error once, for reporting.
    #[must_use]
    pub fn take_fault(&mut self) -> Option<DetectorError> {
        self.fault.take()
    }
}

impl fmt::Debug for FallbackDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FallbackDetector")
            .field("primary", &self.primary.is_some())
            .field("fault", &self.fault)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const CHUNK: usize = EnginePolicy::DETECTOR_CHUNK_SAMPLES;
    const SILENT: [f32; CHUNK] = [0.0; CHUNK];
    const QUIET: [f32; CHUNK] = [0.0005; CHUNK];
    const LOUD: [f32; CHUNK] = [0.05; CHUNK];

    struct Primary {
        outcomes: std::vec::IntoIter<Result<bool, DetectorError>>,
        calls: Arc<AtomicUsize>,
    }

    impl SpeechDetector for Primary {
        fn classify(&mut self, _chunk: &[f32]) -> Result<bool, DetectorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcomes.next().unwrap_or(Ok(true))
        }
    }

    fn primary(
        outcomes: Vec<Result<bool, DetectorError>>,
    ) -> (Box<dyn SpeechDetector>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let primary = Primary {
            outcomes: outcomes.into_iter(),
            calls: Arc::clone(&calls),
        };
        (Box::new(primary), calls)
    }

    #[test]
    fn miri_energy_detector_matches_the_silence_gate() {
        let mut detector = EnergyDetector;
        for (chunk, speech) in [(&SILENT, false), (&QUIET, false), (&LOUD, true)] {
            assert_eq!(speech, !EnginePolicy::is_silence(chunk));
            assert_eq!(detector.classify(chunk).ok(), Some(speech));
        }
    }

    #[test]
    fn miri_fallback_passes_primary_decisions_through() {
        let (primary, calls) = primary(vec![Ok(true), Ok(false), Ok(true)]);
        let mut detector = FallbackDetector::new(primary);

        assert!(detector.classify(&SILENT));
        assert!(!detector.classify(&LOUD));
        assert!(detector.classify(&QUIET));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(detector.take_fault().is_none());
    }

    #[test]
    fn miri_fallback_answers_the_failed_chunk_with_loudness_and_never_calls_the_primary_again() {
        let (primary, calls) = primary(vec![
            Ok(false),
            Err(DetectorError::inference("window compute failed")),
            Ok(false),
            Ok(true),
        ]);
        let mut detector = FallbackDetector::new(primary);

        assert!(!detector.classify(&LOUD), "the primary decides first");
        assert!(detector.classify(&LOUD), "the failed chunk is not dropped");
        assert!(detector.classify(&LOUD));
        assert!(!detector.classify(&SILENT));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn miri_fallback_reports_the_first_fault_once() {
        let (primary, _calls) = primary(vec![
            Ok(true),
            Err(DetectorError::inference("first")),
            Err(DetectorError::inference("second")),
        ]);
        let mut detector = FallbackDetector::new(primary);

        assert!(detector.classify(&SILENT));
        assert!(detector.take_fault().is_none());
        assert!(detector.classify(&LOUD));
        assert!(detector.classify(&LOUD));
        let fault = detector.take_fault().expect("the first error is reported");
        assert!(matches!(&fault, DetectorError::Inference(message) if message == "first"));
        assert!(detector.take_fault().is_none());
        assert!(!detector.classify(&SILENT));
        assert!(detector.take_fault().is_none());
    }

    #[test]
    fn miri_energy_fallback_uses_loudness_and_never_faults() {
        let mut detector = FallbackDetector::energy();

        assert!(!detector.classify(&SILENT));
        assert!(!detector.classify(&QUIET));
        assert!(detector.classify(&LOUD));
        assert!(detector.take_fault().is_none());
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
