//! Pinned native runtimes and GGUF cache for gateway-owned local inference.
//!
//! Downloads land under the operator cache (`~/.promptforge` by default). The
//! `llama-server` build is pinned to b10082,
//! preferring GPU-enabled archives (Vulkan on Windows/Linux, Metal on macOS).
//! Speech-to-text uses a separately pinned whisper.cpp shared-library bundle.
//!
//! The module is split into cohesive units: `assets` (release table),
//! `digest` (hashing + pin validation), `archive` (extraction),
//! `confine` (cache-root path safety), `download` (HTTP transfer, scoped
//! HF auth, and the activity text reporters), and `verified`
//! (verified-digest markers). This file owns `ArtifactStore`, the
//! orchestration that ties them together.
//!
//! Progress is one line of text: a caller that runs an
//! [`Activity`] passes it down and each stage writes what it is doing
//! (`"Downloading qwen.gguf 45%"`, `"Verifying qwen.gguf 80%"`,
//! `"Extracting llama-b10082.zip 12%"`) into it; failures surface as
//! errors to the caller, which owns the log line.

mod archive;
mod assets;
mod blob;
pub(crate) mod confine;
mod digest;
mod download;
mod install;
mod staging;
mod verified;

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use gateway_config::LlamaBackend;
use gateway_progress::Activity;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::error::LocalError;

use assets::FileAsset;
use assets::{WHISPER_RELEASE, WhisperAsset};
use verified::{path_source_marker, verify_blob_with_progress};

// Re-exports consumed elsewhere in the crate (`runtime.rs`, `cache.rs`,
// `testsupport.rs`). Test-only helpers are imported directly from their
// submodules by `tests.rs`.
pub(crate) use confine::{
    enforce_private_cache_root, ensure_cache_directory, part_path, remove_cache_entry,
    rename_confined, safe_relative_path, validate_cache_path, write_synced,
};
pub(crate) use digest::hex_digest;
pub use digest::parse_expected_digest;
pub use download::{DownloadProgress, PercentText};
pub(crate) use download::{download_with_progress, hub_bearer_token_from_env};

const INSTALL_MARKER: &str = ".promptforge-install";
/// Connect timeout for artifact downloads (bounds a stalled connect).
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Whole-request timeout for an artifact download (ART-003).
///
/// The blocking reqwest client exposes no per-read timeout, so the read
/// loop enforces the idle bound itself (see `download.rs`); this
/// generous ceiling stays as the final backstop: large enough for
/// multi-gigabyte GGUF weights on a slow link, but finite so a peer that
/// accepts the connection and then sends nothing can never pin the
/// provisioning thread forever - and a reader thread parked past the idle
/// bound reaps when the ceiling drops its body.
const DOWNLOAD_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_hours(2);

type Result<T> = std::result::Result<T, LocalError>;

/// A provisioned `llama-server`: the executable plus the directories its
/// child's `PATH` must be prefixed with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProvisionedServer {
    /// Absolute path of the `llama-server` executable.
    pub(crate) executable: PathBuf,
    /// Child `PATH` prefix. Empty: every managed install ships its runtime
    /// DLLs beside the executable.
    pub(crate) path_prefix: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug)]
struct InstallAsset<'a> {
    family: &'a str,
    release: &'a str,
    platform: &'a str,
    archives: &'a [assets::ArchiveRef<'a>],
    required_name: &'a str,
    allow_cached_fallback: bool,
}

fn whisper_install_asset<'a>(
    asset: WhisperAsset<'a>,
    archives: &'a [assets::ArchiveRef<'a>],
) -> InstallAsset<'a> {
    InstallAsset {
        family: "whisper.cpp",
        release: WHISPER_RELEASE,
        platform: asset.platform,
        archives,
        required_name: asset.library_name,
        allow_cached_fallback: false,
    }
}

/// How the `llama-server` executable is chosen, from the `[local]` config
/// section.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ServerSelection<'a> {
    /// `llama_server_path`: an explicit executable path that wins over the
    /// `PROMPTFORGE_LLAMA_SERVER` environment variable and the managed
    /// download.
    pub(crate) server_path: Option<&'a str>,
    /// `llama_backend`: which build to download on Windows x86-64.
    pub(crate) backend: LlamaBackend,
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

    /// [`Self::ensure_model`] variant that writes the download and verify
    /// stages into `activity`'s text, when given. A path source has no
    /// download to report. An unpinned URL hashes once when an older cache
    /// hit lacks listing metadata, then reuses that metadata.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model_with_progress(
        &self,
        source: &str,
        sha256: Option<&str>,
        activity: Option<&Activity>,
    ) -> Result<PathBuf> {
        self.ensure_model_with_cancellation(source, sha256, activity, None)
    }

    /// [`Self::ensure_model_with_progress`] variant that stops at download
    /// chunk boundaries when `token` fires, returning
    /// [`LocalError::Cancelled`]; the staged partial stays in place for a
    /// later resume.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub fn ensure_model_with_cancellation(
        &self,
        source: &str,
        sha256: Option<&str>,
        activity: Option<&Activity>,
        token: Option<&CancellationToken>,
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
            self.ensure_blob_with_progress(asset, &destination, activity, token)?;
            return Ok(destination);
        }
        // A path source is already local: there is no download to report.
        let path = expand_tilde(source)?;
        if !path.is_file() {
            return Err(LocalError::InvalidSource {
                value: source.to_owned(),
                reason: "path is not an existing file".to_owned(),
            });
        }
        if let Some(pin) = sha256 {
            let expected = parse_expected_digest(pin)?;
            let marker = path_source_marker(&self.cache, &path)?;
            let _outcome =
                verify_blob_with_progress(&self.cache, &path, &expected, &marker, activity)?;
        }
        Ok(path)
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

/// Resolves an already-present model artifact without downloading or writing.
///
/// URL sources use the same source-keyed cache slot as [`ArtifactStore`].
/// Filesystem sources expand `~` with the same rules as provisioning.
///
/// # Errors
/// Returns [`LocalError`] when the source URL, home directory, or an existing
/// cache path is invalid.
pub fn existing_model_path(cache_root: &Path, source: &str) -> Result<Option<PathBuf>> {
    let path = if looks_like_url(source) {
        let name = filename_from_url(source)?;
        let relative = Path::new("models")
            .join(source_cache_key(source))
            .join(name);
        if !safe_relative_path(&relative) {
            return Err(LocalError::UnsafeCachePath {
                path: cache_root.join(relative),
            });
        }
        cache_root.join(relative)
    } else {
        expand_tilde(source)?
    };
    if !path.is_file() {
        return Ok(None);
    }
    if path.starts_with(cache_root) {
        validate_cache_path(cache_root, &path)?;
    }
    Ok(Some(path))
}

/// The blocking HTTP client shared by artifact provisioning and the blob
/// cache: gateway user agent, bounded connect, and a generous whole-request
/// ceiling (ART-003) behind the read loop's own idle bound.
///
/// # Errors
/// Returns [`LocalError::HttpClient`] when the client cannot be built.
pub(crate) fn download_client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("gateway/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_REQUEST_TIMEOUT)
        .build()
        .map_err(|source| LocalError::HttpClient(source.into()))
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
    if source == "~" || source.starts_with("~/") || source.starts_with("~\\") {
        return Ok(expand_tilde_against(source, &default_home_checked()?));
    }
    Ok(PathBuf::from(source))
}

/// The pure core of [`expand_tilde`]: a leading `~`, `~/`, or `~\` resolves
/// against `home`; every other spelling passes through untouched.
pub(crate) fn expand_tilde_against(source: &str, home: &Path) -> PathBuf {
    if let Some(rest) = source.strip_prefix("~/") {
        return home.join(rest);
    }
    if let Some(rest) = source.strip_prefix("~\\") {
        return home.join(rest);
    }
    if source == "~" {
        return home.to_path_buf();
    }
    PathBuf::from(source)
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
