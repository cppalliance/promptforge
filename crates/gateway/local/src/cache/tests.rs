//! Blob cache download, hit-test, and sidecar tests.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tempfile::TempDir;

use super::*;
use crate::artifacts::confine::source_marker_path;
use crate::testsupport::{FakeServer, hex_sha256};

#[path = "tests-listing.rs"]
mod listing;

/// Test double recording the progress callbacks a download drives.
struct RecordingProgress {
    total: Mutex<Option<u64>>,
    bytes: AtomicU64,
}

impl RecordingProgress {
    fn new() -> Self {
        Self {
            total: Mutex::new(None),
            bytes: AtomicU64::new(0),
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
fn download_to_cache_rejects_digest_mismatch_and_keeps_the_partial() {
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
    assert_eq!(
        progress.bytes.load(Ordering::Relaxed),
        body.len() as u64,
        "the whole body streamed before the digest gate rejected it"
    );

    let destination = cache.destination(&url).expect("destination");
    assert!(!destination.exists(), "mismatched blob must not publish");
    // The failed publication keeps its `.part`, but the completed
    // transfer removed the provenance marker, so the next attempt
    // restarts from zero rather than resuming poison bytes.
    assert!(
        part_path(&destination).is_file(),
        "the failed publication keeps its .part"
    );
    assert!(
        !source_marker_path(&part_path(&destination)).exists(),
        "a transfer that completed keeps no resume marker"
    );
    assert!(
        !meta_path(&destination).exists(),
        "sidecar must not be written"
    );
}

#[test]
fn download_to_cache_resumes_an_interrupted_partial() {
    // A staged partial with a provenance marker resumes from its
    // offset: the server sees the Range request, the digest gates
    // publication, and the published blob is whole.
    let body = b"cache-resume-fixture-bytes-for-an-interrupted-download";
    let digest = hex_sha256(body);
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let cache = BlobCache::new(temp.path()).expect("cache");
    let url = server.url("resume.gguf");
    let destination = cache.destination(&url).expect("destination");
    let staging = part_path(&destination);
    fs::create_dir_all(staging.parent().expect("parent")).expect("mkdir slot");
    let offset = 17_u64;
    fs::write(
        &staging,
        &body[..usize::try_from(offset).expect("fixture offset")],
    )
    .expect("seed partial");
    fs::write(source_marker_path(&staging), &url).expect("seed marker");

    let blob = cache
        .download_to_cache(&url, Some(&digest), &RecordingProgress::new())
        .expect("resume completes");
    assert_eq!(blob.sha256, digest);
    assert_eq!(fs::read(&blob.path).expect("read blob"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(offset)],
        "the retry resumes at the partial's offset"
    );
    assert!(!staging.exists(), "the published blob moved out of staging");
    assert!(
        !source_marker_path(&staging).exists(),
        "the marker is gone after publication"
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
        let first =
            scope.spawn(|| cache.download_to_cache(&url, Some(&digest), &RecordingProgress::new()));
        let second =
            scope.spawn(|| cache.download_to_cache(&url, Some(&digest), &RecordingProgress::new()));
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
