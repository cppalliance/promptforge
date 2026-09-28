//! Tests for the whisper rows per platform: single-build platforms, the
//! x86 baseline, when the probe runs, and the pinned release table.

use super::*;

#[test]
fn single_build_whisper_platforms_ignore_the_backend() {
    for (os, arch, platform) in [
        ("macos", "aarch64", "macos-aarch64-metal"),
        ("macos", "x86_64", "macos-x86_64"),
        ("linux", "aarch64", "linux-aarch64"),
    ] {
        for backend in WHISPER_BACKENDS {
            for gpus in [None, Some(&[(8, 6)][..])] {
                let asset = whisper_asset(os, arch, backend, gpus).expect("single whisper build");
                assert_eq!(asset.platform, platform, "{backend:?} with {gpus:?}");
            }
        }
    }
}

#[test]
fn auto_probes_where_both_whisper_builds_exist_and_follows_the_answer() {
    for os in ["windows", "linux"] {
        for (answer, platform) in [
            (Some(NVIDIA.to_vec()), format!("{os}-x86_64-cuda")),
            (Some(Vec::new()), format!("{os}-x86_64")),
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
            let (pick, probes) = pick_with_probe(os, "x86_64", backend, Some(NVIDIA.to_vec()));
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
            let (pick, probes) = pick_with_probe(os, arch, backend, Some(NVIDIA.to_vec()));
            assert_eq!(probes, 0, "{os}-{arch} with {backend:?}");
            assert_eq!(
                pick.ok(),
                whisper_asset(os, arch, backend, None).ok(),
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
                    whisper_asset(os, arch, backend, None),
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
        let asset = whisper_asset(os, arch, backend.unwrap_or_default(), None)
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
