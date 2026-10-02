//! Tests for the whisper rows per platform: single-build platforms, the
//! x86 baseline, when the probe runs, and the pinned release table.

use super::*;

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
            let (pick, probes, host_reads) = pick_with_probe(os, arch, backend, Some(rtx_3090s()));
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
