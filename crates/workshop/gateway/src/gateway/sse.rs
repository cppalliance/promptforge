//! The SSE decode half of the gateway client: response-body capture and
//! the incremental decoder turning arbitrary byte chunks into `data:`
//! payloads.

use std::collections::VecDeque;

use futures_util::stream::{self, StreamExt};

use super::{GatewayError, GatewayResponse, SsePayloadStream};

/// Whether the gateway answered with an SSE body.
pub(super) fn is_event_stream(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

/// Captures the status and raw body of a gateway response.
pub(super) async fn read(response: reqwest::Response) -> Result<GatewayResponse, GatewayError> {
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| GatewayError::ReadBody(Box::new(source)))?
        .to_vec();
    Ok(GatewayResponse { status, body })
}

/// Decodes a gateway SSE response body into its `data:` payload stream.
///
/// Byte chunks arrive on arbitrary TCP boundaries, so the decoder buffers
/// partial lines; a mid-stream transport failure surfaces as one error item
/// that ends the stream.
pub(super) fn payload_stream(response: reqwest::Response) -> SsePayloadStream {
    let state = (response.bytes_stream(), SseDecoder::default(), false);
    let payloads = stream::try_unfold(state, |(mut bytes, mut decoder, mut eof)| async move {
        loop {
            if let Some(payload) = decoder.pop() {
                return Ok(Some((payload, (bytes, decoder, eof))));
            }
            if eof {
                return Ok(None);
            }
            match bytes.next().await {
                Some(Ok(chunk)) => decoder.feed(&chunk),
                Some(Err(source)) => return Err(GatewayError::ReadBody(Box::new(source))),
                None => {
                    decoder.finish();
                    eof = true;
                }
            }
        }
    });
    Box::pin(payloads)
}

/// Incremental SSE decoder: turns arbitrary byte chunks into `data:`
/// payloads, one per event, in arrival order.
///
/// Only `data:` fields are collected; `event:`, `id:`, `retry:`, and
/// comments are dropped, matching what an OpenAI-compatible stream carries.
/// Multiple `data:` lines in one event are joined with `\n` per the SSE
/// specification.
#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    /// Bytes received but not yet terminated by `\n`.
    partial: Vec<u8>,
    /// Joined `data:` lines of the event currently being accumulated.
    data: String,
    /// Whether the current event carries at least one `data:` line.
    has_data: bool,
    /// Completed payloads awaiting pickup.
    out: VecDeque<String>,
}

impl SseDecoder {
    /// Feeds one byte chunk, completing every event it terminates.
    pub(crate) fn feed(&mut self, chunk: &[u8]) {
        self.partial.extend_from_slice(chunk);
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.line(&line[..line.len() - 1]);
        }
    }

    /// Flushes a trailing unterminated line and any pending event at EOF.
    pub(crate) fn finish(&mut self) {
        if !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.line(&line);
        }
        self.dispatch();
    }

    /// Takes the oldest completed payload, if any.
    pub(crate) fn pop(&mut self) -> Option<String> {
        self.out.pop_front()
    }

    /// Handles one line without its `\n`; a blank line ends the event.
    fn line(&mut self, raw: &[u8]) {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if line.is_empty() {
            self.dispatch();
            return;
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if self.has_data {
                self.data.push('\n');
            }
            self.data.push_str(&String::from_utf8_lossy(value));
            self.has_data = true;
        }
    }

    /// Queues the accumulated event, dropping events with no `data:` line.
    fn dispatch(&mut self) {
        if self.has_data {
            self.out.push_back(std::mem::take(&mut self.data));
            self.has_data = false;
        }
    }
}
