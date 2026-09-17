//! Test doubles shared by the crate's unit tests: a blocking fake HTTP server
//! and a SHA-256 hex helper.

use std::io::{self, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::artifacts::hex_digest;

/// A blocking one-response HTTP server on an ephemeral loopback port.
///
/// The socket is bound before the accept thread starts, so the kernel backlog
/// holds any early client connection until `accept` runs. There is no startup
/// race and thus no startup sleep, and blocking `accept` needs no WouldBlock
/// poll loop. `Drop` wakes the final blocking `accept` with a self-connect
/// after setting `shutdown`.
pub(crate) struct FakeServer {
    address: String,
    requests: Arc<AtomicUsize>,
    ranges: Arc<Mutex<Vec<Option<u64>>>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

/// The `Range: bytes=<start>-` offset of one request head, if it carried one.
fn parse_range_start(request: &[u8]) -> Option<u64> {
    let head = String::from_utf8_lossy(request);
    for line in head.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("range:") {
            let spec = value.trim().strip_prefix("bytes=")?;
            let (start, _) = spec.split_once('-')?;
            return start.trim().parse().ok();
        }
    }
    None
}

impl FakeServer {
    /// A server that ignores `Range` headers and always answers 200 with the
    /// full body, like a bare static host.
    pub(crate) fn new(body: &[u8]) -> Self {
        Self::serve(body, false)
    }

    /// A server that honors `Range: bytes=<start>-` like a real static host:
    /// 206 with the tail and a `Content-Range` header, 416 when the start is
    /// at or past the end of the body.
    pub(crate) fn new_range_aware(body: &[u8]) -> Self {
        Self::serve(body, true)
    }

    fn serve(body: &[u8], honor_range: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let address = listener.local_addr().expect("local addr").to_string();
        let requests = Arc::new(AtomicUsize::new(0));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_ranges = Arc::clone(&ranges);
        let thread_shutdown = Arc::clone(&shutdown);
        let body = body.to_owned();
        // The thread returns an `io::Result`: a genuine write/flush failure while
        // serving a real client is surfaced on join (HYGIENE-RESULT-001) instead
        // of being swallowed, so a broken fixture cannot masquerade as success.
        // The shutdown self-connect is skipped by the top-of-loop `shutdown`
        // check, so it never counts as a serve error.
        let thread = thread::spawn(move || -> io::Result<()> {
            for stream in listener.incoming() {
                if thread_shutdown.load(Ordering::Acquire) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
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
                let range_start = parse_range_start(&request).filter(|_| honor_range);
                thread_ranges
                    .lock()
                    .expect("ranges lock")
                    .push(parse_range_start(&request));
                let body_len = body.len() as u64;
                let (head, slice) = match range_start {
                    Some(start) if start < body_len => (
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            body_len - start,
                            start,
                            body_len - 1,
                            body_len
                        ),
                        // The guard bounds the start to the body's length.
                        &body[usize::try_from(start).expect("the range guard bounds the start")..],
                    ),
                    Some(_) => (
                        format!(
                            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nContent-Range: bytes */{body_len}\r\nConnection: close\r\n\r\n"
                        ),
                        &body[..0],
                    ),
                    None => (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
                        ),
                        &body[..],
                    ),
                };
                stream.write_all(head.as_bytes())?;
                stream.write_all(slice)?;
                stream.flush()?;
                thread_requests.fetch_add(1, Ordering::AcqRel);
            }
            Ok(())
        });
        Self {
            address,
            requests,
            ranges,
            shutdown,
            thread: Some(thread),
        }
    }

    pub(crate) fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.address)
    }

    pub(crate) fn requests(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }

    /// The `Range` offset of every request received, in order; `None` for a
    /// request with no Range header.
    pub(crate) fn ranges(&self) -> Vec<Option<u64>> {
        self.ranges.lock().expect("ranges lock").clone()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let joined = thread.join();
            // Don't mask an in-flight test panic, but otherwise a serve-side
            // transport failure must surface rather than be silently dropped.
            if !std::thread::panicking() {
                joined
                    .expect("fake server thread panicked")
                    .expect("fake server encountered a socket write/flush error");
            }
        }
    }
}

/// Lowercase hex SHA-256 of `bytes`, for test fixtures.
pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher)
}
