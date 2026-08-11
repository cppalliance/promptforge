//! Shared HTTP client policy: bounded body reads and outbound timeouts.
//!
//! Every outbound call in the gateway reads bodies through [`read_body_capped`]
//! so a malicious or mistaken backend cannot force an unbounded allocation, and
//! builds its client through [`bounded_client`] so a stalled peer cannot pin a
//! request or a concurrency slot forever.

use std::time::Duration;

/// Maximum bytes read from a non-success (error) body, kept for diagnostics.
pub(crate) const MAX_ERROR_BODY: usize = 64 * 1024;

/// Maximum bytes read from a success JSON body before decoding.
pub(crate) const MAX_JSON_BODY: usize = 4 * 1024 * 1024;

/// Connect timeout for outbound calls.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Whole-request timeout for outbound calls.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Build a reqwest client with bounded connect and whole-request timeouts.
pub(crate) fn bounded_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Read at most `cap` bytes from `response`, stopping early once the cap is hit.
///
/// The body is streamed chunk by chunk so an oversized or stalled response never
/// allocates beyond `cap`. Returns a lossy UTF-8 string of the bytes read.
pub(crate) async fn read_body_capped(response: reqwest::Response, cap: usize) -> String {
    let mut response = response;
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        let remaining = cap.saturating_sub(buffer.len());
        if remaining == 0 {
            break;
        }
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let take = remaining.min(chunk.len());
                buffer.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

/// Read at most `cap` bytes from `response`, propagating a transport error if a
/// chunk read fails.
///
/// Unlike [`read_body_capped`], this surfaces the read result explicitly so a
/// caller can distinguish a genuine transport failure from a short body before
/// deserializing.
///
/// # Errors
/// Returns the underlying [`reqwest::Error`] when streaming a chunk fails.
pub(crate) async fn read_bytes_capped(
    response: reqwest::Response,
    cap: usize,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut response = response;
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < cap {
        match response.chunk().await? {
            Some(chunk) => {
                let remaining = cap - buffer.len();
                let take = remaining.min(chunk.len());
                buffer.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            None => break,
        }
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use super::*;

    /// A one-shot server that returns a `200 OK` with `body` bytes.
    fn serve_body(body: Vec<u8>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        });
        (format!("http://{addr}"), handle)
    }

    async fn get(url: &str) -> reqwest::Response {
        reqwest::Client::new().get(url).send().await.expect("send")
    }

    #[tokio::test]
    async fn read_bytes_capped_keeps_exact_limit_and_truncates_over_limit() {
        // Success-body path: a body of exactly `cap` bytes is returned whole; a
        // body one byte over the cap is truncated to `cap` (never unbounded).
        let (base, handle) = serve_body(vec![b'a'; 10]);
        let bytes = read_bytes_capped(get(&base).await, 10).await.expect("read");
        assert_eq!(bytes.len(), 10);
        handle.join().expect("join");

        let (base, handle) = serve_body(vec![b'a'; 11]);
        let bytes = read_bytes_capped(get(&base).await, 10).await.expect("read");
        assert_eq!(bytes.len(), 10, "over-limit body truncated to cap");
        handle.join().expect("join");

        let (base, handle) = serve_body(vec![b'a'; 4]);
        let bytes = read_bytes_capped(get(&base).await, 10).await.expect("read");
        assert_eq!(bytes.len(), 4, "under-limit body returned whole");
        handle.join().expect("join");
    }

    #[tokio::test]
    async fn read_body_capped_keeps_exact_limit_and_truncates_over_limit() {
        // Error-body path: same exact/over/under boundary behavior, as a string.
        let (base, handle) = serve_body(vec![b'z'; 10]);
        let body = read_body_capped(get(&base).await, 10).await;
        assert_eq!(body.chars().count(), 10);
        handle.join().expect("join");

        let (base, handle) = serve_body(vec![b'z'; 11]);
        let body = read_body_capped(get(&base).await, 10).await;
        assert_eq!(body.chars().count(), 10, "over-limit error body truncated");
        handle.join().expect("join");
    }
}
