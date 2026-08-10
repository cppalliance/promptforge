//! Shared HTTP client policy: bounded body reads and outbound timeouts.
//!
//! Every outbound call in the gateway reads bodies through [`read_body_capped`]
//! so a malicious or mistaken backend cannot force an unbounded allocation, and
//! builds its client through [`bounded_client`] so a stalled peer cannot pin a
//! request or a concurrency slot forever.

use std::time::Duration;

/// Maximum bytes read from a non-success (error) body, kept for diagnostics.
pub(crate) const MAX_ERROR_BODY: usize = 64 * 1024;

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
