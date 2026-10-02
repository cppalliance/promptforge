//! The whisper `auto` pick on the platforms with both a CPU and a CUDA build.

use gateway_config::WhisperBackend;

use super::{
    CudaMachine, NvidiaProbe, WHISPER_ASSETS, WhisperAsset, cuda_visible_devices_hides_every_gpu,
    libstdcxx_defines,
};

/// The whisper `auto` pick on a platform with both builds: an NVIDIA GPU
/// gets the CUDA build when `CUDA_VISIBLE_DEVICES` leaves a GPU visible, the
/// platform's CUDA row sets no driver floor or the driver meets it, the row
/// lists no native compute capabilities or every probed GPU's is among
/// them, and the row names no `min_glibcxx` or the machine's C++ runtime
/// defines it. Anything else - including a failed probe, GPUs hidden from
/// CUDA, an unreadable driver version under a floor, any GPU without native
/// code, or no runtime read - gets the CPU build. A `None`
/// `cuda_machine` counts as [`CudaMachine::default`].
pub(super) fn auto_whisper_backend(
    os: &str,
    arch: &str,
    gpus: Option<&NvidiaProbe>,
    cuda_machine: Option<&CudaMachine>,
) -> WhisperBackend {
    let Some(probe) = gpus.filter(|probe| !probe.compute_caps.is_empty()) else {
        return WhisperBackend::Cpu;
    };
    let unread = CudaMachine::default();
    let machine = cuda_machine.unwrap_or(&unread);
    if cuda_visible_devices_hides_every_gpu(
        machine.visible_devices.as_deref(),
        probe.compute_caps.len(),
    ) {
        return WhisperBackend::Cpu;
    }
    let cuda = whisper_row(os, arch, Some(WhisperBackend::Cuda));
    let meets_floor = cuda
        .and_then(|cuda| cuda.min_driver_major)
        .is_none_or(|floor| probe.driver_major.is_some_and(|major| major >= floor));
    // Every GPU counts, not only the first: whisper decodes on CUDA's device
    // 0, and CUDA orders devices fastest first, which need not match
    // `nvidia-smi`'s order.
    let all_native = cuda
        .and_then(|cuda| cuda.native_compute_caps)
        .is_none_or(|native| probe.compute_caps.iter().all(|cap| native.contains(cap)));
    let has_runtime = cuda
        .and_then(|cuda| cuda.min_glibcxx)
        .is_none_or(|version| {
            machine
                .libstdcxx
                .as_deref()
                .is_some_and(|library| libstdcxx_defines(library, version))
        });
    if meets_floor && all_native && has_runtime {
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
