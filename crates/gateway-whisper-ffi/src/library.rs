//! Dynamic library ownership and symbol resolution.

use std::ffi::CStr;
use std::fmt;
use std::io;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::WhisperError;
use crate::log;
use crate::raw;

#[derive(Clone, Copy)]
pub(crate) struct Functions {
    pub(crate) context_default_params: raw::ContextDefaultParams,
    pub(crate) init_from_file_with_params: raw::InitFromFileWithParams,
    pub(crate) init_state: raw::InitState,
    pub(crate) tokenize: raw::Tokenize,
    pub(crate) full_default_params: raw::FullDefaultParams,
    pub(crate) full_with_state: raw::FullWithState,
    pub(crate) full_n_segments_from_state: raw::FullNSegmentsFromState,
    pub(crate) full_get_segment_text_from_state: raw::FullGetSegmentTextFromState,
    print_system_info: raw::PrintSystemInfo,
    pub(crate) free: raw::Free,
    pub(crate) free_state: raw::FreeState,
    log_set: raw::LogSet,
}

impl Functions {
    fn load(library: &libloading::Library) -> Result<Self, WhisperError> {
        Ok(Self {
            context_default_params: load_symbol(
                library,
                b"whisper_context_default_params\0",
                "whisper_context_default_params",
            )?,
            init_from_file_with_params: load_symbol(
                library,
                b"whisper_init_from_file_with_params\0",
                "whisper_init_from_file_with_params",
            )?,
            init_state: load_symbol(library, b"whisper_init_state\0", "whisper_init_state")?,
            tokenize: load_symbol(library, b"whisper_tokenize\0", "whisper_tokenize")?,
            full_default_params: load_symbol(
                library,
                b"whisper_full_default_params\0",
                "whisper_full_default_params",
            )?,
            full_with_state: load_symbol(
                library,
                b"whisper_full_with_state\0",
                "whisper_full_with_state",
            )?,
            full_n_segments_from_state: load_symbol(
                library,
                b"whisper_full_n_segments_from_state\0",
                "whisper_full_n_segments_from_state",
            )?,
            full_get_segment_text_from_state: load_symbol(
                library,
                b"whisper_full_get_segment_text_from_state\0",
                "whisper_full_get_segment_text_from_state",
            )?,
            print_system_info: load_symbol(
                library,
                b"whisper_print_system_info\0",
                "whisper_print_system_info",
            )?,
            free: load_symbol(library, b"whisper_free\0", "whisper_free")?,
            free_state: load_symbol(library, b"whisper_free_state\0", "whisper_free_state")?,
            log_set: load_symbol(library, b"whisper_log_set\0", "whisper_log_set")?,
        })
    }
}

fn load_symbol<T: Copy>(
    library: &libloading::Library,
    name: &'static [u8],
    display: &'static str,
) -> Result<T, WhisperError> {
    // SAFETY: T is the pinned b4938 function-pointer declaration for `name`,
    // and the returned pointer is copied while `library` remains owned.
    let symbol = unsafe { library.get::<T>(name) }.map_err(|source| WhisperError::LoadSymbol {
        symbol: display,
        source: io::Error::other(source),
    })?;
    Ok(*symbol)
}

pub(crate) struct LibraryInner {
    pub(crate) functions: Functions,
    _library: libloading::Library,
}

impl fmt::Debug for LibraryInner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryInner")
            .finish_non_exhaustive()
    }
}

/// One loaded whisper.cpp runtime.
///
/// Clones share the same platform library handle. Contexts and states keep
/// their own clone, so the native code cannot unload while a raw pointer
/// remains live.
#[derive(Clone, Debug)]
pub struct WhisperLibrary {
    pub(crate) inner: Arc<LibraryInner>,
}

impl WhisperLibrary {
    /// Opens `path` and resolves every C symbol PromptForge uses.
    ///
    /// # Errors
    /// Returns [`WhisperError::OpenLibrary`] when the platform loader rejects
    /// the file, or [`WhisperError::LoadSymbol`] when the library is not the
    /// pinned whisper.cpp ABI.
    pub fn load(path: &Path) -> Result<Self, WhisperError> {
        let library = open_library(path).map_err(|source| WhisperError::OpenLibrary {
            path: path.to_path_buf(),
            source,
        })?;
        let functions = Functions::load(&library)?;
        Ok(Self {
            inner: Arc::new(LibraryInner {
                functions,
                _library: library,
            }),
        })
    }

    /// Returns whisper.cpp's native system-information line.
    ///
    /// # Errors
    /// Returns [`WhisperError::NullSystemInfo`] if the native library violates
    /// its contract and returns a null pointer.
    pub fn system_info(&self) -> Result<String, WhisperError> {
        // SAFETY: the function takes no arguments and returns a library-owned,
        // null-terminated string whose bytes are copied before this call ends.
        let text = unsafe { (self.inner.functions.print_system_info)() };
        let Some(text) = NonNull::new(text.cast_mut()) else {
            return Err(WhisperError::NullSystemInfo);
        };
        // SAFETY: whisper_print_system_info promises a null-terminated static
        // string; NonNull established that the pointer is not null.
        let text = unsafe { CStr::from_ptr(text.as_ptr()) };
        Ok(text.to_string_lossy().into_owned())
    }

    /// Reports whether the loaded runtime was built with CUDA or Metal.
    ///
    /// # Errors
    /// Returns [`WhisperError::NullSystemInfo`] if the native probe returns no
    /// system-information string.
    pub fn gpu_available(&self) -> Result<bool, WhisperError> {
        Ok(system_info_has_gpu(&self.system_info()?))
    }

    /// Installs the process-wide bridge routing whisper.cpp and ggml log
    /// output into `tracing` events at the `whisper_cpp` target, replacing
    /// the default `stderr` writes.
    ///
    /// whisper.cpp holds one global callback, so repeated calls are
    /// idempotent: every call installs the same static function.
    pub fn set_log_callback(&self) {
        // SAFETY: `log::TRACING_BRIDGE` is an `extern "C" fn` with the pinned
        // b4938 `ggml_log_callback` signature; it never unwinds and reads no
        // user_data, so a null user_data pointer is valid.
        unsafe {
            (self.inner.functions.log_set)(log::TRACING_BRIDGE, std::ptr::null_mut());
        }
    }
}

#[cfg(target_os = "windows")]
fn open_library(path: &Path) -> Result<libloading::Library, io::Error> {
    use libloading::os::windows::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32};

    let absolute = path.canonicalize()?;
    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    // SAFETY: the absolute path satisfies LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR;
    // LibraryInner retains the handle for every copied symbol and native
    // context, and dependencies resolve only beside it or from System32.
    let library = unsafe { libloading::os::windows::Library::load_with_flags(&absolute, flags) }
        .map_err(io::Error::other)?;
    Ok(library.into())
}

#[cfg(not(target_os = "windows"))]
fn open_library(path: &Path) -> Result<libloading::Library, io::Error> {
    // SAFETY: LibraryInner retains the handle for at least as long as every
    // copied symbol and every native context using those symbols.
    unsafe { libloading::Library::new(path) }.map_err(io::Error::other)
}

pub(crate) fn system_info_has_gpu(info: &str) -> bool {
    let info = info.to_ascii_uppercase();
    ["CUDA = 1", "CUDA :", "METAL = 1", "METAL :"]
        .iter()
        .any(|marker| info.contains(marker))
}
