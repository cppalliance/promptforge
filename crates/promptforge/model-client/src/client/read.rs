//! The transport-independent half of reading a completion off the wire:
//! the byte cap on a body, the SSE loop to the `[DONE]` sentinel, and the
//! client-side timing, over a caller-supplied [`ChunkSource`].
//!
//! No HTTP happens here and no clock is read. The transport supplies the
//! chunks and the clock; this module applies the one rule set every
//! transport shares, so the harness's gateway client and the engine's
//! test client differ only in how they send. A transport that grew its own
//! copy of this loop would be one more place the byte cap, the sentinel
//! rule, and the timing arithmetic could drift.

use std::future::Future;
use std::time::{Duration, Instant};

use promptforge_api_types::metrics::ClientTiming;
use serde_json::Value;

use super::{Applied, Completion, SseScanner, StreamAccumulator, StreamDelta};
use crate::Error;
use crate::model::CompletionError;

/// A response body read one chunk at a time: the transport's side of the
/// reassembly.
///
/// `#[doc(hidden)]`: a cross-crate seam for the stream transports (the
/// harness's model client and the engine's test client), not host API.
#[doc(hidden)]
pub trait ChunkSource {
    /// One chunk of body bytes, in whatever buffer the transport yields.
    type Chunk: AsRef<[u8]>;

    /// Returns the next chunk, or `None` once the body is exhausted.
    ///
    /// The transport maps its own read failure onto the
    /// [`CompletionError`] it reports, wrapping a timeout in
    /// [`Timeout`](crate::Timeout) so `is_timeout` survives the erasure.
    fn next_chunk(
        &mut self,
    ) -> impl Future<Output = Result<Option<Self::Chunk>, CompletionError>> + Send;
}

/// Reads a whole response body from `source`, refusing it once it would
/// exceed `cap` bytes.
///
/// `content_length` is the advertised length when the transport knows it;
/// it short-circuits an oversize body, and the streamed chunks are bounded
/// so a gateway that omits or lies about the length still cannot force an
/// unbounded allocation before decoding.
///
/// `#[doc(hidden)]`: a cross-crate seam for the stream transports, not
/// host API.
///
/// # Errors
/// Returns a `MalformedResponse`-kind [`CompletionError`] when the body
/// would exceed `cap`, and the source's own error when a read fails.
#[doc(hidden)]
pub async fn read_body_capped<S: ChunkSource>(
    source: &mut S,
    content_length: Option<u64>,
    cap: u64,
) -> Result<Vec<u8>, CompletionError> {
    if let Some(len) = content_length
        && len > cap
    {
        return Err(CompletionError::from(Error::MalformedResponse(format!(
            "response body of {len} bytes exceeds the {cap}-byte limit"
        ))));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = source.next_chunk().await? {
        let bytes = chunk.as_ref();
        if body.len() as u64 + bytes.len() as u64 > cap {
            return Err(CompletionError::from(Error::MalformedResponse(format!(
                "response body exceeds the {cap}-byte limit"
            ))));
        }
        body.extend_from_slice(bytes);
    }
    Ok(body)
}

/// Reads a completion's SSE stream from `source` to its `[DONE]` sentinel,
/// bounded by `max_bytes`, forwarding each live delta to `on_delta`, and
/// finishes the accumulation into the [`Completion`].
///
/// `started` is the transport's clock reading from before it sent the
/// request and `now` is that clock; the TTFT, mean inter-token latency,
/// and end-to-end figures on the completion's [`ClientTiming`] are
/// measured against them. Reading the clock is the transport's business:
/// this crate never does.
///
/// `#[doc(hidden)]`: a cross-crate seam for the stream transports, not
/// host API.
///
/// # Errors
/// Returns a `MalformedResponse`-kind [`CompletionError`] when the stream
/// exceeds `max_bytes` or ends without the sentinel, the source's own
/// error when a read fails, and the reassembly's errors otherwise (a
/// malformed chunk, a mid-stream error envelope, a truncated tool-call
/// batch, an empty turn).
#[doc(hidden)]
pub async fn read_completion_stream<S: ChunkSource>(
    source: &mut S,
    request_body: Value,
    max_bytes: u64,
    on_delta: impl Fn(StreamDelta),
    started: Instant,
    now: impl Fn() -> Instant,
) -> Result<Completion, CompletionError> {
    let mut scanner = SseScanner::new();
    let mut accumulator = StreamAccumulator::new();
    let mut received: u64 = 0;
    let mut first_delta: Option<Instant> = None;
    let mut last_delta: Option<Instant> = None;
    let mut delta_chunks: u32 = 0;
    let mut done = false;
    'read: while let Some(chunk) = source.next_chunk().await? {
        let bytes = chunk.as_ref();
        received += bytes.len() as u64;
        if received > max_bytes {
            return Err(CompletionError::from(Error::MalformedResponse(format!(
                "response stream exceeds the {max_bytes}-byte limit"
            ))));
        }
        scanner.extend(bytes);
        while let Some(data) = scanner.next_data() {
            match accumulator.apply(&data, &on_delta)? {
                Applied::Done => {
                    done = true;
                    break 'read;
                }
                Applied::Chunk { delta: true } => {
                    let at = now();
                    first_delta.get_or_insert(at);
                    last_delta = Some(at);
                    delta_chunks += 1;
                }
                Applied::Chunk { delta: false } => {}
            }
        }
    }
    // A stream that ends without the sentinel was cut off; its
    // accumulation may be missing the tail, so it must never pass for a
    // complete turn.
    if !done {
        return Err(CompletionError::from(Error::MalformedResponse(
            "completion stream ended without the [DONE] sentinel".into(),
        )));
    }
    let client_timing = ClientTiming {
        ttft_ms: first_delta.map(|at| duration_ms(at.duration_since(started))),
        mean_itl_ms: match (first_delta, last_delta) {
            (Some(first), Some(last)) if delta_chunks >= 2 => {
                Some(duration_ms(last.duration_since(first)) / f64::from(delta_chunks - 1))
            }
            _ => None,
        },
        e2e_ms: duration_ms(now().duration_since(started)),
    };
    // The truncation rule, the strict turn normalizer, and the lenient
    // metadata parser all run inside `finish`: one rule set for every
    // transport.
    accumulator.finish(request_body, Some(client_timing))
}

/// A duration as fractional milliseconds.
fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[cfg(test)]
#[path = "read-tests.rs"]
mod tests;
