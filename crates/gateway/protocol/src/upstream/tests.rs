//! Tests for the upstream seam, with the shared mock backends and log capture.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Map;

use super::*;

mod chat;
mod embedding;
mod rerank;
mod speech;
mod stream;

/// A one-shot mock backend: serves a single canned `(status, body)` and
/// returns its base URL plus the captured raw request for assertions.
fn serve_once(status_line: &str, body: &str) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock backend");
    let addr = listener.local_addr().expect("addr");
    let response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let handle = thread::spawn(move || -> String {
        let (mut stream, _) = listener.accept().expect("accept");
        // A short read timeout bounds request capture without a sleep: once
        // the client has sent its request and is awaiting a response, the
        // next read simply times out and we reply.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let mut request = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        String::from_utf8_lossy(&request).into_owned()
    });
    (format!("http://{addr}"), handle)
}

fn request(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.to_owned(),
        messages: vec![serde_json::json!({ "role": "user", "content": "hi" })],
        stream: false,
        rest: Map::new(),
    }
}

/// A shared buffer that captures what the parser logs, so tests can
/// assert on malformed-chunk warnings.
#[derive(Clone, Default)]
struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl LogBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
    }
}

impl std::io::Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Installs a subscriber at `max_level` writing to a fresh capture buffer
/// for the current thread (tokio's current-thread test runtime keeps every
/// poll on this thread, so the events land in the buffer).
fn capture_logs(max_level: tracing::Level) -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_ansi(false)
        .with_max_level(max_level)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    (buffer, guard)
}

/// Installs a WARN-level subscriber writing to a fresh capture buffer for
/// the current thread, for the parser's warning assertions.
fn capture_warnings() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
    capture_logs(tracing::Level::WARN)
}

/// A server that accepts the connection and then never sends a response, so
/// the client's request deadline (not an idle read) is what must fire.
fn serve_stalled() -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled backend");
    let addr = listener.local_addr().expect("addr");
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // A read timeout bounds the server thread: the client's request
        // deadline fires first; this only stops the thread from blocking
        // forever if the client keeps the socket open in its pool.
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
        let mut buf = [0_u8; 1024];
        let _ = stream.read(&mut buf); // consume request head
        // Never write a response; the client must fail on its own deadline.
        let _ = stream.read(&mut buf);
    });
    (format!("http://{addr}"), handle)
}
