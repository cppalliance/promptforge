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
                return Ok(true);
            }
        }
        Ok(false)
    }
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
}
