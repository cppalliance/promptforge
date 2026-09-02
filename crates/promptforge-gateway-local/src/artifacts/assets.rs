//! Pinned `llama-server` release assets and the host->asset selection table.

use promptforge_gateway_config::LlamaBackend;

use super::Result;
use crate::error::LocalError;

/// The `llama.cpp` release tag every managed `llama-server` build is pinned to.
pub(super) const LLAMA_RELEASE: &str = "b10082";

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
// the same install folder; the host then needs only the NVIDIA driver.
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
// workflow from `crates/llama-cuda-build`. The zip ships the CUDA runtime
// DLLs, so the host needs only the NVIDIA driver.
//
const WINDOWS_X86_64_CUDA_BLACKWELL: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    backend: Some(LlamaBackend::CudaBlackwell),
    platform: "windows-x86_64-cuda-blackwell",
    archives: &[ArchiveRef {
        archive_name: "llama-server-cuda-blackwell-b10082-win-x64.zip",
        url: "https://github.com/cppalliance/promptforge/releases/download/llama-cuda-blackwell-b10082/llama-server-cuda-blackwell-b10082-win-x64.zip",
        sha256: "adcdadfc2e3494171ab913671669c6e0008ecead01e04b38dc48b840683493ed",
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

/// Selects the pinned GPU-capable `llama-server` asset for `(os, arch)`.
///
/// `backend` (the `[local] llama_backend` setting) and `gpus` (the probed
/// NVIDIA compute capabilities, when a probe was needed and worked) are
/// consulted only on Windows x86-64, the one platform with a choice; every
/// other platform has exactly one row.
///
/// # Errors
/// Returns [`LocalError::UnsupportedPlatform`] when no asset matches the host.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blackwell_gpus_select_the_blackwell_build() {
        let asset = server_asset("windows", "x86_64", LlamaBackend::Auto, Some(&[(12, 0)]))
            .expect("blackwell asset");
        assert_eq!(asset.platform, "windows-x86_64-cuda-blackwell");
    }

    #[test]
    fn older_nvidia_gpus_select_the_upstream_cuda_build() {
        let asset = server_asset("windows", "x86_64", LlamaBackend::Auto, Some(&[(8, 9)]))
            .expect("cuda asset");
        assert_eq!(asset.platform, "windows-x86_64-cuda");
        assert_eq!(asset.archives.len(), 2);
    }

    #[test]
    fn no_nvidia_gpu_selects_vulkan() {
        for gpus in [None, Some(&[][..])] {
            let asset =
                server_asset("windows", "x86_64", LlamaBackend::Auto, gpus).expect("vulkan asset");
            assert_eq!(asset.platform, "windows-x86_64-vulkan");
        }
    }

    #[test]
    fn an_explicit_backend_needs_no_gpu_evidence() {
        let asset = server_asset("windows", "x86_64", LlamaBackend::CudaBlackwell, None)
            .expect("explicit blackwell asset");
        assert_eq!(asset.platform, "windows-x86_64-cuda-blackwell");
        let asset = server_asset("windows", "x86_64", LlamaBackend::Vulkan, Some(&[(12, 0)]))
            .expect("explicit vulkan asset");
        assert_eq!(asset.platform, "windows-x86_64-vulkan");
    }

    #[test]
    fn non_windows_platforms_ignore_the_backend() {
        let asset = server_asset("linux", "x86_64", LlamaBackend::CudaBlackwell, None)
            .expect("linux asset");
        assert_eq!(asset.platform, "linux-x86_64-vulkan");
        let asset =
            server_asset("macos", "aarch64", LlamaBackend::Auto, None).expect("macos asset");
        assert_eq!(asset.platform, "macos-aarch64");
    }

    #[test]
    fn unsupported_platforms_are_an_error() {
        assert!(server_asset("freebsd", "x86_64", LlamaBackend::Auto, None).is_err());
    }
}
