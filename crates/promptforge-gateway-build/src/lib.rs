//! Build-time support for the promptforge-gateway `llama-cuda` feature.
//!
//! Compiles the pinned llama.cpp submodule into a host-native CUDA
//! `llama-server` bundle during the Cargo build, accounts for the PE
//! dependency closure, emits a canonical versioned manifest, and generates
//! the Rust source that embeds the bundle into the gateway binary.

pub mod arch;
pub mod cmake;
pub mod deps;
pub mod manifest;
pub mod probe;
pub mod submodule;
pub mod target;
pub mod toolchain;

mod bundle;

pub use bundle::{BuildReport, build};
