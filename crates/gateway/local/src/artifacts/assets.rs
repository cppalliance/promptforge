//! Pinned `llama-server` release assets and the host->asset selection table.

use gateway_config::{LlamaBackend, WhisperBackend};

use super::Result;
use crate::error::LocalError;

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
    pub(super) arch: &'a str,
    pub(super) backend: Option<WhisperBackend>,
    pub(super) min_driver_major: Option<u64>,
    pub(super) platform: &'a str,
    pub(super) archive: ArchiveRef<'a>,
    pub(super) library_name: &'a str,
}

const WHISPER_ASSETS: &[WhisperAsset<'static>] = &[
    WhisperAsset {
        os: "windows",
        arch: "x86_64",
        backend: Some(WhisperBackend::Cpu),
        min_driver_major: None,
        platform: "windows-x86_64",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-windows-x86_64.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-windows-x86_64.zip",
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "whisper.dll",
    },
    WhisperAsset {
        os: "windows",
        arch: "x86_64",
        backend: Some(WhisperBackend::Cuda),
        min_driver_major: None,
        platform: "windows-x86_64-cuda",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-windows-x86_64-cuda.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-windows-x86_64-cuda.zip",
            sha256: "f1bc54d7288e21ee826ccb5767249836b780fc316bec4a0374873e73163dae12",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "whisper.dll",
    },
    WhisperAsset {
        os: "macos",
        arch: "aarch64",
        backend: None,
        min_driver_major: None,
        platform: "macos-aarch64-metal",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-macos-aarch64-metal.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-macos-aarch64-metal.zip",
            sha256: "2315c758f1a7a0a8a98e887d1b49b2418c1e95e75e12dd063472d855bfbe2f78",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.dylib",
    },
    WhisperAsset {
        os: "macos",
        arch: "x86_64",
        backend: None,
        min_driver_major: None,
        platform: "macos-x86_64",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-macos-x86_64.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-macos-x86_64.zip",
            sha256: "425664a05f844683bc1c9c26c52311cfc6546b9f72f3ce8fd4f096c5e93df22b",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.dylib",
    },
    WhisperAsset {
        os: "linux",
        arch: "x86_64",
        backend: Some(WhisperBackend::Cpu),
        min_driver_major: None,
        platform: "linux-x86_64",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-linux-x86_64.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-linux-x86_64.zip",
            sha256: "0dc1a6adc29bfaecb6c2c8c8fc9ec2f903b25e6bfadd67bbdb239521f9101155",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.so",
    },
    WhisperAsset {
        os: "linux",
        arch: "x86_64",
        backend: Some(WhisperBackend::Cuda),
        // CUDA 12.8 runs on Linux driver 570 or later.
        min_driver_major: Some(570),
        platform: "linux-x86_64-cuda",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-linux-x86_64-cuda.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-linux-x86_64-cuda.zip",
            sha256: "0000000000000000000000000000000000000000000000000000000000000000",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.so",
    },
    WhisperAsset {
        os: "linux",
        arch: "aarch64",
        backend: None,
        min_driver_major: None,
        platform: "linux-aarch64",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-linux-aarch64.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-linux-aarch64.zip",
            sha256: "1400ed00171e15596838ce839e5e90fab176f8653ead5b940dfda36bc5e68fc3",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.so",
    },
];

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
// workflow from `crates/build-llama-cuda`. The zip ships the CUDA runtime
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

/// The whisper `auto` pick on a platform with both builds: an NVIDIA GPU
/// gets the CUDA build when the platform's CUDA row sets no driver floor or
/// the driver meets it, and anything else - including a failed probe or an
/// unreadable driver version under a floor - gets the CPU build.
fn auto_whisper_backend(os: &str, arch: &str, gpus: Option<&NvidiaProbe>) -> WhisperBackend {
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
fn whisper_backend_applies(os: &str, arch: &str) -> bool {
    WHISPER_ASSETS
        .iter()
        .any(|asset| asset.os == os && asset.arch == arch && asset.backend.is_some())
}

/// The whisper row for `(os, arch)` and `backend`, which is `None` on a
/// platform with one build.
fn whisper_row(
    os: &str,
    arch: &str,
    backend: Option<WhisperBackend>,
) -> Option<WhisperAsset<'static>> {
    WHISPER_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch && asset.backend == backend)
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

    /// Every `[stt] whisper_backend` value.
    const WHISPER_BACKENDS: [WhisperBackend; 3] = [
        WhisperBackend::Auto,
        WhisperBackend::Cpu,
        WhisperBackend::Cuda,
    ];

    /// A probe answer that reports one NVIDIA GPU on driver `driver_major`.
    fn nvidia(driver_major: Option<u64>) -> NvidiaProbe {
        NvidiaProbe {
            compute_caps: vec![(8, 6)],
            driver_major,
        }
    }

    /// A probe answer with two RTX 3090s on driver 591, above every floor.
    fn rtx_3090s() -> NvidiaProbe {
        NvidiaProbe {
            compute_caps: vec![(8, 6), (8, 6)],
            driver_major: Some(591),
        }
    }

    /// A probe answer that names no GPU.
    fn no_gpu() -> NvidiaProbe {
        NvidiaProbe {
            compute_caps: Vec::new(),
            driver_major: Some(591),
        }
    }

    #[test]
    fn an_nvidia_gpu_selects_the_cuda_whisper_build() {
        for (os, platform) in [
            ("windows", "windows-x86_64-cuda"),
            ("linux", "linux-x86_64-cuda"),
        ] {
            for caps in [vec![(8, 6)], vec![(12, 0)]] {
                let probe = NvidiaProbe {
                    compute_caps: caps,
                    driver_major: Some(591),
                };
                let asset = whisper_asset(
                    os,
                    "x86_64",
                    WhisperBackend::Auto,
                    Some(&probe),
                    X86_BASELINE,
                )
                .expect("cuda whisper asset");
                assert_eq!(asset.platform, platform, "{probe:?}");
            }
        }
    }

    #[test]
    fn no_nvidia_gpu_selects_the_cpu_whisper_build() {
        let empty = no_gpu();
        for (os, platform) in [("windows", "windows-x86_64"), ("linux", "linux-x86_64")] {
            for gpus in [None, Some(&empty)] {
                let asset = whisper_asset(os, "x86_64", WhisperBackend::Auto, gpus, X86_BASELINE)
                    .expect("cpu whisper asset");
                assert_eq!(asset.platform, platform, "{gpus:?}");
            }
        }
    }

    #[test]
    fn auto_takes_the_linux_cuda_whisper_build_from_driver_570() {
        for (driver_major, platform) in [
            (None, "linux-x86_64"),
            (Some(0), "linux-x86_64"),
            (Some(569), "linux-x86_64"),
            (Some(570), "linux-x86_64-cuda"),
            (Some(591), "linux-x86_64-cuda"),
        ] {
            let probe = nvidia(driver_major);
            let asset = whisper_asset(
                "linux",
                "x86_64",
                WhisperBackend::Auto,
                Some(&probe),
                X86_BASELINE,
            )
            .expect("auto linux whisper asset");
            assert_eq!(asset.platform, platform, "driver {driver_major:?}");
        }
    }

    #[test]
    fn auto_takes_the_windows_cuda_whisper_build_at_any_driver_version() {
        for driver_major in [None, Some(0), Some(569), Some(570), Some(591)] {
            let probe = nvidia(driver_major);
            let asset = whisper_asset(
                "windows",
                "x86_64",
                WhisperBackend::Auto,
                Some(&probe),
                X86_BASELINE,
            )
            .expect("auto windows whisper asset");
            assert_eq!(
                asset.platform, "windows-x86_64-cuda",
                "driver {driver_major:?}"
            );
        }
    }

    #[test]
    fn an_explicit_whisper_backend_ignores_the_probe() {
        // The drivers below the Linux CUDA floor are included: an explicit
        // `cuda` is honored there.
        let probes = [
            None,
            Some(no_gpu()),
            Some(rtx_3090s()),
            Some(nvidia(Some(569))),
            Some(nvidia(None)),
        ];
        for os in ["windows", "linux"] {
            for gpus in &probes {
                let cpu = whisper_asset(
                    os,
                    "x86_64",
                    WhisperBackend::Cpu,
                    gpus.as_ref(),
                    X86_BASELINE,
                )
                .expect("explicit cpu whisper asset");
                assert_eq!(cpu.platform, format!("{os}-x86_64"), "{gpus:?}");
                let cuda = whisper_asset(
                    os,
                    "x86_64",
                    WhisperBackend::Cuda,
                    gpus.as_ref(),
                    X86_BASELINE,
                )
                .expect("explicit cuda whisper asset");
                assert_eq!(cuda.platform, format!("{os}-x86_64-cuda"), "{gpus:?}");
            }
        }
    }

    #[test]
    fn single_build_whisper_platforms_ignore_the_backend() {
        let probe = rtx_3090s();
        for (os, arch, platform) in [
            ("macos", "aarch64", "macos-aarch64-metal"),
            ("macos", "x86_64", "macos-x86_64"),
            ("linux", "aarch64", "linux-aarch64"),
        ] {
            for backend in WHISPER_BACKENDS {
                for gpus in [None, Some(&probe)] {
                    let asset = whisper_asset(os, arch, backend, gpus, X86_BASELINE)
                        .expect("single whisper build");
                    assert_eq!(asset.platform, platform, "{backend:?} with {gpus:?}");
                }
            }
        }
    }

    #[test]
    fn a_cpu_missing_a_baseline_extension_fails_every_x86_64_whisper_row() {
        let probe = rtx_3090s();
        for &dropped in X86_BASELINE {
            let extensions: Vec<&str> = X86_BASELINE
                .iter()
                .copied()
                .filter(|&extension| extension != dropped)
                .collect();
            for os in ["windows", "macos", "linux"] {
                for backend in WHISPER_BACKENDS {
                    for gpus in [None, Some(&probe)] {
                        let label = format!("{os} {backend:?} with {gpus:?} without {dropped}");
                        let selected = whisper_asset(os, "x86_64", backend, gpus, X86_BASELINE)
                            .expect("the full baseline selects a row");
                        let (platform, required, missing) =
                            match whisper_asset(os, "x86_64", backend, gpus, &extensions) {
                                Err(LocalError::UnsupportedCpu {
                                    platform,
                                    required,
                                    missing,
                                }) => (platform, required, missing),
                                other => panic!("{label}: {other:?}"),
                            };
                        assert_eq!(platform, selected.platform, "{label}");
                        assert_eq!(required, X86_BASELINE, "{label}");
                        assert_eq!(missing, [dropped], "{label}");
                    }
                }
            }
        }
        let error = whisper_asset(
            "linux",
            "x86_64",
            WhisperBackend::Cpu,
            None,
            &["sse4.2", "avx", "bmi2", "f16c"],
        )
        .expect_err("a CPU without avx2 and fma fails");
        assert_eq!(
            error.to_string(),
            "whisper.cpp build `linux-x86_64` requires x86-64 extensions \
             sse4.2, avx, avx2, bmi2, fma, f16c; this CPU lacks avx2, fma"
        );
    }

    #[test]
    fn aarch64_whisper_rows_need_no_x86_extensions() {
        let probe = rtx_3090s();
        for (os, platform) in [("macos", "macos-aarch64-metal"), ("linux", "linux-aarch64")] {
            for backend in WHISPER_BACKENDS {
                for gpus in [None, Some(&probe)] {
                    let asset = whisper_asset(os, "aarch64", backend, gpus, &[])
                        .expect("aarch64 whisper build");
                    assert_eq!(asset.platform, platform, "{backend:?} with {gpus:?}");
                }
            }
        }
    }

    /// Runs [`whisper_asset_with_probe`] on the full x86 baseline with a
    /// probe that reports `answer`, returning the pick and how many times the
    /// probe ran.
    fn pick_with_probe(
        os: &str,
        arch: &str,
        backend: WhisperBackend,
        answer: Option<NvidiaProbe>,
    ) -> (Result<WhisperAsset<'static>>, usize) {
        let mut probes = 0;
        let pick = whisper_asset_with_probe(
            os,
            arch,
            backend,
            || {
                probes += 1;
                answer
            },
            X86_BASELINE,
        );
        (pick, probes)
    }

    #[test]
    fn auto_probes_where_both_whisper_builds_exist_and_follows_the_answer() {
        for os in ["windows", "linux"] {
            for (answer, platform) in [
                (Some(rtx_3090s()), format!("{os}-x86_64-cuda")),
                (Some(no_gpu()), format!("{os}-x86_64")),
                (None, format!("{os}-x86_64")),
            ] {
                let label = format!("{os} with {answer:?}");
                let (pick, probes) = pick_with_probe(os, "x86_64", WhisperBackend::Auto, answer);
                assert_eq!(probes, 1, "{label}");
                assert_eq!(
                    pick.expect("auto whisper asset").platform,
                    platform,
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn the_whisper_probe_runs_only_for_auto_where_both_builds_exist() {
        for os in ["windows", "linux"] {
            for (backend, platform) in [
                (WhisperBackend::Cpu, format!("{os}-x86_64")),
                (WhisperBackend::Cuda, format!("{os}-x86_64-cuda")),
            ] {
                let (pick, probes) = pick_with_probe(os, "x86_64", backend, Some(rtx_3090s()));
                assert_eq!(probes, 0, "{os} with {backend:?}");
                assert_eq!(
                    pick.expect("explicit whisper asset").platform,
                    platform,
                    "{os} with {backend:?}"
                );
            }
        }
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "aarch64"),
            ("freebsd", "x86_64"),
            ("windows", "aarch64"),
        ] {
            for backend in WHISPER_BACKENDS {
                let (pick, probes) = pick_with_probe(os, arch, backend, Some(rtx_3090s()));
                assert_eq!(probes, 0, "{os}-{arch} with {backend:?}");
                assert_eq!(
                    pick.ok(),
                    whisper_asset(os, arch, backend, None, X86_BASELINE).ok(),
                    "{os}-{arch} with {backend:?}"
                );
            }
        }
    }

    #[test]
    fn unsupported_whisper_platforms_are_an_error() {
        for (os, arch) in [("freebsd", "x86_64"), ("windows", "aarch64")] {
            for backend in WHISPER_BACKENDS {
                assert!(
                    matches!(
                        whisper_asset(os, arch, backend, None, X86_BASELINE),
                        Err(LocalError::UnsupportedPlatform { .. })
                    ),
                    "{os}-{arch} with {backend:?}"
                );
            }
        }
    }

    #[test]
    fn whisper_assets_cover_the_seven_release_builds() {
        use WhisperBackend::{Cpu, Cuda};

        let builds = [
            ("windows", "x86_64", Some(Cpu), "windows-x86_64"),
            ("windows", "x86_64", Some(Cuda), "windows-x86_64-cuda"),
            ("macos", "aarch64", None, "macos-aarch64-metal"),
            ("macos", "x86_64", None, "macos-x86_64"),
            ("linux", "x86_64", Some(Cpu), "linux-x86_64"),
            ("linux", "x86_64", Some(Cuda), "linux-x86_64-cuda"),
            ("linux", "aarch64", None, "linux-aarch64"),
        ];
        assert_eq!(WHISPER_ASSETS.len(), builds.len(), "one row per build");
        for (os, arch, backend, platform) in builds {
            let library = match os {
                "windows" => "whisper.dll",
                "macos" => "libwhisper.dylib",
                _ => "libwhisper.so",
            };
            let asset = whisper_asset(os, arch, backend.unwrap_or_default(), None, X86_BASELINE)
                .expect("supported whisper build");
            assert_eq!(asset.platform, platform);
            assert_eq!(asset.backend, backend, "{platform}");
            assert_eq!(asset.library_name, library, "{platform}");
            assert_eq!(
                asset.archive.archive_name,
                format!("whisper-{WHISPER_RELEASE}-{platform}.zip")
            );
            assert_eq!(asset.archive.archive_kind, ArchiveKind::Zip, "{platform}");
            assert_eq!(asset.archive.sha256.len(), 64, "{platform}");
            assert!(
                asset
                    .archive
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit()),
                "{platform}"
            );
            assert!(
                asset
                    .archive
                    .url
                    .contains("/releases/download/whisper-lib-b4938/"),
                "{platform}"
            );
            assert!(
                asset.archive.url.ends_with(asset.archive.archive_name),
                "{platform}"
            );
        }
    }
}
