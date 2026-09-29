//! What the machine reports for runtime selection: the NVIDIA probe, the
//! CUDA environment, the C++ runtime, and the CPU's x86 extensions.

use super::super::assets::NvidiaProbe;
#[cfg(target_arch = "x86_64")]
use super::super::assets::X86_BASELINE;

/// Queries the machine's NVIDIA GPUs and driver version through `nvidia-smi`.
/// Returns `None` when the driver or the tool is absent or fails, or it
/// reports no GPU; the `llama-server` pick then falls back to the Vulkan
/// build and the whisper pick to the CPU build.
pub(super) fn nvidia_probe() -> Option<NvidiaProbe> {
    let mut command = std::process::Command::new("nvidia-smi");
    command.args([
        "--query-gpu=compute_cap,driver_version",
        "--format=csv,noheader",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(crate::CREATE_NO_WINDOW);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_nvidia_probe(&String::from_utf8_lossy(&output.stdout))
}

/// Reads `nvidia-smi --query-gpu=compute_cap,driver_version
/// --format=csv,noheader` output, one `8.6, 591.86` line per GPU. A line
/// without a readable compute capability names no GPU, and `None` means no
/// line did. Every GPU reports the host's one driver, so the lowest reading
/// stands for it, and an unreadable one reads lowest of all.
fn parse_nvidia_probe(stdout: &str) -> Option<NvidiaProbe> {
    let (compute_caps, driver_majors): (Vec<(u64, u64)>, Vec<Option<u64>>) = stdout
        .lines()
        .filter_map(|line| {
            let (cap, driver) = line.split_once(',').unwrap_or((line, ""));
            let (major, minor) = cap.trim().split_once('.')?;
            let cap = (major.trim().parse().ok()?, minor.trim().parse().ok()?);
            let driver_major = driver
                .trim()
                .split('.')
                .next()
                .and_then(|major| major.parse().ok());
            Some((cap, driver_major))
        })
        .unzip();
    if compute_caps.is_empty() {
        return None;
    }
    Some(NvidiaProbe {
        compute_caps,
        // `None` orders below every `Some`.
        driver_major: driver_majors.into_iter().min().flatten(),
    })
}

/// The [`X86_BASELINE`] extensions this CPU reports, in baseline
/// order; none off x86-64, where no whisper row needs them.
pub(super) fn host_x86_extensions() -> Vec<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        // The detection macro takes only a literal, so each baseline
        // spelling needs its arm here. A spelling without one reads as
        // absent, and selection then names it instead of passing unchecked.
        X86_BASELINE
            .iter()
            .copied()
            .filter(|&extension| match extension {
                "sse4.2" => std::arch::is_x86_feature_detected!("sse4.2"),
                "avx" => std::arch::is_x86_feature_detected!("avx"),
                "avx2" => std::arch::is_x86_feature_detected!("avx2"),
                "bmi2" => std::arch::is_x86_feature_detected!("bmi2"),
                "fma" => std::arch::is_x86_feature_detected!("fma"),
                "f16c" => std::arch::is_x86_feature_detected!("f16c"),
                _ => false,
            })
            .collect()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nvidia_probe_reads_each_gpu_and_the_driver_major() {
        // Two RTX 3090s on driver 591.86, recorded on one host under Windows
        // (CRLF) and under WSL (LF).
        for stdout in [
            "8.6, 591.86\r\n8.6, 591.86\r\n",
            "8.6, 591.86\n8.6, 591.86\n",
        ] {
            assert_eq!(
                parse_nvidia_probe(stdout),
                Some(NvidiaProbe {
                    compute_caps: vec![(8, 6), (8, 6)],
                    driver_major: Some(591),
                }),
                "{stdout:?}"
            );
        }
        for (stdout, compute_caps, driver_major) in [
            // A Linux driver version has three parts; only the major counts.
            ("12.0, 570.133.07\n", vec![(12, 0)], Some(570)),
            // An unreadable version keeps the GPU and reads no major.
            ("8.9, [N/A]\n", vec![(8, 9)], None),
            // The GPUs share one driver; the lowest reading stands for it
            // whatever the line order, and an unreadable one is the lowest.
            ("8.6, 591.86\n8.9, [N/A]\n", vec![(8, 6), (8, 9)], None),
            ("8.9, [N/A]\n8.6, 591.86\n", vec![(8, 9), (8, 6)], None),
            (
                "8.6, 570.10\n8.9, 591.86\n",
                vec![(8, 6), (8, 9)],
                Some(570),
            ),
            // A line without a readable compute capability names no GPU.
            ("[N/A], 591.86\n8.6, 591.86\n", vec![(8, 6)], Some(591)),
        ] {
            assert_eq!(
                parse_nvidia_probe(stdout),
                Some(NvidiaProbe {
                    compute_caps,
                    driver_major,
                }),
                "{stdout:?}"
            );
        }
    }

    #[test]
    fn parse_nvidia_probe_answers_none_without_a_gpu() {
        for stdout in ["", "\r\n", "No devices were found\n", "[N/A], 591.86\n"] {
            assert_eq!(parse_nvidia_probe(stdout), None, "{stdout:?}");
        }
    }
}
