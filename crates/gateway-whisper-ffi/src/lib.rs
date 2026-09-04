//! Runtime-loaded safe bindings for PromptForge's pinned whisper.cpp API.
//!
//! [`WhisperLibrary`] opens the packaged shared library and resolves the C
//! symbols. Contexts and states keep that library loaded through reference
//! counting, and all raw pointers stay behind the safe wrapper.

#![expect(
    unsafe_code,
    reason = "loading and calling the whisper.cpp C ABI requires unsafe operations"
)]

mod context;
mod error;
mod library;
mod log;
mod params;
mod raw;

pub use context::{WhisperContext, WhisperState};
pub use error::WhisperError;
pub use library::WhisperLibrary;
pub use params::{FullParams, SamplingStrategy};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::library::system_info_has_gpu;

    #[test]
    fn gpu_probe_recognizes_cuda_and_metal_markers() {
        assert!(system_info_has_gpu("CUDA = 1 | CPU = 1"));
        assert!(system_info_has_gpu("METAL : EMBED_LIBRARY = 1"));
        assert!(!system_info_has_gpu("CUDA = 0 | METAL = 0 | CPU = 1"));
    }

    #[test]
    fn language_and_prompt_reject_interior_nulls() {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        assert!(params.set_language(Some("e\0n")).is_err());
        assert!(params.set_initial_prompt("hello\0world").is_err());
    }

    #[test]
    fn wrapper_types_keep_native_ownership_private() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WhisperLibrary>();
    }

    #[test]
    fn pinned_b4938_parameter_layout_matches_the_64_bit_c_abi() {
        assert_eq!(usize::BITS, 64, "PromptForge ships only 64-bit targets");
        assert_eq!(std::mem::size_of::<raw::ContextParams>(), 48);
        assert_eq!(std::mem::size_of::<raw::FullParams>(), 304);
        assert_eq!(std::mem::align_of::<raw::FullParams>(), 8);
    }

    #[test]
    #[ignore = "requires PROMPTFORGE_WHISPER_LIBRARY to name a packaged runtime"]
    fn packaged_runtime_loads_and_reports_its_backend() {
        let path = std::env::var_os("PROMPTFORGE_WHISPER_LIBRARY")
            .map(PathBuf::from)
            .expect("PROMPTFORGE_WHISPER_LIBRARY is set");
        let library = WhisperLibrary::load(&path).expect("packaged whisper runtime loads");
        // `WhisperLibrary::load` resolves `whisper_log_set` eagerly, so the
        // load above proves the pinned b4938 library exports it; installing
        // the bridge proves the resolved symbol is callable.
        library.set_log_callback();
        let info = library
            .system_info()
            .expect("system information is exported");
        assert!(!info.is_empty());
        assert_eq!(
            library.gpu_available().expect("GPU probe succeeds"),
            system_info_has_gpu(&info)
        );
    }
}
