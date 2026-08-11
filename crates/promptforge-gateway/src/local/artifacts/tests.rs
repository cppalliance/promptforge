use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::archive::safe_archive_path;
use super::download::{hub_bearer_token, is_huggingface_https};
use super::progress::{DownloadProgress, download_label, progress_for_download};
use super::*;

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

struct FakeServer {
    address: String,
    requests: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    fn new(body: &[u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake server");
        let address = listener.local_addr().expect("local addr").to_string();
        let requests = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_shutdown = Arc::clone(&shutdown);
        let body = body.to_owned();
        let thread = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if thread_shutdown.load(Ordering::Acquire) {
                            break;
                        }
                        stream
                            .set_nonblocking(false)
                            .expect("make accepted socket blocking");
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                        let mut request = Vec::new();
                        let mut buf = [0_u8; 1024];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    request.extend_from_slice(&buf[..n]);
                                    if request.windows(4).any(|w| w == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                            }
                        }
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(&body);
                        let _ = stream.flush();
                        thread_requests.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });
        // Give the accept loop a moment to start before clients connect.
        thread::sleep(Duration::from_millis(10));
        Self {
            address,
            requests,
            shutdown,
            thread: Some(thread),
        }
    }

    fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.address)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher)
}

#[test]
fn download_label_uses_url_basename() {
    assert_eq!(
        download_label("https://huggingface.co/google/gemma/resolve/main/gemma-3-27b-it-q4_0.gguf"),
        "gemma-3-27b-it-q4_0.gguf"
    );
    assert_eq!(
        download_label("https://example.com/file.gguf?download=true"),
        "file.gguf"
    );
}

#[test]
fn progress_for_download_picks_bar_on_tty_and_log_off_tty() {
    let tty = progress_for_download("x.gguf", true);
    let log = progress_for_download("x.gguf", false);
    // Type names are enough: both implement the trait and accept a finish.
    tty.set_len(Some(10));
    tty.inc(10);
    tty.finish();
    log.set_len(Some(10));
    log.inc(10);
    log.finish();
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
