//! HTTP client for the PromptForge gateway's OpenAI-compatible API.
//!
//! [`GatewayClient`] wraps `reqwest` with bearer authentication and returns
//! responses as raw bytes so the workbench routes can relay them to the
//! caller byte-for-byte. A non-success status from the gateway is *not* an
//! error here: it is part of the relayed response. Streaming chat requests
//! are decoded from SSE into a [`SsePayloadStream`] of `data:` payloads.

use std::collections::VecDeque;
use std::pin::Pin;

use futures_util::stream::{self, Stream, StreamExt};
use serde::{Deserialize, Serialize};

/// A non-streaming chat completion request forwarded to the gateway.
///
/// This is the body accepted by the workbench's `POST /chat` and sent
/// upstream to `POST /v1/chat/completions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The model name from the gateway catalog.
    pub model: String,
    /// OpenAI chat messages, relayed without inspecting their shape.
    pub messages: Vec<serde_json::Value>,
}

/// A gateway HTTP response captured for verbatim relay.
#[derive(Debug)]
pub struct GatewayResponse {
    /// The gateway's status code, relayed unchanged.
    pub status: reqwest::StatusCode,
    /// The gateway's response body, relayed byte-for-byte.
    pub body: Vec<u8>,
}

/// A stream of SSE `data:` payloads from the gateway, in arrival order.
///
/// Each item is one event's data, verbatim; the OpenAI terminal sentinel
/// arrives as the payload `"[DONE]"`. A transport failure mid-stream yields
/// one error item and then ends the stream.
pub type SsePayloadStream = Pin<Box<dyn Stream<Item = Result<String, GatewayError>> + Send>>;

/// The outcome of a streaming chat request to the gateway.
#[non_exhaustive]
pub enum ChatStream {
    /// The gateway accepted the stream; payloads arrive in order.
    #[non_exhaustive]
    Stream {
        /// The gateway's success status, relayed unchanged.
        status: reqwest::StatusCode,
        /// The SSE payload stream, ending with the `"[DONE]"` payload.
        payloads: SsePayloadStream,
    },

    /// The gateway answered with an ordinary (non-SSE) response, buffered
    /// for verbatim relay; this is how a declined stream reports its error
    /// envelope.
    #[non_exhaustive]
    Relay(GatewayResponse),
}

// Manual because the boxed payload stream has no `Debug` impl.
impl std::fmt::Debug for ChatStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream { status, .. } => f
                .debug_struct("ChatStream")
                .field("status", status)
                .finish_non_exhaustive(),
            Self::Relay(response) => f.debug_tuple("Relay").field(response).finish(),
        }
    }
}

/// A gateway request failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayError {
    /// The HTTP client could not be built.
    #[non_exhaustive]
    #[error("build gateway http client")]
    Build(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The request could not be sent or no response arrived (connect
    /// refused, DNS, TLS, timeout).
    #[non_exhaustive]
    #[error("gateway transport error")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The response body could not be read to completion.
    #[non_exhaustive]
    #[error("read gateway response body")]
    ReadBody(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The chat request could not be serialized to JSON.
    #[non_exhaustive]
    #[error("serialize chat request")]
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Bearer-authenticated client for the gateway's OpenAI-compatible
/// endpoints.
#[derive(Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

// Manual so the bearer key is never written to logs.
impl std::fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Builds a client for `base_url` authenticating with `api_key`.
    ///
    /// A trailing slash on `base_url` is trimmed so route joins stay clean.
    ///
    /// # Errors
    /// Returns [`GatewayError::Build`] if the TLS backend cannot initialize.
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, GatewayError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|source| GatewayError::Build(Box::new(source)))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        })
    }

    /// Fetches the gateway's model catalog from `GET /v1/models`.
    ///
    /// A non-success status is relayed in the returned
    /// [`GatewayResponse`], not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed and [`GatewayError::ReadBody`] if the response body cannot
    /// be read.
    pub async fn list_models(&self) -> Result<GatewayResponse, GatewayError> {
        let response = self
            .http
            .get(format!("{}/v1/models", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        read(response).await
    }

    /// Posts a non-streaming chat completion to
    /// `POST /v1/chat/completions`.
    ///
    /// A non-success status is relayed in the returned
    /// [`GatewayResponse`], not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed and [`GatewayError::ReadBody`] if the response body cannot
    /// be read.
    pub async fn chat_completion(
        &self,
        request: &ChatRequest,
    ) -> Result<GatewayResponse, GatewayError> {
        let response = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(request)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        read(response).await
    }

    /// Posts a streaming chat completion to `POST /v1/chat/completions`
    /// with `"stream": true` added to the request body.
    ///
    /// A success status yields [`ChatStream::Stream`] carrying the SSE
    /// payload stream; any other status is buffered and returned as
    /// [`ChatStream::Relay`] so the caller can relay the gateway's error
    /// envelope verbatim.
    ///
    /// # Errors
    /// Returns [`GatewayError::Serialize`] if the request cannot be
    /// serialized, [`GatewayError::Transport`] if the request cannot be
    /// completed, and [`GatewayError::ReadBody`] if a declined stream's
    /// error body cannot be read.
    pub async fn chat_completion_stream(
        &self,
        request: &ChatRequest,
    ) -> Result<ChatStream, GatewayError> {
        let mut body = serde_json::to_value(request)
            .map_err(|source| GatewayError::Serialize(Box::new(source)))?;
        if let Some(object) = body.as_object_mut() {
            object.insert("stream".to_string(), serde_json::Value::Bool(true));
        }
        let response = self
            .http
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        let status = response.status();
        if !status.is_success() {
            return read(response).await.map(ChatStream::Relay);
        }
        Ok(ChatStream::Stream {
            status,
            payloads: payload_stream(response),
        })
    }
}

/// Captures the status and raw body of a gateway response.
async fn read(response: reqwest::Response) -> Result<GatewayResponse, GatewayError> {
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
fn payload_stream(response: reqwest::Response) -> SsePayloadStream {
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
struct SseDecoder {
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
    fn feed(&mut self, chunk: &[u8]) {
        self.partial.extend_from_slice(chunk);
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.line(&line[..line.len() - 1]);
        }
    }

    /// Flushes a trailing unterminated line and any pending event at EOF.
    fn finish(&mut self) {
        if !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.line(&line);
        }
        self.dispatch();
    }

    /// Takes the oldest completed payload, if any.
    fn pop(&mut self) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_slash_is_trimmed_from_base_url() {
        let client = GatewayClient::new("http://127.0.0.1:8081/", "k").expect("client builds");
        assert_eq!(client.base_url, "http://127.0.0.1:8081");
    }

    #[test]
    fn debug_redacts_the_api_key() {
        let client = GatewayClient::new("http://127.0.0.1:8081", "secret-key").expect("client");
        let rendered = format!("{client:?}");
        assert!(!rendered.contains("secret-key"), "key leaked: {rendered}");
    }

    fn drain(decoder: &mut SseDecoder) -> Vec<String> {
        let mut payloads = Vec::new();
        while let Some(payload) = decoder.pop() {
            payloads.push(payload);
        }
        payloads
    }

    #[test]
    fn decoder_emits_the_same_events_regardless_of_chunk_boundaries() {
        let wire = "data: {\"a\":1}\n\ndata: [DONE]\n\n";
        let mut whole = SseDecoder::default();
        whole.feed(wire.as_bytes());
        whole.finish();

        let mut drip = SseDecoder::default();
        for byte in wire.as_bytes() {
            drip.feed(std::slice::from_ref(byte));
        }
        drip.finish();

        let whole_out = drain(&mut whole);
        assert_eq!(drain(&mut drip), whole_out, "chunking must not matter");
        assert_eq!(
            whole_out,
            ["{\"a\":1}".to_string(), "[DONE]".to_string()],
            "payloads arrive verbatim, including the terminal sentinel"
        );
    }

    #[test]
    fn decoder_joins_multi_line_data_and_ignores_other_fields() {
        let wire = ": comment\nevent: message\nid: 7\ndata: first\ndata: second\n\n";
        let mut decoder = SseDecoder::default();
        decoder.feed(wire.as_bytes());
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("first\nsecond"));
        assert!(decoder.pop().is_none(), "one event, one payload");
    }

    #[test]
    fn decoder_accepts_crlf_line_endings() {
        let mut decoder = SseDecoder::default();
        decoder.feed(b"data: one\r\n\r\n");
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("one"));
    }

    #[test]
    fn decoder_flushes_an_unterminated_final_line_at_eof() {
        let mut decoder = SseDecoder::default();
        decoder.feed(b"data: tail");
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("tail"));
    }

    #[test]
    fn decoder_drops_events_without_data() {
        let mut decoder = SseDecoder::default();
        decoder.feed(b"event: ping\n\ndata: kept\n\n");
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("kept"));
        assert!(decoder.pop().is_none());
    }
}
