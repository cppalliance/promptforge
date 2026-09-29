//! The whisper `auto` pick on the platforms with both a CPU and a CUDA build.

use gateway_config::WhisperBackend;

use super::{NvidiaProbe, WHISPER_ASSETS, WhisperAsset};

/// The whisper `auto` pick on a platform with both builds: an NVIDIA GPU
/// gets the CUDA build when the platform's CUDA row sets no driver floor or
/// the driver meets it, and anything else - including a failed probe or an
/// unreadable driver version under a floor - gets the CPU build.
pub(super) fn auto_whisper_backend(
    os: &str,
    arch: &str,
    gpus: Option<&NvidiaProbe>,
) -> WhisperBackend {
    let Some(probe) = gpus.filter(|probe| !probe.compute_caps.is_empty()) else {
        return WhisperBackend::Cpu;
    };
    let floor =
        whisper_row(os, arch, Some(WhisperBackend::Cuda)).and_then(|cuda| cuda.min_driver_major);
    if floor.is_none_or(|floor| probe.driver_major.is_some_and(|major| major >= floor)) {
        WhisperBackend::Cuda
    } else {
        WhisperBackend::Cpu
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

/// The whisper row for `(os, arch)` and `backend`, which is `None` on a
/// platform with one build.
pub(super) fn whisper_row(
    os: &str,
    arch: &str,
    backend: Option<WhisperBackend>,
) -> Option<WhisperAsset<'static>> {
    WHISPER_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch && asset.backend == backend)
}
