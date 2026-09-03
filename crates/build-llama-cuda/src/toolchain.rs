//! Build tool resolution and version parsing (nvcc, CMake, dumpbin).

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// Minimum CUDA Toolkit version: Blackwell `sm_120a` support starts at 12.8.
pub const MIN_TOOLKIT: (u64, u64) = (12, 8);

/// Resolved path plus version of one build tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    /// Absolute path of the resolved executable.
    pub path: PathBuf,
    /// Version string reported by the tool.
    pub version: String,
}

/// Finds `name` among `paths`, trying each `PATHEXT`-style extension.
///
/// The bare name is tried first so an exact match (including an existing
/// extension) wins over extension probing.
#[must_use]
pub fn resolve_on_path(name: &str, paths: &[PathBuf], extensions: &[&str]) -> Option<PathBuf> {
    for dir in paths {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        for ext in extensions {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolves `name` against the process `PATH` and `PATHEXT` read through `env`.
#[must_use]
pub fn resolve_tool(name: &str, env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let paths: Vec<PathBuf> = env("PATH")
        .map(|value| std::env::split_paths(std::ffi::OsStr::new(&value)).collect())
        .unwrap_or_default();
    let extensions: Vec<String> = env("PATHEXT")
        .map(|value| value.split(';').map(str::to_string).collect())
        .unwrap_or_default();
    resolve_on_path(
        name,
        &paths,
        &extensions.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

/// Parses `nvcc --version` output into `(toolkit release, full version)`.
///
/// Looks for the line `Cuda compilation tools, release 13.3, V13.3.73`.
#[must_use]
pub fn parse_nvcc_version(output: &str) -> Option<(String, String)> {
    for line in output.lines() {
        if let Some(rest) = line.trim().strip_prefix("Cuda compilation tools, release ") {
            let (release, rest) = rest.split_once(',')?;
            let full = rest.trim().strip_prefix('V')?.trim().to_string();
            return Some((release.trim().to_string(), full));
        }
    }
    None
}

/// Parses the first line of `cmake --version` (`cmake version 4.4.2`).
#[must_use]
pub fn parse_cmake_version(output: &str) -> Option<String> {
    output
        .lines()
        .next()?
        .trim()
        .strip_prefix("cmake version ")
        .map(|version| version.trim().to_string())
}

/// Requires CUDA Toolkit >= [`MIN_TOOLKIT`].
///
/// # Errors
/// Returns an error when `release` is malformed or below the minimum.
pub fn require_toolkit(release: &str) -> anyhow::Result<()> {
    let (major, minor) = release
        .split_once('.')
        .and_then(|(major, minor)| Some((major.parse().ok()?, minor.parse().ok()?)))
        .with_context(|| format!("malformed CUDA Toolkit release `{release}`"))?;
    anyhow::ensure!(
        (major, minor) >= MIN_TOOLKIT,
        "CUDA Toolkit {release} is too old: llama-cuda requires >= {}.{} \
         (Blackwell sm_120a support)",
        MIN_TOOLKIT.0,
        MIN_TOOLKIT.1
    );
    Ok(())
}

/// Returns the canonical path of the `vswhere` locator, if installed.
#[must_use]
pub fn vswhere_path(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let root = env("ProgramFiles(x86)")?;
    let path = Path::new(&root).join("Microsoft Visual Studio/Installer/vswhere.exe");
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NVCC_OUTPUT: &str = "nvcc: NVIDIA (R) Cuda compiler driver\n\
         Copyright (c) 2005-2026 NVIDIA Corporation\n\
         Cuda compilation tools, release 13.3, V13.3.73\n\
         Build cuda_13.3.r13.3/compiler.38244171_0\n";

    #[test]
    fn parses_nvcc_release_and_full_version() {
        let (release, full) = parse_nvcc_version(NVCC_OUTPUT).unwrap();
        assert_eq!(release, "13.3");
        assert_eq!(full, "13.3.73");
    }

    #[test]
    fn rejects_unrecognized_nvcc_output() {
        assert!(parse_nvcc_version("not nvcc").is_none());
    }

    #[test]
    fn parses_cmake_version() {
        assert_eq!(
            parse_cmake_version("cmake version 4.4.2\n\nCMake suite"),
            Some("4.4.2".into())
        );
    }

    #[test]
    fn toolkit_floor_accepts_12_8_and_newer() {
        require_toolkit("12.8").unwrap();
        require_toolkit("13.3").unwrap();
    }

    #[test]
    fn toolkit_floor_rejects_older_and_malformed() {
        assert!(
            require_toolkit("12.7")
                .unwrap_err()
                .to_string()
                .contains("too old")
        );
        assert!(require_toolkit("11.8").is_err());
        assert!(require_toolkit("abc").is_err());
    }

    #[test]
    fn resolve_on_path_honors_extensions() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("nvcc.exe"), b"").unwrap();
        let found = resolve_on_path("nvcc", &[temp.path().to_path_buf()], &[".exe"]);
        assert_eq!(found, Some(temp.path().join("nvcc.exe")));
    }

    #[test]
    fn resolve_on_path_prefers_bare_match() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join("cmake"), b"").unwrap();
        let found = resolve_on_path("cmake", &[temp.path().to_path_buf()], &[".exe"]);
        assert_eq!(found, Some(temp.path().join("cmake")));
    }
}
