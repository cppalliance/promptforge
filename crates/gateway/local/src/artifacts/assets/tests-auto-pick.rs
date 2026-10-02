//! Tests for the whisper `auto` pick: the probe's GPUs and driver, the
//! CUDA environment, the C++ runtime, and explicit settings.

use super::*;

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
