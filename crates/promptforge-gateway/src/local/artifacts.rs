//! Pinned `llama-server` binaries and GGUF cache for gateway-owned local inference.
//!
//! Downloads land under the operator cache (`~/.promptforge` by default). The
//! `llama-server` build is the same b10082 pin used by `promptforge-core-tests`,
//! preferring GPU-enabled archives (Vulkan on Windows/Linux, Metal on macOS).
//!
//! The module is split into cohesive units: [`assets`] (release table),
//! [`digest`] (hashing + pin validation), [`archive`] (extraction),
//! [`confine`] (cache-root path safety), [`progress`] (download reporting), and
//! [`download`] (HTTP transfer + scoped HF auth). This file owns
//! [`ArtifactStore`], the orchestration that ties them together.

mod archive;
mod assets;
mod confine;
mod digest;
mod download;
mod progress;

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use reqwest::blocking::Client;
use sha2::{Digest, Sha256};

use crate::local::error::LocalError;

use archive::{extract_archive, find_executable, require_executable};
use assets::{ArchiveKind, FileAsset, LLAMA_RELEASE, ServerAsset, server_asset};
use confine::{
    ensure_cache_directory, part_path, remove_cache_entry, rename_confined, safe_relative_path,
    validate_cache_path, validate_tree_path, write_synced,
};
use digest::{file_digest, hex_digest, parse_expected_digest, tree_digest};

// Re-export consumed elsewhere in the crate (`local/mod.rs`). Test-only helpers
// are imported directly from their submodules by `tests.rs`.
pub(crate) use download::hub_bearer_token_from_env;

const INSTALL_MARKER: &str = ".promptforge-install";
/// Connect timeout for artifact downloads (bounds a stalled connect).
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

type Result<T> = std::result::Result<T, LocalError>;

/// Cache root plus HTTP client for provisioning local inference artifacts.
#[derive(Debug)]
pub(crate) struct ArtifactStore {
    cache: PathBuf,
    client: Client,
}

impl ArtifactStore {
    /// Creates a store rooted at `cache`, creating the directory if needed.
    ///
    /// # Errors
    /// Returns [`LocalError::Io`] or [`LocalError::HttpClient`] on setup failure.
    pub(crate) fn new(cache: impl Into<PathBuf>) -> Result<Self> {
        let cache = cache.into();
        ensure_cache_directory(&cache, &cache)?;
        let client = Client::builder()
            .user_agent(concat!("promptforge-gateway/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
            .build()
            .map_err(LocalError::HttpClient)?;
        Ok(Self { cache, client })
    }

    /// Ensures the pinned GPU-capable `llama-server` for this host is installed.
    ///
    /// # Errors
    /// Returns a [`LocalError`] when the platform is unsupported or provisioning fails.
    pub(crate) fn provision_llama_server(&self) -> Result<PathBuf> {
        let asset = server_asset(std::env::consts::OS, std::env::consts::ARCH)?;
        self.provision_server(asset)
    }

    /// Ensures a GGUF (or other blob) from `source` is available locally.
    ///
    /// `source` is either an `http(s)://` URL or a filesystem path (`~` expanded).
    /// When `sha256` is `Some`, the digest is verified after download and on cache hit.
    ///
    /// # Errors
    /// Returns a [`LocalError`] on download, verification, or path failures.
    pub(crate) fn ensure_model(&self, source: &str, sha256: Option<&str>) -> Result<PathBuf> {
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
            self.ensure_blob(asset, &destination)?;
            return Ok(destination);
        }
        let path = expand_tilde(source);
        if !path.is_file() {
            return Err(LocalError::InvalidSource {
                value: source.to_owned(),
                reason: "path is not an existing file".to_owned(),
            });
        }
        if let Some(expected) = sha256 {
            let expected = parse_expected_digest(expected)?;
            let actual = file_digest(&path)?;
            if actual != expected {
                return Err(LocalError::DigestMismatch {
                    name: path.display().to_string(),
                    expected,
                    actual,
                });
            }
        }
        Ok(path)
    }

    fn provision_server(&self, asset: ServerAsset<'_>) -> Result<PathBuf> {
        let archive = self.cache_path(Path::new("downloads").join(asset.archive_name))?;
        let archive_asset = FileAsset {
            name: asset.archive_name,
            url: asset.url,
            sha256: Some(asset.sha256),
        };
        self.ensure_blob(archive_asset, &archive)?;

        let install = self.cache_path(
            Path::new("llama.cpp").join(format!("{LLAMA_RELEASE}-{}", asset.platform)),
        )?;
        let _lock = self.lock_artifact(&install)?;
        validate_cache_path(&self.cache, &install)?;
        if Self::install_is_valid(&install, asset.sha256)? {
            return find_executable(&install, asset.executable_name, asset.archive_name);
        }
        self.ensure_blob(archive_asset, &archive)?;
        validate_cache_path(&self.cache, &archive)?;

        remove_cache_entry(&self.cache, &install)?;
        let staging = part_path(&install);
        remove_cache_entry(&self.cache, &staging)?;
        ensure_cache_directory(&self.cache, &staging)?;

        if let Err(error) = extract_archive(&archive, &staging, asset.archive_kind) {
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

    fn ensure_blob(&self, asset: FileAsset<'_>, destination: &Path) -> Result<()> {
        let _lock = self.lock_artifact(destination)?;
        validate_cache_path(&self.cache, destination)?;
        let staging = part_path(destination);
        remove_cache_entry(&self.cache, &staging)?;

        // Validate/canonicalize the pin once, at the boundary, so both the
        // cache-hit and post-download comparisons are case-insensitive and a
        // malformed pin fails fast rather than always mismatching.
        let expected_digest = asset.sha256.map(parse_expected_digest).transpose()?;

        if destination.is_file() {
            match expected_digest.as_deref() {
                Some(expected) => {
                    if file_digest(destination)? == expected {
                        return Ok(());
                    }
                    remove_cache_entry(&self.cache, destination)?;
                }
                None => return Ok(()),
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
        let actual = match self.download(asset.url, &staging) {
            Ok(actual) => actual,
            Err(error) => {
                let _ignored = fs::remove_file(&staging);
                return Err(error);
            }
        };
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
        rename_confined(&self.cache, &staging, destination)
    }

    fn download(&self, url: &str, destination: &Path) -> Result<String> {
        download::download(&self.client, url, destination)
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
        validate_cache_path(&self.cache, artifact)?;
        let relative =
            artifact
                .strip_prefix(&self.cache)
                .map_err(|_| LocalError::UnsafeCachePath {
                    path: artifact.to_owned(),
                })?;
        let mut hasher = Sha256::new();
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        let lock_directory = self.cache.join(".locks");
        ensure_cache_directory(&self.cache, &lock_directory)?;
        let lock_path = lock_directory.join(format!("{}.lock", hex_digest(hasher)));
        validate_cache_path(&self.cache, &lock_path)?;
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
        validate_cache_path(&self.cache, artifact)?;
        Ok(lock)
    }
}

fn looks_like_url(source: &str) -> bool {
    source.starts_with("https://") || source.starts_with("http://")
}

/// A stable, filesystem-safe cache-slot key derived from the full source URL.
///
/// Two different URLs that share a filename map to different slots (ART-004),
/// while the same URL always maps to the same slot so a cache hit is stable.
fn source_cache_key(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    hex_digest(hasher).chars().take(16).collect()
}

fn filename_from_url(url: &str) -> Result<String> {
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

fn expand_tilde(source: &str) -> PathBuf {
    if let Some(rest) = source.strip_prefix("~/") {
        return default_home().join(rest);
    }
    if let Some(rest) = source.strip_prefix("~\\") {
        return default_home().join(rest);
    }
    if source == "~" {
        return default_home();
    }
    PathBuf::from(source)
}

/// Returns the operator home directory used for `~` expansion and defaults.
#[must_use]
pub(crate) fn default_home() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
    }
}

/// Default root for gateway local artifacts (`~/.promptforge`).
#[must_use]
pub(crate) fn default_promptforge_root() -> PathBuf {
    default_home().join(".promptforge")
}

#[cfg(test)]
mod tests;
