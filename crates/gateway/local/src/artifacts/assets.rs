//! Pinned `llama-server` release assets and the platform->asset selection table.

use gateway_config::{LlamaBackend, WhisperBackend};

use super::Result;
use crate::error::LocalError;

mod whisper_auto;
mod whisper_rows;

#[cfg(test)]
mod tests;

use whisper_auto::{auto_whisper_backend, whisper_backend_applies, whisper_row};
use whisper_rows::WHISPER_ASSETS;

/// The `llama.cpp` release tag every managed `llama-server` build is pinned to.
pub(super) const LLAMA_RELEASE: &str = "b10082";
/// The whisper.cpp release tag every managed shared library is pinned to.
pub(super) const WHISPER_RELEASE: &str = "b4938";
/// The x86-64 extensions every x86-64 [`WHISPER_RELEASE`] build executes,
/// in `is_x86_feature_detected!` spelling. They are what whisper.cpp
/// `371b5a75` (the tag's commit) enables under `GGML_NATIVE=OFF`: the
/// `INS_ENB` options in `ggml/CMakeLists.txt` and the x86 flags in
/// `ggml/src/ggml-cpu/CMakeLists.txt`, where MSVC's `/arch:AVX2` implies FMA
/// and F16C. A release bump re-derives the list.
pub(super) const X86_BASELINE: &[&str] = &["sse4.2", "avx", "avx2", "bmi2", "fma", "f16c"];

/// What the host's `nvidia-smi` reported: each GPU's compute capability as
/// `(major, minor)`, and the driver version's major number, `None` when it
/// cannot be read.
#[derive(Debug, Eq, PartialEq)]
pub(super) struct NvidiaProbe {
    pub(super) compute_caps: Vec<(u64, u64)>,
    pub(super) driver_major: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArchiveKind {
    TarGz,
    Zip,
}

/// One downloadable archive of a server asset: a URL with its pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArchiveRef<'a> {
    pub(super) archive_name: &'a str,
    pub(super) url: &'a str,
    pub(super) sha256: &'a str,
    pub(super) archive_kind: ArchiveKind,
}

/// One downloadable file: a URL with an optional pin. Used for GGUF blobs
/// and for each archive of a server asset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FileAsset<'a> {
    pub(super) name: &'a str,
    pub(super) url: &'a str,
    pub(super) sha256: Option<&'a str>,
}

/// A pinned `llama-server` install: one or more archives extracted into the
/// same install folder (the generic CUDA row adds the `cudart` runtime zip
/// beside the server zip), plus the executable the install must contain.
/// `backend` is `Some` only on the Windows x86-64 rows, the one platform
/// with a choice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ServerAsset<'a> {
    pub(super) os: &'a str,
    pub(super) arch: &'a str,
    pub(super) backend: Option<LlamaBackend>,
    pub(super) platform: &'a str,
    pub(super) archives: &'a [ArchiveRef<'a>],
    pub(super) executable_name: &'a str,
}

/// A pinned whisper.cpp runtime archive and its loadable library.
/// `backend` is `Some` only on the Windows x86-64 and Linux x86-64 rows,
/// the two platforms with both a CPU and a CUDA build; the macOS and
/// linux-aarch64 rows are each their platform's one build and carry `None`.
/// `min_driver_major` is the lowest NVIDIA driver major version the build
/// runs on, which only the `auto` pick consults; `None` sets no floor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WhisperAsset<'a> {
    pub(super) os: &'a str,
    arch: &'a str,
    pub(super) backend: Option<WhisperBackend>,
    pub(super) min_driver_major: Option<u64>,
    pub(super) platform: &'a str,
    pub(super) archive: ArchiveRef<'a>,
    pub(super) library_name: &'a str,
}

const WINDOWS_AARCH64_CPU: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "aarch64",
    backend: None,
    platform: "windows-aarch64",
    archives: &[ArchiveRef {
        archive_name: "llama-b10082-bin-win-cpu-arm64.zip",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cpu-arm64.zip",
        sha256: "50dab63396f579cc0ceb4a4fc4b985414d55aaebd4722f363ad03696648711a4",
        archive_kind: ArchiveKind::Zip,
    }],
    executable_name: "llama-server.exe",
};

// The macOS release tars are already Metal-enabled, so both kinds share them.
const MACOS_X86_64: ServerAsset<'static> = ServerAsset {
    os: "macos",
    arch: "x86_64",
    backend: None,
    platform: "macos-x86_64",
    archives: &[ArchiveRef {
        archive_name: "llama-b10082-bin-macos-x64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-x64.tar.gz",
        sha256: "5a28fad0f05bf283c1adb92224c1bf3c25ee06acd0f4065b170016c14b490473",
        archive_kind: ArchiveKind::TarGz,
    }],
    executable_name: "llama-server",
};

const MACOS_AARCH64: ServerAsset<'static> = ServerAsset {
    os: "macos",
    arch: "aarch64",
    backend: None,
    platform: "macos-aarch64",
    archives: &[ArchiveRef {
        archive_name: "llama-b10082-bin-macos-arm64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-arm64.tar.gz",
        sha256: "d644e16eefef3402e4fa86c0fcdce3b00a6786db68c3f216875ce87b45d29173",
        archive_kind: ArchiveKind::TarGz,
    }],
    executable_name: "llama-server",
};

const WINDOWS_X86_64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    backend: Some(LlamaBackend::Vulkan),
    platform: "windows-x86_64-vulkan",
    archives: &[ArchiveRef {
        archive_name: "llama-b10082-bin-win-vulkan-x64.zip",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-vulkan-x64.zip",
        sha256: "0a4b2e41cfb950da9a749baf8978e0626690fbead3b0ca96860785484cda5bde",
        archive_kind: ArchiveKind::Zip,
    }],
    executable_name: "llama-server.exe",
};

// The upstream CUDA 13 build plus its matching runtime zip, extracted into
// the same install folder; the machine then needs only the NVIDIA driver.
const WINDOWS_X86_64_CUDA: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    backend: Some(LlamaBackend::Cuda),
    platform: "windows-x86_64-cuda",
    archives: &[
        ArchiveRef {
            archive_name: "llama-b10082-bin-win-cuda-13.3-x64.zip",
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cuda-13.3-x64.zip",
            sha256: "994c0ebd8acba65cacbe17a7fe41abf634492442afe94d32ddc1f1d078a637b9",
            archive_kind: ArchiveKind::Zip,
        },
        ArchiveRef {
            archive_name: "cudart-llama-bin-win-cuda-13.3-x64.zip",
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/cudart-llama-bin-win-cuda-13.3-x64.zip",
            sha256: "1462a050eb4c684921ba51dcc4cc488a036674c3e73e9945ee705b854808d03e",
            archive_kind: ArchiveKind::Zip,
        },
    ],
    executable_name: "llama-server.exe",
};

// The PromptForge Blackwell build, produced by the llama-cuda-blackwell
// workflow from `crates/build-llama-cuda`. The zip ships the CUDA runtime
// DLLs, so the machine needs only the NVIDIA driver.
//
const WINDOWS_X86_64_CUDA_BLACKWELL: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    backend: Some(LlamaBackend::CudaBlackwell),
    platform: "windows-x86_64-cuda-blackwell",
    archives: &[ArchiveRef {
        archive_name: "llama-server-cuda-blackwell-b10082-win-x64.zip",
        url: "https://github.com/cppalliance/promptforge/releases/download/llama-cuda-blackwell-b10082/llama-server-cuda-blackwell-b10082-win-x64.zip",
        sha256: "10dcd278f0051060bd9adeee75e1d0024e7d19fe359c2df2e20b1ffc7937168c",
        archive_kind: ArchiveKind::Zip,
    }],
    executable_name: "llama-server.exe",
};

const LINUX_X86_64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "x86_64",
    backend: None,
    platform: "linux-x86_64-vulkan",
    archives: &[ArchiveRef {
        archive_name: "llama-b10082-bin-ubuntu-vulkan-x64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-x64.tar.gz",
        sha256: "9003ea32e3d5d8a01da3e4b5d3124e0d21c63d51e112c40f5dcdef91ffaca7cc",
        archive_kind: ArchiveKind::TarGz,
    }],
    executable_name: "llama-server",
};

const LINUX_AARCH64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "aarch64",
    backend: None,
    platform: "linux-aarch64-vulkan",
    archives: &[ArchiveRef {
        archive_name: "llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz",
        url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz",
        sha256: "2805902c3074f615a0105a5325ee29799500c8e29c90ccb986b59e1141df551e",
        archive_kind: ArchiveKind::TarGz,
    }],
    executable_name: "llama-server",
};

// No Vulkan build exists for Windows arm64 in release b10082, so the dev
// table falls back to the CPU archive there.
const DEV_SERVER_ASSETS: &[ServerAsset<'static>] = &[
    WINDOWS_X86_64_VULKAN,
    WINDOWS_X86_64_CUDA,
    WINDOWS_X86_64_CUDA_BLACKWELL,
    WINDOWS_AARCH64_CPU,
    LINUX_X86_64_VULKAN,
    LINUX_AARCH64_VULKAN,
    MACOS_X86_64,
    MACOS_AARCH64,
];

/// The `auto` pick on Windows x86-64: a Blackwell GPU (compute capability
/// 12.x) gets the PromptForge CUDA build, any other NVIDIA GPU gets the
/// upstream CUDA build, and anything else - including a failed probe -
/// gets Vulkan.
fn auto_backend(gpus: Option<&[(u64, u64)]>) -> LlamaBackend {
    match gpus {
        Some(caps) if caps.iter().any(|&(major, _)| major == 12) => LlamaBackend::CudaBlackwell,
        Some(caps) if !caps.is_empty() => LlamaBackend::Cuda,
        _ => LlamaBackend::Vulkan,
    }
}

/// Selects the pinned whisper.cpp runtime for `(os, arch)`.
///
/// `backend` (the `[stt] whisper_backend` setting) and `gpus` (what the
/// NVIDIA probe reported, when a probe was needed and worked) are consulted
/// only on Windows x86-64 and Linux x86-64, the two platforms with a
/// choice; every other platform has exactly one row. On x86-64, under every
/// setting, the selected row needs every [`X86_BASELINE`] extension in
/// `x86_extensions`, the ones the host CPU reports.
///
/// # Errors
/// Returns [`LocalError::UnsupportedPlatform`] when no asset matches the
/// host, and [`LocalError::UnsupportedCpu`] when an x86-64 host lacks a
/// baseline extension.
pub(super) fn whisper_asset(
    os: &str,
    arch: &str,
    backend: WhisperBackend,
    gpus: Option<&NvidiaProbe>,
    x86_extensions: &[&str],
) -> Result<WhisperAsset<'static>> {
    let wanted = if whisper_backend_applies(os, arch) {
        Some(match backend {
            WhisperBackend::Auto => auto_whisper_backend(os, arch, gpus),
            explicit => explicit,
        })
    } else {
        None
    };
    let asset = whisper_row(os, arch, wanted).ok_or_else(|| LocalError::UnsupportedPlatform {
        os: os.to_owned(),
        arch: arch.to_owned(),
    })?;
    if arch == "x86_64" {
        let missing: Vec<String> = X86_BASELINE
            .iter()
            .filter(|extension| !x86_extensions.contains(extension))
            .map(|&extension| extension.to_owned())
            .collect();
        if !missing.is_empty() {
            return Err(LocalError::UnsupportedCpu {
                platform: asset.platform.to_owned(),
                required: X86_BASELINE
                    .iter()
                    .map(|&extension| extension.to_owned())
                    .collect(),
                missing,
            });
        }
    }
    Ok(asset)
}

/// [`whisper_asset`] with the GPU evidence gathered on demand: `probe`
/// (the host's `nvidia-smi` query in production) runs only for `auto` on a
/// platform with both builds, because every explicit backend and every
/// other platform already knows its row. `x86_extensions` passes through.
///
/// # Errors
/// Returns [`LocalError::UnsupportedPlatform`] when no asset matches the
/// host, and [`LocalError::UnsupportedCpu`] when an x86-64 host lacks a
/// baseline extension.
pub(super) fn whisper_asset_with_probe(
    os: &str,
    arch: &str,
    backend: WhisperBackend,
    probe: impl FnOnce() -> Option<NvidiaProbe>,
    x86_extensions: &[&str],
) -> Result<WhisperAsset<'static>> {
    let gpus = if backend == WhisperBackend::Auto && whisper_backend_applies(os, arch) {
        probe()
    } else {
        None
    };
    whisper_asset(os, arch, backend, gpus.as_ref(), x86_extensions)
}

/// Selects the pinned GPU-capable `llama-server` asset for `(os, arch)`.
///
/// `backend` (the `[local] llama_backend` setting) and `gpus` (the probed
/// NVIDIA compute capabilities, when a probe was needed and worked) are
/// consulted only on Windows x86-64, the one platform with a choice; every
/// other platform has exactly one row.
///
/// # Errors
/// Returns [`LocalError::UnsupportedPlatform`] when no asset matches the platform.
pub(super) fn server_asset(
    os: &str,
    arch: &str,
    backend: LlamaBackend,
    gpus: Option<&[(u64, u64)]>,
) -> Result<ServerAsset<'static>> {
    let wanted = if os == "windows" && arch == "x86_64" {
        Some(match backend {
            LlamaBackend::Auto => auto_backend(gpus),
            explicit => explicit,
        })
    } else {
        None
    };
    DEV_SERVER_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch && asset.backend == wanted)
        .ok_or_else(|| LocalError::UnsupportedPlatform {
            os: os.to_owned(),
            arch: arch.to_owned(),
        })
}
