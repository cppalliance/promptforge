//! CUDA `llama-server` release builder.
//!
//! Compiles a llama.cpp checkout into a host-native CUDA `llama-server`
//! with CMake, accounts for the PE dependency closure, copies the CUDA
//! runtime DLLs the executable imports (so the end user needs only the
//! NVIDIA driver, not the CUDA Toolkit), emits a canonical versioned
//! manifest, and packs everything into a release zip with a checksum.
//!
//! The command-line entry point lives in `main.rs`; the pipeline here is
//! library code so its tests can drive it through the [`probe`] seam.

pub mod arch;
pub mod cmake;
pub mod deps;
pub mod manifest;
pub mod probe;
pub mod toolchain;

mod bundle;

pub use bundle::{BuildOutcome, BuildRequest, build};
