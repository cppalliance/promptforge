//! Cargo target selection for the CUDA bundle.

use anyhow::Context as _;

/// Cargo-provided target and host identity for one build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    /// `CARGO_CFG_TARGET_ARCH` (for example `x86_64`).
    pub arch: String,
    /// `CARGO_CFG_TARGET_OS` (for example `windows`).
    pub os: String,
    /// `TARGET` triple being built for.
    pub target: String,
    /// `HOST` triple performing the build.
    pub host: String,
}

impl TargetInfo {
    /// Reads the target identity from Cargo build-script environment variables.
    ///
    /// # Errors
    /// Returns an error when any of `CARGO_CFG_TARGET_ARCH`,
    /// `CARGO_CFG_TARGET_OS`, `TARGET`, or `HOST` is unset.
    pub fn from_env(env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let read = |name: &str| {
            env(name).with_context(|| format!("Cargo environment variable {name} is unset"))
        };
        Ok(Self {
            arch: read("CARGO_CFG_TARGET_ARCH")?,
            os: read("CARGO_CFG_TARGET_OS")?,
            target: read("TARGET")?,
            host: read("HOST")?,
        })
    }

    /// Returns true when the CUDA bundle applies: Windows on x86-64.
    #[must_use]
    pub fn is_windows_x86_64(&self) -> bool {
        self.arch == "x86_64" && self.os == "windows"
    }

    /// Rejects cross-compilation: the bundle compiles for the build host's
    /// visible GPUs, so host and target must be the same triple.
    ///
    /// # Errors
    /// Returns an error when `HOST` differs from `TARGET`.
    pub fn require_native(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.host == self.target,
            "llama-cuda requires a native build: host `{}` differs from target `{}`; \
             cross-compilation is not supported because the bundle is compiled for the \
             build machine's GPUs",
            self.host,
            self.target
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_string())
        }
    }

    fn windows_native() -> TargetInfo {
        TargetInfo::from_env(env_of(&[
            ("CARGO_CFG_TARGET_ARCH", "x86_64"),
            ("CARGO_CFG_TARGET_OS", "windows"),
            ("TARGET", "x86_64-pc-windows-msvc"),
            ("HOST", "x86_64-pc-windows-msvc"),
        ]))
        .unwrap()
    }

    #[test]
    fn windows_x86_64_is_supported() {
        assert!(windows_native().is_windows_x86_64());
    }

    #[test]
    fn linux_target_is_not_supported() {
        let target = TargetInfo::from_env(env_of(&[
            ("CARGO_CFG_TARGET_ARCH", "x86_64"),
            ("CARGO_CFG_TARGET_OS", "linux"),
            ("TARGET", "x86_64-unknown-linux-gnu"),
            ("HOST", "x86_64-unknown-linux-gnu"),
        ]))
        .unwrap();
        assert!(!target.is_windows_x86_64());
    }

    #[test]
    fn native_build_passes() {
        windows_native().require_native().unwrap();
    }

    #[test]
    fn cross_compilation_is_rejected() {
        let target = TargetInfo {
            host: "aarch64-pc-windows-msvc".to_string(),
            ..windows_native()
        };
        let err = target.require_native().unwrap_err();
        assert!(
            err.to_string()
                .contains("cross-compilation is not supported")
        );
    }

    #[test]
    fn missing_variable_is_an_error() {
        let err = TargetInfo::from_env(|_| None).unwrap_err();
        assert!(err.to_string().contains("CARGO_CFG_TARGET_ARCH"));
    }
}
