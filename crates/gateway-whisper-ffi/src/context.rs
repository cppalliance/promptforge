//! Safe whisper context and decoding-state ownership.

use std::ffi::{CStr, CString, c_int};
use std::fmt;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use crate::library::{LibraryInner, WhisperLibrary};
use crate::raw;
use crate::{FullParams, WhisperError};

struct ContextInner {
    pointer: NonNull<raw::Context>,
    library: Arc<LibraryInner>,
}

impl fmt::Debug for ContextInner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextInner")
            .field("pointer", &self.pointer)
            .finish_non_exhaustive()
    }
}

impl Drop for ContextInner {
    fn drop(&mut self) {
        // SAFETY: pointer came from whisper_init_from_file_with_params, has not
        // been freed, and the library handle remains live in self.library.
        unsafe {
            (self.library.functions.free)(self.pointer.as_ptr());
        }
    }
}

/// A loaded whisper model context.
#[derive(Debug)]
pub struct WhisperContext {
    inner: Rc<ContextInner>,
}

impl WhisperContext {
    /// Loads `model` through `library`.
    ///
    /// # Errors
    /// Returns [`WhisperError::NonUtf8ModelPath`] or
    /// [`WhisperError::InteriorNull`] when the path cannot cross the narrow C
    /// boundary, or [`WhisperError::NullContext`] when whisper rejects it.
    pub fn new(library: &WhisperLibrary, model: &Path) -> Result<Self, WhisperError> {
        let Some(model_text) = model.to_str() else {
            return Err(WhisperError::NonUtf8ModelPath {
                path: model.to_path_buf(),
            });
        };
        let model_text = CString::new(model_text).map_err(|_| WhisperError::InteriorNull {
            value: "whisper model path",
        })?;
        // SAFETY: the function pointer matches b4938, model_text is a live
        // null-terminated path, and params came from the same loaded library.
        let pointer = unsafe {
            let params = (library.inner.functions.context_default_params)();
            (library.inner.functions.init_from_file_with_params)(model_text.as_ptr(), params)
        };
        let Some(pointer) = NonNull::new(pointer) else {
            return Err(WhisperError::NullContext {
                path: model.to_path_buf(),
            });
        };
        Ok(Self {
            inner: Rc::new(ContextInner {
                pointer,
                library: Arc::clone(&library.inner),
            }),
        })
    }

    /// Allocates an independent decoding state for this context.
    ///
    /// # Errors
    /// Returns [`WhisperError::NullState`] when whisper cannot allocate it.
    pub fn create_state(&self) -> Result<WhisperState, WhisperError> {
        // SAFETY: the context pointer is live for this call and for the state
        // because WhisperState retains an Rc to ContextInner.
        let pointer =
            unsafe { (self.inner.library.functions.init_state)(self.inner.pointer.as_ptr()) };
        let Some(pointer) = NonNull::new(pointer) else {
            return Err(WhisperError::NullState);
        };
        Ok(WhisperState {
            pointer,
            context: Rc::clone(&self.inner),
        })
    }

    /// Tokenizes `text` into at most `max` token identifiers.
    ///
    /// # Errors
    /// Returns [`WhisperError::InteriorNull`] for embedded null bytes,
    /// [`WhisperError::CountOverflow`] when `max` exceeds the C integer range,
    /// [`WhisperError::TokenBufferTooSmall`] when whisper needs more entries,
    /// or [`WhisperError::Tokenize`] for another native failure.
    pub fn tokenize(&self, text: &str, max: usize) -> Result<Vec<c_int>, WhisperError> {
        let max_c =
            c_int::try_from(max).map_err(|_| WhisperError::CountOverflow { value: "token" })?;
        let text = CString::new(text).map_err(|_| WhisperError::InteriorNull {
            value: "tokenization text",
        })?;
        let mut tokens = vec![0; max];
        // SAFETY: text is null-terminated, tokens has max_c writable entries,
        // and the context pointer is live for the duration of the call.
        let count = unsafe {
            (self.inner.library.functions.tokenize)(
                self.inner.pointer.as_ptr(),
                text.as_ptr(),
                tokens.as_mut_ptr(),
                max_c,
            )
        };
        if count < 0 {
            let required = count
                .checked_abs()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(WhisperError::Tokenize { code: count })?;
            return Err(WhisperError::TokenBufferTooSmall { required });
        }
        let count = usize::try_from(count).map_err(|_| WhisperError::Tokenize { code: count })?;
        if count > max {
            return Err(WhisperError::TokenBufferTooSmall { required: count });
        }
        tokens.truncate(count);
        Ok(tokens)
    }
}

/// A whisper decoding state tied to its model context.
#[derive(Debug)]
pub struct WhisperState {
    pointer: NonNull<raw::State>,
    context: Rc<ContextInner>,
}

impl WhisperState {
    /// Runs one full decoding pass over 16 kHz floating-point PCM.
    ///
    /// # Errors
    /// Returns [`WhisperError::CountOverflow`] when the sample count exceeds
    /// the C integer range, or [`WhisperError::Inference`] when whisper rejects
    /// the pass.
    pub fn full(&mut self, params: &FullParams, samples: &[f32]) -> Result<(), WhisperError> {
        let count = c_int::try_from(samples.len())
            .map_err(|_| WhisperError::CountOverflow { value: "sample" })?;
        let strategy = params.strategy.native_value();
        // SAFETY: this function pointer and returned value use the pinned
        // b4938 ABI and belong to the same live library as the state.
        let mut native = unsafe { (self.context.library.functions.full_default_params)(strategy) };
        params.apply(&mut native);
        // SAFETY: context and state are live and paired, native's borrowed
        // string pointers remain owned by params through this call, and
        // samples contains count readable f32 values.
        let code = unsafe {
            (self.context.library.functions.full_with_state)(
                self.context.pointer.as_ptr(),
                self.pointer.as_ptr(),
                native,
                samples.as_ptr(),
                count,
            )
        };
        if code == 0 {
            Ok(())
        } else {
            Err(WhisperError::Inference { code })
        }
    }

    /// Returns the number of text segments from the most recent pass.
    #[must_use]
    pub fn segment_count(&self) -> c_int {
        // SAFETY: the state pointer is live and the function only reads the
        // result metadata owned by that state.
        unsafe {
            (self.context.library.functions.full_n_segments_from_state)(self.pointer.as_ptr())
        }
    }

    /// Copies one segment's text from the most recent pass.
    ///
    /// # Errors
    /// Returns [`WhisperError::InvalidSegment`] when `segment` is outside the
    /// latest result, or [`WhisperError::NullSegmentText`] when whisper returns
    /// a null pointer for a valid index.
    pub fn segment_text(&self, segment: c_int) -> Result<String, WhisperError> {
        let count = self.segment_count();
        if segment < 0 || segment >= count {
            return Err(WhisperError::InvalidSegment { segment, count });
        }
        // SAFETY: the state pointer is live and segment is passed through for
        // a range checked against the state's most recent result.
        let text = unsafe {
            (self
                .context
                .library
                .functions
                .full_get_segment_text_from_state)(self.pointer.as_ptr(), segment)
        };
        let Some(text) = NonNull::new(text.cast_mut()) else {
            return Err(WhisperError::NullSegmentText { segment });
        };
        // SAFETY: whisper returns a null-terminated state-owned string and it
        // is copied before the state can be used again.
        let text = unsafe { CStr::from_ptr(text.as_ptr()) };
        Ok(text.to_string_lossy().into_owned())
    }
}

impl Drop for WhisperState {
    fn drop(&mut self) {
        // SAFETY: pointer came from whisper_init_state, has not been freed,
        // and self.context keeps its originating context and library live.
        unsafe {
            (self.context.library.functions.free_state)(self.pointer.as_ptr());
        }
    }
}
