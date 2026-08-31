use std::io::{self, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use promptforge_progress::{EventState, ProgressHub};

use super::archive::{extract_archive_with_progress, safe_archive_path};
use super::digest::file_digest;
use super::download::{hub_bearer_token, is_huggingface_https};
use super::progress::{DownloadProgress, TreeProgress};
#[cfg(not(llama_cuda_embedded))]
use super::verified::write_marker;
use super::verified::{VerifyOutcome, blob_marker_path, verify_blob, verify_blob_with_progress};
use super::*;
use crate::testsupport::{FakeServer, hex_sha256};

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

#[test]
fn safe_archive_path_rejects_traversal_and_absolute() {
    assert!(!safe_archive_path(std::path::Path::new("../evil")));
    assert!(!safe_archive_path(std::path::Path::new("/etc/passwd")));
    assert!(!safe_archive_path(std::path::Path::new("a/../../b")));
    assert!(safe_archive_path(std::path::Path::new("bin/llama-server")));
}

#[test]
fn extract_zip_rejects_traversal_entry_and_cleans_up() {
    use std::io::Write as _;
    use zip::write::SimpleFileOptions;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("evil.zip");
    // Build a zip whose single entry escapes the destination. `start_file`
    // does not sanitize the name, so this exercises the extractor's own guard.
    {
        let file = std::fs::File::create(&archive).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("../escape.txt", SimpleFileOptions::default())
            .expect("start traversal entry");
        writer.write_all(b"pwned").expect("write entry");
        writer.finish().expect("finish zip");
    }

    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let result = extract_archive(&archive, &dest, ArchiveKind::Zip);
    assert!(matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })));
    // The traversal target must never have been written outside the destination.
    assert!(!dir.path().join("escape.txt").exists());
}

fn tar_gz_with_symlink() -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_path("evil-link").expect("set path");
    header.set_link_name("/etc/passwd").expect("set link");
    header.set_cksum();
    builder
        .append(&header, io::empty())
        .expect("append symlink");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gz")
}

#[test]
fn extract_tar_gz_rejects_symlink_entries() {
    // ART-007: a tar entry that is neither a regular file nor a directory (here
    // a symlink) is rejected rather than materialized in the cache tree.
    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("evil.tar.gz");
    std::fs::write(&archive, tar_gz_with_symlink()).expect("write archive");
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let result = extract_archive(&archive, &dest, ArchiveKind::TarGz);
    assert!(matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })));
    assert!(!dest.join("evil-link").exists());
}

fn tar_gz_entry(entry_type: tar::EntryType, name: &str, link: Option<&str>) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(entry_type);
    header.set_size(0);
    header.set_mode(0o644);
    header.set_path(name).expect("set path");
    if let Some(link) = link {
        header.set_link_name(link).expect("set link");
    }
    header.set_cksum();
    builder.append(&header, io::empty()).expect("append entry");
    builder
        .into_inner()
        .expect("finish tar")
        .finish()
        .expect("finish gz")
}

fn zip_symlink(name: &str, target: &str) -> Vec<u8> {
    use zip::write::SimpleFileOptions;

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        writer
            .add_symlink(name, target, SimpleFileOptions::default())
            .expect("add symlink");
        writer.finish().expect("finish zip");
    }
    cursor.into_inner()
}

#[test]
fn extract_rejects_every_non_regular_entry_class() {
    // ART-007: table-driven rejection of each unsafe/unsupported archive entry
    // class - tar symlink/hardlink/char/block/fifo and a zip symlink - so none
    // is materialized in the cache tree.
    let tar_cases: &[(tar::EntryType, Option<&str>)] = &[
        (tar::EntryType::Symlink, Some("/etc/passwd")),
        (tar::EntryType::Link, Some("llama-server")),
        (tar::EntryType::Char, None),
        (tar::EntryType::Block, None),
        (tar::EntryType::Fifo, None),
    ];
    for (entry_type, link) in tar_cases {
        let dir = TempDir::new().expect("tempdir");
        let archive = dir.path().join("evil.tar.gz");
        std::fs::write(&archive, tar_gz_entry(*entry_type, "entry", *link)).expect("write archive");
        let dest = dir.path().join("out");
        std::fs::create_dir(&dest).expect("mkdir dest");
        let result = extract_archive(&archive, &dest, ArchiveKind::TarGz);
        assert!(
            matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })),
            "tar {entry_type:?} should be rejected, got {result:?}"
        );
        assert!(!dest.join("entry").exists());
    }

    // A zip symlink entry (unix mode S_IFLNK) is likewise rejected.
    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("link.zip");
    std::fs::write(&archive, zip_symlink("link", "/etc/passwd")).expect("write archive");
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");
    let result = extract_archive(&archive, &dest, ArchiveKind::Zip);
    assert!(
        matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })),
        "zip symlink should be rejected, got {result:?}"
    );
    assert!(!dest.join("link").exists());
}

#[test]
fn find_executable_rejects_duplicates_and_reports_missing() {
    // ART-007: two matching executables are a hard error, not a silent pick;
    // zero matches is a distinct missing error.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir(root.join("a")).expect("mkdir a");
    std::fs::create_dir(root.join("b")).expect("mkdir b");
    std::fs::write(root.join("a").join("llama-server"), b"x").expect("write a");
    std::fs::write(root.join("b").join("llama-server"), b"y").expect("write b");
    assert!(matches!(
        find_executable(root, "llama-server", "arc"),
        Err(LocalError::DuplicateExecutable { .. })
    ));
    assert!(matches!(
        find_executable(root, "absent", "arc"),
        Err(LocalError::MissingExecutable { .. })
    ));
}

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
    // destination slot carries no provenance marker, so the new download
    // truncates and replaces it before publishing.
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
    let tree_sha = super::tree_digest(&install).expect("tree digest");
    let marker = install.join(INSTALL_MARKER);

    std::fs::write(&marker, format!("{archive_sha}\n{tree_sha}\n")).expect("write marker");
    assert!(ArtifactStore::install_is_valid(&install, &archive_sha).expect("valid"));
    // Wrong recorded archive digest.
    assert!(!ArtifactStore::install_is_valid(&install, &"b".repeat(64)).expect("check"));
    // Corrupt recorded tree digest.
    std::fs::write(&marker, format!("{archive_sha}\n{}\n", "0".repeat(64))).expect("rewrite");
    assert!(!ArtifactStore::install_is_valid(&install, &archive_sha).expect("check"));
    // Malformed marker with an unexpected trailing line.
    std::fs::write(&marker, format!("{archive_sha}\n{tree_sha}\nextra\n")).expect("rewrite");
    assert!(!ArtifactStore::install_is_valid(&install, &archive_sha).expect("check"));
    // Missing marker.
    std::fs::remove_file(&marker).expect("remove marker");
    assert!(!ArtifactStore::install_is_valid(&install, &archive_sha).expect("check"));
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

#[cfg(windows)]
#[test]
fn artifact_store_enforces_private_windows_dacl() {
    // ART-006: opening the store restricts the cache root's DACL so no broad
    // principal (Everyone / Authenticated Users / Users) retains access, even
    // for a cache path configured outside the default profile tree.
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir");
    let _store = ArtifactStore::new(&root).expect("store");

    let output = std::process::Command::new("icacls")
        .arg(&root)
        .output()
        .expect("icacls query");
    assert!(output.status.success(), "icacls query failed");
    let listing = String::from_utf8_lossy(&output.stdout);
    for principal in ["Everyone:", "Authenticated Users:", "\\Users:"] {
        assert!(
            !listing.contains(principal),
            "broad principal {principal} still present in DACL:\n{listing}"
        );
    }
}

#[cfg(unix)]
#[test]
fn artifact_store_enforces_owner_private_cache_root() {
    // ART-006: opening the store tightens a group/world-accessible cache root to
    // owner-only, enforcing the private-cache precondition the confinement relies
    // on rather than merely documenting it.
    use std::os::unix::fs::PermissionsExt as _;
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).expect("loosen");
    let _store = ArtifactStore::new(&root).expect("store");
    let mode = std::fs::metadata(&root)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700, "cache root must be tightened to owner-only");
}

#[cfg(unix)]
#[test]
fn validate_cache_path_rejects_symlink_component() {
    // ART-007: a symlink planted as an interior component is refused so a write
    // cannot be redirected outside the cache root.
    use std::os::unix::fs::symlink;
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().join("cache");
    std::fs::create_dir(&root).expect("mkdir root");
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).expect("mkdir outside");
    symlink(&outside, root.join("link")).expect("symlink");
    let escaped = root.join("link").join("f.bin");
    assert!(matches!(
        validate_cache_path(&root, &escaped),
        Err(LocalError::UnsafeCachePath { .. })
    ));
}

/// Test double that records set_len / inc / finish / abandon calls.
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
fn hf_token_host_allowlist() {
    assert!(is_huggingface_https(
        "https://huggingface.co/org/repo/resolve/main/model.gguf"
    ));
    assert!(is_huggingface_https(
        "https://cdn-lfs.huggingface.co/repo/model.gguf"
    ));
    // Plaintext HTTP, arbitrary hosts, and look-alikes get no token.
    assert!(!is_huggingface_https(
        "http://huggingface.co/org/repo/model.gguf"
    ));
    assert!(!is_huggingface_https("https://evil.example/model.gguf"));
    assert!(!is_huggingface_https(
        "https://huggingface.co.evil.example/model.gguf"
    ));
    assert!(!is_huggingface_https("not a url"));
}

#[test]
fn download_with_progress_reports_content_length_and_bytes() {
    let body = b"progress-fixture-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let progress = RecordingProgress::new();
    let dest = temp.path().join("out.gguf");
    let digest = store
        .download_with_progress(&server.url("out.gguf"), &dest, &progress)
        .expect("download");
    assert_eq!(digest, hex_sha256(body));
    assert_eq!(
        *progress.total.lock().expect("total"),
        Some(body.len() as u64)
    );
    assert_eq!(progress.bytes.load(Ordering::Relaxed), body.len() as u64);
    progress.finish();
    assert_eq!(progress.finished.load(Ordering::Relaxed), 1);
}

/// Seeds an interrupted download: `partial` bytes at `dest` plus the
/// provenance marker naming `source`.
fn seed_partial(dest: &std::path::Path, partial: &[u8], source: &str) {
    std::fs::write(dest, partial).expect("write partial");
    std::fs::write(source_marker_path(dest), source).expect("write provenance marker");
}

#[test]
fn an_interrupted_download_resumes_from_the_partials_offset() {
    let body = b"resume-fixture: a body long enough to have a middle";
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("resumed.gguf");
    let dest = temp.path().join("resumed.gguf.part");
    let offset = 20_u64;
    seed_partial(
        &dest,
        &body[..usize::try_from(offset).expect("fixture offset")],
        &url,
    );
    let progress = RecordingProgress::new();

    let digest = store
        .download_with_progress(&url, &dest, &progress)
        .expect("resume completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(offset)],
        "the retry continues at the partial's offset"
    );
    assert_eq!(
        *progress.total.lock().expect("total"),
        Some(body.len() as u64),
        "the declared total covers the whole blob"
    );
    assert_eq!(
        progress.bytes.load(Ordering::Relaxed),
        body.len() as u64,
        "the resumed bytes count toward the total"
    );
    assert!(
        !source_marker_path(&dest).exists(),
        "a completed transfer removes the marker"
    );
}

#[test]
fn a_200_answer_to_a_range_request_restarts_from_zero() {
    // A server that ignores the Range header answers 200 with the whole
    // body; the partial is truncated and the transfer starts over.
    let body = b"restart-fixture-body";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("restart.gguf");
    let dest = temp.path().join("restart.gguf.part");
    seed_partial(&dest, &body[..10], &url);

    let digest = store
        .download_with_progress(&url, &dest, &RecordingProgress::new())
        .expect("restart completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(10), None],
        "the Range attempt is followed by a plain GET"
    );
}

#[test]
fn a_partial_larger_than_the_declared_size_restarts() {
    // The partial cannot belong to a blob smaller than itself: the Range
    // request is unsatisfiable (416) and the transfer restarts from zero.
    let body = b"declared-size-fixture";
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("oversized.gguf");
    let dest = temp.path().join("oversized.gguf.part");
    let oversized = body.len() as u64 + 9;
    seed_partial(
        &dest,
        &vec![b'x'; usize::try_from(oversized).expect("fixture size")],
        &url,
    );

    let digest = store
        .download_with_progress(&url, &dest, &RecordingProgress::new())
        .expect("restart completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[Some(oversized), None],
        "the unsatisfiable Range is followed by a plain GET"
    );
}

#[test]
fn a_partial_with_a_mismatched_marker_is_discarded() {
    // Provenance is the resume gate: a partial recorded against another
    // source is never appended to.
    let body = b"provenance-fixture-body";
    let server = FakeServer::new_range_aware(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let url = server.url("provenance.gguf");
    let dest = temp.path().join("provenance.gguf.part");
    seed_partial(&dest, b"foreign-bytes", "http://other.example/foreign.gguf");

    let digest = store
        .download_with_progress(&url, &dest, &RecordingProgress::new())
        .expect("fresh download completes");

    assert_eq!(digest, hex_sha256(body));
    assert_eq!(std::fs::read(&dest).expect("read partial"), body);
    assert_eq!(
        server.ranges().as_slice(),
        &[None],
        "no Range is sent for a foreign partial"
    );
}

#[test]
fn a_short_transfer_keeps_the_partial_and_marker_for_resume() {
    // A body that ends early against its declared length is a failed
    // transfer: the partial and its provenance marker stay on disk so the
    // next attempt resumes from the offset.
    let body = b"short-transfer-fixture-body";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind short server");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf); // consume the request head
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body[..8]); // part of the body, then close
        let _ = stream.flush();
    });

    let client = Client::builder().build().expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("short.bin");
    let url = format!("http://{addr}/short.bin");
    let err =
        super::download::download_with_progress(&client, &url, &dest, &RecordingProgress::new())
            .expect_err("a short body must fail");
    assert!(
        matches!(
            err,
            LocalError::DownloadRead { .. } | LocalError::Download { .. }
        ),
        "unexpected error {err:?}"
    );
    assert_eq!(
        std::fs::read(&dest).expect("partial kept"),
        &body[..8],
        "the transferred prefix stays on disk"
    );
    assert_eq!(
        std::fs::read_to_string(source_marker_path(&dest)).expect("marker kept"),
        url,
        "the provenance marker survives the failure"
    );
    // Unblock the server's pending state so the thread can exit.
    let _ = TcpStream::connect(addr);
    let _ = handle.join();
}

#[test]
fn hub_bearer_token_prefers_hf_token() {
    let token = hub_bearer_token(|key| match key {
        "HF_TOKEN" => Some(" hf_primary ".to_owned()),
        "HUGGING_FACE_HUB_TOKEN" => Some("hf_secondary".to_owned()),
        _ => None,
    });
    assert_eq!(token.as_deref(), Some("hf_primary"));
}

#[test]
fn hub_bearer_token_falls_back_to_hugging_face_hub_token() {
    let token = hub_bearer_token(|key| match key {
        "HUGGING_FACE_HUB_TOKEN" => Some("hf_fallback".to_owned()),
        _ => None,
    });
    assert_eq!(token.as_deref(), Some("hf_fallback"));
}

#[test]
fn hub_bearer_token_ignores_empty_and_missing() {
    assert!(hub_bearer_token(|_| None).is_none());
    assert!(hub_bearer_token(|_| Some(String::new())).is_none());
    assert!(hub_bearer_token(|_| Some("   ".to_owned())).is_none());
    assert_eq!(
        hub_bearer_token(|key| match key {
            "HF_TOKEN" => Some(String::new()),
            "HUGGING_FACE_HUB_TOKEN" => Some("hf_ok".to_owned()),
            _ => None,
        })
        .as_deref(),
        Some("hf_ok")
    );
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

#[test]
fn download_read_timeout_fails_on_a_stalled_body() {
    // ART-003: a peer that sends headers then stalls the body must fail on the
    // client's idle read timeout, not pin the download thread indefinitely.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled server");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf); // consume request head
        // Promise a body but send none; the client's read timeout must fire.
        let head = "HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.flush();
        // Hold the connection open by blocking on a read until the client drops.
        let _ = stream.read(&mut buf);
    });

    let client = Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .expect("client");
    let temp = TempDir::new().expect("tempdir");
    let dest = temp.path().join("stalled.bin");
    let progress = RecordingProgress::new();
    let err = super::download::download_with_progress(
        &client,
        &format!("http://{addr}/stalled.bin"),
        &dest,
        &progress,
    )
    .expect_err("stalled body must fail");
    assert!(
        matches!(
            err,
            LocalError::DownloadRead { .. } | LocalError::Download { .. }
        ),
        "unexpected error {err:?}"
    );
    // Unblock the server's pending read so the thread can exit.
    let _ = TcpStream::connect(addr);
    let _ = handle.join();
}

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
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn ensure_model_with_progress_reports_download_and_verify_leaves() {
    // A URL source flows through `ensure_blob`: the download leaf rides the
    // transfer, and the verify leaf completes on the inline pin check.
    let body = b"model-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let model = tree.register("model", 1.0);
    let url = server.url("model.gguf");
    let pin = hex_sha256(body);

    let path = store
        .ensure_model_with_progress(&url, Some(&pin), Some(&model))
        .expect("ensure model");
    assert_eq!(std::fs::read(&path).expect("read model"), body);
    let snapshot = hub.snapshot();
    let nodes = &snapshot[0].nodes;
    let paths: Vec<&str> = nodes.iter().map(|node| node.path.as_str()).collect();
    assert_eq!(paths, ["model", "model/download", "model/verify"]);
    assert!(
        nodes.iter().all(|node| node.fraction == 1.0),
        "both stages complete after a pinned download: {nodes:?}"
    );

    // A warm-cache repeat under a fresh subtree: the marker hit completes
    // verify without a hash pass, and the download leaf completes with no
    // transfer at all.
    let cached = tree.register("cached", 1.0);
    store
        .ensure_model_with_progress(&url, Some(&pin), Some(&cached))
        .expect("ensure model from cache");
    let snapshot = hub.snapshot();
    let nodes = &snapshot[0].nodes;
    let paths: Vec<&str> = nodes.iter().map(|node| node.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "model",
            "model/download",
            "model/verify",
            "cached",
            "cached/download",
            "cached/verify",
        ]
    );
    assert!(
        nodes.iter().all(|node| node.fraction == 1.0),
        "a cache hit completes both stages without work: {nodes:?}"
    );
    assert_eq!(server.requests(), 1, "the cache hit re-downloads nothing");
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn ensure_blob_mismatch_repair_finishes_the_verify_leaf_exactly_once() {
    // A cached blob whose content no longer matches the pin is repaired by
    // re-downloading: the first hash pass finishes the verify leaf, and the
    // pin recheck against the fresh download's inline digest must not emit
    // the leaf's terminal event a second time.
    let body = b"repaired-blob-bytes";
    let server = FakeServer::new(body);
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");
    let destination = temp.path().join("downloads").join("model.gguf");
    std::fs::create_dir_all(destination.parent().expect("downloads parent"))
        .expect("mkdir downloads");
    std::fs::write(&destination, b"stale-bytes").expect("write stale blob");

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let blob = tree.register("blob", 1.0);
    let download = blob.child("download", 4.0);
    let verify = blob.child("verify", 1.0);
    let url = server.url("model.gguf");
    let pin = hex_sha256(body);
    let asset = FileAsset {
        name: "model.gguf",
        url: &url,
        sha256: Some(&pin),
    };

    store
        .ensure_blob_with_progress(asset, &destination, Some(&download), Some(&verify))
        .expect("mismatch repair re-downloads");
    assert_eq!(std::fs::read(&destination).expect("read blob"), body);
    assert_eq!(download.fraction(), 1.0);
    assert_eq!(verify.fraction(), 1.0);
    assert_eq!(server.requests(), 1, "the repair downloads once");

    let mut verify_finished = 0;
    while let Ok(event) = rx.try_recv() {
        if event.path == "blob/verify" && matches!(event.state, EventState::Finished { .. }) {
            verify_finished += 1;
        }
    }
    assert_eq!(
        verify_finished, 1,
        "the pin recheck after repair must not re-emit the terminal event"
    );
}

#[test]
fn extract_failure_fails_the_leaf() {
    use zip::write::SimpleFileOptions;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("evil.zip");
    {
        let file = std::fs::File::create(&archive).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file("../escape.txt", SimpleFileOptions::default())
            .expect("start traversal entry");
        writer.write_all(b"pwned").expect("write entry");
        writer.finish().expect("finish zip");
    }
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("extract", 1.0);

    let result = extract_archive_with_progress(&archive, &dest, ArchiveKind::Zip, Some(&leaf));
    assert!(matches!(result, Err(LocalError::UnsafeArchiveEntry { .. })));

    let mut failed = false;
    while let Ok(event) = rx.try_recv() {
        if let EventState::Finished { ok } = event.state {
            failed = !ok;
        }
    }
    assert!(
        failed,
        "an extraction error ends the leaf with a failure terminal"
    );
}

#[test]
fn ensure_model_with_progress_fails_the_verify_leaf_on_a_bad_pin() {
    // A path source whose pin cannot be parsed returns before any verify
    // work; the registered verify leaf still owes its terminal event.
    let dir = TempDir::new().expect("tempdir");
    let model = dir.path().join("model.gguf");
    std::fs::write(&model, b"model-bytes").expect("write model");
    let store = ArtifactStore::new(dir.path().join("cache")).expect("store");

    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let parent = tree.register("model", 1.0);

    let result = store.ensure_model_with_progress(
        model.to_str().expect("utf8 path"),
        Some("abc"),
        Some(&parent),
    );
    assert!(matches!(result, Err(LocalError::InvalidDigest { .. })));

    let nodes = &hub.snapshot()[0].nodes;
    let download = nodes
        .iter()
        .find(|node| node.path == "model/download")
        .expect("download leaf");
    let verify = nodes
        .iter()
        .find(|node| node.path == "model/verify")
        .expect("verify leaf");
    assert!(
        download.finished && download.ok,
        "a path source completes the download leaf: {download:?}"
    );
    assert!(
        verify.finished && !verify.ok,
        "the unparseable pin fails the verify leaf: {verify:?}"
    );
}

#[cfg(not(llama_cuda_embedded))]
#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn provision_server_completes_download_verify_extract_leaves_on_a_warm_cache() {
    // A warm cache - the archive blob with a current verified marker and a
    // valid install tree - runs no download, hash, or extraction, but every
    // stage leaf still reaches its terminal event.
    let asset = server_asset(std::env::consts::OS, std::env::consts::ARCH).expect("host asset");
    let temp = TempDir::new().expect("tempdir");
    let store = ArtifactStore::new(temp.path()).expect("store");

    let archive = temp.path().join("downloads").join(asset.archive_name);
    std::fs::create_dir_all(archive.parent().expect("downloads parent")).expect("mkdir downloads");
    std::fs::write(&archive, b"mock-archive-bytes").expect("write archive");
    // A marker hit trusts the recorded digest plus size and mtime without
    // re-hashing, so the fixture can record the pinned digest directly.
    write_marker(&blob_marker_path(&archive), &archive, asset.sha256).expect("write marker");

    let install = temp
        .path()
        .join("llama.cpp")
        .join(format!("{LLAMA_RELEASE}-{}", asset.platform));
    std::fs::create_dir_all(&install).expect("mkdir install");
    std::fs::write(install.join(asset.executable_name), b"mock-server").expect("write executable");
    let tree_digest = super::digest::tree_digest(&install).expect("tree digest");
    std::fs::write(
        install.join(INSTALL_MARKER),
        format!("{}\n{tree_digest}\n", asset.sha256),
    )
    .expect("write install marker");

    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let server = tree.register("llama-server", 1.0);

    let provisioned = store
        .provision_llama_server_with_progress(Some(&server))
        .expect("warm-cache provision");
    assert_eq!(provisioned.executable, install.join(asset.executable_name));
    assert!(provisioned.path_prefix.is_empty());

    let snapshot = hub.snapshot();
    let nodes = &snapshot[0].nodes;
    let paths: Vec<&str> = nodes.iter().map(|node| node.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "llama-server",
            "llama-server/download",
            "llama-server/verify",
            "llama-server/extract",
        ]
    );
    assert!(
        nodes.iter().all(|node| node.fraction == 1.0),
        "a valid install completes every stage without work: {nodes:?}"
    );
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn tree_progress_drives_handle_fraction_per_byte() {
    let hub = Arc::new(ProgressHub::new());
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    let progress = TreeProgress::new(leaf.clone());

    progress.set_len(Some(200));
    progress.inc(50);
    assert_eq!(leaf.fraction(), 0.25);
    progress.inc(150);
    assert_eq!(leaf.fraction(), 1.0);
    progress.finish();
    assert_eq!(leaf.fraction(), 1.0);

    // Without a Content-Length the leaf stays indeterminate until finish.
    let unknown = tree.register("unknown-length", 1.0);
    let progress = TreeProgress::new(unknown.clone());
    progress.set_len(None);
    progress.inc(10);
    assert_eq!(unknown.fraction(), 0.0);
    progress.finish();
    assert_eq!(unknown.fraction(), 1.0);

    // Abandon completes the leaf: the handle vocabulary has no failure
    // terminal, so the owner carries failure through its own exit path.
    let abandoned = tree.register("abandoned", 1.0);
    let progress = TreeProgress::new(abandoned.clone());
    progress.set_len(Some(100));
    progress.inc(40);
    progress.abandon();
    assert_eq!(abandoned.fraction(), 1.0);
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn verify_blob_reports_bytes_read_during_hash() {
    // Two full 64 KiB read chunks: 0.5 after the first, 1.0 after the second.
    let body = vec![0xAB_u8; 128 * 1024];
    let (dir, blob, digest, marker) = pinned_blob_fixture(&body);
    let root = dir.path().join("cache");

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("verify", 1.0);

    let outcome =
        verify_blob_with_progress(&root, &blob, &digest, &marker, Some(&leaf)).expect("verify");
    assert_eq!(outcome, VerifyOutcome::Hashed);
    assert_eq!(leaf.fraction(), 1.0);

    let mut updates = Vec::new();
    let mut finished = false;
    while let Ok(event) = rx.try_recv() {
        match event.state {
            EventState::Updated { fraction } => updates.push(fraction),
            EventState::Finished { ok } => finished = ok,
            _ => {}
        }
    }
    assert_eq!(updates, vec![0.5, 1.0]);
    assert!(finished, "the hash pass ends with a terminal event");
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn verify_blob_marker_hit_completes_the_leaf_without_updates() {
    let body = b"blob-bytes";
    let (dir, blob, digest, marker) = pinned_blob_fixture(body);
    let root = dir.path().join("cache");
    let first = verify_blob(&root, &blob, &digest, &marker).expect("first verify");
    assert_eq!(first, VerifyOutcome::Hashed);

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("verify", 1.0);

    let outcome =
        verify_blob_with_progress(&root, &blob, &digest, &marker, Some(&leaf)).expect("verify");
    assert_eq!(outcome, VerifyOutcome::MarkerHit);
    assert_eq!(leaf.fraction(), 1.0);

    let mut updates = 0;
    let mut finished = false;
    while let Ok(event) = rx.try_recv() {
        match event.state {
            EventState::Updated { .. } => updates += 1,
            EventState::Finished { ok } => finished = ok,
            _ => {}
        }
    }
    assert_eq!(updates, 0, "a marker hit reads nothing and reports nothing");
    assert!(finished, "a marker hit still ends with a terminal event");
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn verify_blob_completes_the_leaf_before_a_digest_mismatch() {
    let body = b"blob-bytes";
    let (dir, blob, _digest, marker) = pinned_blob_fixture(body);
    let root = dir.path().join("cache");
    let wrong = hex_sha256(b"other-bytes");

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("verify", 1.0);

    let result = verify_blob_with_progress(&root, &blob, &wrong, &marker, Some(&leaf));
    assert!(matches!(result, Err(LocalError::DigestMismatch { .. })));
    assert_eq!(leaf.fraction(), 1.0);

    let mut finished = false;
    while let Ok(event) = rx.try_recv() {
        if let EventState::Finished { ok } = event.state {
            finished = ok;
        }
    }
    assert!(
        finished,
        "the hash pass ends with a terminal event even on mismatch"
    );
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn extract_zip_reports_entry_counts() {
    use zip::write::SimpleFileOptions;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("bundle.zip");
    {
        let file = std::fs::File::create(&archive).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        for index in 0..4 {
            writer
                .start_file(format!("file-{index}.txt"), SimpleFileOptions::default())
                .expect("start entry");
            writer.write_all(b"data").expect("write entry");
        }
        writer.finish().expect("finish zip");
    }
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("extract", 1.0);

    extract_archive_with_progress(&archive, &dest, ArchiveKind::Zip, Some(&leaf)).expect("extract");
    assert_eq!(leaf.fraction(), 1.0);

    let mut updates = Vec::new();
    let mut finished = false;
    while let Ok(event) = rx.try_recv() {
        match event.state {
            EventState::Updated { fraction } => updates.push(fraction),
            EventState::Finished { ok } => finished = ok,
            _ => {}
        }
    }
    assert_eq!(updates, vec![0.25, 0.5, 0.75, 1.0]);
    assert!(finished, "extraction ends with a terminal event");
}

#[test]
#[expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]
fn extract_tar_gz_reports_entry_counts() {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let dir = TempDir::new().expect("tempdir");
    let archive = dir.path().join("bundle.tar.gz");
    {
        let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
        for name in ["a.txt", "b.txt"] {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(4);
            header.set_mode(0o644);
            header.set_path(name).expect("set path");
            header.set_cksum();
            builder.append(&header, &b"data"[..]).expect("append entry");
        }
        let bytes = builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gz");
        std::fs::write(&archive, bytes).expect("write archive");
    }
    let dest = dir.path().join("out");
    std::fs::create_dir(&dest).expect("mkdir dest");

    let hub = Arc::new(ProgressHub::new());
    let mut rx = hub.subscribe();
    let tree = hub.operation();
    let leaf = tree.register("extract", 1.0);

    extract_archive_with_progress(&archive, &dest, ArchiveKind::TarGz, Some(&leaf))
        .expect("extract");
    assert_eq!(leaf.fraction(), 1.0);

    let mut updates = Vec::new();
    let mut finished = false;
    while let Ok(event) = rx.try_recv() {
        match event.state {
            EventState::Updated { fraction } => updates.push(fraction),
            EventState::Finished { ok } => finished = ok,
            _ => {}
        }
    }
    assert_eq!(updates, vec![0.5, 1.0]);
    assert!(finished, "extraction ends with a terminal event");
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
