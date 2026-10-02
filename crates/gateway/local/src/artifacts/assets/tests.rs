//! Tests for the `llama-server` and whisper.cpp asset selection.

use super::*;

#[path = "tests-auto-pick.rs"]
mod auto_pick;
#[path = "tests-platforms.rs"]
mod platforms;

#[test]
fn blackwell_gpus_select_the_blackwell_build() {
    let asset = server_asset("windows", "x86_64", LlamaBackend::Auto, Some(&[(12, 0)]))
        .expect("blackwell asset");
    assert_eq!(asset.platform, "windows-x86_64-cuda-blackwell");
}

#[test]
fn older_nvidia_gpus_select_the_upstream_cuda_build() {
    let asset =
        server_asset("windows", "x86_64", LlamaBackend::Auto, Some(&[(8, 9)])).expect("cuda asset");
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
    let asset =
        server_asset("linux", "x86_64", LlamaBackend::CudaBlackwell, None).expect("linux asset");
    assert_eq!(asset.platform, "linux-x86_64-vulkan");
    let asset = server_asset("macos", "aarch64", LlamaBackend::Auto, None).expect("macos asset");
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

/// The `auto` pick on `os` x86-64 for GPUs at `compute_caps` on driver
/// `driver_major`.
fn auto_platform(os: &str, compute_caps: &[(u64, u64)], driver_major: u64) -> &'static str {
    let probe = NvidiaProbe {
        compute_caps: compute_caps.to_vec(),
        driver_major: Some(driver_major),
    };
    whisper_asset(
        os,
        "x86_64",
        WhisperBackend::Auto,
        Some(&probe),
        X86_BASELINE,
    )
    .expect("auto whisper asset")
    .platform
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
