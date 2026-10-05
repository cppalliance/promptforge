//! On-demand blob cache behind the `/v1/cache` routes.
//!
//! A blob is downloaded once into the same `models/<source-key>/<filename>`
//! slot layout local provisioning uses (so a cache-API download is a
//! provisioning cache hit for the same URL, and vice versa), staged through a
//! `<file>.part` sibling and renamed into place only after its digest verifies
//! (Amendment E). Each published blob gets a `<file>.meta.json` sidecar holding
//! its source URL, SHA-256, and size, so listing and lookup never re-hash a
//! multi-gigabyte blob (Amendment C). Blobs without sidecars - pre-existing
//! local model files - are not cache entries: they are neither listed nor
//! treated as hits.

mod orphan_scan;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::artifacts::{
    DownloadProgress, download_client, download_with_progress, enforce_private_cache_root,
    ensure_cache_directory, filename_from_url, lock_artifact, parse_expected_digest, part_path,
    remove_cache_entry, rename_confined, safe_relative_path, source_cache_key, validate_cache_path,
    write_synced,
};
use crate::error::LocalError;
pub use orphan_scan::{OrphanEntry, orphans};

/// The sidecar suffix marking a blob as a cache-API entry.
const META_SUFFIX: &str = ".meta.json";

/// A blob present in the cache: its path, content digest, and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBlob {
    /// Absolute path of the blob under the cache root.
    pub path: PathBuf,
    /// Lowercase hex SHA-256 of the blob's bytes.
    pub sha256: String,
    /// Blob length in bytes.
    pub size_bytes: u64,
}

/// The `<file>.meta.json` sidecar written when a cache download completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BlobMeta {
    source: String,
    sha256: String,
    size_bytes: u64,
}

/// Writes the cache-list metadata for a blob provisioned by ArtifactStore.
pub(crate) fn write_blob_meta(
    root: &Path,
    blob: &Path,
    source: &str,
    sha256: &str,
) -> Result<(), LocalError> {
    let size_bytes = fs::metadata(blob)
        .map_err(|source_err| LocalError::Io {
            operation: "stat cached blob",
            path: blob.to_owned(),
            source: source_err,
        })?
        .len();
    let meta = BlobMeta {
        source: source.to_owned(),
        sha256: sha256.to_owned(),
        size_bytes,
    };
    let metadata = meta_path(blob);
    let encoded = serde_json::to_vec(&meta).map_err(|source_err| LocalError::Io {
        operation: "encode cache sidecar",
        path: metadata.clone(),
        source: io::Error::other(source_err),
    })?;
    validate_cache_path(root, &metadata)?;
    write_synced(&metadata, &encoded)
}

/// One entry of the cache listing: a blob plus the source it was fetched from.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CacheEntry {
    /// The URL the blob was downloaded from.
    pub source: String,
    /// Absolute path of the blob under the cache root.
    pub path: PathBuf,
    /// Lowercase hex SHA-256 of the blob's bytes.
    pub sha256: String,
    /// Blob length in bytes.
    pub size_bytes: u64,
}

/// The sidecar path for a cached blob: `<blob>.meta.json`.
fn meta_path(blob: &Path) -> PathBuf {
    let mut name = blob.as_os_str().to_owned();
    name.push(META_SUFFIX);
    PathBuf::from(name)
}

/// Reads the sidecar beside `blob`, returning `None` when it is absent.
///
/// A corrupt sidecar is logged and treated as absent, so the blob falls back
/// to a re-download rather than failing the request.
///
/// # Errors
/// Returns [`LocalError::Io`] when an existing sidecar cannot be read.
fn read_meta(blob: &Path) -> Result<Option<BlobMeta>, LocalError> {
    let path = meta_path(blob);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(LocalError::Io {
                operation: "read cache sidecar",
                path,
                source,
            });
        }
    };
    match serde_json::from_str(&text) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "ignoring corrupt cache sidecar"
            );
            Ok(None)
        }
    }
}

/// Whether a blob already has usable listing metadata for `source`.
pub(crate) fn blob_meta_matches(blob: &Path, source: &str) -> Result<bool, LocalError> {
    let Some(meta) = read_meta(blob)? else {
        return Ok(false);
    };
    let size_bytes = fs::metadata(blob)
        .map_err(|source_err| LocalError::Io {
            operation: "stat cached blob",
            path: blob.to_owned(),
            source: source_err,
        })?
        .len();
    Ok(meta.source == source
        && meta.size_bytes == size_bytes
        && parse_expected_digest(&meta.sha256).is_ok())
}

/// The cache-hit test: blob and sidecar present, and the sidecar's digest
/// matching `expected` when a pin is named (Amendment E). The blob's bytes are
/// never re-hashed; the sidecar written at download completion is the record
/// of truth (Amendment C).
fn cached(destination: &Path, expected: Option<&str>) -> Result<Option<CachedBlob>, LocalError> {
    if !destination.is_file() {
        return Ok(None);
    }
    let Some(meta) = read_meta(destination)? else {
        return Ok(None);
    };
    if let Some(expected) = expected
        && meta.sha256 != expected
    {
        return Ok(None);
    }
    Ok(Some(CachedBlob {
        path: destination.to_owned(),
        sha256: meta.sha256,
        size_bytes: meta.size_bytes,
    }))
}

/// The cache root plus the shared blocking HTTP client.
///
/// Construction enforces the same owner-private-root precondition as artifact
/// provisioning (ART-006), since the cache writes into the same tree.
#[derive(Debug)]
pub struct BlobCache {
    root: PathBuf,
    client: reqwest::blocking::Client,
}

impl BlobCache {
    /// Opens the cache at `root`, creating and owner-restricting it if needed.
    ///
    /// # Errors
    /// Returns [`LocalError::Io`], [`LocalError::CacheNotPrivate`], or
    /// [`LocalError::HttpClient`] on setup failure.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, LocalError> {
        let root = root.into();
        ensure_cache_directory(&root, &root)?;
        enforce_private_cache_root(&root)?;
        Ok(Self {
            root,
            client: download_client()?,
        })
    }

    /// The cache-slot destination for `source`: `models/<key>/<filename>`.
    fn destination(&self, source: &str) -> Result<PathBuf, LocalError> {
        let name = filename_from_url(source)?;
        let key = source_cache_key(source);
        let relative = Path::new("models").join(&key).join(&name);
        if !safe_relative_path(&relative) {
            return Err(LocalError::UnsafeCachePath {
                path: self.root.join(relative),
            });
        }
        let path = self.root.join(relative);
        validate_cache_path(&self.root, &path)?;
        Ok(path)
    }

    /// Returns the cached blob for `source` when the cache-hit test passes.
    ///
    /// # Errors
    /// Returns [`LocalError::InvalidDigest`] for a malformed pin, or
    /// [`LocalError`] on filesystem failure.
    pub fn lookup(
        &self,
        source: &str,
        expected_sha256: Option<&str>,
    ) -> Result<Option<CachedBlob>, LocalError> {
        let expected = expected_sha256.map(parse_expected_digest).transpose()?;
        let destination = self.destination(source)?;
        cached(&destination, expected.as_deref())
    }

    /// Ensures `source` is cached, downloading it when the cache-hit test
    /// fails, and returns the published blob.
    ///
    /// The download is staged to `<file>.part` and renamed into place only
    /// after the digest verifies against `expected_sha256` (when named). A
    /// failed transfer keeps the staged partial and its provenance marker so
    /// the next attempt resumes from the offset; a digest mismatch keeps the
    /// partial too, but its marker is gone, so the next attempt restarts
    /// from zero. Concurrent publishers of the same source serialize on the
    /// artifact lock, and the hit test is repeated under the lock so exactly
    /// one of them downloads.
    ///
    /// # Errors
    /// Returns [`LocalError`] on transport, digest, confinement, or filesystem
    /// failure.
    pub fn download_to_cache(
        &self,
        source: &str,
        expected_sha256: Option<&str>,
        progress: &dyn DownloadProgress,
    ) -> Result<CachedBlob, LocalError> {
        let expected = expected_sha256.map(parse_expected_digest).transpose()?;
        let destination = self.destination(source)?;
        let _lock = lock_artifact(&self.root, &destination)?;
        if let Some(blob) = cached(&destination, expected.as_deref())? {
            return Ok(blob);
        }
        let staging = part_path(&destination);
        let Some(parent) = destination.parent() else {
            return Err(LocalError::InvalidPath {
                path: destination.clone(),
            });
        };
        ensure_cache_directory(&self.root, parent)?;
        validate_cache_path(&self.root, &staging)?;
        // A failed transfer keeps the staged partial for resume. The blob
        // cache routes pass `None` for the cancellation token, so the
        // transfer runs to its own end.
        let actual = download_with_progress(&self.client, source, &staging, progress, None)?;
        if let Some(expected) = expected.as_deref()
            && actual != expected
        {
            return Err(LocalError::DigestMismatch {
                name: filename_from_url(source)?,
                expected: expected.to_owned(),
                actual,
            });
        }
        // A stale or sidecar-less blob at the destination is replaced only
        // after the new content is verified (Windows rename refuses an
        // existing target).
        remove_cache_entry(&self.root, &destination)?;
        rename_confined(&self.root, &staging, &destination)?;
        let size_bytes = fs::metadata(&destination)
            .map_err(|source_err| LocalError::Io {
                operation: "stat cached blob",
                path: destination.clone(),
                source: source_err,
            })?
            .len();
        let meta = BlobMeta {
            source: source.to_owned(),
            sha256: actual.clone(),
            size_bytes,
        };
        let meta_json = serde_json::to_vec(&meta).map_err(|source_err| LocalError::Io {
            operation: "encode cache sidecar",
            path: meta_path(&destination),
            source: io::Error::other(source_err),
        })?;
        write_synced(&meta_path(&destination), &meta_json)?;
        Ok(CachedBlob {
            path: destination,
            sha256: actual,
            size_bytes,
        })
    }

    /// Lists every cache entry: blobs under `models/` that have a sidecar.
    ///
    /// Reads sidecars only - blob bytes are never hashed (Amendment C), so
    /// listing stays cheap with multi-gigabyte entries. Blobs without
    /// sidecars (pre-existing local model files) and sidecars whose blob is
    /// gone are not listed. Entries sort by source for a stable response.
    ///
    /// # Errors
    /// Returns [`LocalError::Io`] when the cache tree cannot be walked.
    pub fn list(&self) -> Result<Vec<CacheEntry>, LocalError> {
        let models = self.root.join("models");
        let key_dirs = match fs::read_dir(&models) {
            Ok(key_dirs) => key_dirs,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(LocalError::Io {
                    operation: "read cache models directory",
                    path: models,
                    source,
                });
            }
        };
        let mut entries = Vec::new();
        for key_dir in key_dirs {
            let key_dir = key_dir.map_err(|source| LocalError::Io {
                operation: "read cache models entry",
                path: models.clone(),
                source,
            })?;
            // `file_type` does not follow links, so a planted symlinked key
            // directory or blob is skipped rather than read through.
            let Ok(file_type) = key_dir.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let slot = key_dir.path();
            let slot_entries = fs::read_dir(&slot).map_err(|source| LocalError::Io {
                operation: "read cache slot directory",
                path: slot.clone(),
                source,
            })?;
            for slot_entry in slot_entries {
                let slot_entry = slot_entry.map_err(|source| LocalError::Io {
                    operation: "read cache slot entry",
                    path: slot.clone(),
                    source,
                })?;
                let Ok(file_type) = slot_entry.file_type() else {
                    continue;
                };
                if !file_type.is_file() {
                    continue;
                }
                let sidecar = slot_entry.path();
                let Some(name) = sidecar.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                let Some(blob_name) = name.strip_suffix(META_SUFFIX) else {
                    continue;
                };
                let blob = sidecar.with_file_name(blob_name);
                if !blob.is_file() {
                    continue;
                }
                let Some(meta) = read_meta(&blob)? else {
                    continue;
                };
                entries.push(CacheEntry {
                    source: meta.source,
                    path: blob,
                    sha256: meta.sha256,
                    size_bytes: meta.size_bytes,
                });
            }
        }
        entries.sort_by(|left, right| left.source.cmp(&right.source));
        Ok(entries)
    }

    /// Removes the cache entry whose sidecar records `sha256`, returning
    /// whether one was found.
    ///
    /// Matches on the sidecar digest (never a re-hash, Amendment C) and
    /// removes the blob and its sidecar through the confinement-checked
    /// removal path.
    ///
    /// # Errors
    /// Returns [`LocalError::InvalidDigest`] for a malformed digest, or
    /// [`LocalError`] on filesystem failure.
    pub fn remove(&self, sha256: &str) -> Result<bool, LocalError> {
        let wanted = parse_expected_digest(sha256)?;
        for entry in self.list()? {
            if entry.sha256 == wanted {
                remove_cache_entry(&self.root, &entry.path)?;
                remove_cache_entry(&self.root, &meta_path(&entry.path))?;
                let mut marker = entry.path.as_os_str().to_owned();
                marker.push(".verified");
                remove_cache_entry(&self.root, &PathBuf::from(marker))?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests;
