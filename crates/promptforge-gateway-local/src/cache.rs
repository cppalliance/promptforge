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

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use promptforge_gateway_config::LocalModelConfig;
use serde::{Deserialize, Serialize};

use crate::artifacts::{
    DownloadProgress, download_client, download_with_progress, enforce_private_cache_root,
    ensure_cache_directory, expand_tilde, filename_from_url, lock_artifact, looks_like_url,
    parse_expected_digest, part_path, remove_cache_entry, rename_confined, safe_relative_path,
    source_cache_key, validate_cache_path, write_synced,
};
use crate::error::LocalError;

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
    /// after the digest verifies against `expected_sha256` (when named); any
    /// failure removes the staging file and leaves no sidecar. Concurrent
    /// publishers of the same source serialize on the artifact lock, and the
    /// hit test is repeated under the lock so exactly one of them downloads.
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
        remove_cache_entry(&self.root, &staging)?;
        let Some(parent) = destination.parent() else {
            return Err(LocalError::InvalidPath {
                path: destination.clone(),
            });
        };
        ensure_cache_directory(&self.root, parent)?;
        validate_cache_path(&self.root, &staging)?;
        let actual = match download_with_progress(&self.client, source, &staging, progress) {
            Ok(actual) => actual,
            Err(error) => {
                progress.abandon();
                let _ignored = fs::remove_file(&staging);
                return Err(error);
            }
        };
        if let Some(expected) = expected.as_deref()
            && actual != expected
        {
            progress.abandon();
            remove_cache_entry(&self.root, &staging)?;
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
        progress.finish();
        Ok(CachedBlob {
            path: destination,
            sha256: actual,
            size_bytes,
        })
    }

    /// Lists every cache entry: blobs under `models/` that carry a sidecar.
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

/// A file under the cache's `models/` tree that no loaded `[[local_model]]`
/// entry references.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OrphanEntry {
    /// Path relative to the cache root, `/`-separated on every platform.
    pub path: String,
    /// File length in bytes.
    pub size_bytes: u64,
    /// Lowercase hex SHA-256 recorded by the blob's cache sidecar, when one
    /// exists. `None` for files the cache API never downloaded: blobs are
    /// multi-gigabyte, so their bytes are never re-hashed to fill the field
    /// (Amendment C).
    pub sha256: Option<String>,
}

/// Store bookkeeping suffixes that are never orphans: cache sidecars,
/// model-card sidecars, verified markers, and staging files.
const BOOKKEEPING_SUFFIXES: [&str; 4] = [META_SUFFIX, ".md", ".verified", ".part"];

/// The absolute path a `[[local_model]]` source occupies on disk: the
/// provisioning cache slot (`models/<key>/<filename>`) for a URL source, the
/// tilde-expanded path itself for a path source. `None` when the source
/// cannot resolve (no home directory, no URL filename segment); such a
/// source cannot name an on-disk file.
fn configured_path(root: &Path, source: &str) -> Option<PathBuf> {
    if looks_like_url(source) {
        let name = filename_from_url(source).ok()?;
        let key = source_cache_key(source);
        Some(root.join("models").join(key).join(name))
    } else {
        expand_tilde(source).ok()
    }
}

/// Lists files under `<root>/models/` that no entry of `models` references.
///
/// A model references its `source` plus the sources of its speculative and
/// multimodal-projector companions; each resolves to the same on-disk path
/// provisioning uses (a URL source to its cache slot, a path source to the
/// tilde-expanded path). Comparison falls back to canonicalized paths, so a
/// path source spelled with different case or separators still matches its
/// file. Store bookkeeping (`.meta.json` cache sidecars, `.md` model-card
/// sidecars, `.verified` markers, `.part` staging files) is never reported,
/// and symlinked entries are skipped as in [`BlobCache::list`]. A missing `models/` directory
/// yields an empty list. Entries sort by path for a stable response.
///
/// # Errors
/// Returns [`LocalError::Io`] when the tree cannot be walked or a file
/// cannot be inspected.
pub fn orphans(
    root: &Path,
    models: &[LocalModelConfig],
    extra_sources: &[&str],
) -> Result<Vec<OrphanEntry>, LocalError> {
    let mut configured = HashSet::new();
    for model in models {
        let mut sources = vec![model.source()];
        if let Some(speculative) = model.speculative() {
            sources.push(speculative.source());
        }
        if let Some(projector) = model.multimodal_projector() {
            sources.push(projector.source());
        }
        for source in sources {
            let Some(path) = configured_path(root, source) else {
                continue;
            };
            if let Ok(canonical) = fs::canonicalize(&path) {
                configured.insert(canonical);
            }
            configured.insert(path);
        }
    }
    for source in extra_sources {
        let Some(path) = configured_path(root, source) else {
            continue;
        };
        if let Ok(canonical) = fs::canonicalize(&path) {
            configured.insert(canonical);
        }
        configured.insert(path);
    }

    let models_dir = root.join("models");
    let top = match fs::read_dir(&models_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LocalError::Io {
                operation: "read cache models directory",
                path: models_dir,
                source,
            });
        }
    };
    let mut found = Vec::new();
    let mut directories = Vec::new();
    collect_orphans(
        root,
        &models_dir,
        top,
        &configured,
        &mut directories,
        &mut found,
    )?;
    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| LocalError::Io {
            operation: "read cache models directory",
            path: directory.clone(),
            source,
        })?;
        collect_orphans(
            root,
            &directory,
            entries,
            &configured,
            &mut directories,
            &mut found,
        )?;
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}

/// Feeds one directory's entries into the orphan scan: unreferenced files
/// become entries of `found`, subdirectories queue on `directories` for a
/// later pass, and symlinks are skipped.
fn collect_orphans(
    root: &Path,
    directory: &Path,
    entries: fs::ReadDir,
    configured: &HashSet<PathBuf>,
    directories: &mut Vec<PathBuf>,
    found: &mut Vec<OrphanEntry>,
) -> Result<(), LocalError> {
    for entry in entries {
        let entry = entry.map_err(|source| LocalError::Io {
            operation: "read cache models entry",
            path: directory.to_owned(),
            source,
        })?;
        // `file_type` does not follow links, so a planted symlinked directory
        // or blob is skipped rather than read through.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            directories.push(path);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if BOOKKEEPING_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            continue;
        }
        if configured.contains(&path)
            || fs::canonicalize(&path).is_ok_and(|canonical| configured.contains(&canonical))
        {
            continue;
        }
        let size_bytes = entry
            .metadata()
            .map_err(|source| LocalError::Io {
                operation: "stat cache models entry",
                path: path.clone(),
                source,
            })?
            .len();
        let sha256 = read_meta(&path)?.map(|meta| meta.sha256);
        let relative = path.strip_prefix(root).unwrap_or(&path);
        found.push(OrphanEntry {
            path: relative.to_string_lossy().replace('\\', "/"),
            size_bytes,
            sha256,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tempfile::TempDir;

    use super::*;
    use crate::testsupport::{FakeServer, hex_sha256};

    /// Test double recording the progress callbacks a download drives.
    struct RecordingProgress {
        total: Mutex<Option<u64>>,
        bytes: AtomicU64,
        finished: AtomicU64,
        abandoned: AtomicU64,
    }

    impl RecordingProgress {
        fn new() -> Self {
            Self {
                total: Mutex::new(None),
                bytes: AtomicU64::new(0),
                finished: AtomicU64::new(0),
                abandoned: AtomicU64::new(0),
            }
        }
    }

    impl DownloadProgress for RecordingProgress {
        fn set_len(&self, total: Option<u64>) {
            *self.total.lock().expect("progress total lock") = total;
        }

        fn inc(&self, n: u64) {
            self.bytes.fetch_add(n, Ordering::Relaxed);
        }

        fn finish(&self) {
            self.finished.fetch_add(1, Ordering::Relaxed);
        }

        fn abandon(&self) {
            self.abandoned.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn download_to_cache_downloads_verifies_and_writes_sidecar() {
        let body = b"cache-api-fixture-bytes";
        let digest = hex_sha256(body);
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("model.gguf");
        let progress = RecordingProgress::new();

        let blob = cache
            .download_to_cache(&url, Some(&digest), &progress)
            .expect("download");
        assert_eq!(blob.sha256, digest);
        assert_eq!(blob.size_bytes, body.len() as u64);
        assert_eq!(fs::read(&blob.path).expect("read blob"), body);
        assert_eq!(server.requests(), 1);
        assert_eq!(progress.finished.load(Ordering::Relaxed), 1);
        assert_eq!(progress.bytes.load(Ordering::Relaxed), body.len() as u64);
        assert_eq!(
            *progress.total.lock().expect("total"),
            Some(body.len() as u64)
        );

        // The sidecar records source, digest, and size; the staging file is gone.
        let meta_text = fs::read_to_string(meta_path(&blob.path)).expect("read sidecar");
        let meta: BlobMeta = serde_json::from_str(&meta_text).expect("parse sidecar");
        assert_eq!(meta.source, url);
        assert_eq!(meta.sha256, digest);
        assert_eq!(meta.size_bytes, body.len() as u64);
        assert!(!part_path(&blob.path).exists(), "stale .part left behind");
    }

    #[test]
    fn download_to_cache_rejects_digest_mismatch_and_cleans_up() {
        let body = b"wrong-bytes-for-the-pin";
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("pinned.gguf");
        let progress = RecordingProgress::new();

        let error = cache
            .download_to_cache(&url, Some(&"0".repeat(64)), &progress)
            .expect_err("digest mismatch");
        assert!(matches!(error, LocalError::DigestMismatch { .. }));
        assert_eq!(progress.abandoned.load(Ordering::Relaxed), 1);

        let destination = cache.destination(&url).expect("destination");
        assert!(!destination.exists(), "mismatched blob must not publish");
        assert!(!part_path(&destination).exists(), "stale .part left behind");
        assert!(
            !meta_path(&destination).exists(),
            "sidecar must not be written"
        );
    }

    #[test]
    fn download_to_cache_hit_skips_the_download() {
        let body = b"cached-once-fixture";
        let digest = hex_sha256(body);
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("hit.gguf");

        let first = cache
            .download_to_cache(&url, Some(&digest), &RecordingProgress::new())
            .expect("first download");
        let second = cache
            .download_to_cache(&url, Some(&digest), &RecordingProgress::new())
            .expect("cache hit");
        assert_eq!(first, second);
        assert_eq!(server.requests(), 1, "a hit must not re-download");

        // The same hit is visible through the read-only lookup path.
        let looked_up = cache.lookup(&url, Some(&digest)).expect("lookup");
        assert_eq!(looked_up, Some(first));
    }

    #[test]
    fn download_to_cache_without_pin_caches_by_source() {
        let body = b"unpinned-cache-fixture";
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("free.gguf");

        let blob = cache
            .download_to_cache(&url, None, &RecordingProgress::new())
            .expect("download");
        assert_eq!(blob.sha256, hex_sha256(body));
        let hit = cache
            .download_to_cache(&url, None, &RecordingProgress::new())
            .expect("cache hit");
        assert_eq!(hit, blob);
        assert_eq!(server.requests(), 1);
    }

    #[test]
    fn artifact_store_downloads_appear_in_the_cache_listing() {
        let body = b"artifact-store-model";
        let digest = hex_sha256(body);
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let url = server.url("listed.gguf");
        let store = crate::artifacts::ArtifactStore::new(temp.path()).expect("artifact store");

        let path = store
            .ensure_model(&url, Some(&digest))
            .expect("provision model");
        let entries = BlobCache::new(temp.path())
            .expect("blob cache")
            .list()
            .expect("list cache");

        assert_eq!(
            entries,
            vec![CacheEntry {
                source: url,
                path: path.clone(),
                sha256: digest.clone(),
                size_bytes: body.len() as u64,
            }],
            "Apply-provisioned models feed the Local Models file status"
        );
        let mut marker = path.as_os_str().to_owned();
        marker.push(".verified");
        let marker = PathBuf::from(marker);
        assert!(marker.is_file(), "the pinned artifact has a verify marker");
        assert!(
            BlobCache::new(temp.path())
                .expect("blob cache")
                .remove(&digest)
                .expect("remove artifact"),
            "the listed artifact is removable"
        );
        assert!(!marker.exists(), "cache deletion removes the verify marker");
    }

    #[test]
    fn artifact_store_migrates_an_unpinned_cache_hit_into_the_listing() {
        let body = b"legacy-unpinned-artifact";
        let digest = hex_sha256(body);
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let url = server.url("legacy-unpinned.gguf");
        let store = crate::artifacts::ArtifactStore::new(temp.path()).expect("artifact store");

        let path = store.ensure_model(&url, None).expect("initial provision");
        fs::remove_file(meta_path(&path)).expect("remove new sidecar to model an old cache");
        let reused = store.ensure_model(&url, None).expect("migrate cache hit");

        assert_eq!(reused, path);
        assert_eq!(
            server.requests(),
            1,
            "the existing blob is not downloaded again"
        );
        assert_eq!(
            BlobCache::new(temp.path())
                .expect("blob cache")
                .list()
                .expect("list cache"),
            vec![CacheEntry {
                source: url,
                path,
                sha256: digest,
                size_bytes: body.len() as u64,
            }],
            "the migrated hit feeds the Local Models file status"
        );
    }

    #[test]
    fn sidecar_less_blob_is_not_a_hit_and_is_replaced() {
        // Amendment E: a blob without a sidecar (a pre-existing local model
        // file) is not a cache entry, so the source is re-downloaded and the
        // verified replacement is published with a sidecar.
        let body = b"fresh-download-over-legacy-blob";
        let digest = hex_sha256(body);
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("legacy.gguf");
        let destination = cache.destination(&url).expect("destination");
        fs::create_dir_all(destination.parent().expect("parent")).expect("mkdir");
        fs::write(&destination, b"legacy-untracked-bytes").expect("seed bare blob");

        assert!(
            cache.lookup(&url, None).expect("lookup").is_none(),
            "a blob without a sidecar is not a cache hit"
        );
        let blob = cache
            .download_to_cache(&url, Some(&digest), &RecordingProgress::new())
            .expect("re-download over bare blob");
        assert_eq!(fs::read(&blob.path).expect("read blob"), body);
        assert!(meta_path(&blob.path).is_file(), "sidecar written");
        assert_eq!(server.requests(), 1);
    }

    #[test]
    fn list_returns_sidecar_bearing_blobs_with_metadata() {
        let body_a = b"listing-fixture-a";
        let body_b = b"listing-fixture-bb";
        let server_a = FakeServer::new(body_a);
        let server_b = FakeServer::new(body_b);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url_a = server_a.url("a.gguf");
        let url_b = server_b.url("b.gguf");
        let blob_a = cache
            .download_to_cache(&url_a, None, &RecordingProgress::new())
            .expect("download a");
        let blob_b = cache
            .download_to_cache(&url_b, None, &RecordingProgress::new())
            .expect("download b");

        // A bare blob without a sidecar (a pre-existing local model file) is
        // not listed; nor is a stale sidecar whose blob is gone.
        let bare_dir = temp.path().join("models").join("0123456789abcdef");
        fs::create_dir_all(&bare_dir).expect("mkdir bare slot");
        fs::write(bare_dir.join("bare.gguf"), b"bare").expect("write bare blob");
        let stale_dir = temp.path().join("models").join("fedcba9876543210");
        fs::create_dir_all(&stale_dir).expect("mkdir stale slot");
        fs::write(
            stale_dir.join("gone.gguf.meta.json"),
            r#"{"source":"http://x/gone.gguf","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","size_bytes":4}"#,
        )
        .expect("write stale sidecar");

        let entries = cache.list().expect("list");
        // Sorted by source for a stable response. The fake servers bind
        // ephemeral ports, so which of the two sources sorts first is not
        // fixed; build the expectation and sort it the same way.
        let mut expected = vec![
            CacheEntry {
                source: url_a,
                path: blob_a.path,
                sha256: hex_sha256(body_a),
                size_bytes: body_a.len() as u64,
            },
            CacheEntry {
                source: url_b,
                path: blob_b.path,
                sha256: hex_sha256(body_b),
                size_bytes: body_b.len() as u64,
            },
        ];
        expected.sort_by(|left, right| left.source.cmp(&right.source));
        assert_eq!(entries, expected);
    }

    #[test]
    fn remove_deletes_blob_and_sidecar_by_digest() {
        let body = b"delete-me-fixture";
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("gone.gguf");
        let blob = cache
            .download_to_cache(&url, None, &RecordingProgress::new())
            .expect("download");
        let sidecar = meta_path(&blob.path);
        assert!(blob.path.is_file() && sidecar.is_file());

        assert!(cache.remove(&blob.sha256).expect("remove"));
        assert!(!blob.path.exists(), "blob removed");
        assert!(!sidecar.exists(), "sidecar removed");
        assert!(cache.list().expect("list").is_empty());

        // A second removal of the same digest reports not-found.
        assert!(!cache.remove(&blob.sha256).expect("remove again"));
        // A malformed digest is rejected at the boundary.
        assert!(matches!(
            cache.remove("not-hex"),
            Err(LocalError::InvalidDigest { .. })
        ));
    }

    #[test]
    fn concurrent_publishers_converge_on_one_download() {
        // Two racing publishers of one source serialize on the artifact lock,
        // and the hit test repeated under the lock means exactly one of them
        // downloads (design entry 54).
        let body = b"racing-publishers-fixture";
        let digest = hex_sha256(body);
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("raced.gguf");

        let (first, second) = std::thread::scope(|scope| {
            let first = scope
                .spawn(|| cache.download_to_cache(&url, Some(&digest), &RecordingProgress::new()));
            let second = scope
                .spawn(|| cache.download_to_cache(&url, Some(&digest), &RecordingProgress::new()));
            (
                first.join().expect("first publisher panicked"),
                second.join().expect("second publisher panicked"),
            )
        });
        let first = first.expect("first download");
        let second = second.expect("second download");
        assert_eq!(first, second);
        assert_eq!(server.requests(), 1, "exactly one publisher downloads");
    }

    #[test]
    fn corrupt_sidecar_is_skipped_and_not_a_hit() {
        // A sidecar that does not parse is treated as absent (design entry
        // 55): the blob is neither a hit nor listed, and neither read fails.
        let body = b"corrupt-sidecar-fixture";
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("corrupt.gguf");
        let blob = cache
            .download_to_cache(&url, None, &RecordingProgress::new())
            .expect("download");
        fs::write(meta_path(&blob.path), b"not json").expect("corrupt sidecar");

        assert!(
            cache.lookup(&url, None).expect("lookup").is_none(),
            "a corrupt sidecar is not a cache hit"
        );
        assert!(
            cache.list().expect("list").is_empty(),
            "a corrupt sidecar is skipped, not listed"
        );
    }

    #[test]
    fn mismatched_pin_against_sidecar_forces_redownload() {
        // A request naming a pin that differs from the sidecar's digest is not
        // a hit; the re-downloaded content still fails verification.
        let body = b"real-content-bytes";
        let server = FakeServer::new(body);
        let temp = TempDir::new().expect("tempdir");
        let cache = BlobCache::new(temp.path()).expect("cache");
        let url = server.url("repin.gguf");
        cache
            .download_to_cache(&url, None, &RecordingProgress::new())
            .expect("initial download");

        let error = cache
            .download_to_cache(&url, Some(&"f".repeat(64)), &RecordingProgress::new())
            .expect_err("pin mismatch");
        assert!(matches!(error, LocalError::DigestMismatch { .. }));
        assert_eq!(server.requests(), 2, "the miss re-downloads");
    }

    #[test]
    fn orphans_reports_only_unreferenced_files() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path();
        let models = root.join("models");
        fs::create_dir_all(&models).expect("mkdir models");

        // Configured coverage: a URL source in its provisioning cache slot,
        // a path source, and companion (speculative + projector) path
        // sources. None of these may appear as orphans.
        let url = "https://example.test/repo/pinned.gguf";
        let slot = models.join(source_cache_key(url));
        fs::create_dir_all(&slot).expect("mkdir slot");
        fs::write(slot.join("pinned.gguf"), b"pinned-bytes").expect("write pinned");
        let local = models.join("local.gguf");
        let draft = models.join("draft.gguf");
        let projector = models.join("mmproj.gguf");
        fs::write(&local, b"local-bytes").expect("write local");
        fs::write(&draft, b"draft-bytes").expect("write draft");
        fs::write(&projector, b"mmproj-bytes").expect("write projector");
        // A path source spelled through a redundant `..` component never
        // equals the walked path component-wise, so only the canonicalize
        // fallback can match it; this file turning up as an orphan means
        // that fallback broke.
        let variant = models.join("variant.gguf");
        fs::write(&variant, b"variant-bytes").expect("write variant");
        let variant_spelling = models.join("..").join("models").join("variant.gguf");

        // Orphans: a bare top-level file, a slot-nested file with a cache
        // sidecar (its digest is reused, never re-hashed), and a file whose
        // corrupt sidecar downgrades to no digest.
        fs::write(models.join("stray.gguf"), b"stray-bytes").expect("write stray");
        let cached_body: &[u8] = b"cached-bytes";
        let cached_slot = models.join("0123456789abcdef");
        fs::create_dir_all(&cached_slot).expect("mkdir cached slot");
        fs::write(cached_slot.join("cached.gguf"), cached_body).expect("write cached");
        let cached_digest = hex_sha256(cached_body);
        fs::write(
            cached_slot.join("cached.gguf.meta.json"),
            serde_json::json!({
                "source": "http://seeded.example/cached.gguf",
                "sha256": cached_digest,
                "size_bytes": cached_body.len(),
            })
            .to_string(),
        )
        .expect("write cached sidecar");
        fs::write(models.join("corrupt.gguf"), b"corrupt-body").expect("write corrupt");
        fs::write(models.join("corrupt.gguf.meta.json"), b"not json").expect("corrupt sidecar");

        // Bookkeeping noise that must never be listed: a model-card sidecar
        // and a staging file.
        fs::write(models.join("local.md"), b"card").expect("write card");
        fs::write(models.join("local.gguf.verified"), b"marker").expect("write marker");
        fs::write(models.join("stray.gguf.part"), b"partial").expect("write staging");

        let config = promptforge_gateway_config::Config::from_toml_str(&format!(
            r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "pinned"
description = "a url-sourced model"
source = "{url}"
sha256 = "{pin}"
context = 4096

[[local_model]]
name = "local"
description = "a path-sourced model with companions"
source = '{local}'
context = 4096

[local_model.speculative]
type = "draft-mtp"
source = '{draft}'
draft_max = 2

[local_model.multimodal_projector]
source = '{projector}'

[[local_model]]
name = "variant"
description = "a path-sourced model spelled through a redundant component"
source = '{variant_spelling}'
context = 4096
"#,
            pin = hex_sha256(b"pinned-bytes"),
            local = local.display(),
            draft = draft.display(),
            projector = projector.display(),
            variant_spelling = variant_spelling.display(),
        ))
        .expect("config");

        let entries = orphans(root, config.local_models(), &[]).expect("orphans");
        assert_eq!(
            entries,
            vec![
                OrphanEntry {
                    path: "models/0123456789abcdef/cached.gguf".to_owned(),
                    size_bytes: cached_body.len() as u64,
                    sha256: Some(cached_digest),
                },
                OrphanEntry {
                    path: "models/corrupt.gguf".to_owned(),
                    size_bytes: b"corrupt-body".len() as u64,
                    sha256: None,
                },
                OrphanEntry {
                    path: "models/stray.gguf".to_owned(),
                    size_bytes: b"stray-bytes".len() as u64,
                    sha256: None,
                },
            ]
        );
    }

    #[test]
    fn orphans_without_a_models_directory_is_empty() {
        let temp = TempDir::new().expect("tempdir");
        assert_eq!(orphans(temp.path(), &[], &[]).expect("orphans"), Vec::new());
    }
}
