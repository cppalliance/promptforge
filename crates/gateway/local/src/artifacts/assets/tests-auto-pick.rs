//! Tests for the whisper `auto` pick: the probe's GPUs and driver, the
//! CUDA environment, the C++ runtime, and explicit settings.

use super::*;

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
