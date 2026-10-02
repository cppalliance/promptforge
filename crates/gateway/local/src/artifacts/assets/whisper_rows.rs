//! The pinned whisper.cpp runtime rows, one per release build.

use gateway_config::WhisperBackend;

use super::{ArchiveKind, ArchiveRef, WhisperAsset};

pub(super) const WHISPER_ASSETS: &[WhisperAsset<'static>] = &[
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
