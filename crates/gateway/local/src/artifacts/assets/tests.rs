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
