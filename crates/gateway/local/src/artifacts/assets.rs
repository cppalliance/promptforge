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

/// What the whisper `auto` pick reads from the host beside the NVIDIA
/// probe: `visible_devices`, the `CUDA_VISIBLE_DEVICES` value, `None` when
/// unset, and `libstdcxx`, the bytes of the host's `libstdc++.so.6`, `None`
/// when none was read.
#[derive(Debug, Default)]
pub(super) struct CudaHost {
    pub(super) visible_devices: Option<String>,
    pub(super) libstdcxx: Option<Vec<u8>>,
}

/// Whether a `CUDA_VISIBLE_DEVICES` of `value` hides every one of
/// `gpu_count` GPUs. CUDA exposes only the devices listed before the first
/// invalid entry, so a set value hides them all unless its first entry is a
/// device index below `gpu_count` or a `GPU-` or `MIG-` identifier. The
/// probe reads no UUIDs, so an identifier counts as visible without being
/// matched: one that names no GPU, or an abbreviated one that names several,
/// hides every GPU from CUDA but not here. An unset value hides none.
pub(super) fn cuda_visible_devices_hides_every_gpu(value: Option<&str>, gpu_count: usize) -> bool {
    let Some(value) = value else {
        return false;
    };
    let first = value.split(',').next().unwrap_or_default().trim();
    let visible = first.starts_with("GPU-")
        || first.starts_with("MIG-")
        || first.parse::<usize>().is_ok_and(|index| index < gpu_count);
    !visible
}

/// Whether the C++ runtime `library` defines the symbol version `version`:
/// its bytes hold the name followed by a NUL, so a longer version that
/// shares the prefix, such as `GLIBCXX_3.4.300` for `GLIBCXX_3.4.30`, does
/// not count.
pub(super) fn libstdcxx_defines(library: &[u8], version: &str) -> bool {
    let name = version.as_bytes();
    library
        .windows(name.len() + 1)
        .any(|window| window.ends_with(&[0]) && window.starts_with(name))
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
/// `native_compute_caps` lists the GPU compute capabilities, as
/// `(major, minor)`, the build carries native code for, which only the
/// `auto` pick consults: every probed GPU must be in the list. `None` checks
/// nothing. `min_glibcxx` is the `libstdc++` symbol version the build needs,
/// which only the `auto` pick consults: the host's C++ runtime must define
/// it. `None` checks nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct WhisperAsset<'a> {
    pub(super) os: &'a str,
    pub(super) arch: &'a str,
    pub(super) backend: Option<WhisperBackend>,
    pub(super) min_driver_major: Option<u64>,
    pub(super) native_compute_caps: Option<&'a [(u64, u64)]>,
    pub(super) min_glibcxx: Option<&'a str>,
    pub(super) platform: &'a str,
    pub(super) archive: ArchiveRef<'a>,
    pub(super) library_name: &'a str,
}

const WHISPER_ASSETS: &[WhisperAsset<'static>] = &[
    // The sha256 pin is filled in once the whisper-lib-b4938 release holds
    // the archive; until then the row is fail-closed (the pin can never
    // match, so the download is refused rather than trusted).
    WhisperAsset {
        os: "windows",
        arch: "x86_64",
        backend: Some(WhisperBackend::Cpu),
        min_driver_major: None,
        native_compute_caps: None,
        min_glibcxx: None,
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
        // CUDA 13 runs on Windows driver 580 or later.
        min_driver_major: Some(580),
        // The native code in the pinned archive's `ggml-cuda.dll` fatbinary,
        // as read on 2026-10-02. Its PTX for compute_75, compute_80, and
        // compute_90 is ISA 9.3 from CUDA 13.3, which an older driver cannot
        // compile. Like `X86_BASELINE`, the list is tied to `WHISPER_RELEASE`,
        // and a release bump re-reads it from the new archive.
        native_compute_caps: Some(&[(8, 6), (8, 9), (12, 0), (12, 1)]),
        min_glibcxx: None,
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
        native_compute_caps: None,
        min_glibcxx: None,
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
        native_compute_caps: None,
        min_glibcxx: None,
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
        native_compute_caps: None,
        min_glibcxx: None,
        platform: "linux-x86_64",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-linux-x86_64.zip",
            url: "https://github.com/cppalliance/promptforge/releases/download/whisper-lib-b4938/whisper-b4938-linux-x86_64.zip",
            sha256: "0dc1a6adc29bfaecb6c2c8c8fc9ec2f903b25e6bfadd67bbdb239521f9101155",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.so",
    },
    // The sha256 pin is filled in once the whisper-lib-b4938 release holds
    // the archive; until then the row is fail-closed (the pin can never
    // match, so the download is refused rather than trusted).
    WhisperAsset {
        os: "linux",
        arch: "x86_64",
        backend: Some(WhisperBackend::Cuda),
        // CUDA 12.8 runs on Linux driver 570 or later.
        min_driver_major: Some(570),
        native_compute_caps: None,
        // The version the pinned archive's `libggml-cuda.so.0` needs for
        // `std::condition_variable::wait`, from GCC 12's runtime. Like the
        // Windows native list, it is tied to `WHISPER_RELEASE`.
        min_glibcxx: Some("GLIBCXX_3.4.30"),
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
        native_compute_caps: None,
        min_glibcxx: None,
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
/// gets the CUDA build when `CUDA_VISIBLE_DEVICES` leaves a GPU visible, the
/// platform's CUDA row sets no driver floor or the driver meets it, the row
/// lists no native compute capabilities or every probed GPU's is among
/// them, and the row names no `min_glibcxx` or the host's C++ runtime
/// defines it. Anything else - including a failed probe, GPUs hidden from
/// CUDA, an unreadable driver version under a floor, any GPU without native
/// code, or no runtime read - gets the CPU build. A `None` host reads as
/// [`CudaHost::default`].
fn auto_whisper_backend(
    os: &str,
    arch: &str,
    gpus: Option<&NvidiaProbe>,
    cuda_host: Option<&CudaHost>,
) -> WhisperBackend {
    let Some(probe) = gpus.filter(|probe| !probe.compute_caps.is_empty()) else {
        return WhisperBackend::Cpu;
    };
    let unread = CudaHost::default();
    let host = cuda_host.unwrap_or(&unread);
    if cuda_visible_devices_hides_every_gpu(
        host.visible_devices.as_deref(),
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
            host.libstdcxx
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
/// `backend` (the `[stt] whisper_backend` setting), `gpus` (what the
/// NVIDIA probe reported, when a probe was needed and worked), and
/// `cuda_host` (what was read from the host for the CUDA row, when it was
/// read) are consulted only on Windows x86-64 and Linux x86-64, the two
/// platforms with a choice; every other platform has exactly one row. There
/// `auto` takes the CUDA row only when `gpus` names a GPU that
/// `CUDA_VISIBLE_DEVICES` leaves visible, the driver meets the row's
/// `min_driver_major`, every GPU's compute capability is in the row's
/// `native_compute_caps` when it lists them, and the host's C++ runtime
/// defines the row's `min_glibcxx` when it names one. On x86-64, under
/// every setting, the selected row needs every [`X86_BASELINE`] extension
/// in `x86_extensions`, the ones the host CPU reports.
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
    cuda_host: Option<&CudaHost>,
    x86_extensions: &[&str],
) -> Result<WhisperAsset<'static>> {
    let wanted = if whisper_backend_applies(os, arch) {
        Some(match backend {
            WhisperBackend::Auto => auto_whisper_backend(os, arch, gpus, cuda_host),
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
/// other platform already knows its row. `cuda_host` (the host's
/// `CUDA_VISIBLE_DEVICES` and C++ runtime in production) runs there too,
/// and only after the probe reports a GPU. `x86_extensions` passes through.
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
    cuda_host: impl FnOnce() -> CudaHost,
    x86_extensions: &[&str],
) -> Result<WhisperAsset<'static>> {
    let (gpus, host) = if backend == WhisperBackend::Auto && whisper_backend_applies(os, arch) {
        let gpus = probe();
        let host = gpus
            .as_ref()
            .is_some_and(|probe| !probe.compute_caps.is_empty())
            .then(cuda_host);
        (gpus, host)
    } else {
        (None, None)
    };
    whisper_asset(
        os,
        arch,
        backend,
        gpus.as_ref(),
        host.as_ref(),
        x86_extensions,
    )
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

    /// The bytes of a C++ runtime that defines each of `versions`, laid out
    /// as NUL-terminated names the way a library's string table holds them.
    fn runtime_defining(versions: &[&str]) -> Vec<u8> {
        let mut library = b"\x7fELF\0libstdc++.so.6\0".to_vec();
        for version in versions {
            library.extend_from_slice(version.as_bytes());
            library.push(0);
        }
        library
    }

    /// A host that leaves `CUDA_VISIBLE_DEVICES` unset and whose C++
    /// runtime defines `GLIBCXX_3.4.30`, so it meets every CUDA row.
    fn cuda_ready() -> CudaHost {
        CudaHost {
            visible_devices: None,
            libstdcxx: Some(runtime_defining(&["GLIBCXX_3.4.29", "GLIBCXX_3.4.30"])),
        }
    }

    /// [`cuda_ready`] with `CUDA_VISIBLE_DEVICES` set to `value`.
    fn visible(value: &str) -> CudaHost {
        CudaHost {
            visible_devices: Some(value.to_owned()),
            ..cuda_ready()
        }
    }

    /// [`cuda_ready`] with the C++ runtime `libstdcxx`, `None` for none read.
    fn runtime(libstdcxx: Option<Vec<u8>>) -> CudaHost {
        CudaHost {
            libstdcxx,
            ..cuda_ready()
        }
    }

    #[test]
    fn cuda_visible_devices_hides_every_gpu_by_cudas_rule() {
        for value in [
            None,
            Some("0"),
            Some("1"),
            Some("1,0"),
            Some("0,-1"),
            Some("GPU-8f6e2c1a-5b3d-4e7f-9a0b-1c2d3e4f5a6b"),
            Some("MIG-8f6e2c1a-5b3d-4e7f-9a0b-1c2d3e4f5a6b"),
        ] {
            assert!(!cuda_visible_devices_hides_every_gpu(value, 2), "{value:?}");
        }
        for value in ["", "-1", "2", "none", "-1,0", "2,0"] {
            assert!(
                cuda_visible_devices_hides_every_gpu(Some(value), 2),
                "{value:?}"
            );
        }
    }

    #[test]
    fn libstdcxx_defines_matches_only_the_whole_version() {
        let version = "GLIBCXX_3.4.30";
        assert!(libstdcxx_defines(&runtime_defining(&[version]), version));
        assert!(libstdcxx_defines(
            &runtime_defining(&["GLIBCXX_3.4.29", version, "GLIBCXX_3.4.31"]),
            version
        ));
        for others in [
            &["GLIBCXX_3.4.29"][..],
            &["GLIBCXX_3.4.300"],
            &["GLIBCXX_3.4.29", "GLIBCXX_3.4.300"],
            &[],
        ] {
            assert!(
                !libstdcxx_defines(&runtime_defining(others), version),
                "{others:?}"
            );
        }
        assert!(!libstdcxx_defines(b"GLIBCXX_3.4.30", version), "no NUL");
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
                    Some(&cuda_ready()),
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
                let asset = whisper_asset(
                    os,
                    "x86_64",
                    WhisperBackend::Auto,
                    gpus,
                    Some(&cuda_ready()),
                    X86_BASELINE,
                )
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
                Some(&cuda_ready()),
                X86_BASELINE,
            )
            .expect("auto linux whisper asset");
            assert_eq!(asset.platform, platform, "driver {driver_major:?}");
        }
    }

    #[test]
    fn auto_takes_the_windows_cuda_whisper_build_from_driver_580() {
        for (driver_major, platform) in [
            (None, "windows-x86_64"),
            (Some(0), "windows-x86_64"),
            (Some(579), "windows-x86_64"),
            (Some(580), "windows-x86_64-cuda"),
            (Some(591), "windows-x86_64-cuda"),
        ] {
            let probe = nvidia(driver_major);
            let asset = whisper_asset(
                "windows",
                "x86_64",
                WhisperBackend::Auto,
                Some(&probe),
                Some(&cuda_ready()),
                X86_BASELINE,
            )
            .expect("auto windows whisper asset");
            assert_eq!(asset.platform, platform, "driver {driver_major:?}");
        }
    }

    /// The `auto` pick on `os` x86-64 for GPUs at `compute_caps` on driver
    /// `driver_major`, on a [`cuda_ready`] host.
    fn auto_platform(os: &str, compute_caps: &[(u64, u64)], driver_major: u64) -> &'static str {
        let probe = NvidiaProbe {
            compute_caps: compute_caps.to_vec(),
            driver_major: Some(driver_major),
        };
        auto_pick(os, &probe, Some(&cuda_ready()))
    }

    /// The `auto` pick on `os` x86-64 for `probe` on `host`.
    fn auto_pick(os: &str, probe: &NvidiaProbe, host: Option<&CudaHost>) -> &'static str {
        whisper_asset(
            os,
            "x86_64",
            WhisperBackend::Auto,
            Some(probe),
            host,
            X86_BASELINE,
        )
        .expect("auto whisper asset")
        .platform
    }

    #[test]
    fn auto_takes_the_windows_cpu_whisper_build_for_a_gpu_without_native_code() {
        for foreign in [(7, 5), (8, 0), (9, 0), (6, 1)] {
            for caps in [vec![foreign], vec![foreign, (8, 6)], vec![(8, 6), foreign]] {
                assert_eq!(
                    auto_platform("windows", &caps, 591),
                    "windows-x86_64",
                    "{caps:?}"
                );
            }
        }
        for caps in [
            vec![(8, 6)],
            vec![(8, 9)],
            vec![(12, 0)],
            vec![(12, 1)],
            vec![(8, 6), (12, 0)],
        ] {
            assert_eq!(
                auto_platform("windows", &caps, 591),
                "windows-x86_64-cuda",
                "{caps:?}"
            );
        }
    }

    #[test]
    fn auto_takes_the_linux_cuda_whisper_build_for_any_gpu_above_its_floor() {
        assert_eq!(auto_platform("linux", &[(7, 5)], 570), "linux-x86_64-cuda");
    }

    #[test]
    fn auto_takes_the_linux_cuda_whisper_build_only_with_glibcxx_3_4_30() {
        let probe = nvidia(Some(591));
        let hosts = [
            (Some(cuda_ready()), "linux-x86_64-cuda"),
            (
                Some(runtime(Some(runtime_defining(&["GLIBCXX_3.4.30"])))),
                "linux-x86_64-cuda",
            ),
            (
                Some(runtime(Some(runtime_defining(&["GLIBCXX_3.4.29"])))),
                "linux-x86_64",
            ),
            (
                Some(runtime(Some(runtime_defining(&["GLIBCXX_3.4.300"])))),
                "linux-x86_64",
            ),
            (Some(runtime(None)), "linux-x86_64"),
            (None, "linux-x86_64"),
        ];
        for (index, (host, platform)) in hosts.iter().enumerate() {
            assert_eq!(
                auto_pick("linux", &probe, host.as_ref()),
                *platform,
                "host {index}"
            );
            assert_eq!(
                auto_pick("windows", &probe, host.as_ref()),
                "windows-x86_64-cuda",
                "host {index}"
            );
        }
    }

    #[test]
    fn gpus_hidden_from_cuda_select_the_cpu_whisper_build() {
        let probe = rtx_3090s();
        for os in ["windows", "linux"] {
            for value in ["", "-1", "2", "none"] {
                assert_eq!(
                    auto_pick(os, &probe, Some(&visible(value))),
                    format!("{os}-x86_64"),
                    "{os} with CUDA_VISIBLE_DEVICES={value:?}"
                );
            }
            for value in ["0", "1", "1,0", "0,-1", "GPU-8f6e2c1a"] {
                assert_eq!(
                    auto_pick(os, &probe, Some(&visible(value))),
                    format!("{os}-x86_64-cuda"),
                    "{os} with CUDA_VISIBLE_DEVICES={value:?}"
                );
            }
        }
    }

    #[test]
    fn an_explicit_whisper_backend_ignores_the_probe() {
        // The drivers below each CUDA floor, a GPU without native code in the
        // Windows build, GPUs hidden from CUDA, and a C++ runtime without the
        // Linux build's version are included: an explicit `cuda` is honored
        // there.
        let hosts = [
            None,
            Some(cuda_ready()),
            Some(visible("-1")),
            Some(runtime(None)),
            Some(runtime(Some(runtime_defining(&["GLIBCXX_3.4.29"])))),
        ];
        let probes = [
            None,
            Some(no_gpu()),
            Some(rtx_3090s()),
            Some(nvidia(Some(569))),
            Some(nvidia(Some(579))),
            Some(nvidia(None)),
            Some(NvidiaProbe {
                compute_caps: vec![(7, 5)],
                driver_major: Some(591),
            }),
        ];
        for os in ["windows", "linux"] {
            for gpus in &probes {
                for (index, host) in hosts.iter().enumerate() {
                    let label = format!("{gpus:?} on host {index}");
                    let cpu = whisper_asset(
                        os,
                        "x86_64",
                        WhisperBackend::Cpu,
                        gpus.as_ref(),
                        host.as_ref(),
                        X86_BASELINE,
                    )
                    .expect("explicit cpu whisper asset");
                    assert_eq!(cpu.platform, format!("{os}-x86_64"), "{label}");
                    let cuda = whisper_asset(
                        os,
                        "x86_64",
                        WhisperBackend::Cuda,
                        gpus.as_ref(),
                        host.as_ref(),
                        X86_BASELINE,
                    )
                    .expect("explicit cuda whisper asset");
                    assert_eq!(cuda.platform, format!("{os}-x86_64-cuda"), "{label}");
                }
            }
        }
    }

    #[test]
    fn single_build_whisper_platforms_ignore_the_backend() {
        let probe = rtx_3090s();
        let host = cuda_ready();
        for (os, arch, platform) in [
            ("macos", "aarch64", "macos-aarch64-metal"),
            ("macos", "x86_64", "macos-x86_64"),
            ("linux", "aarch64", "linux-aarch64"),
        ] {
            for backend in WHISPER_BACKENDS {
                for gpus in [None, Some(&probe)] {
                    let asset = whisper_asset(os, arch, backend, gpus, Some(&host), X86_BASELINE)
                        .expect("single whisper build");
                    assert_eq!(asset.platform, platform, "{backend:?} with {gpus:?}");
                }
            }
        }
    }

    #[test]
    fn a_cpu_missing_a_baseline_extension_fails_every_x86_64_whisper_row() {
        let probe = rtx_3090s();
        let host = cuda_ready();
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
                        let selected =
                            whisper_asset(os, "x86_64", backend, gpus, Some(&host), X86_BASELINE)
                                .expect("the full baseline selects a row");
                        let (platform, required, missing) = match whisper_asset(
                            os,
                            "x86_64",
                            backend,
                            gpus,
                            Some(&host),
                            &extensions,
                        ) {
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
        let host = cuda_ready();
        for (os, platform) in [("macos", "macos-aarch64-metal"), ("linux", "linux-aarch64")] {
            for backend in WHISPER_BACKENDS {
                for gpus in [None, Some(&probe)] {
                    let asset = whisper_asset(os, "aarch64", backend, gpus, Some(&host), &[])
                        .expect("aarch64 whisper build");
                    assert_eq!(asset.platform, platform, "{backend:?} with {gpus:?}");
                }
            }
        }
    }

    /// Runs [`whisper_asset_with_probe`] on the full x86 baseline with a
    /// probe that reports `answer` and a [`cuda_ready`] host, returning the
    /// pick, how many times the probe ran, and how many times the host was
    /// read.
    fn pick_with_probe(
        os: &str,
        arch: &str,
        backend: WhisperBackend,
        answer: Option<NvidiaProbe>,
    ) -> (Result<WhisperAsset<'static>>, usize, usize) {
        let mut probes = 0;
        let mut host_reads = 0;
        let pick = whisper_asset_with_probe(
            os,
            arch,
            backend,
            || {
                probes += 1;
                answer
            },
            || {
                host_reads += 1;
                cuda_ready()
            },
            X86_BASELINE,
        );
        (pick, probes, host_reads)
    }

    #[test]
    fn auto_probes_where_both_whisper_builds_exist_and_follows_the_answer() {
        for os in ["windows", "linux"] {
            for (answer, platform, reads) in [
                (Some(rtx_3090s()), format!("{os}-x86_64-cuda"), 1),
                (Some(no_gpu()), format!("{os}-x86_64"), 0),
                (None, format!("{os}-x86_64"), 0),
            ] {
                let label = format!("{os} with {answer:?}");
                let (pick, probes, host_reads) =
                    pick_with_probe(os, "x86_64", WhisperBackend::Auto, answer);
                assert_eq!(probes, 1, "{label}");
                assert_eq!(host_reads, reads, "{label}");
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
                let (pick, probes, host_reads) =
                    pick_with_probe(os, "x86_64", backend, Some(rtx_3090s()));
                assert_eq!(probes, 0, "{os} with {backend:?}");
                assert_eq!(host_reads, 0, "{os} with {backend:?}");
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
                let (pick, probes, host_reads) =
                    pick_with_probe(os, arch, backend, Some(rtx_3090s()));
                assert_eq!(probes, 0, "{os}-{arch} with {backend:?}");
                assert_eq!(host_reads, 0, "{os}-{arch} with {backend:?}");
                assert_eq!(
                    pick.ok(),
                    whisper_asset(os, arch, backend, None, None, X86_BASELINE).ok(),
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
                        whisper_asset(os, arch, backend, None, None, X86_BASELINE),
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

        const WINDOWS_CUDA_NATIVE: &[(u64, u64)] = &[(8, 6), (8, 9), (12, 0), (12, 1)];
        // What only `auto` consults: `min_driver_major`,
        // `native_compute_caps`, and `min_glibcxx`.
        type AutoNeeds = (
            Option<u64>,
            Option<&'static [(u64, u64)]>,
            Option<&'static str>,
        );
        const NONE: AutoNeeds = (None, None, None);
        let builds = [
            ("windows", "x86_64", Some(Cpu), "windows-x86_64", NONE),
            (
                "windows",
                "x86_64",
                Some(Cuda),
                "windows-x86_64-cuda",
                (Some(580), Some(WINDOWS_CUDA_NATIVE), None),
            ),
            ("macos", "aarch64", None, "macos-aarch64-metal", NONE),
            ("macos", "x86_64", None, "macos-x86_64", NONE),
            ("linux", "x86_64", Some(Cpu), "linux-x86_64", NONE),
            (
                "linux",
                "x86_64",
                Some(Cuda),
                "linux-x86_64-cuda",
                (Some(570), None, Some("GLIBCXX_3.4.30")),
            ),
            ("linux", "aarch64", None, "linux-aarch64", NONE),
        ];
        assert_eq!(WHISPER_ASSETS.len(), builds.len(), "one row per build");
        for (os, arch, backend, platform, auto_needs) in builds {
            let library = match os {
                "windows" => "whisper.dll",
                "macos" => "libwhisper.dylib",
                _ => "libwhisper.so",
            };
            let asset = whisper_asset(
                os,
                arch,
                backend.unwrap_or_default(),
                None,
                None,
                X86_BASELINE,
            )
            .expect("supported whisper build");
            assert_eq!(asset.platform, platform);
            assert_eq!(asset.backend, backend, "{platform}");
            assert_eq!(
                (
                    asset.min_driver_major,
                    asset.native_compute_caps,
                    asset.min_glibcxx
                ),
                auto_needs,
                "{platform}"
            );
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
