//! Pinned `llama-server` release assets and the host->asset selection table.

use super::Result;
use crate::local::error::LocalError;

/// The `llama.cpp` release tag every managed `llama-server` build is pinned to.
pub(super) const LLAMA_RELEASE: &str = "b10082";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ServerAsset<'a> {
    pub(super) os: &'a str,
    pub(super) arch: &'a str,
    pub(super) platform: &'a str,
    pub(super) archive_name: &'a str,
    pub(super) url: &'a str,
    pub(super) sha256: &'a str,
    pub(super) archive_kind: ArchiveKind,
    pub(super) executable_name: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FileAsset<'a> {
    pub(super) name: &'a str,
    pub(super) url: &'a str,
    pub(super) sha256: Option<&'a str>,
}

const WINDOWS_AARCH64_CPU: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "aarch64",
    platform: "windows-aarch64",
    archive_name: "llama-b10082-bin-win-cpu-arm64.zip",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-cpu-arm64.zip",
    sha256: "50dab63396f579cc0ceb4a4fc4b985414d55aaebd4722f363ad03696648711a4",
    archive_kind: ArchiveKind::Zip,
    executable_name: "llama-server.exe",
};

// The macOS release tars are already Metal-enabled, so both kinds share them.
const MACOS_X86_64: ServerAsset<'static> = ServerAsset {
    os: "macos",
    arch: "x86_64",
    platform: "macos-x86_64",
    archive_name: "llama-b10082-bin-macos-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-x64.tar.gz",
    sha256: "5a28fad0f05bf283c1adb92224c1bf3c25ee06acd0f4065b170016c14b490473",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const MACOS_AARCH64: ServerAsset<'static> = ServerAsset {
    os: "macos",
    arch: "aarch64",
    platform: "macos-aarch64",
    archive_name: "llama-b10082-bin-macos-arm64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-macos-arm64.tar.gz",
    sha256: "d644e16eefef3402e4fa86c0fcdce3b00a6786db68c3f216875ce87b45d29173",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const WINDOWS_X86_64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "windows",
    arch: "x86_64",
    platform: "windows-x86_64-vulkan",
    archive_name: "llama-b10082-bin-win-vulkan-x64.zip",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-win-vulkan-x64.zip",
    sha256: "0a4b2e41cfb950da9a749baf8978e0626690fbead3b0ca96860785484cda5bde",
    archive_kind: ArchiveKind::Zip,
    executable_name: "llama-server.exe",
};

const LINUX_X86_64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "x86_64",
    platform: "linux-x86_64-vulkan",
    archive_name: "llama-b10082-bin-ubuntu-vulkan-x64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-x64.tar.gz",
    sha256: "9003ea32e3d5d8a01da3e4b5d3124e0d21c63d51e112c40f5dcdef91ffaca7cc",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

const LINUX_AARCH64_VULKAN: ServerAsset<'static> = ServerAsset {
    os: "linux",
    arch: "aarch64",
    platform: "linux-aarch64-vulkan",
    archive_name: "llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz",
    url: "https://github.com/ggml-org/llama.cpp/releases/download/b10082/llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz",
    sha256: "2805902c3074f615a0105a5325ee29799500c8e29c90ccb986b59e1141df551e",
    archive_kind: ArchiveKind::TarGz,
    executable_name: "llama-server",
};

// No Vulkan build exists for Windows arm64 in release b10082, so the dev
// table falls back to the CPU archive there.
const DEV_SERVER_ASSETS: &[ServerAsset<'static>] = &[
    WINDOWS_X86_64_VULKAN,
    WINDOWS_AARCH64_CPU,
    LINUX_X86_64_VULKAN,
    LINUX_AARCH64_VULKAN,
    MACOS_X86_64,
    MACOS_AARCH64,
];

/// Selects the pinned GPU-capable `llama-server` asset for `(os, arch)`.
///
/// # Errors
/// Returns [`LocalError::UnsupportedPlatform`] when no asset matches the host.
pub(super) fn server_asset(os: &str, arch: &str) -> Result<ServerAsset<'static>> {
    DEV_SERVER_ASSETS
        .iter()
        .copied()
        .find(|asset| asset.os == os && asset.arch == arch)
        .ok_or_else(|| LocalError::UnsupportedPlatform {
            os: os.to_owned(),
            arch: arch.to_owned(),
        })
}
