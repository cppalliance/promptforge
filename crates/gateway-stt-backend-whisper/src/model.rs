//! Whisper model factory, decoder, progress, and error translation.

use std::io::Read;
use std::path::Path;

use gateway_stt_engine::{
    DecodeMode, DecodeRequest, Decoder, EnginePolicy, ModelFactory, TranscribeError,
};
use gateway_whisper_ffi::{
    FullParams, SamplingStrategy, WhisperContext, WhisperLibrary, WhisperState,
};
use shared_progress::ProgressHandle;

use crate::WhisperConfig;
use crate::prompt::{GLOSSARY_TOKEN_BUDGET, final_prompt, fit_glossary, sanitize_prompt};

const MAX_PROMPT_TOKENS: usize = 224;
const PREWARM_CHUNK: usize = 4 * 1024 * 1024;

/// Factory for safe Whisper decoders backed by provisioned runtime artifacts.
#[derive(Debug)]
pub struct WhisperModelFactory {
    config: WhisperConfig,
    library: WhisperLibrary,
    gpu_available: bool,
}

impl WhisperModelFactory {
    /// Loads the runtime library and validates the configured model paths.
    ///
    /// Model contexts are created later on their owning engine workers.
    ///
    /// # Errors
    /// Returns a backend or model construction failure translated into the
    /// engine's backend-neutral error type.
    pub fn new(config: WhisperConfig) -> Result<Self, TranscribeError> {
        require_model_file(&config.interim_model)?;
        if let Some(final_model) = &config.final_model {
            require_model_file(final_model)?;
        }
        let library =
            WhisperLibrary::load(&config.library).map_err(TranscribeError::initialize_backend)?;
        library.set_log_callback();
        let gpu_available = library.gpu_available().unwrap_or_else(|error| {
            tracing::warn!(%error, "could not inspect whisper GPU support");
            false
        });
        Ok(Self {
            config,
            library,
            gpu_available,
        })
    }

    /// Whether the loaded runtime reports hardware acceleration.
    #[must_use]
    pub fn gpu_available(&self) -> bool {
        self.gpu_available
    }
}

impl ModelFactory for WhisperModelFactory {
    fn create(&self, mode: DecodeMode) -> Result<Option<Box<dyn Decoder>>, TranscribeError> {
        let (path, progress_name) = match mode {
            DecodeMode::Interim => (&self.config.interim_model, "interim"),
            DecodeMode::Final => {
                let Some(path) = &self.config.final_model else {
                    return Ok(None);
                };
                (path, "final")
            }
            _ => return Ok(None),
        };
        let progress = self
            .config
            .progress
            .as_ref()
            .map(|handle| handle.child(progress_name, 1.0));
        WhisperDecoder::load(&self.library, path, progress.as_ref())
            .map(|decoder| Some(Box::new(decoder) as Box<dyn Decoder>))
    }
}

#[derive(Debug)]
struct WhisperDecoder {
    context: WhisperContext,
    state: WhisperState,
}

impl WhisperDecoder {
    fn load(
        library: &WhisperLibrary,
        path: &Path,
        progress: Option<&ProgressHandle>,
    ) -> Result<Self, TranscribeError> {
        let prewarm_leaf = progress.map(|handle| handle.child("prewarm", 1.0));
        prewarm(path, prewarm_leaf.as_ref())?;
        let init_leaf = progress.map(|handle| handle.child("init", 1.0));
        let context =
            WhisperContext::new(library, path).map_err(|source| load_model_error(path, source))?;
        let state = context
            .create_state()
            .map_err(|source| load_model_error(path, source))?;
        if let Some(leaf) = &init_leaf {
            leaf.complete();
        }
        Ok(Self { context, state })
    }
}

impl Decoder for WhisperDecoder {
    fn decode(&mut self, request: DecodeRequest) -> Result<String, TranscribeError> {
        let final_pass = request.mode() == DecodeMode::Final;
        if final_pass
            && (request.samples().len() < EnginePolicy::MIN_WINDOW_SAMPLES
                || EnginePolicy::is_silence(request.samples()))
        {
            return Ok(String::new());
        }
        let glossary_budget = if final_pass {
            GLOSSARY_TOKEN_BUDGET
        } else {
            MAX_PROMPT_TOKENS
        };
        let glossary = fit_glossary(&self.context, request.guidance(), glossary_budget);
        let prompt = if final_pass {
            Some(final_prompt(
                &self.context,
                glossary.as_deref(),
                request.finalized(),
            ))
        } else {
            glossary
        };
        transcribe_blocking(
            &mut self.state,
            request.samples(),
            prompt.as_deref(),
            !final_pass,
        )
    }
}

fn require_model_file(path: &Path) -> Result<(), TranscribeError> {
    let metadata = std::fs::metadata(path).map_err(|source| load_model_error(path, source))?;
    if metadata.is_file() {
        Ok(())
    } else {
        Err(load_model_error(
            path,
            std::io::Error::other("model path is not a file"),
        ))
    }
}

fn load_model_error(
    path: &Path,
    source: impl std::error::Error + Send + Sync + 'static,
) -> TranscribeError {
    TranscribeError::load_model(path.to_path_buf(), source)
}

fn inference_error(source: impl std::error::Error + Send + Sync + 'static) -> TranscribeError {
    TranscribeError::inference(source)
}

fn prewarm(path: &Path, progress: Option<&ProgressHandle>) -> Result<(), TranscribeError> {
    let total = std::fs::metadata(path)
        .map_err(|source| load_model_error(path, source))?
        .len();
    let mut file = std::fs::File::open(path).map_err(|source| load_model_error(path, source))?;
    let mut buffer = vec![0u8; PREWARM_CHUNK];
    let mut done = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| load_model_error(path, source))?;
        if read == 0 {
            break;
        }
        done += read as u64;
        if let Some(leaf) = progress {
            leaf.set_units(done, total);
        }
    }
    if let Some(leaf) = progress {
        leaf.complete();
    }
    Ok(())
}

fn transcribe_blocking(
    state: &mut WhisperState,
    samples: &[f32],
    prompt: Option<&str>,
    single_segment: bool,
) -> Result<String, TranscribeError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some("en")).map_err(inference_error)?;
    params.set_translate(false);
    params.set_no_context(true);
    params.set_single_segment(single_segment);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    if let Some(prompt) = prompt {
        let prompt = sanitize_prompt(prompt);
        if !prompt.is_empty() {
            params
                .set_initial_prompt(&prompt)
                .map_err(inference_error)?;
        }
    }
    state.full(&params, samples).map_err(inference_error)?;
    let mut text = String::new();
    for segment in 0..state.segment_count() {
        text.push_str(&state.segment_text(segment).map_err(inference_error)?);
    }
    Ok(text.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use shared_progress::ProgressHub;

    use super::*;

    #[test]
    fn prewarm_of_a_plain_file_completes_progress() {
        let directory = tempfile::tempdir().expect("temporary model directory");
        let path = directory.path().join("model.bin");
        std::fs::write(&path, vec![0u8; 1024]).expect("fake model writes");
        let hub = Arc::new(ProgressHub::new());
        let tree = hub.operation();
        let leaf = tree.register("prewarm", 1.0);
        prewarm(&path, Some(&leaf)).expect("prewarm reads the model");
        assert!((leaf.fraction() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn prewarm_failure_is_a_model_error_naming_the_path() {
        let path = Path::new("definitely-missing-prewarm-model.bin");
        let error = prewarm(path, None).expect_err("missing model must fail");
        assert!(matches!(error, TranscribeError::LoadModel { .. }));
        assert!(
            error
                .to_string()
                .contains("definitely-missing-prewarm-model.bin")
        );
    }

    #[test]
    fn missing_interim_model_fails_before_library_loading() {
        let config = WhisperConfig::new(
            "unused-library".into(),
            "definitely-missing-interim-model.bin".into(),
            None,
            None,
        );
        let error = WhisperModelFactory::new(config).expect_err("missing model must fail");
        assert!(matches!(error, TranscribeError::LoadModel { .. }));
        assert!(
            error
                .to_string()
                .contains("definitely-missing-interim-model.bin")
        );
    }

    #[test]
    fn missing_final_model_fails_before_library_loading() {
        let directory = tempfile::tempdir().expect("temporary model directory");
        let interim = directory.path().join("interim.bin");
        std::fs::write(&interim, b"model").expect("interim fixture writes");
        let config = WhisperConfig::new(
            "unused-library".into(),
            interim,
            Some("definitely-missing-final-model.bin".into()),
            None,
        );
        let error = WhisperModelFactory::new(config).expect_err("missing model must fail");
        assert!(matches!(error, TranscribeError::LoadModel { .. }));
        assert!(
            error
                .to_string()
                .contains("definitely-missing-final-model.bin")
        );
    }
}
