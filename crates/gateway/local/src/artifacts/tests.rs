//! Tests for artifact digests, archive extraction safety, publication, and cache confinement.

use std::io::{self, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use gateway_progress::ProgressHub;
use tokio_util::sync::CancellationToken;

use super::archive::{extract_archive, extract_archive_with_progress, safe_archive_path};
use super::assets::ArchiveRef;
use super::confine::source_marker_path;
use super::digest::file_digest;
use super::download::{
    ActivityProgress, DownloadProgress, PercentText, hub_bearer_token, is_huggingface_https,
};
use super::verified::write_marker;
use super::verified::{VerifyOutcome, blob_marker_path, verify_blob, verify_blob_with_progress};
use super::*;
use crate::testsupport::{FakeServer, hex_sha256};

mod extraction;
mod private_root;
mod publication;
mod stage_text;
mod transfer;
mod transfer_stop;
mod verification;

#[test]
fn parse_expected_digest_normalizes_and_validates() {
    let lower = "a".repeat(64);
    assert_eq!(parse_expected_digest(&lower).unwrap(), lower);
    // Uppercase and surrounding whitespace normalize to canonical lowercase.
    let upper = format!("  {}  ", "A".repeat(64));
    assert_eq!(parse_expected_digest(&upper).unwrap(), "a".repeat(64));
    // Wrong length and non-hex are rejected at the boundary.
    assert!(matches!(
        parse_expected_digest("abc"),
        Err(LocalError::InvalidDigest { .. })
    ));
    assert!(matches!(
        parse_expected_digest(&"z".repeat(64)),
        Err(LocalError::InvalidDigest { .. })
    ));
}

#[test]
fn source_cache_key_is_stable_and_distinguishes_urls() {
    // ART-004: the same URL is stable; distinct URLs sharing a filename differ.
    let a = source_cache_key("https://host-a.example/repo/model.gguf");
    let a2 = source_cache_key("https://host-a.example/repo/model.gguf");
    let b = source_cache_key("https://host-b.example/other/model.gguf");
    assert_eq!(a, a2);
    assert_ne!(a, b);
    assert_eq!(a.len(), 16);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn existing_model_path_uses_the_provisioning_slot_without_writing() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir");
    let source = "https://host.example/repo/model.gguf";
    assert_eq!(
        existing_model_path(&root, source).expect("missing lookup"),
        None
    );

    let cached = root
        .join("models")
        .join(source_cache_key(source))
        .join("model.gguf");
    std::fs::create_dir_all(cached.parent().expect("cached parent")).expect("mkdir model slot");
    std::fs::write(&cached, b"model").expect("write model");

    assert_eq!(
        existing_model_path(&root, source).expect("cached lookup"),
        Some(cached)
    );
}

#[test]
fn validate_cache_path_rejects_escape() {
    // ART-006/007: a path outside the cache root is refused.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir");
    let escape = root.join("..").join("outside.bin");
    assert!(matches!(
        validate_cache_path(&root, &escape),
        Err(LocalError::UnsafeCachePath { .. })
    ));
    assert!(validate_cache_path(&root, &root.join("models").join("ok.gguf")).is_ok());
}

/// Test double that records set_len / inc / finish / abandon calls.
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
fn tilde_sources_resolve_against_the_operator_home() {
    // STT and local-model path sources share this resolution: `~/...` and
    // `~\...` expand, a bare `~` is the home itself, and every other
    // spelling passes through untouched.
    let home = PathBuf::from("C:\\Users\\op");
    assert_eq!(
        expand_tilde_against("~/models/whisper.bin", &home),
        home.join("models/whisper.bin")
    );
    assert_eq!(
        expand_tilde_against("~\\models\\whisper.bin", &home),
        home.join("models\\whisper.bin")
    );
    assert_eq!(expand_tilde_against("~", &home), home);
    assert_eq!(
        expand_tilde_against("C:\\absolute\\model.gguf", &home),
        PathBuf::from("C:\\absolute\\model.gguf")
    );
    assert_eq!(
        expand_tilde_against("relative/model.gguf", &home),
        PathBuf::from("relative/model.gguf")
    );
    // `~other` is not the operator home spelling and stays literal.
    assert_eq!(
        expand_tilde_against("~other/model.gguf", &home),
        PathBuf::from("~other/model.gguf")
    );
}

#[test]
fn home_or_missing_rejects_absent_or_empty_home() {
    // ART-009: artifact home resolution returns a typed error instead of
    // silently using the working directory when the home variable is unset.
    assert!(matches!(
        super::home_or_missing("HOME", None),
        Err(LocalError::MissingHome { var: "HOME" })
    ));
    assert!(matches!(
        super::home_or_missing("HOME", Some(std::ffi::OsString::new())),
        Err(LocalError::MissingHome { .. })
    ));
    assert_eq!(
        super::home_or_missing("HOME", Some(std::ffi::OsString::from("/home/op"))).unwrap(),
        PathBuf::from("/home/op")
    );
}
