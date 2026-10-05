//! Blob verification tests: digest pins, verified markers, and the hash pass text.

use super::*;

#[test]
fn downloads_verifies_and_reuses_cached_blob() {
    let body = b"tiny-gguf-fixture";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");

    let first = store
        .ensure_model(&server.url("fixture.gguf"), Some(&digest))
        .expect("first download");
    assert!(first.is_file());
    assert_eq!(server.requests(), 1);
    assert_eq!(file_digest(&first).expect("digest"), digest);

    let second = store
        .ensure_model(&server.url("fixture.gguf"), Some(&digest))
        .expect("cache hit");
    assert_eq!(first, second);
    assert_eq!(server.requests(), 1);
}

#[test]
fn rejects_digest_mismatch() {
    let body = b"wrong-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let err = store
        .ensure_model(
            &server.url("bad.gguf"),
            Some("0000000000000000000000000000000000000000000000000000000000000000"),
        )
        .expect_err("digest mismatch");
    assert!(matches!(err, LocalError::DigestMismatch { .. }));
}

#[test]
fn reuses_unpinned_blob_without_redownload() {
    let body = b"unpinned";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let first = store
        .ensure_model(&server.url("free.gguf"), None)
        .expect("download");
    let second = store
        .ensure_model(&server.url("free.gguf"), None)
        .expect("reuse");
    assert_eq!(first, second);
    assert_eq!(server.requests(), 1);
}

/// A cache root holding one pinned blob, returning `(root, blob, digest, marker)`.
fn pinned_blob_fixture(body: &[u8]) -> (TempDir, PathBuf, String, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir cache");
    let blob = root.join("m.gguf");
    std::fs::write(&blob, body).expect("write blob");
    let marker = blob_marker_path(&blob);
    (dir, blob, hex_sha256(body), marker)
}

#[test]
fn first_verification_hashes_and_writes_marker() {
    // With no marker present the blob is hashed and a correct three-line
    // marker (digest, size, mtime) is written.
    let body = b"blob-bytes";
    let (dir, blob, digest, marker) = pinned_blob_fixture(body);
    let root = dir.path().join("cache");

    let outcome = verify_blob(&root, &blob, &digest, &marker).expect("verify");
    assert_eq!(outcome, VerifyOutcome::Hashed);
    let text = std::fs::read_to_string(&marker).expect("marker");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some(digest.as_str()));
    assert_eq!(lines.next(), Some(body.len().to_string().as_str()));
    let mtime = lines.next().expect("mtime line");
    assert!(mtime.split_once('.').is_some(), "mtime is `<secs>.<nanos>`");
    assert!(lines.next().is_none(), "marker has exactly three lines");
}

#[test]
fn second_verification_hits_marker_without_rehash() {
    // The VerifyOutcome return is the seam: once the marker exists, the second
    // verification is a MarkerHit, which by construction performs no hash pass.
    let (dir, blob, digest, marker) = pinned_blob_fixture(b"blob-bytes");
    let root = dir.path().join("cache");

    let first = verify_blob(&root, &blob, &digest, &marker).expect("first");
    assert_eq!(first, VerifyOutcome::Hashed);
    let second = verify_blob(&root, &blob, &digest, &marker).expect("second");
    assert_eq!(second, VerifyOutcome::MarkerHit);
}

#[test]
fn verify_blob_writes_the_hash_pass_percent() {
    // Two full 64 KiB read chunks: 50% after the first, 100% after the second.
    let body = vec![0xAB_u8; 128 * 1024];
    let (dir, blob, digest, marker) = pinned_blob_fixture(&body);
    let root = dir.path().join("cache");

    let hub = ProgressHub::new();
    let activity = hub.begin("verify");

    let outcome =
        verify_blob_with_progress(&root, &blob, &digest, &marker, Some(&activity)).expect("verify");
    assert_eq!(outcome, VerifyOutcome::Hashed);
    assert_eq!(hub.current().text, "Verifying m.gguf 100%");
}

#[test]
fn verify_blob_marker_hit_writes_no_text() {
    let body = b"blob-bytes";
    let (dir, blob, digest, marker) = pinned_blob_fixture(body);
    let root = dir.path().join("cache");
    let first = verify_blob(&root, &blob, &digest, &marker).expect("first verify");
    assert_eq!(first, VerifyOutcome::Hashed);

    let hub = ProgressHub::new();
    let activity = hub.begin("verify");

    let outcome =
        verify_blob_with_progress(&root, &blob, &digest, &marker, Some(&activity)).expect("verify");
    assert_eq!(outcome, VerifyOutcome::MarkerHit);
    assert_eq!(
        hub.current().text,
        "verify",
        "a marker hit reads nothing and names no stage"
    );
}

#[test]
fn verify_blob_names_the_hash_pass_before_a_digest_mismatch() {
    let body = b"blob-bytes";
    let (dir, blob, _digest, marker) = pinned_blob_fixture(body);
    let root = dir.path().join("cache");
    let wrong = hex_sha256(b"other-bytes");

    let hub = ProgressHub::new();
    let activity = hub.begin("verify");

    let result = verify_blob_with_progress(&root, &blob, &wrong, &marker, Some(&activity));
    assert!(matches!(result, Err(LocalError::DigestMismatch { .. })));
    assert_eq!(
        hub.current().text,
        "Verifying m.gguf 100%",
        "the hash pass ran to its end; the mismatch error reports the failure"
    );
}

#[test]
fn changed_content_rehashes_and_mismatches() {
    // Rewriting the blob (new size and mtime) invalidates the marker, so the
    // blob is re-hashed and the pin mismatch still raises DigestMismatch; the
    // stale marker is deleted.
    let (dir, blob, digest, marker) = pinned_blob_fixture(b"blob-bytes");
    let root = dir.path().join("cache");
    let first = verify_blob(&root, &blob, &digest, &marker).expect("first");
    assert_eq!(first, VerifyOutcome::Hashed);

    std::fs::write(&blob, b"different-longer-bytes").expect("rewrite blob");
    let err = verify_blob(&root, &blob, &digest, &marker).expect_err("mismatch");
    assert!(matches!(err, LocalError::DigestMismatch { .. }));
    assert!(!marker.exists(), "stale marker must be deleted");
}

#[test]
fn wrong_pin_or_corrupt_marker_falls_back_to_hashing() {
    // A corrupt marker and a marker recording a different digest are cache
    // misses, never errors: both fall through to hashing, which succeeds and
    // refreshes the marker.
    let (dir, blob, digest, marker) = pinned_blob_fixture(b"blob-bytes");
    let root = dir.path().join("cache");

    std::fs::write(&marker, b"not-a-marker").expect("corrupt marker");
    let outcome = verify_blob(&root, &blob, &digest, &marker).expect("verify over corrupt");
    assert_eq!(outcome, VerifyOutcome::Hashed);

    let wrong = format!("{}\n10\n0.0\n", "0".repeat(64));
    std::fs::write(&marker, wrong).expect("wrong-pin marker");
    let outcome = verify_blob(&root, &blob, &digest, &marker).expect("verify over wrong pin");
    assert_eq!(outcome, VerifyOutcome::Hashed);
    let text = std::fs::read_to_string(&marker).expect("refreshed marker");
    assert_eq!(text.lines().next(), Some(digest.as_str()));
}

#[test]
fn post_download_success_writes_marker() {
    // A successful pinned download leaves a marker beside the blob, and the
    // next ensure_model is a cache hit with no re-download.
    let body = b"marker-after-download";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("m.gguf");

    let path = store.ensure_model(&url, Some(&digest)).expect("download");
    let marker = blob_marker_path(&path);
    let text = std::fs::read_to_string(&marker).expect("marker written after download");
    assert_eq!(text.lines().next(), Some(digest.as_str()));

    let second = store.ensure_model(&url, Some(&digest)).expect("cache hit");
    assert_eq!(path, second);
    assert_eq!(server.requests(), 1);
}

#[test]
fn path_source_uses_marker_on_second_call() {
    // A pinned path source records its marker under `<cache>/markers/`; the
    // second ensure_model verifies through the marker and does not rewrite it.
    let body = b"path-source-bytes";
    let digest = hex_sha256(body);
    let source_dir = TempDir::new().expect("source dir");
    let source = source_dir.path().join("local.gguf");
    std::fs::write(&source, body).expect("write source");
    let source_str = source.to_str().expect("utf-8 source path");
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");

    let first = store
        .ensure_model(source_str, Some(&digest))
        .expect("first");
    assert_eq!(first, source);
    let marker = temp.path().join("markers").join(format!(
        "{}.verified",
        source_cache_key(&source.to_string_lossy())
    ));
    let text = std::fs::read_to_string(&marker).expect("path-source marker");
    assert_eq!(text.lines().next(), Some(digest.as_str()));

    let marker_mtime = std::fs::metadata(&marker)
        .expect("marker metadata")
        .modified()
        .expect("marker mtime");
    let second = store
        .ensure_model(source_str, Some(&digest))
        .expect("second");
    assert_eq!(second, source);
    let after = std::fs::metadata(&marker)
        .expect("marker metadata")
        .modified()
        .expect("marker mtime");
    assert_eq!(
        marker_mtime, after,
        "a marker hit must not refresh the marker"
    );
}

/// Makes `path` a read-only file holding `contents` so a `File::create` on it
/// fails deterministically, runs `run`, then restores writability so
/// `TempDir` cleanup is not blocked.
#[expect(
    clippy::permissions_set_readonly_false,
    reason = "restores the default writable state of a temp fixture"
)]
fn with_readonly_file(path: &Path, contents: &[u8], run: impl FnOnce()) {
    std::fs::write(path, contents).expect("write blocking file");
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions).expect("set read-only");
    run();
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_readonly(false);
    std::fs::set_permissions(path, permissions).expect("restore writable");
}

#[test]
fn marker_persistence_failure_still_verifies() {
    // The marker only skips a re-hash, so a failed refresh (a read-only file
    // blocking the marker path) degrades to a warning and the successful hash
    // still reports `Hashed`.
    let (dir, blob, digest, marker) = pinned_blob_fixture(b"blob-bytes");
    let root = dir.path().join("cache");

    let mut outcome = None;
    with_readonly_file(&marker, b"stale", || {
        outcome = Some(verify_blob(&root, &blob, &digest, &marker).expect("verify"));
    });

    assert_eq!(outcome, Some(VerifyOutcome::Hashed));
    assert_eq!(
        std::fs::read_to_string(&marker).expect("marker"),
        "stale",
        "the blocked marker must be left untouched"
    );
}

#[test]
fn post_download_marker_persistence_failure_still_publishes() {
    // A read-only file blocking the marker path makes the post-download
    // marker write fail; the downloaded bytes already matched the pin, so
    // publication still succeeds.
    let body = b"marker-write-fails-after-download";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("m.gguf");
    let key = source_cache_key(&url);
    let dest = temp.path().join("models").join(&key).join("m.gguf");
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
    let marker = blob_marker_path(&dest);

    let mut published = None;
    with_readonly_file(&marker, b"blocking", || {
        published = Some(store.ensure_model(&url, Some(&digest)).expect("publish"));
    });

    assert_eq!(published.as_deref(), Some(dest.as_path()));
    assert_eq!(file_digest(&dest).expect("digest"), digest);
    assert_eq!(server.requests(), 1);
    assert_eq!(
        std::fs::read_to_string(&marker).expect("marker"),
        "blocking",
        "the blocked marker must be left untouched"
    );
}
