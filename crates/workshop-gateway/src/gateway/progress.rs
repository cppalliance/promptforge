//! The `GET /admin/progress` subscription: a long-lived SSE stream of
//! [`ProgressEvent`]s, decoded block-by-block under a hard size bound.
//!
//! Unlike the switch and cache streams, a progress subscription never
//! terminates on its own and carries events the workshop imports into
//! the progress hub verbatim, so the decode keeps the stricter posture
//! the subscriber always had: only blank-line-terminated blocks
//! dispatch (an incomplete trailing block is discarded), and a block
//! that grows past `MAX_EVENT_BLOCK` without its terminator is
//! refused rather than buffered unbounded.

use std::pin::Pin;

use futures_util::Stream;
use shared_progress::ProgressEvent;

use super::GatewayError;

/// The largest single SSE event block buffered before the stream refuses
/// it, in bytes. A peer that never sends a blank-line terminator would
/// otherwise grow the reassembly buffer unbounded; sized well above any
/// realistic progress event.
pub(crate) const MAX_EVENT_BLOCK: usize = 1024 * 1024;

/// The largest error body kept for a subscription diagnostic, in bytes.
const MAX_ERROR_BODY: usize = 2000;

/// A stream of decoded [`ProgressEvent`]s from the gateway, in arrival
/// order.
///
/// A `data:` block that does not decode is yielded as one error item
/// without ending the stream; a read failure or an event block oversized
/// beyond `MAX_EVENT_BLOCK` is yielded as one error item that ends the
/// stream. The stream ends when the gateway closes the body; whether to
/// resubscribe is the caller's decision.
pub type ProgressEventStream =
    Pin<Box<dyn Stream<Item = Result<ProgressEvent, GatewayError>> + Send>>;

/// Turns an answered `GET /admin/progress` request into the event stream.
///
/// The endpoint answers only an event stream on success, so a
/// non-success status is [`GatewayError::Status`] carrying a bounded,
/// control-escaped body rather than a relayed response.
pub(super) async fn subscribe(
    response: reqwest::Response,
) -> Result<ProgressEventStream, GatewayError> {
    let status = response.status();
    if !status.is_success() {
        return Err(GatewayError::Status {
            status,
            body: error_body(response).await?,
        });
    }
    Ok(decode(response))
}

/// Reads at most [`MAX_ERROR_BODY`] bytes of a non-success response
/// body, escaping control characters so a hostile body cannot forge log
/// lines or smuggle terminal control sequences into a diagnostic.
async fn error_body(mut response: reqwest::Response) -> Result<String, GatewayError> {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < MAX_ERROR_BODY {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let take = (MAX_ERROR_BODY - buffer.len()).min(chunk.len());
                buffer.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            Ok(None) => break,
            Err(source) => return Err(GatewayError::ReadBody(Box::new(source))),
        }
    }
    if buffer.is_empty() {
        return Ok("(empty body)".to_owned());
    }
    let lossy = String::from_utf8_lossy(&buffer);
    let mut escaped = String::with_capacity(lossy.len());
    for ch in lossy.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    Ok(escaped)
}

/// Decodes an SSE body into a progress-event stream: chunks are buffered
/// until a blank line terminates an event block (LF or CRLF line endings
/// alike), comment-only blocks (heartbeats) are skipped, and an
/// undecodable block becomes one error item rather than killing the
/// stream. A mid-stream read failure likewise surfaces as one error
/// item, after which the stream ends. A block that grows past
/// [`MAX_EVENT_BLOCK`] without a terminator is refused as one error
/// item, after which the stream ends, so a peer cannot buffer the client
/// unbounded.
fn decode(response: reqwest::Response) -> ProgressEventStream {
    let events = futures_util::stream::unfold(
        (response, Vec::new(), false),
        |(mut response, mut buffer, mut failed)| async move {
            loop {
                if let Some(item) = next_buffered_event(&mut buffer) {
                    return Some((item, (response, buffer, failed)));
                }
                if failed {
                    return None;
                }
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if buffer.len() + chunk.len() > MAX_EVENT_BLOCK {
                            failed = true;
                            let item = Err(GatewayError::Malformed {
                                message: format!(
                                    "progress event block exceeds the {MAX_EVENT_BLOCK}-byte limit"
                                ),
                                source: None,
                            });
                            return Some((item, (response, buffer, failed)));
                        }
                        buffer.extend_from_slice(&chunk);
                    }
                    // An incomplete trailing block is discarded, matching
                    // the SSE rule that only blank-line-terminated blocks
                    // dispatch.
                    Ok(None) => return None,
                    Err(source) => {
                        failed = true;
                        let item = Err(GatewayError::ReadBody(Box::new(source)));
                        return Some((item, (response, buffer, failed)));
                    }
                }
            }
        },
    );
    Box::pin(events)
}

/// Pops the next decodable event out of `buffer`, or `None` when no
/// complete block is buffered yet. Comment-only blocks (heartbeats) are
/// consumed and skipped.
fn next_buffered_event(buffer: &mut Vec<u8>) -> Option<Result<ProgressEvent, GatewayError>> {
    loop {
        let end = block_end(buffer)?;
        let block: Vec<u8> = buffer.drain(..end).collect();
        if let Some(item) = parse_event_block(&block) {
            return Some(item);
        }
    }
}

/// The end (terminator included) of the first complete event block: a
/// blank line, whether the peer terminates its lines with LF or CRLF.
fn block_end(buffer: &[u8]) -> Option<usize> {
    let lf = buffer
        .windows(2)
        .position(|pair| pair == b"\n\n")
        .map(|at| at + 2);
    let crlf = buffer
        .windows(4)
        .position(|quad| quad == b"\r\n\r\n")
        .map(|at| at + 4);
    [lf, crlf].into_iter().flatten().min()
}

/// Decodes one SSE event block: `data:` lines join into the payload,
/// comment lines and unrecognized fields are ignored, and a block with
/// no payload (a heartbeat) yields `None`.
fn parse_event_block(block: &[u8]) -> Option<Result<ProgressEvent, GatewayError>> {
    let mut data: Vec<u8> = Vec::new();
    for line in block.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(rest) = line.strip_prefix(b"data:") {
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(rest.strip_prefix(b" ").unwrap_or(rest));
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(
        serde_json::from_slice(&data).map_err(|source| GatewayError::Malformed {
            message: "progress event was not valid JSON".to_owned(),
            source: Some(Box::new(source)),
        }),
    )
}
