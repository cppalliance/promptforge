//! A scripted Silero source and the generation whose takes load it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gateway_stt_engine::test_fixtures::{ScriptedDecoder, ScriptedDetector};
use gateway_stt_engine::{DetectorError, SpeechDetector};

use crate::generation::{GenerationLease, GenerationState, SileroSource};

/// The verified model path a scripted Silero generation hands its source.
pub(crate) const SCRIPTED_SILERO_MODEL: &str = "ggml-silero.bin";

/// Loads `detector` until the load numbered `failure.0`, counted from
/// zero, and fails that load and every later one with `failure.1`,
/// recording each model path it is asked to load.
#[derive(Debug)]
pub(crate) struct ScriptedSilero {
    detector: ScriptedDetector,
    failure: Option<(usize, String)>,
    loaded: Mutex<Vec<PathBuf>>,
}

impl ScriptedSilero {
    pub(crate) fn new(detector: ScriptedDetector) -> Arc<Self> {
        Arc::new(Self {
            detector,
            failure: None,
            loaded: Mutex::default(),
        })
    }

    pub(crate) fn failing_from(load: usize, message: &str) -> Arc<Self> {
        Arc::new(Self {
            detector: ScriptedDetector::new([]),
            failure: Some((load, message.to_owned())),
            loaded: Mutex::default(),
        })
    }

    pub(crate) fn loaded(&self) -> Vec<PathBuf> {
        self.loaded.lock().expect("load record lock").clone()
    }
}

impl SileroSource for ScriptedSilero {
    fn load(&self, model: &Path) -> Result<Box<dyn SpeechDetector>, DetectorError> {
        let mut loaded = self.loaded.lock().expect("load record lock");
        loaded.push(model.to_path_buf());
        match &self.failure {
            Some((from, message)) if loaded.len() > *from => {
                Err(DetectorError::load(message.clone()))
            }
            _ => Ok(Box::new(self.detector.clone())),
        }
    }
}

/// Publishes a scripted generation without a final pass whose takes load
/// Silero from [`SCRIPTED_SILERO_MODEL`] through `source`, and admits one
/// lease on it. Shut the generation down once the lease is dropped.
pub(crate) fn scripted_silero_generation(
    source: Arc<ScriptedSilero>,
) -> (GenerationState, GenerationLease) {
    scripted_guided_generation(source, Vec::new())
}

/// [`scripted_silero_generation`] for a runtime that holds `guidance` as its
/// configured `[stt] vocabulary`.
pub(crate) fn scripted_guided_generation(
    source: Arc<ScriptedSilero>,
    guidance: Vec<String>,
) -> (GenerationState, GenerationLease) {
    let state = GenerationState::default();
    state
        .publish_scripted_guided(
            ScriptedDecoder::new(),
            PathBuf::from(SCRIPTED_SILERO_MODEL),
            source,
            guidance,
        )
        .expect("the scripted runtime loads");
    let lease = state.active().expect("the published runtime admits");
    (state, lease)
}
