//! Embedded-asset staging regression tests.

use std::path::Path;
use std::sync::Arc;
use std::thread;

use tempfile::TempDir;

use super::*;
use crate::artifacts::verified::blob_marker_path;
use crate::testsupport::hex_sha256;

#[test]
fn writes_the_asset_and_verified_marker() {
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let contents = b"{{ messages }}";
    let relative = Path::new("chat-templates/qwen-3.jinja");

    let path = store
        .stage_verified_asset(relative, contents)
        .expect("stage verified asset");

    assert_eq!(path, temp.path().join(relative));
    assert_eq!(std::fs::read(&path).expect("read staged asset"), contents);
    let marker_text =
        std::fs::read_to_string(blob_marker_path(&path)).expect("read verified marker");
    assert_eq!(
        marker_text.lines().next(),
        Some(hex_sha256(contents).as_str())
    );
}

#[test]
fn refreshes_a_corrupt_marker_without_replacing_valid_bytes() {
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let contents = b"{{ messages }}";
    let relative = Path::new("chat-templates/qwen-3.jinja");
    let path = store
        .stage_verified_asset(relative, contents)
        .expect("initial staging");
    let marker = blob_marker_path(&path);
    std::fs::write(&marker, b"corrupt marker").expect("corrupt marker");

    let repeated = store
        .stage_verified_asset(relative, contents)
        .expect("refresh marker");

    assert_eq!(repeated, path);
    assert_eq!(std::fs::read(&path).expect("read staged asset"), contents);
    let refreshed = std::fs::read_to_string(marker).expect("read refreshed marker");
    assert_eq!(
        refreshed.lines().next(),
        Some(hex_sha256(contents).as_str())
    );
}

#[test]
fn repairs_drift_from_a_verified_staging_file() {
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let relative = Path::new("chat-templates/qwen-3.jinja");
    let path = store
        .stage_verified_asset(relative, b"expected")
        .expect("initial staging");
    std::fs::write(&path, b"drifted bytes").expect("corrupt staged asset");

    let repaired = store
        .stage_verified_asset(relative, b"expected")
        .expect("repair staged asset");

    assert_eq!(repaired, path);
    assert_eq!(
        std::fs::read(&path).expect("read repaired asset"),
        b"expected"
    );
    assert!(
        !part_path(&path).exists(),
        "repair must leave no staging file"
    );
    let marker = std::fs::read_to_string(blob_marker_path(&path)).expect("read marker");
    assert_eq!(
        marker.lines().next(),
        Some(hex_sha256(b"expected").as_str())
    );
}

#[test]
fn repairs_an_obstructing_directory_and_stale_part() {
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let relative = Path::new("chat-templates/qwen-3.jinja");
    let destination = temp.path().join(relative);
    std::fs::create_dir_all(&destination).expect("create obstructing directory");
    std::fs::write(destination.join("stale"), b"stale").expect("write obstruction");
    let staging = part_path(&destination);
    std::fs::write(&staging, b"interrupted staging").expect("write stale part");

    let published = store
        .stage_verified_asset(relative, b"expected")
        .expect("repair obstructions");

    assert_eq!(published, destination);
    assert_eq!(
        std::fs::read(&destination).expect("read published asset"),
        b"expected"
    );
    assert!(!staging.exists());
}

#[test]
fn concurrent_publishers_converge_on_one_verified_asset() {
    let temp = TempDir::new().expect("tempdir");
    let store = Arc::new(ArtifactStore::new(temp.path()).expect("store"));
    let relative = Path::new("chat-templates/qwen-3.jinja");
    let contents = b"{{ messages }}";

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            thread::spawn(move || {
                store
                    .stage_verified_asset(relative, contents)
                    .expect("concurrent staging")
            })
        })
        .collect();
    let paths: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("staging thread"))
        .collect();

    assert!(paths.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(std::fs::read(&paths[0]).expect("read asset"), contents);
    assert!(!part_path(&paths[0]).exists());
    let marker = std::fs::read_to_string(blob_marker_path(&paths[0])).expect("read marker");
    assert_eq!(marker.lines().next(), Some(hex_sha256(contents).as_str()));
}

#[test]
fn rejects_cache_escape() {
    let temp = TempDir::new().expect("tempdir");
    let cache = temp.path().join("cache");
    let store = ArtifactStore::new(&cache).expect("store");
    let outside = temp.path().join("escape.jinja");

    let error = store
        .stage_verified_asset(Path::new("../escape.jinja"), b"escape")
        .expect_err("cache escape must be refused");

    assert!(matches!(error, LocalError::UnsafeCachePath { .. }));
    assert!(!outside.exists());
}
