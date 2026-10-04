//! Blob publication tests: partials, stale staging, occupied destinations, install markers, and races.

use super::super::assets::{ArchiveKind, ServerAsset};
use super::*;

#[test]
fn failed_publication_keeps_the_partial_without_its_marker() {
    // ART-007: a failed publication (digest mismatch) keeps the `.part`
    // staging file, but its resume provenance marker is gone - the bytes
    // failed the digest gate whole, so the next attempt restarts from zero
    // instead of resuming poison.
    let body = b"partial-or-wrong-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("m.gguf");
    let err = store
        .ensure_model(&url, Some(&"0".repeat(64)))
        .expect_err("digest mismatch");
    assert!(matches!(err, LocalError::DigestMismatch { .. }));
    let key = source_cache_key(&url);
    let staging = temp.path().join("models").join(&key).join("m.gguf.part");
    assert!(staging.is_file(), "the failed publication keeps its .part");
    let mut marker = staging.as_os_str().to_owned();
    marker.push(".source");
    assert!(
        !PathBuf::from(marker).exists(),
        "a transfer that completed keeps no resume marker"
    );
}

#[test]
fn stale_staging_part_is_cleaned_before_publish() {
    // ART-007: a pre-existing `.part` from an interrupted prior run at the
    // destination slot has unknown provenance, so the new download truncates
    // and replaces it before publishing.
    let body = b"good-artifact-bytes";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("m.gguf");
    let key = source_cache_key(&url);
    let dest_dir = temp.path().join("models").join(&key);
    std::fs::create_dir_all(&dest_dir).expect("mkdir dest");
    let stale = dest_dir.join("m.gguf.part");
    std::fs::write(&stale, b"garbage-from-a-crash").expect("write stale part");

    let path = store
        .ensure_model(&url, Some(&digest))
        .expect("provision over stale part");
    assert_eq!(file_digest(&path).expect("digest"), digest);
    assert!(!stale.exists(), "stale .part not cleaned before publish");
}

#[test]
fn existing_final_file_at_destination_is_reused_without_download() {
    // ART-007: a completed artifact already occupying the final destination is
    // reused (digest match) without a re-download.
    let body = b"already-published-artifact";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("m.gguf");
    let key = source_cache_key(&url);
    let dest = temp.path().join("models").join(&key).join("m.gguf");
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
    std::fs::write(&dest, body).expect("pre-place completed artifact");

    let path = store
        .ensure_model(&url, Some(&digest))
        .expect("reuse existing destination");
    assert_eq!(path, dest);
    assert_eq!(
        server.requests(),
        0,
        "a matching final artifact must not trigger a download"
    );
}

#[test]
fn existing_directory_at_destination_is_replaced_by_the_artifact() {
    // ART-007: a directory occupying the final destination path is removed and
    // the artifact is published in its place.
    let body = b"artifact-published-over-a-directory";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("m.gguf");
    let key = source_cache_key(&url);
    let dest = temp.path().join("models").join(&key).join("m.gguf");
    std::fs::create_dir_all(&dest).expect("create dir at destination");
    std::fs::write(dest.join("leftover"), b"stale").expect("stale content");

    let path = store
        .ensure_model(&url, Some(&digest))
        .expect("replace directory at destination");
    assert!(path.is_file(), "destination must be the published file");
    assert_eq!(file_digest(&path).expect("digest"), digest);
    assert_eq!(server.requests(), 1);
}

#[test]
fn racing_publishers_over_an_occupied_destination_converge() {
    // ART-007: a stale/wrong file occupies the final destination while several
    // threads race to publish; the artifact lock serializes them so exactly one
    // re-downloads and all converge on the one correct final artifact.
    let body = b"correct-final-artifact-bytes";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = Arc::new(ArtifactStore::new(temp.path()).expect("store"));
    let url = server.url("m.gguf");
    let key = source_cache_key(&url);
    let dest = temp.path().join("models").join(&key).join("m.gguf");
    std::fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
    std::fs::write(&dest, b"stale-wrong-bytes").expect("pre-place wrong final file");

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            let url = url.clone();
            let digest = digest.clone();
            thread::spawn(move || store.ensure_model(&url, Some(&digest)).expect("publish"))
        })
        .collect();
    let paths: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    assert!(paths.windows(2).all(|pair| pair[0] == pair[1]), "{paths:?}");
    assert_eq!(file_digest(&paths[0]).expect("digest"), digest);
    assert_eq!(
        server.requests(),
        1,
        "exactly one publisher re-downloads over the stale destination"
    );
}

#[test]
fn install_is_valid_detects_marker_drift() {
    // ART-007: a corrupt, mismatched, or malformed install marker invalidates
    // the install so it is re-provisioned rather than trusted.
    let dir = TempDir::new().expect("tempdir");
    let install = dir.path().join("install");
    std::fs::create_dir(&install).expect("mkdir install");
    std::fs::write(install.join("llama-server"), b"binary").expect("write file");
    let archive_sha = "a".repeat(64);
    let tree_sha = super::super::digest::tree_digest(&install).expect("tree digest");
    let marker = install.join(INSTALL_MARKER);

    let archives = [ArchiveRef {
        archive_name: "a.zip",
        url: "https://example.invalid/a.zip",
        sha256: &archive_sha,
        archive_kind: ArchiveKind::Zip,
    }];
    let asset = ServerAsset {
        os: "test",
        arch: "test",
        backend: None,
        platform: "test",
        archives: &archives,
        executable_name: "llama-server",
    };
    let wrong_sha = "b".repeat(64);
    let wrong_archives = [ArchiveRef {
        sha256: &wrong_sha,
        ..archives[0]
    }];
    let wrong_asset = ServerAsset {
        archives: &wrong_archives,
        ..asset
    };

    std::fs::write(&marker, format!("{archive_sha}\n{tree_sha}\n")).expect("write marker");
    assert!(ArtifactStore::install_is_valid(&install, &asset).expect("valid"));
    // Wrong recorded archive digest.
    assert!(!ArtifactStore::install_is_valid(&install, &wrong_asset).expect("check"));
    // Corrupt recorded tree digest.
    std::fs::write(&marker, format!("{archive_sha}\n{}\n", "0".repeat(64))).expect("rewrite");
    assert!(!ArtifactStore::install_is_valid(&install, &asset).expect("check"));
    // Malformed marker with an unexpected trailing line.
    std::fs::write(&marker, format!("{archive_sha}\n{tree_sha}\nextra\n")).expect("rewrite");
    assert!(!ArtifactStore::install_is_valid(&install, &asset).expect("check"));
    // Missing marker.
    std::fs::remove_file(&marker).expect("remove marker");
    assert!(!ArtifactStore::install_is_valid(&install, &asset).expect("check"));
}

#[test]
fn concurrent_provisioning_of_same_url_is_safe() {
    // ART-007: several threads provisioning the same URL concurrently all
    // resolve to one correct cached blob; the artifact lock serializes them.
    let body = b"concurrent-fixture-bytes";
    let digest = hex_sha256(body);
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = Arc::new(ArtifactStore::new(temp.path()).expect("store"));
    let url = server.url("shared.gguf");

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            let url = url.clone();
            let digest = digest.clone();
            thread::spawn(move || store.ensure_model(&url, Some(&digest)).expect("provision"))
        })
        .collect();
    let paths: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("thread"))
        .collect();
    assert!(paths.windows(2).all(|pair| pair[0] == pair[1]), "{paths:?}");
    assert_eq!(file_digest(&paths[0]).expect("digest"), digest);
    assert!(server.requests() >= 1);
}
