//! Canonical, versioned bundle manifest.

use std::fmt::Write as _;

use anyhow::Context as _;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// Bundle format version embedded in every manifest.
pub const BUNDLE_FORMAT_VERSION: u32 = 1;

/// Linkage policy: project libraries static, CUDA Toolkit runtime external.
pub const LINKAGE_POLICY: &str = "static-project-external-cuda";

/// Identity of the pinned llama.cpp source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceIdentity {
    /// Repository URL the submodule is added from.
    pub url: String,
    /// Exact commit the submodule is checked out at.
    pub commit: String,
}

/// Resolved path and version of one build tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolIdentity {
    /// Absolute path of the resolved executable.
    pub path: String,
    /// Version string reported by the tool.
    pub version: String,
}

/// One runtime file in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BundleFile {
    /// File name within the bundle directory.
    pub name: String,
    /// Lowercase hex SHA-256 of the file contents.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
}

/// The canonical bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Manifest {
    /// Bundle format version; see [`BUNDLE_FORMAT_VERSION`].
    pub bundle_format_version: u32,
    /// Pinned llama.cpp source identity.
    pub source: SourceIdentity,
    /// Target triple the bundle was compiled for.
    pub target_triple: String,
    /// Host triple that performed the build.
    pub host_triple: String,
    /// MSVC compiler identity recovered from the CMake cache.
    pub msvc: ToolIdentity,
    /// CMake identity used to configure and build.
    pub cmake: ToolIdentity,
    /// NVCC identity used as the CUDA compiler.
    pub nvcc: ToolIdentity,
    /// CUDA Toolkit release (for example `13.3`).
    pub toolkit_version: String,
    /// Normalized `CMAKE_CUDA_ARCHITECTURES` entries compiled for.
    pub architectures: Vec<String>,
    /// Full material CMake option set, sorted `-DKEY=VALUE` entries.
    pub cmake_options: Vec<String>,
    /// Linkage policy; see [`LINKAGE_POLICY`].
    pub linkage: String,
    /// External DLL names the runtime host must provide, sorted.
    pub external_dlls: Vec<String>,
    /// Runtime files in the bundle, sorted by name.
    pub files: Vec<BundleFile>,
}

impl Manifest {
    /// Serializes canonically: struct field order, two-space indent,
    /// trailing newline. Equal manifests render byte-identically.
    ///
    /// # Errors
    /// Returns an error when serialization fails.
    pub fn render(&self) -> anyhow::Result<String> {
        let body = serde_json::to_string_pretty(self).context("render manifest")?;
        Ok(format!("{body}\n"))
    }
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            bundle_format_version: BUNDLE_FORMAT_VERSION,
            source: SourceIdentity {
                url: "https://github.com/ggml-org/llama.cpp.git".to_string(),
                commit: "fb0e6b621917488d623437349fb5361e0ac21c70".to_string(),
            },
            target_triple: "x86_64-pc-windows-msvc".to_string(),
            host_triple: "x86_64-pc-windows-msvc".to_string(),
            msvc: ToolIdentity {
                path: "C:/VS/cl.exe".to_string(),
                version: "19.44".to_string(),
            },
            cmake: ToolIdentity {
                path: "C:/CMake/bin/cmake.exe".to_string(),
                version: "4.4.2".into(),
            },
            nvcc: ToolIdentity {
                path: "C:/CUDA/bin/nvcc.exe".to_string(),
                version: "13.3.73".into(),
            },
            toolkit_version: "13.3".to_string(),
            architectures: vec!["120a-real".to_string()],
            cmake_options: vec!["-DGGML_CUDA=ON".to_string()],
            linkage: LINKAGE_POLICY.to_string(),
            external_dlls: vec!["cublas64_13.dll".to_string()],
            files: vec![BundleFile {
                name: "llama-server.exe".to_string(),
                sha256: sha256_hex(b"exe-bytes"),
                size: 9,
            }],
        }
    }

    #[test]
    fn same_inputs_render_byte_identical() {
        assert_eq!(sample().render().unwrap(), sample().render().unwrap());
    }

    #[test]
    fn render_is_stable_json_with_trailing_newline() {
        let rendered = sample().render().unwrap();
        assert!(rendered.ends_with("}\n"));
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(parsed["bundle_format_version"], 1);
        assert_eq!(
            parsed["source"]["commit"],
            "fb0e6b621917488d623437349fb5361e0ac21c70"
        );
        assert_eq!(parsed["linkage"], LINKAGE_POLICY);
    }

    #[test]
    fn field_changes_change_the_rendering() {
        let mut other = sample();
        other.toolkit_version = "12.8".to_string();
        assert_ne!(sample().render().unwrap(), other.render().unwrap());
    }

    #[test]
    fn sha256_hex_is_64_lowercase_hex_chars() {
        let hex = sha256_hex(b"promptforge");
        assert_eq!(hex.len(), 64);
        assert!(
            hex.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }
}
