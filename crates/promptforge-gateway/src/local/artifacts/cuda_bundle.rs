//! Runtime staging of the embedded CUDA `llama-server` bundle.
//!
//! A `llama-cuda` Windows x86-64 build embeds the manifest and file bytes the
//! build script produced (see `crate::llama_cuda_bundle`). This module is the
//! only consumer: it decodes the manifest through a narrow runtime-side schema
//! (the gateway never depends on the build-support crate at runtime), validates
//! the payload against it, verifies the host provides the declared external
//! CUDA Toolkit DLLs, and publishes the files into the operator cache through
//! the same advisory lock, private staging directory, tree digest, install
//! marker, and atomic rename the archive path uses.
//!
//! # Toolkit dependency check
//!
//! The manifest records the CUDA Toolkit version the bundle was compiled
//! against and the external DLL names the host must provide. The runtime
//! directory is resolved from the environment the CUDA Toolkit installer
//! registers: `CUDA_PATH_V<major>_<minor>` (for example `CUDA_PATH_V13_3`)
//! wins so a multi-toolkit host selects the matching release, with the
//! version-agnostic `CUDA_PATH` as the single-toolkit fallback; the runtime
//! directory is `<root>/bin`. Each external DLL must then be present either in
//! that directory or, for Windows system DLLs such as `KERNEL32.dll`, in
//! `<SystemRoot>/System32`. A DLL resolvable in neither place fails staging
//! before anything is published.
//!
//! # Ordering
//!
//! The embedded payload is fully validated (schema, filenames, sizes,
//! digests, target, toolkit) before the cache is consulted, so tampered
//! embedded bytes fail even when a valid installation already exists. A valid
//! matching installation then returns immediately without restaging.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::confine::{
    ensure_cache_directory, part_path, remove_cache_entry, rename_confined, safe_relative_path,
    validate_cache_path, write_synced,
};
use super::digest::tree_digest;
use super::{ArtifactStore, INSTALL_MARKER, Result, hex_digest, lock_artifact};
use crate::local::error::LocalError;

/// Bundle format version this runtime decodes. Mirrors the build-side
/// contract constant; the runtime deliberately does not import it.
const SUPPORTED_FORMAT: u32 = 1;
/// Linkage policy this runtime stages: project libraries are bundled, the
/// CUDA Toolkit runtime stays external.
const EXPECTED_LINKAGE: &str = "static-project-external-cuda";
/// The only target triple an embedded CUDA bundle is produced for.
const BUNDLE_TARGET: &str = "x86_64-pc-windows-msvc";
/// The server executable every bundle must contain.
const SERVER_EXECUTABLE: &str = "llama-server.exe";

/// A failure validating or extracting the embedded CUDA bundle.
///
/// Wrapped by [`LocalError::CudaBundle`]; build-script failures never reach
/// this type - they fail the Cargo build itself.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum BundleError {
    /// The embedded manifest JSON did not decode into the runtime schema.
    #[error("decode embedded manifest")]
    ManifestDecode(#[source] serde_json::Error),

    /// The manifest's bundle format version is not supported.
    #[error("unsupported bundle format version {found}")]
    UnsupportedFormat {
        /// The version the manifest declared.
        found: u32,
    },

    /// The manifest's linkage policy is not the expected one.
    #[error("unexpected linkage policy `{found}`")]
    UnexpectedLinkage {
        /// The linkage policy the manifest declared.
        found: String,
    },

    /// The bundle was compiled for a different target than this build.
    #[error("bundle target `{found}` does not match this build's `{expected}`")]
    TargetMismatch {
        /// The target this runtime stages for.
        expected: String,
        /// The target the manifest declared.
        found: String,
    },

    /// A manifest file or DLL name is not a bare, safe filename.
    #[error("unsafe bundle file name `{name}`")]
    UnsafeFileName {
        /// The offending name.
        name: String,
    },

    /// A manifest digest is not 64 lowercase hexadecimal characters.
    #[error("malformed sha-256 for `{name}`")]
    MalformedDigest {
        /// The file whose digest is malformed.
        name: String,
    },

    /// The payload's byte length disagrees with the manifest.
    #[error("size mismatch for `{name}`: manifest says {expected} bytes, payload has {actual}")]
    SizeMismatch {
        /// The file whose size disagrees.
        name: String,
        /// The manifest's recorded size.
        expected: u64,
        /// The payload's actual size.
        actual: u64,
    },

    /// The payload's contents disagree with the manifest digest.
    #[error("sha-256 mismatch for `{name}`: expected {expected}, got {actual}")]
    DigestMismatch {
        /// The file whose digest disagrees.
        name: String,
        /// The manifest's recorded lowercase hex digest.
        expected: String,
        /// The payload's actual lowercase hex digest.
        actual: String,
    },

    /// The payload does not contain a manifest-listed file.
    #[error("payload is missing `{name}`")]
    MissingFile {
        /// The manifest-listed name absent from the payload.
        name: String,
    },

    /// The payload contains a file the manifest does not list.
    #[error("payload contains unlisted file `{name}`")]
    UnlistedFile {
        /// The payload name absent from the manifest.
        name: String,
    },

    /// The bundle contains no `llama-server.exe`.
    #[error("bundle contains no {SERVER_EXECUTABLE}")]
    MissingExecutable,

    /// No CUDA Toolkit runtime directory could be resolved.
    #[error(
        "no CUDA Toolkit {version} runtime directory found; set CUDA_PATH_V{} or CUDA_PATH",
        version.replace('.', "_")
    )]
    ToolkitNotFound {
        /// The toolkit version the bundle was compiled against.
        version: String,
    },

    /// An external DLL is resolvable neither in the toolkit runtime directory
    /// nor in the system directory.
    #[error("external DLL `{dll}` not found in `{directory}` or the system directory")]
    MissingToolkitDependency {
        /// The unresolvable DLL name.
        dll: String,
        /// The toolkit runtime directory that was probed.
        directory: PathBuf,
    },
}

/// The embedded bundle payload: canonical manifest JSON plus file bytes.
#[derive(Clone, Copy, Debug)]
pub(super) struct BundlePayload<'a> {
    /// Canonical pretty-JSON manifest text.
    pub(super) manifest: &'a str,
    /// File name to contents, exactly as embedded by the build.
    pub(super) files: &'a [(&'a str, &'a [u8])],
}

/// A verified, published CUDA bundle installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct StagedCudaBundle {
    /// Absolute path of the staged `llama-server.exe`.
    pub(super) executable: PathBuf,
    /// Directories the child's `PATH` prepends, in order: the staged
    /// directory, then the CUDA Toolkit runtime directory.
    pub(super) path_prefix: Vec<PathBuf>,
}

/// The runtime-side decode of one manifest file entry.
#[derive(Debug, Deserialize)]
struct RuntimeFile {
    name: String,
    sha256: String,
    size: u64,
}

/// The narrow runtime-side decode of the canonical manifest. Build-only
/// fields (tool identities, CMake options, architectures) are ignored.
#[derive(Debug, Deserialize)]
struct RuntimeManifest {
    bundle_format_version: u32,
    target_triple: String,
    toolkit_version: String,
    linkage: String,
    external_dlls: Vec<String>,
    files: Vec<RuntimeFile>,
}

impl RuntimeManifest {
    /// Decodes and validates the manifest schema: format version, linkage,
    /// target, bare safe filenames, well-formed digests, and the presence of
    /// the server executable.
    ///
    /// # Errors
    /// Returns the matching [`BundleError`] variant for the first violation.
    fn decode(json: &str) -> std::result::Result<RuntimeManifest, BundleError> {
        let manifest: RuntimeManifest =
            serde_json::from_str(json).map_err(BundleError::ManifestDecode)?;
        if manifest.bundle_format_version != SUPPORTED_FORMAT {
            return Err(BundleError::UnsupportedFormat {
                found: manifest.bundle_format_version,
            });
        }
        if manifest.linkage != EXPECTED_LINKAGE {
            return Err(BundleError::UnexpectedLinkage {
                found: manifest.linkage,
            });
        }
        if manifest.target_triple != BUNDLE_TARGET {
            return Err(BundleError::TargetMismatch {
                expected: BUNDLE_TARGET.to_owned(),
                found: manifest.target_triple,
            });
        }
        for file in &manifest.files {
            if !is_bare_filename(&file.name) {
                return Err(BundleError::UnsafeFileName {
                    name: file.name.clone(),
                });
            }
            if !is_lower_hex_digest(&file.sha256) {
                return Err(BundleError::MalformedDigest {
                    name: file.name.clone(),
                });
            }
        }
        for dll in &manifest.external_dlls {
            if !is_bare_filename(dll) {
                return Err(BundleError::UnsafeFileName { name: dll.clone() });
            }
        }
        if !manifest
            .files
            .iter()
            .any(|file| file.name == SERVER_EXECUTABLE)
        {
            return Err(BundleError::MissingExecutable);
        }
        Ok(manifest)
    }
}

/// A name is safe to stage when it is exactly one normal path component.
fn is_bare_filename(name: &str) -> bool {
    let path = Path::new(name);
    safe_relative_path(path) && path.components().count() == 1
}

/// The canonical digest form: exactly 64 lowercase hex characters, matching
/// what [`hex_digest`] produces so comparisons never fail on case alone.
fn is_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Cross-checks the payload against the manifest: every listed file present
/// with matching size and digest, and no unlisted payload files.
///
/// # Errors
/// Returns [`BundleError::MissingFile`], [`BundleError::UnlistedFile`],
/// [`BundleError::SizeMismatch`], or [`BundleError::DigestMismatch`].
fn validate_payload(
    manifest: &RuntimeManifest,
    payload: &BundlePayload<'_>,
) -> std::result::Result<(), BundleError> {
    for file in &manifest.files {
        let Some((_, bytes)) = payload.files.iter().find(|(name, _)| *name == file.name) else {
            return Err(BundleError::MissingFile {
                name: file.name.clone(),
            });
        };
        if bytes.len() as u64 != file.size {
            return Err(BundleError::SizeMismatch {
                name: file.name.clone(),
                expected: file.size,
                actual: bytes.len() as u64,
            });
        }
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let actual = hex_digest(hasher);
        if actual != file.sha256 {
            return Err(BundleError::DigestMismatch {
                name: file.name.clone(),
                expected: file.sha256.clone(),
                actual,
            });
        }
    }
    for (name, _) in payload.files {
        if !manifest.files.iter().any(|file| file.name == *name) {
            return Err(BundleError::UnlistedFile {
                name: (*name).to_owned(),
            });
        }
    }
    Ok(())
}

/// Resolves the CUDA Toolkit runtime (`bin`) directory for `toolkit_version`.
///
/// See the module docs for the mechanism: the versioned installer variable
/// wins, `CUDA_PATH` is the fallback, and the directory must exist.
///
/// # Errors
/// Returns [`BundleError::ToolkitNotFound`] when no candidate resolves to an
/// existing `bin` directory.
fn toolkit_bin_dir(
    env: &dyn Fn(&str) -> Option<OsString>,
    toolkit_version: &str,
) -> std::result::Result<PathBuf, BundleError> {
    let versioned = format!("CUDA_PATH_V{}", toolkit_version.replace('.', "_"));
    for variable in [versioned.as_str(), "CUDA_PATH"] {
        if let Some(root) = env(variable).filter(|value| !value.is_empty()) {
            let bin = PathBuf::from(root).join("bin");
            if bin.is_dir() {
                return Ok(bin);
            }
        }
    }
    Err(BundleError::ToolkitNotFound {
        version: toolkit_version.to_owned(),
    })
}

/// Requires every manifest-declared external DLL to resolve: in the toolkit
/// runtime directory, or in `<SystemRoot>/System32` for Windows system DLLs.
///
/// # Errors
/// Returns [`BundleError::MissingToolkitDependency`] for the first DLL found
/// in neither place.
fn require_external_dlls(
    env: &dyn Fn(&str) -> Option<OsString>,
    manifest: &RuntimeManifest,
    toolkit_bin: &Path,
) -> std::result::Result<(), BundleError> {
    let system32 = env("SystemRoot")
        .filter(|value| !value.is_empty())
        .map(|root| PathBuf::from(root).join("System32"));
    for dll in &manifest.external_dlls {
        let in_system = system32.as_ref().is_some_and(|dir| dir.join(dll).is_file());
        if in_system || toolkit_bin.join(dll).is_file() {
            continue;
        }
        return Err(BundleError::MissingToolkitDependency {
            dll: dll.clone(),
            directory: toolkit_bin.to_owned(),
        });
    }
    Ok(())
}

/// The cache-relative install directory name for the embedded bundle.
fn install_dir_name() -> String {
    format!("cuda-{}-{BUNDLE_TARGET}", super::assets::LLAMA_RELEASE)
}

/// Validates and publishes `payload` under `cache`, returning the staged
/// executable and the child `PATH` prefix.
///
/// A valid matching installation (marker identity plus tree digest) returns
/// immediately without restaging. Staging writes into the private `.part`
/// sibling and publishes with an atomic rename under the advisory artifact
/// lock; a staging failure removes the partial directory, and a stale `.part`
/// from an interrupted run is removed before restaging.
///
/// # Errors
/// Returns [`LocalError::CudaBundle`] for manifest, payload, target, or
/// toolkit validation failures, and the shared [`LocalError`] I/O and
/// confinement variants for cache failures.
pub(super) fn stage_bundle(
    cache: &Path,
    payload: &BundlePayload<'_>,
    env: &dyn Fn(&str) -> Option<OsString>,
) -> Result<StagedCudaBundle> {
    let manifest = RuntimeManifest::decode(payload.manifest)?;
    validate_payload(&manifest, payload)?;
    let toolkit_bin = toolkit_bin_dir(env, &manifest.toolkit_version)?;
    require_external_dlls(env, &manifest, &toolkit_bin)?;

    let mut identity_hasher = Sha256::new();
    identity_hasher.update(payload.manifest.as_bytes());
    let identity = hex_digest(identity_hasher);

    let install = cache.join("llama.cpp").join(install_dir_name());
    let _lock = lock_artifact(cache, &install)?;
    validate_cache_path(cache, &install)?;
    if ArtifactStore::install_is_valid(&install, &identity)? {
        return Ok(StagedCudaBundle {
            executable: install.join(SERVER_EXECUTABLE),
            path_prefix: vec![install, toolkit_bin],
        });
    }

    remove_cache_entry(cache, &install)?;
    let staging = part_path(&install);
    remove_cache_entry(cache, &staging)?;
    ensure_cache_directory(cache, &staging)?;

    if let Err(error) = stage_files(cache, &staging, &manifest, payload, &identity) {
        let _ignored = fs::remove_dir_all(&staging);
        return Err(error);
    }
    rename_confined(cache, &staging, &install)?;
    Ok(StagedCudaBundle {
        executable: install.join(SERVER_EXECUTABLE),
        path_prefix: vec![install, toolkit_bin],
    })
}

/// Writes the payload files, tree digest, and install marker into `staging`.
///
/// # Errors
/// Returns the shared [`LocalError`] I/O and confinement variants; the caller
/// removes the partial staging directory.
fn stage_files(
    cache: &Path,
    staging: &Path,
    manifest: &RuntimeManifest,
    payload: &BundlePayload<'_>,
    identity: &str,
) -> Result<()> {
    for file in &manifest.files {
        let (_, bytes) = payload
            .files
            .iter()
            .find(|(name, _)| *name == file.name)
            .ok_or_else(|| BundleError::MissingFile {
                name: file.name.clone(),
            })?;
        let path = staging.join(&file.name);
        validate_cache_path(cache, &path)?;
        write_synced(&path, bytes)?;
    }
    let tree = tree_digest(staging)?;
    let marker = staging.join(INSTALL_MARKER);
    validate_cache_path(cache, &marker)?;
    write_synced(&marker, format!("{identity}\n{tree}\n").as_bytes())
}

/// Stages the build-embedded bundle from `crate::llama_cuda_bundle` against
/// the real process environment.
///
/// # Errors
/// See [`stage_bundle`].
#[cfg(llama_cuda_embedded)]
pub(super) fn stage_embedded(cache: &Path) -> Result<StagedCudaBundle> {
    let payload = BundlePayload {
        manifest: crate::llama_cuda_bundle::MANIFEST,
        files: crate::llama_cuda_bundle::FILES,
    };
    stage_bundle(cache, &payload, &|name| std::env::var_os(name))
}

#[cfg(test)]
mod tests;
