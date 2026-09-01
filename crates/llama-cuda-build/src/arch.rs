//! Local GPU compute-capability detection and CUDA architecture normalization.

use anyhow::Context as _;

use crate::probe::{CommandRequest, Probe};

/// Parses `nvidia-smi --query-gpu=compute_cap --format=csv,noheader` output
/// into `(major, minor)` pairs, one per visible GPU.
///
/// # Errors
/// Returns an error on a malformed line or an empty GPU list.
pub fn parse_compute_caps(csv: &str) -> anyhow::Result<Vec<(u64, u64)>> {
    let mut caps = Vec::new();
    for line in csv.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (major, minor) = line
            .split_once('.')
            .with_context(|| format!("malformed compute capability `{line}`"))?;
        let major = major
            .trim()
            .parse::<u64>()
            .with_context(|| format!("malformed compute capability `{line}`"))?;
        let minor = minor
            .trim()
            .parse::<u64>()
            .with_context(|| format!("malformed compute capability `{line}`"))?;
        caps.push((major, minor));
    }
    anyhow::ensure!(!caps.is_empty(), "nvidia-smi reported no GPUs");
    Ok(caps)
}

/// Normalizes one compute capability into a `CMAKE_CUDA_ARCHITECTURES`
/// entry naming a real (non-virtual) architecture.
///
/// Architecture-specific (`a`) forms exist from Hopper onward, so 12.0
/// becomes `120a-real` (Blackwell) while 8.9 becomes `89-real`.
#[must_use]
pub fn normalize_arch(major: u64, minor: u64) -> String {
    let digits = major * 10 + minor;
    if major >= 9 {
        format!("{digits}a-real")
    } else {
        format!("{digits}-real")
    }
}

/// Detects the visible GPUs through `nvidia-smi` and returns the sorted,
/// deduplicated CUDA architecture list to compile for.
///
/// # Errors
/// Returns an error when `nvidia-smi` fails or reports no usable GPU.
pub fn detect(probe: &impl Probe) -> anyhow::Result<Vec<String>> {
    let request = CommandRequest::new("nvidia-smi")
        .args(["--query-gpu=compute_cap", "--format=csv,noheader"]);
    let output = probe
        .run(&request)
        .context("query GPU compute capabilities")?;
    anyhow::ensure!(
        output.success(),
        "nvidia-smi failed (exit {}):\n{}",
        output.code,
        output.stderr
    );
    let mut archs: Vec<String> = parse_compute_caps(&output.stdout)?
        .into_iter()
        .map(|(major, minor)| normalize_arch(major, minor))
        .collect();
    archs.sort();
    archs.dedup();
    Ok(archs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::fake::{FakeProbe, fail, ok};

    #[test]
    fn parses_one_capability_per_line() {
        let caps = parse_compute_caps("12.0\n8.9\n").unwrap();
        assert_eq!(caps, vec![(12, 0), (8, 9)]);
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(parse_compute_caps("twelve\n").is_err());
    }

    #[test]
    fn rejects_empty_gpu_list() {
        assert!(parse_compute_caps("\n").is_err());
    }

    #[test]
    fn blackwell_12_0_maps_to_120a_real() {
        assert_eq!(normalize_arch(12, 0), "120a-real");
    }

    #[test]
    fn normalization_covers_generations() {
        assert_eq!(normalize_arch(7, 5), "75-real");
        assert_eq!(normalize_arch(8, 9), "89-real");
        assert_eq!(normalize_arch(9, 0), "90a-real");
        assert_eq!(normalize_arch(10, 0), "100a-real");
        assert_eq!(normalize_arch(12, 1), "121a-real");
    }

    #[test]
    fn detect_sorts_and_deduplicates() {
        let probe = FakeProbe::default().on("nvidia-smi", ok("12.0\n8.9\n12.0\n"));
        assert_eq!(detect(&probe).unwrap(), vec!["120a-real", "89-real"]);
    }

    #[test]
    fn detect_fails_when_nvidia_smi_fails() {
        let probe = FakeProbe::default().on("nvidia-smi", fail(9, "no devices"));
        let err = detect(&probe).unwrap_err();
        assert!(format!("{err:#}").contains("no devices"));
    }
}
