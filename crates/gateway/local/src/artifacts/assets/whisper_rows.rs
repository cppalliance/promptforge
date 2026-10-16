//! The pinned whisper.cpp runtime rows, one per release build.

use gateway_config::WhisperBackend;

use super::{ArchiveKind, ArchiveRef, WhisperAsset};

pub(super) const WHISPER_ASSETS: &[WhisperAsset<'static>] = &[
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
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-windows-x86_64.zip",
            sha256: "7d4b402495dd8ce1574abcdde3fb37823155571efc3f056caaf3c2b4220de42f",
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
        // as read on 2026-10-16. Its PTX for compute_75, compute_80, and
        // compute_90 is ISA 9.3 from CUDA 13.3, which an older driver cannot
        // compile. Like `X86_BASELINE`, the list is tied to `WHISPER_RELEASE`,
        // and a release bump re-reads it from the new archive.
        native_compute_caps: Some(&[(8, 6), (8, 9), (12, 0), (12, 1)]),
        min_glibcxx: None,
        platform: "windows-x86_64-cuda",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-windows-x86_64-cuda.zip",
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-windows-x86_64-cuda.zip",
            sha256: "b2683fe38fd28d80fdb42d123c3e03d480786095b4a0affa0542ae1ce88ac752",
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
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-macos-aarch64-metal.zip",
            sha256: "3afe9e7b5754af9bb0231e6b817f2a3dc164e8d72f7e5dab11aaab529cd0e37f",
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
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-macos-x86_64.zip",
            sha256: "7dc019031785d1d9d5da3e0235cfd4e161b9b28dee842ef743d9a11965af081a",
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
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-linux-x86_64.zip",
            sha256: "1f29906b27587b5ed3412bdcaaa335cd2058e2291c496a73c03a8977e387d427",
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
        native_compute_caps: None,
        // The version the pinned archive's `libggml-cuda.so.0` needs for
        // `std::condition_variable::wait`, from GCC 12's runtime. Like the
        // Windows native list, it is tied to `WHISPER_RELEASE`.
        min_glibcxx: Some("GLIBCXX_3.4.30"),
        platform: "linux-x86_64-cuda",
        archive: ArchiveRef {
            archive_name: "whisper-b4938-linux-x86_64-cuda.zip",
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-linux-x86_64-cuda.zip",
            sha256: "d08cfcdfc72ee373745b19a71da8fd67d32fa42ac8d63cfb4a28034937eafa97",
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
            url: "https://github.com/cppalliance/runvil/releases/download/whisper-lib-b4938/whisper-b4938-linux-aarch64.zip",
            sha256: "30033c694a77dc5a82a73a99f846461af38820efc558fd0e2c0665c43fbeee1f",
            archive_kind: ArchiveKind::Zip,
        },
        library_name: "libwhisper.so",
    },
];
