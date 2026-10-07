//! Safe ownership of whisper.cpp's streaming Silero speech detector.

use std::ffi::{CString, c_int};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::WhisperError;
use crate::library::{LibraryInner, WhisperLibrary};
use crate::raw;

/// A loaded Silero speech-detection model and its streaming LSTM state.
///
/// Every call continues from the state the previous call left, so one context
/// follows one audio stream until [`reset`](Self::reset).
pub struct VadContext {
    pointer: NonNull<raw::VadContext>,
    library: Arc<LibraryInner>,
}

impl fmt::Debug for VadContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VadContext")
            .field("pointer", &self.pointer)
            .finish_non_exhaustive()
    }
}

// SAFETY: the native context is a heap object on the CPU backend with no
// thread affinity, and every native call goes through `&mut self` or Drop, so
// moving it hands exclusive use to the receiving thread. It is not Sync:
// whisper.cpp rewrites its graph, probability buffer, and LSTM state on
// every detection call.
unsafe impl Send for VadContext {}

impl VadContext {
    /// Samples in one Silero window of 16 kHz PCM.
    pub const CHUNK_SAMPLES: usize = 512;

    /// Loads the Silero model at `model` through `library`, computing on one
    /// CPU thread.
    ///
    /// whisper.cpp reads the file without validating it, and a malformed
    /// model can throw a C++ exception across this boundary, so callers
    /// verify the pinned digest first.
    ///
    /// # Errors
    /// Returns [`WhisperError::NonUtf8ModelPath`] or
    /// [`WhisperError::InteriorNull`] when the path cannot cross the narrow C
    /// boundary, or [`WhisperError::NullVadContext`] when whisper rejects it.
    pub fn new(library: &WhisperLibrary, model: &Path) -> Result<Self, WhisperError> {
        let Some(model_text) = model.to_str() else {
            return Err(WhisperError::NonUtf8ModelPath {
                path: model.to_path_buf(),
            });
        };
        let model_text = CString::new(model_text).map_err(|source| WhisperError::InteriorNull {
            value: "VAD model path",
            source,
        })?;
        // SAFETY: the function pointer matches b4938 and takes no arguments.
        let mut native = unsafe { (library.inner.functions.vad_default_context_params)() };
        // Detection computes on the CPU regardless, and a GPU request aborts
        // CUDA builds (https://github.com/ggml-org/whisper.cpp/issues/3508).
        native.use_gpu = false;
        native.n_threads = 1;
        // SAFETY: the function pointer matches b4938, model_text is a live
        // null-terminated path, and native came from the same loaded library.
        let pointer = unsafe {
            (library.inner.functions.vad_init_from_file_with_params)(model_text.as_ptr(), native)
        };
        let Some(pointer) = NonNull::new(pointer) else {
            return Err(WhisperError::NullVadContext {
                path: model.to_path_buf(),
            });
        };
        let mut context = Self {
            pointer,
            library: Arc::clone(&library.inner),
        };
        // whisper.cpp allocates the LSTM state without clearing it.
        context.reset();
        Ok(context)
    }

    /// Classifies one [`CHUNK_SAMPLES`](Self::CHUNK_SAMPLES) chunk of 16 kHz
    /// PCM, continuing the streaming state, and returns its speech
    /// probability.
    ///
    /// # Errors
    /// Returns [`WhisperError::VadChunkLength`] unless `chunk` is exactly one
    /// window, [`WhisperError::VadDetect`] when whisper cannot allocate its
    /// compute graph, or [`WhisperError::VadProbabilityCount`] when whisper
    /// reports other than one probability.
    pub fn detect_chunk(&mut self, chunk: &[f32]) -> Result<f32, WhisperError> {
        if chunk.len() != Self::CHUNK_SAMPLES {
            return Err(WhisperError::VadChunkLength {
                samples: chunk.len(),
            });
        }
        Ok(self.detect(chunk)?[0])
    }

    /// Classifies every [`CHUNK_SAMPLES`](Self::CHUNK_SAMPLES) window of
    /// 16 kHz PCM, zero-padding a final partial window and continuing the
    /// streaming state, and returns one speech probability per window.
    ///
    /// # Errors
    /// Returns [`WhisperError::CountOverflow`] when the sample count exceeds
    /// the C integer range, [`WhisperError::VadDetect`] when whisper cannot
    /// allocate its compute graph, or [`WhisperError::VadProbabilityCount`]
    /// when whisper reports other than one probability per window.
    pub fn probabilities(&mut self, samples: &[f32]) -> Result<Vec<f32>, WhisperError> {
        self.detect(samples).map(<[f32]>::to_vec)
    }

    /// Clears the streaming state so the next call starts a new stream.
    pub fn reset(&mut self) {
        // SAFETY: the context pointer is live and exclusively borrowed.
        unsafe {
            (self.library.functions.vad_reset_state)(self.pointer.as_ptr());
        }
    }

    /// Runs detection and borrows whisper's probability buffer, which the
    /// next call on this context rebuilds.
    fn detect(&mut self, samples: &[f32]) -> Result<&[f32], WhisperError> {
        let count = c_int::try_from(samples.len())
            .map_err(|_| WhisperError::CountOverflow { value: "sample" })?;
        let expected = samples.len().div_ceil(Self::CHUNK_SAMPLES);
        // SAFETY: the context pointer is live and exclusively borrowed, and
        // samples holds count readable f32 values for the whole call.
        let detected = unsafe {
            (self.library.functions.vad_detect_speech_no_reset)(
                self.pointer.as_ptr(),
                samples.as_ptr(),
                count,
            )
        };
        if !detected {
            return Err(WhisperError::VadDetect);
        }
        // SAFETY: the context pointer is live and the call only reads the
        // probability count.
        let actual = unsafe { (self.library.functions.vad_n_probs)(self.pointer.as_ptr()) };
        if usize::try_from(actual).ok() != Some(expected) {
            return Err(WhisperError::VadProbabilityCount { expected, actual });
        }
        if expected == 0 {
            return Ok(&[]);
        }
        // SAFETY: the context pointer is live and the call only reads the
        // probability buffer's address.
        let probabilities = unsafe { (self.library.functions.vad_probs)(self.pointer.as_ptr()) };
        let Some(probabilities) = NonNull::new(probabilities) else {
            return Err(WhisperError::VadDetect);
        };
        // SAFETY: whisper holds `expected` initialized floats at this address
        // until the next detection call, and the returned slice borrows self
        // mutably, so no call can rebuild the buffer while it is alive.
        Ok(unsafe { std::slice::from_raw_parts(probabilities.as_ptr(), expected) })
    }
}

impl Drop for VadContext {
    fn drop(&mut self) {
        // SAFETY: pointer came from whisper_vad_init_from_file_with_params,
        // has not been freed, and self.library keeps the library loaded.
        unsafe {
            (self.library.functions.vad_free)(self.pointer.as_ptr());
        }
    }
}

#[cfg(test)]
#[path = "vad-tests.rs"]
mod tests;
