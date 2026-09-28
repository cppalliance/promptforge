//! The whisper `auto` pick on the platforms with both a CPU and a CUDA build.

use gateway_config::WhisperBackend;

use super::WHISPER_ASSETS;

/// The whisper `auto` pick on a platform with both builds: any NVIDIA GPU
/// gets the CUDA build, and anything else - including a failed probe - gets
/// the CPU build.
pub(super) fn auto_whisper_backend(gpus: Option<&[(u64, u64)]>) -> WhisperBackend {
    match gpus {
        Some(caps) if !caps.is_empty() => WhisperBackend::Cuda,
        _ => WhisperBackend::Cpu,
    }
}

/// Whether `(os, arch)` has both a CPU and a CUDA whisper build (Windows
/// x86-64 and Linux x86-64), so the `[stt] whisper_backend` setting and the
/// GPU probe apply there.
pub(super) fn whisper_backend_applies(os: &str, arch: &str) -> bool {
    WHISPER_ASSETS
        .iter()
        .any(|asset| asset.os == os && asset.arch == arch && asset.backend.is_some())
}
