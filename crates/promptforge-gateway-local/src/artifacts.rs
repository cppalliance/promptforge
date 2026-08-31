//! Pinned `llama-server` binaries and GGUF cache for gateway-owned local inference.
//!
//! Downloads land under the operator cache (`~/.promptforge` by default). The
//! `llama-server` build is the same b10082 pin used by `promptforge-core-tests`,
//! preferring GPU-enabled archives (Vulkan on Windows/Linux, Metal on macOS).
//! A `llama-cuda` Windows x86-64 build instead stages the embedded CUDA bundle
//! produced by the build script (see `cuda_bundle`) and never falls back to
//! the Vulkan archive.
//!
//! The module is split into cohesive units: `assets` (release table),
//! `digest` (hashing + pin validation), `archive` (extraction),
//! `confine` (cache-root path safety), `progress` (download reporting),
//! `download` (HTTP transfer + scoped HF auth), and `verified`
//! (verified-digest markers). This file owns `ArtifactStore`, the
//! orchestration that ties them together.

#[cfg(any(not(llama_cuda_embedded), test))]
mod archive;
mod assets;
mod confine;
#[cfg(any(llama_cuda_embedded, test))]
pub mod cuda_bundle;
mod digest;
mod download;
mod progress;
mod staging;
mod verified;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use promptforge_progress::ProgressHandle;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use crate::error::LocalError;

#[cfg(test)]
use archive::extract_archive;
#[cfg(not(llama_cuda_embedded))]
use archive::extract_archive_with_progress;
#[cfg(any(not(llama_cuda_embedded), test))]
use archive::find_executable;
#[cfg(not(llama_cuda_embedded))]
use archive::require_executable;
#[cfg(any(not(llama_cuda_embedded), test))]
use assets::ArchiveKind;
use assets::FileAsset;
#[cfg(not(llama_cuda_embedded))]
use assets::{LLAMA_RELEASE, ServerAsset, server_asset};
use confine::validate_tree_path;
use digest::{file_digest_with_progress, tree_digest};
use verified::{
    blob_marker_path, path_source_marker, verify_blob_with_progress, write_marker_best_effort,
};

// Re-exports consumed elsewhere in the crate (`runtime.rs`, `cache.rs`,
// `testsupport.rs`). Test-only helpers are imported directly from their
// submodules by `tests.rs`.
pub(crate) use confine::{
    enforce_private_cache_root, ensure_cache_directory, part_path, remove_cache_entry,
    rename_confined, safe_relative_path, validate_cache_path, write_synced,
};
pub(crate) use digest::hex_digest;
pub use digest::parse_expected_digest;
pub(crate) use download::{download_with_progress, hub_bearer_token_from_env};
pub use progress::{DownloadProgress, TreeProgress};

const INSTALL_MARKER: &str = ".promptforge-install";
/// Connect timeout for artifact downloads (bounds a stalled connect).
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Whole-request timeout for an artifact download (ART-003).
///
/// The pinned blocking reqwest client exposes no per-read (idle) timeout, so a
/// stalled body is bounded by a generous whole-request ceiling instead: large
/// enough for multi-gigabyte GGUF weights on a slow link, but finite so a peer
/// that accepts the connection and then sends nothing can never pin the
/// provisioning thread forever.
const DOWNLOAD_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

type Result<T> = std::result::Result<T, LocalError>;

/// A provisioned `llama-server`: the executable plus the directories its
/// child's `PATH` must be prefixed with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProvisionedServer {
    /// Absolute path of the `llama-server` executable.
    pub(crate) executable: PathBuf,
    /// Child `PATH` prefix: the staged bundle directory, then the CUDA
    /// Toolkit runtime directory. Empty for archive-installed servers.
    pub(crate) path_prefix: Vec<PathBuf>,
}

/// Cache root plus HTTP client for provisioning local inference artifacts.
#[derive(Debug)]
pub struct ArtifactStore {
    cache: PathBuf,
    client: Client,
}

impl ArtifactStore {
    /// Creates a store rooted at `cache`, creating the directory if needed.
    ///
    /// # Errors
    /// Returns [`LocalError::Io`] or [`LocalError::HttpClient`] on setup failure.
    pub fn new(cache: impl Into<PathBuf>) -> Result<Self> {
        let cache = cache.into();
        ensure_cache_directory(&cache, &cache)?;
        // Enforce the private-cache precondition the confinement design relies on
        // (owner-only root) before trusting the tree (ART-006).
        enforce_private_cache_root(&cache)?;
        Ok(Self {
            cache,
            client: download_client()?,
        })
    }

    /// Ensures the pinned GPU-capable `llama-server` for this host is installed,
    /// reporting the download, verify, and extract stages into child leaves of
    /// `progress`, when given.
    ///
    /// A CUDA-enabled Windows x86-64 build stages its embedded CUDA bundle and
    /// propagates any validation or staging failure; it never silently falls
    /// back to the Vulkan archive. Every other build keeps the archive path.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when the platform is unsupported or provisioning fails.
    pub(crate) fn provision_llama_server_with_progress(
        &self,
        progress: Option<&ProgressHandle>,
    ) -> Result<ProvisionedServer> {
        #[cfg(llama_cuda_embedded)]
        {
            let staged = cuda_bundle::stage_embedded(&self.cache)?;
            // The embedded bundle stages no download/verify/extract work.
            if let Some(handle) = progress {
                handle.complete();
            }
            Ok(ProvisionedServer {
                executable: staged.executable,
                path_prefix: staged.path_prefix,
            })
        }
        #[cfg(not(llama_cuda_embedded))]
        {
            let asset = server_asset(std::env::consts::OS, std::env::consts::ARCH)?;
            let executable = self.provision_server(asset, progress)?;
            Ok(ProvisionedServer {
                executable,
                path_prefix: Vec::new(),
            })
        }
    }

    /// Ensures a GGUF (or other blob) from `source` is available locally.
    ///
    /// `source` is either an `http(s)://` URL or a filesystem path (`~` expanded).
    /// When `sha256` is `Some`, the digest is verified after download and on cache hit.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model(&self, source: &str, sha256: Option<&str>) -> Result<PathBuf> {
        self.ensure_model_with_progress(source, sha256, None)
    }

    /// [`ensure_model`] variant that reports the download and verify stages
    /// into child leaves of `progress`, when given. A path source completes
    /// the download leaf immediately. An unpinned URL hashes once when an
    /// older cache hit lacks listing metadata, then reuses that metadata.
    /// Both stages exist in the subtree whether or not they have work. An
    /// error exit fails any leaf that has not already finished.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model_with_progress(
        &self,
        source: &str,
        sha256: Option<&str>,
        progress: Option<&ProgressHandle>,
    ) -> Result<PathBuf> {
        let download = progress.map(|handle| handle.child("download", 4.0));
        let verify = progress.map(|handle| handle.child("verify", 1.0));
        let result =
            self.ensure_model_reporting(source, sha256, download.as_ref(), verify.as_ref());
        // Terminal state is sticky, so leaves already finished inside (a
        // verified cache hit, a digest mismatch) are not failed twice.
        if result.is_err() {
            if let Some(handle) = &download {
                handle.fail();
            }
            if let Some(handle) = &verify {
                handle.fail();
            }
        }
        result
    }

    fn ensure_model_reporting(
        &self,
        source: &str,
        sha256: Option<&str>,
        download: Option<&ProgressHandle>,
        verify: Option<&ProgressHandle>,
    ) -> Result<PathBuf> {
        if looks_like_url(source) {
            let name = filename_from_url(source)?;
            // Key the cache slot by normalized source identity (ART-004) so two
            // distinct URLs that share a filename cannot collide on one path.
            let key = source_cache_key(source);
            let destination = self.cache_path(Path::new("models").join(&key).join(&name))?;
            let asset = FileAsset {
                name: &name,
                url: source,
                sha256,
            };
            self.ensure_blob_with_progress(asset, &destination, download, verify)?;
            return Ok(destination);
        }
        // A path source is already local: the download stage has no work.
        if let Some(handle) = download {
            handle.complete();
        }
        let path = expand_tilde(source)?;
        if !path.is_file() {
            return Err(LocalError::InvalidSource {
                value: source.to_owned(),
                reason: "path is not an existing file".to_owned(),
            });
        }
        match sha256 {
            Some(pin) => {
                let expected = parse_expected_digest(pin)?;
                let marker = path_source_marker(&self.cache, &path)?;
                let _outcome =
                    verify_blob_with_progress(&self.cache, &path, &expected, &marker, verify)?;
            }
            None => {
                if let Some(handle) = verify {
                    handle.complete();
                }
            }
        }
        Ok(path)
    }

    #[cfg(not(llama_cuda_embedded))]
    fn provision_server(
        &self,
        asset: ServerAsset<'_>,
        progress: Option<&ProgressHandle>,
    ) -> Result<PathBuf> {
        let download = progress.map(|handle| handle.child("download", 4.0));
        let verify = progress.map(|handle| handle.child("verify", 1.0));
        let extract = progress.map(|handle| handle.child("extract", 1.0));
        let archive = self.cache_path(Path::new("downloads").join(asset.archive_name))?;
        let archive_asset = FileAsset {
            name: asset.archive_name,
            url: asset.url,
            sha256: Some(asset.sha256),
        };
        self.ensure_blob_with_progress(
            archive_asset,
            &archive,
            download.as_ref(),
            verify.as_ref(),
        )?;

        let install = self.cache_path(
            Path::new("llama.cpp").join(format!("{LLAMA_RELEASE}-{}", asset.platform)),
        )?;
        let _lock = self.lock_artifact(&install)?;
        validate_cache_path(&self.cache, &install)?;
        if Self::install_is_valid(&install, asset.sha256)? {
            // A valid install skips extraction entirely.
            if let Some(handle) = &extract {
                handle.complete();
            }
            return find_executable(&install, asset.executable_name, asset.archive_name);
        }
        self.ensure_blob(archive_asset, &archive)?;
        validate_cache_path(&self.cache, &archive)?;

        remove_cache_entry(&self.cache, &install)?;
        let staging = part_path(&install);
        remove_cache_entry(&self.cache, &staging)?;
        ensure_cache_directory(&self.cache, &staging)?;

        if let Err(error) =
            extract_archive_with_progress(&archive, &staging, asset.archive_kind, extract.as_ref())
        {
            let _ignored = fs::remove_dir_all(&staging);
            return Err(error);
        }

        let staged_executable =
            find_executable(&staging, asset.executable_name, asset.archive_name)?;
        if asset.archive_kind == ArchiveKind::TarGz {
            require_executable(&staged_executable, asset.archive_name)?;
        }
        let relative_executable =
            staged_executable
                .strip_prefix(&staging)
                .map_err(|source| LocalError::Io {
                    operation: "resolve staged executable",
                    path: staged_executable.clone(),
                    source: io::Error::other(source),
                })?;
        let tree_sha256 = tree_digest(&staging)?;
        let marker = staging.join(INSTALL_MARKER);
        validate_cache_path(&self.cache, &marker)?;
        write_synced(
            &marker,
            format!("{}\n{tree_sha256}\n", asset.sha256).as_bytes(),
        )?;
        rename_confined(&self.cache, &staging, &install)?;
        Ok(install.join(relative_executable))
    }

    fn install_is_valid(install: &Path, archive_sha256: &str) -> Result<bool> {
        if !install.is_dir() {
            return Ok(false);
        }
        let marker = install.join(INSTALL_MARKER);
        validate_tree_path(install, &marker)?;
        let marker_text = match fs::read_to_string(&marker) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "read install marker",
                    path: marker,
                    source,
                });
            }
        };
        let mut lines = marker_text.lines();
        let Some(recorded_archive) = lines.next() else {
            return Ok(false);
        };
        let Some(recorded_tree) = lines.next() else {
            return Ok(false);
        };
        if lines.next().is_some() || recorded_archive != archive_sha256 {
            return Ok(false);
        }
        Ok(tree_digest(install)? == recorded_tree)
    }

    #[cfg(not(llama_cuda_embedded))]
    fn ensure_blob(&self, asset: FileAsset<'_>, destination: &Path) -> Result<()> {
        self.ensure_blob_with_progress(asset, destination, None, None)
    }

    /// `ensure_blob` variant that reports the download and verify stages
    /// into the given leaves. Both leaves reach their terminal event on every
    /// exit path: a cache hit completes the download leaf without work, and
    /// the pin check after a download completes the verify leaf whether the
    /// digest matches or not.
    fn ensure_blob_with_progress(
        &self,
        asset: FileAsset<'_>,
        destination: &Path,
        download: Option<&ProgressHandle>,
        verify: Option<&ProgressHandle>,
    ) -> Result<()> {
        let _lock = self.lock_artifact(destination)?;
        validate_cache_path(&self.cache, destination)?;
        let staging = part_path(destination);
        remove_cache_entry(&self.cache, &staging)?;

        // Validate/canonicalize the pin once, at the boundary, so both the
        // cache-hit and post-download comparisons are case-insensitive and a
        // malformed pin fails fast rather than always mismatching.
        let expected_digest = asset.sha256.map(parse_expected_digest).transpose()?;

        // Set once the verify leaf has emitted its terminal event, so the pin
        // recheck after a mismatch-repair download never emits it twice.
        let mut verify_finished = false;
        if destination.is_file() {
            let Some(expected) = expected_digest.as_deref() else {
                if let Some(handle) = download {
                    handle.complete();
                }
                if !crate::cache::blob_meta_matches(destination, asset.url)? {
                    let actual = file_digest_with_progress(destination, verify)?;
                    crate::cache::write_blob_meta(&self.cache, destination, asset.url, &actual)?;
                }
                if let Some(handle) = verify {
                    handle.complete();
                }
                return Ok(());
            };
            let marker = blob_marker_path(destination);
            match verify_blob_with_progress(&self.cache, destination, expected, &marker, verify) {
                Ok(_) => {
                    // A verified cache hit leaves the download stage
                    // with no work.
                    if let Some(handle) = download {
                        handle.complete();
                    }
                    crate::cache::write_blob_meta(&self.cache, destination, asset.url, expected)?;
                    return Ok(());
                }
                // A pin mismatch on a cached blob is repaired by
                // re-downloading; every other failure propagates.
                Err(LocalError::DigestMismatch { .. }) => {
                    verify_finished = true;
                    remove_cache_entry(&self.cache, destination)?;
                }
                Err(error) => return Err(error),
            }
        } else if destination.exists() {
            remove_cache_entry(&self.cache, destination)?;
        }

        let Some(parent) = destination.parent() else {
            return Err(LocalError::InvalidPath {
                path: destination.to_owned(),
            });
        };
        ensure_cache_directory(&self.cache, parent)?;
        validate_cache_path(&self.cache, &staging)?;
        let actual = match download::download(&self.client, asset.url, &staging, download) {
            Ok(actual) => actual,
            Err(error) => {
                if !verify_finished && let Some(handle) = verify {
                    handle.complete();
                }
                let _ignored = fs::remove_file(&staging);
                return Err(error);
            }
        };
        // The pin is checked against the digest computed inline during the
        // download, so the verify stage's work ends here on every outcome.
        if !verify_finished && let Some(handle) = verify {
            handle.complete();
        }
        if let Some(expected) = expected_digest.as_deref()
            && actual != expected
        {
            remove_cache_entry(&self.cache, &staging)?;
            return Err(LocalError::DigestMismatch {
                name: asset.name.to_owned(),
                expected: expected.to_owned(),
                actual,
            });
        }
        rename_confined(&self.cache, &staging, destination)?;
        if let Some(expected) = expected_digest.as_deref() {
            let marker = blob_marker_path(destination);
            // Confinement stays a hard error; only the marker write degrades.
            validate_cache_path(&self.cache, &marker)?;
            write_marker_best_effort(&marker, destination, expected);
        }
        crate::cache::write_blob_meta(
            &self.cache,
            destination,
            asset.url,
            expected_digest.as_deref().unwrap_or(&actual),
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn download_with_progress(
        &self,
        url: &str,
        destination: &Path,
        progress: &dyn progress::DownloadProgress,
    ) -> Result<String> {
        download::download_with_progress(&self.client, url, destination, progress)
    }

    fn cache_path(&self, relative: PathBuf) -> Result<PathBuf> {
        if !safe_relative_path(&relative) {
            return Err(LocalError::UnsafeCachePath {
                path: self.cache.join(relative),
            });
        }
        let path = self.cache.join(relative);
        validate_cache_path(&self.cache, &path)?;
        Ok(path)
    }

    fn lock_artifact(&self, artifact: &Path) -> Result<File> {
        lock_artifact(&self.cache, artifact)
    }
}

/// The blocking HTTP client shared by artifact provisioning and the blob
/// cache: gateway user agent, bounded connect, and a generous whole-request
/// ceiling (ART-003) in place of a per-read idle timeout.
///
/// # Errors
/// Returns [`LocalError::HttpClient`] when the client cannot be built.
pub(crate) fn download_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("promptforge-gateway/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_REQUEST_TIMEOUT)
        .build()
        .map_err(LocalError::HttpClient)
}

/// Takes the advisory OS lock serializing publishers of `artifact` under
/// `cache`, keyed by the artifact's cache-relative path.
///
/// The returned handle owns the lock; dropping it releases. Both the artifact
/// and the lock file are confinement-checked before use (ART-006/007).
///
/// # Errors
/// Returns [`LocalError`] when a path is unsafe or the lock cannot be taken.
pub(crate) fn lock_artifact(cache: &Path, artifact: &Path) -> Result<File> {
    validate_cache_path(cache, artifact)?;
    let relative = artifact
        .strip_prefix(cache)
        .map_err(|_| LocalError::UnsafeCachePath {
            path: artifact.to_owned(),
        })?;
    let mut hasher = Sha256::new();
    hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
    let lock_directory = cache.join(".locks");
    ensure_cache_directory(cache, &lock_directory)?;
    let lock_path = lock_directory.join(format!("{}.lock", hex_digest(hasher)));
    validate_cache_path(cache, &lock_path)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| LocalError::Io {
            operation: "open artifact lock",
            path: lock_path.clone(),
            source,
        })?;
    lock.lock().map_err(|source| LocalError::Io {
        operation: "lock artifact",
        path: lock_path,
        source,
    })?;
    validate_cache_path(cache, artifact)?;
    Ok(lock)
}

/// Whether a `[[local_model]]` source names a download rather than a path.
pub(crate) fn looks_like_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

/// A stable, filesystem-safe cache-slot key derived from the full source URL.
///
/// Two different URLs that share a filename map to different slots (ART-004),
/// while the same URL always maps to the same slot so a cache hit is stable.
pub(crate) fn source_cache_key(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hex_digest(hasher).chars().take(16).collect()
}

/// The URL's final path segment, validated as a safe relative filename.
///
/// # Errors
/// Returns [`LocalError::InvalidSource`] when the URL has no filename segment
/// or the segment is not a safe relative path.
pub fn filename_from_url(url: &str) -> Result<String> {
    let without_query = url.split('?').next().unwrap_or(url);
    let name = without_query
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| LocalError::InvalidSource {
            value: url.to_owned(),
            reason: "URL has no filename segment".to_owned(),
        })?;
    if !safe_relative_path(Path::new(name)) {
        return Err(LocalError::InvalidSource {
            value: url.to_owned(),
            reason: "URL filename is not a safe relative path".to_owned(),
        });
    }
    Ok(name.to_owned())
}

/// Expands a leading `~` in a path source against the operator home.
///
/// # Errors
/// Returns [`LocalError::MissingHome`] when the source needs a home directory
/// and none is available.
pub(crate) fn expand_tilde(source: &str) -> Result<PathBuf> {
    if let Some(rest) = source.strip_prefix("~/") {
        return Ok(default_home_checked()?.join(rest));
    }
    if let Some(rest) = source.strip_prefix("~\\") {
        return Ok(default_home_checked()?.join(rest));
    }
    if source == "~" {
        return default_home_checked();
    }
    Ok(PathBuf::from(source))
}

/// Resolves the operator home for artifact provisioning, or a typed error.
///
/// Returns [`LocalError::MissingHome`] rather than silently using the working
/// directory when the home variable is unset or empty (ART-009).
pub(crate) fn default_home_checked() -> Result<PathBuf> {
    #[cfg(windows)]
    let (var, value) = ("USERPROFILE", std::env::var_os("USERPROFILE"));
    #[cfg(not(windows))]
    let (var, value) = ("HOME", std::env::var_os("HOME"));
    home_or_missing(var, value)
}

/// Pure resolver: an empty or absent home value is a [`LocalError::MissingHome`].
fn home_or_missing(var: &'static str, value: Option<std::ffi::OsString>) -> Result<PathBuf> {
    match value {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => Err(LocalError::MissingHome { var }),
    }
}

/// Default artifact root (`~/.promptforge`), erroring when home is unset (ART-009).
pub(crate) fn default_promptforge_root_checked() -> Result<PathBuf> {
    Ok(default_home_checked()?.join(".promptforge"))
}

#[cfg(test)]
mod tests;
