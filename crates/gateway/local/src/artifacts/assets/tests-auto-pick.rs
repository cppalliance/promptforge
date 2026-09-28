//! Tests for the whisper `auto` pick: the probe's GPUs and driver, the
//! CUDA environment, the C++ runtime, and explicit settings.

use super::*;

#[test]
fn an_nvidia_gpu_selects_the_cuda_whisper_build() {
    for (os, platform) in [
        ("windows", "windows-x86_64-cuda"),
        ("linux", "linux-x86_64-cuda"),
    ] {
        for caps in [&[(8, 6)][..], &[(12, 0)][..]] {
            let asset = whisper_asset(os, "x86_64", WhisperBackend::Auto, Some(caps))
                .expect("cuda whisper asset");
            assert_eq!(asset.platform, platform, "{caps:?}");
        }
    }
}

#[test]
fn no_nvidia_gpu_selects_the_cpu_whisper_build() {
    for (os, platform) in [("windows", "windows-x86_64"), ("linux", "linux-x86_64")] {
        for gpus in [None, Some(&[][..])] {
            let asset =
                whisper_asset(os, "x86_64", WhisperBackend::Auto, gpus).expect("cpu whisper asset");
            assert_eq!(asset.platform, platform, "{gpus:?}");
        }
    }
}

#[test]
fn an_explicit_whisper_backend_ignores_the_probe() {
    for os in ["windows", "linux"] {
        for gpus in [None, Some(&[][..]), Some(&[(8, 6)][..])] {
            let cpu = whisper_asset(os, "x86_64", WhisperBackend::Cpu, gpus)
                .expect("explicit cpu whisper asset");
            assert_eq!(cpu.platform, format!("{os}-x86_64"), "{gpus:?}");
            let cuda = whisper_asset(os, "x86_64", WhisperBackend::Cuda, gpus)
                .expect("explicit cuda whisper asset");
            assert_eq!(cuda.platform, format!("{os}-x86_64-cuda"), "{gpus:?}");
        }
    }
}
