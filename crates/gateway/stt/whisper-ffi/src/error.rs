//! Errors from loading and calling whisper.cpp.

use std::ffi::c_int;
use std::io;
use std::path::PathBuf;

/// A failure to load or call the pinned whisper.cpp C API.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WhisperError {
    /// The shared library could not be opened.
    #[non_exhaustive]
    #[error("open whisper library {}", path.display())]
    OpenLibrary {
        /// Path passed to the platform loader.
        path: PathBuf,
        /// Platform loader failure.
        #[source]
        source: io::Error,
    },

    /// A required C symbol is absent from the loaded library.
    #[non_exhaustive]
    #[error("load whisper symbol {symbol}")]
    LoadSymbol {
        /// Missing symbol name.
        symbol: &'static str,
        /// Platform loader failure.
        #[source]
        source: io::Error,
    },

    /// A path cannot be represented by whisper.cpp's narrow C API.
    #[non_exhaustive]
    #[error("whisper model path is not valid UTF-8: {}", path.display())]
    NonUtf8ModelPath {
        /// Model path rejected at the C boundary.
        path: PathBuf,
    },

    /// Text passed to C contains an interior null byte.
    #[non_exhaustive]
    #[error("{value} contains an interior null byte")]
    InteriorNull {
        /// Kind of text rejected at the C boundary.
        value: &'static str,
    },

    /// whisper.cpp returned no context for a model.
    #[non_exhaustive]
    #[error("whisper rejected model {}", path.display())]
    NullContext {
        /// Model path passed to whisper.cpp.
        path: PathBuf,
    },

    /// whisper.cpp could not allocate a state for a valid context.
    #[error("whisper could not create a decoding state")]
    NullState,

    /// A token or sample count does not fit the C API's signed integer.
    #[non_exhaustive]
    #[error("{value} count exceeds the whisper C API limit")]
    CountOverflow {
        /// Kind of count that overflowed.
        value: &'static str,
    },

    /// The caller's token buffer was too small.
    #[non_exhaustive]
    #[error("whisper token buffer needs {required} entries")]
    TokenBufferTooSmall {
        /// Required token capacity reported by whisper.cpp.
        required: usize,
    },

    /// whisper.cpp rejected tokenization for another reason.
    #[non_exhaustive]
    #[error("whisper tokenization failed with code {code}")]
    Tokenize {
        /// Native return code.
        code: c_int,
    },

    /// A full decoding pass failed.
    #[non_exhaustive]
    #[error("whisper inference failed with code {code}")]
    Inference {
        /// Native return code.
        code: c_int,
    },

    /// whisper.cpp returned a null string where text was required.
    #[non_exhaustive]
    #[error("whisper returned no text for segment {segment}")]
    NullSegmentText {
        /// Segment index passed to whisper.cpp.
        segment: c_int,
    },

    /// A segment index falls outside the latest decoding result.
    #[non_exhaustive]
    #[error("whisper segment {segment} is outside 0..{count}")]
    InvalidSegment {
        /// Segment index requested by the caller.
        segment: c_int,
        /// Number of available segments.
        count: c_int,
    },

    /// whisper.cpp returned no system-information string.
    #[error("whisper returned no system information")]
    NullSystemInfo,
}
