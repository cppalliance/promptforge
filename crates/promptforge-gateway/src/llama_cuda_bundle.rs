//! Embedded CUDA llama.cpp bundle produced by the build script.
//!
//! Present only on native Windows x86-64 builds with the `llama-cuda`
//! feature; the runtime staging module consumes `MANIFEST` and `FILES`.

include!(concat!(env!("OUT_DIR"), "/llama_cuda_bundle.rs"));
