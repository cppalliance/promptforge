//! The transport-independent half of reading a completion off the wire:
//! the byte cap on a body, the SSE loop to the `[DONE]` sentinel, and the
//! client-side timing, over a caller-supplied [`ChunkSource`].
//!
//! No HTTP happens here and no clock is read. The transport supplies the
//! chunks and the clock; this module applies the one rule set every
//! transport shares, so transports differ only in how they send. A
//! transport that grew its own copy of this loop would be one more place
//! the byte cap, the sentinel rule, and the timing arithmetic could drift.

use std::future::Future;
use std::time::{Duration, Instant};

use promptforge::metrics::ClientTiming;
use promptforge::model::{Completion, CompletionError};
use serde_json::Value;

use super::delta::StreamDelta;
use super::stream::{Applied, SseScanner, StreamAccumulator};
use crate::failure::malformed;

/// A response body that a transport supplies one chunk at a time.
///
/// A transport is the code that sends a model request and receives the
/// reply, for example over HTTP. It answers a `Chat` effect, the Engine's
/// request for one model reply, in four steps:
///
/// 1. It builds the request body with
///    [`build_request_body`](crate::build_request_body). It reads its
///    clock and then sends the request.
/// 2. It wraps the response body in a `ChunkSource`.
/// 3. On a status outside the 2xx range, it reads the error body whole with
///    [`read_body_capped`]. It bounds the body's length and escapes its
///    control characters with [`escape_controls`](crate::escape_controls).
///    It then fails the round with the error that
///    [`classify_http_failure`](crate::classify_http_failure) returns.
/// 4. Otherwise, it passes the source to [`read_completion_stream`]. The
///    returned [`Completion`] answers the effect.
///
/// The transport opens the connection and supplies every clock reading.
/// The source is the only I/O that `read_body_capped` and
/// `read_completion_stream` touch.
pub trait ChunkSource {
    /// One chunk of body bytes, in whatever buffer the transport yields.
    type Chunk: AsRef<[u8]>;

    /// Returns the next chunk, or `None` once the body is exhausted.
    ///
    /// When a read fails, the implementation returns the
    /// [`CompletionError`] that the round fails with. It is a
    /// `Timeout`-kind error when the read ran out of time and a
    /// `Transport`-kind error otherwise. Build it with
    /// [`CompletionError::new`] and attach the transport's own error with
    /// [`CompletionError::with_source`].
    fn next_chunk(
        &mut self,
    ) -> impl Future<Output = Result<Option<Self::Chunk>, CompletionError>> + Send;
}

/// Reads a whole response body from `source`, refusing it once it would
/// exceed `cap` bytes.
///
/// Use it for a body the transport decodes whole, such as the error body
/// of a status outside the 2xx range or a JSON document like the
/// gateway's model list.
///
/// `content_length` is the length the response advertises, when the
/// transport knows it. An advertised length over `cap` fails at once,
/// before any chunk is read. The function also counts bytes as the chunks
/// arrive, so a gateway that omits or misstates the length still cannot
/// force an unbounded allocation before the body is decoded.
///
/// # Errors
/// Returns a `MalformedResponse`-kind [`CompletionError`] when the body
/// would exceed `cap`, and the source's own error when a read fails.
pub async fn read_body_capped<S: ChunkSource>(
    source: &mut S,
    content_length: Option<u64>,
    cap: u64,
) -> Result<Vec<u8>, CompletionError> {
    if let Some(len) = content_length
        && len > cap
    {
        return Err(malformed(format!(
            "response body of {len} bytes exceeds the {cap}-byte limit"
        )));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = source.next_chunk().await? {
        let bytes = chunk.as_ref();
        if body.len() as u64 + bytes.len() as u64 > cap {
            return Err(malformed(format!(
                "response body exceeds the {cap}-byte limit"
            )));
        }
        body.extend_from_slice(bytes);
    }
    Ok(body)
}

/// Reads a streamed model reply from `source` and assembles it into a
/// [`Completion`].
///
/// The reply arrives as server-sent events (SSE). The function reads
/// events until the `[DONE]` sentinel and fails if the stream exceeds
/// `max_bytes` bytes. It passes each [`StreamDelta`] to `on_delta` as soon
/// as it is decoded, so a Host can show the reply as it arrives. The
/// returned completion holds the whole turn either way.
///
/// `request_body` is the body the transport sent, as
/// [`build_request_body`](crate::build_request_body) returned it. The
/// completion carries it back, so a run's debug capture records exactly
/// what was sent. The completion is labeled with the model that
/// `request_body` names.
///
/// `started` is the transport's clock reading from just before it sent
/// the request, and `now` reads that same clock. The completion's
/// [`ClientTiming`] holds three figures measured against them: time to
/// first token, mean inter-token latency, and end-to-end time. It takes
/// every clock reading from `started` and `now`.
///
/// # Errors
/// Returns a `MalformedResponse`-kind [`CompletionError`] when the stream
/// exceeds `max_bytes` or ends before the sentinel, and the source's own
/// error when a read fails. Also returns the error that reassembling the
/// reply raises for a malformed chunk, a mid-stream error envelope, a
/// truncated tool-call batch, or an empty turn.
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
            return Err(malformed(format!(
                "response stream exceeds the {max_bytes}-byte limit"
            )));
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
        return Err(malformed(
            "completion stream ended without the [DONE] sentinel",
        ));
    }
    let client_timing = ClientTiming {
        ttft_ms: first_delta.map(|at| duration_ms(at.duration_since(started))),
        mean_itl_ms: match (first_delta, last_delta) {
            (Some(first), Some(last)) if delta_chunks >= 2 => Some(round_to_microsecond(
                duration_ms(last.duration_since(first)) / f64::from(delta_chunks - 1),
            )),
            _ => None,
        },
        e2e_ms: duration_ms(now().duration_since(started)),
    };
    // The truncation rule, the strict turn normalizer, and the lenient
    // metadata parser all run inside `finish`: one rule set for every
    // transport.
    accumulator.finish(request_body, Some(client_timing))
}

/// A duration as fractional milliseconds, rounded to a whole microsecond
/// so the text the run log stores parses back exactly.
fn duration_ms(duration: Duration) -> f64 {
    round_to_microsecond(duration.as_secs_f64() * 1000.0)
}

/// Rounds fractional-millisecond `ms` to the nearest whole microsecond.
///
/// A run log stores a timing as JSON text and parses that text back. A
/// whole-microsecond value has a short decimal form the parser reproduces
/// exactly, so the parsed timing equals the recorded one.
fn round_to_microsecond(ms: f64) -> f64 {
    (ms * 1000.0).round() / 1000.0
}

#[cfg(test)]
#[path = "read-tests.rs"]
mod tests;
