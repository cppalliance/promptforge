//! The suites' chat client: the one place the engine's own tests do HTTP.
//!
//! The engine never performs a round; its production host, the harness,
//! owns the gateway client, and this crate may not name a harness crate.
//! The suites still drive real rounds against their axum mock gateways,
//! so this client speaks the same protocol over a dev-only `reqwest`:
//! the wire vocabulary's request body goes out, the response is handed to
//! the shared read loop as a chunk source, and the result is the same
//! [`Completion`] a production round yields. The run's request timeout is
//! applied here; the response cap on both the error body and the stream,
//! the `[DONE]` rule, and the timing arithmetic are the shared loop's, so
//! this client and the harness's differ only in how they send.
//!
//! This file names only external crates so the bench target can include
//! it by `#[path]` beside the in-crate suites; it is not part of the
//! `test-support` feature, which has no HTTP.

use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use promptforge_model_client::Error as ClientError;
use promptforge_model_client::client::{
    ChunkSource, Completion, Message, ToolSchema, build_request_body, escape_controls,
    read_body_capped, read_completion_stream,
};
use promptforge_model_client::detail::error_http;
use promptforge_model_client::model::{CompletionError, CompletionOptions};
use promptforge_types::wire::StreamDelta;

/// A chat client bound to one mock gateway's `/v1` root.
#[derive(Clone, Debug)]
pub(crate) struct MockGatewayClient {
    base_url: String,
    key: String,
    http: reqwest::Client,
}

impl MockGatewayClient {
    /// A client for the mock gateway at `addr` presenting `key` as its
    /// bearer; the suites that check the key never leaks pass a
    /// recognizable one.
    #[must_use]
    pub(crate) fn new(addr: SocketAddr, key: &str) -> MockGatewayClient {
        MockGatewayClient {
            base_url: format!("http://{addr}/v1"),
            key: key.to_owned(),
            http: reqwest::Client::new(),
        }
    }

    /// Performs one streamed round: posts the request body, reads the SSE
    /// stream under `max_bytes`, forwards each live delta to `on_delta`,
    /// and finishes the accumulation into the completion.
    ///
    /// # Errors
    /// Returns the [`CompletionError`] the round failed with: `Transport`
    /// for a send or read failure (a request past `timeout` included),
    /// `Backend` for a non-success status with the bounded, escaped body,
    /// `MalformedResponse` for an oversize or truncated stream or a
    /// malformed chunk, and the reassembly's own errors otherwise.
    pub(crate) async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolSchema],
        options: &CompletionOptions,
        timeout: Duration,
        max_bytes: NonZeroU64,
        on_delta: impl Fn(StreamDelta),
    ) -> Result<Completion, CompletionError> {
        let tool_arg = (!tools.is_empty()).then_some(tools);
        let request_body = build_request_body(messages, tool_arg, options);
        let started = Instant::now();
        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .timeout(timeout)
            .bearer_auth(&self.key)
            .json(&request_body)
            .send()
            .await
            .map_err(http)?;
        let status = response.status();
        let content_length = response.content_length();
        let mut chunks = ResponseChunks(response);
        if !status.is_success() {
            let raw = read_body_capped(&mut chunks, content_length, max_bytes.get()).await?;
            let body = escape_controls(&String::from_utf8_lossy(&raw), 2000);
            return Err(CompletionError::from(ClientError::Backend {
                status: status.as_u16(),
                body,
            }));
        }
        read_completion_stream(
            &mut chunks,
            request_body,
            max_bytes.get(),
            on_delta,
            started,
            Instant::now,
        )
        .await
    }
}

/// A [`reqwest::Response`] body as the shared read loop's chunk source.
struct ResponseChunks(reqwest::Response);

impl ChunkSource for ResponseChunks {
    type Chunk = bytes::Bytes;

    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, CompletionError> {
        self.0.chunk().await.map_err(http)
    }
}

/// Wraps a transport failure, marking a timeout so `is_timeout` holds.
fn http(error: reqwest::Error) -> CompletionError {
    if error.is_timeout() {
        return CompletionError::from(error_http(promptforge_model_client::Timeout(Box::new(
            error,
        ))));
    }
    CompletionError::from(error_http(error))
}
