//! The backend-facing side: the [`Upstream`] trait and its OpenAI passthrough.
//!
//! This crate implements the trait once, as [`OpenAiUpstream`], which forwards
//! the OpenAI shape unchanged.

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use gateway_config::Secret;

use crate::error::{ProtocolError, ShutdownError};
use crate::wire::{
    ChatChunk, ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, RerankRequest,
    RerankResponse, SpeechRequest,
};

mod openai;

/// An opened streaming chat completion: the upstream response headers worth
/// forwarding to the client, plus the validated chunk stream.
pub struct StreamedChunks {
    /// The upstream `Content-Type`, forwarded when present; the relay
    /// defaults to `text/event-stream` otherwise.
    pub content_type: Option<String>,
    /// The upstream `Cache-Control`, forwarded when present.
    pub cache_control: Option<String>,
    /// The validated chunk stream. The upstream's terminal `[DONE]` sentinel
    /// is consumed here, never yielded; the relay emits its own. Dropping the
    /// stream drops the upstream response and aborts the connection.
    pub chunks: BoxStream<'static, Result<ChatChunk, ProtocolError>>,
}

impl std::fmt::Debug for StreamedChunks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamedChunks")
            .field("content_type", &self.content_type)
            .field("cache_control", &self.cache_control)
            .finish_non_exhaustive()
    }
}

/// An opened streaming speech synthesis: the upstream `Content-Type` plus the
/// raw audio byte stream.
pub struct StreamedAudio {
    /// The upstream `Content-Type`, forwarded verbatim; empty when the
    /// upstream omitted the header, which is the route's signal to apply its
    /// format-to-MIME fallback.
    pub content_type: String,
    /// The untransformed audio byte stream: audio frames are opaque bytes the
    /// gateway forwards unread. A mid-stream read failure surfaces as an
    /// `Err` item rather than a silently truncated stream. Dropping the
    /// stream drops the upstream response and aborts the connection.
    pub body: BoxStream<'static, Result<Bytes, ProtocolError>>,
}

impl std::fmt::Debug for StreamedAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamedAudio")
            .field("content_type", &self.content_type)
            .finish_non_exhaustive()
    }
}

/// A backend the gateway can forward a chat completion to.
#[async_trait]
pub trait Upstream: Send + Sync {
    /// Forwards `req` to the backend, substituting `upstream_model` for the
    /// caller's model name, and returns the response.
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure, and [`ProtocolError::UpstreamStatus`] on a non-success backend
    /// status.
    async fn send(
        &self,
        req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, ProtocolError>;

    /// Forwards an embeddings `req` to the backend, substituting
    /// `upstream_model` for the caller's model name, and returns the response.
    ///
    /// The default is [`ProtocolError::ModelUnavailable`]: upstreams without an
    /// embeddings implementation (a local chat server, for example) decline
    /// the workload rather than fabricate a response.
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure, [`ProtocolError::UpstreamStatus`] on a non-success backend
    /// status, and [`ProtocolError::ModelUnavailable`] when the upstream
    /// cannot serve embeddings at all.
    async fn send_embeddings(
        &self,
        req: EmbeddingRequest,
        _upstream_model: &str,
    ) -> Result<EmbeddingResponse, ProtocolError> {
        Err(ProtocolError::ModelUnavailable(req.model))
    }

    /// Forwards a rerank `req` to the backend, substituting `upstream_model`
    /// for the caller's model name, and returns the response.
    ///
    /// The default is [`ProtocolError::ModelUnavailable`]: upstreams without a
    /// rerank implementation (a local chat server, for example) decline the
    /// workload rather than fabricate a response.
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure, [`ProtocolError::UpstreamStatus`] on a non-success backend
    /// status, and [`ProtocolError::ModelUnavailable`] when the upstream
    /// cannot serve rerank at all.
    async fn send_rerank(
        &self,
        req: RerankRequest,
        _upstream_model: &str,
    ) -> Result<RerankResponse, ProtocolError> {
        Err(ProtocolError::ModelUnavailable(req.model))
    }

    /// Opens a streaming chat completion for `req`, substituting
    /// `upstream_model` for the caller's model name, and returns the chunk
    /// stream.
    ///
    /// The stream is boxed because the trait is used as `Arc<dyn Upstream>`:
    /// an `impl Stream` return would break object safety. Each item is a
    /// validated [`ChatChunk`]; a malformed chunk is logged and skipped,
    /// while a mid-stream transport failure surfaces as an `Err` item rather
    /// than a silently truncated stream. Dropping the stream aborts the
    /// upstream connection, which is how a client disconnect cancels the
    /// upstream work.
    ///
    /// The default is [`ProtocolError::ModelUnavailable`]: upstreams without a
    /// streaming implementation decline the workload rather than fabricate a
    /// response.
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure before the stream starts, [`ProtocolError::UpstreamStatus`] on
    /// a non-success backend status, and [`ProtocolError::ModelUnavailable`]
    /// when the upstream cannot stream at all.
    async fn stream(
        &self,
        req: ChatRequest,
        _upstream_model: &str,
    ) -> Result<StreamedChunks, ProtocolError> {
        Err(ProtocolError::ModelUnavailable(req.model))
    }

    /// Forwards a speech synthesis `req` to the backend, substituting
    /// `upstream_model` for the caller's model name, and returns the audio
    /// stream.
    ///
    /// The default is [`ProtocolError::ModelUnavailable`]: upstreams without a
    /// speech implementation (a local chat server, for example) decline the
    /// workload rather than fabricate a response.
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure, [`ProtocolError::UpstreamStatus`] on a non-success backend
    /// status, and [`ProtocolError::ModelUnavailable`] when the upstream
    /// cannot serve speech at all.
    async fn send_speech(
        &self,
        req: SpeechRequest,
        _upstream_model: &str,
    ) -> Result<StreamedAudio, ProtocolError> {
        Err(ProtocolError::ModelUnavailable(req.model))
    }

    /// Explicitly releases any owned resources (for example a child process) and
    /// disables further recovery, surfacing any teardown failure.
    ///
    /// The default is a no-op for stateless upstreams. The supervised local
    /// upstream cancels any in-flight recovery, kills its `llama-server` child,
    /// and disables respawn, so an explicit teardown deterministically frees the
    /// resource even while the routing table still holds an `Arc<dyn Upstream>`
    /// clone - dropping the runtime alone cannot guarantee this because it is not
    /// the sole owner (PFGL-MOD-001, PF-GW-SERVER-004).
    ///
    /// # Errors
    /// Returns a [`ShutdownError`] when a child kill/reap or capture-reader
    /// teardown fails, so a caller can refuse to proceed rather than start
    /// replacements while an old child may survive.
    fn shutdown(&self) -> Result<(), ShutdownError> {
        Ok(())
    }
}

/// An OpenAI-compatible backend reached over HTTP.
#[derive(Debug)]
pub struct OpenAiUpstream {
    base_url: String,
    api_key: Secret,
    http: reqwest::Client,
    /// Connect-timeout-only client for the streaming path: reqwest's
    /// whole-request timeout covers the body read and would kill any
    /// long-lived SSE stream, so streams never use `http`.
    http_stream: reqwest::Client,
    /// Client for the speech path: like `http_stream` it has no
    /// whole-request timeout, and it adds TCP keepalive so a silently dead
    /// peer surfaces. It has no `read_timeout` either: reqwest arms that
    /// during the wait for response headers, so the speech path applies its
    /// own split deadlines in `send_speech` instead.
    http_audio: reqwest::Client,
}

/// Parses an upstream SSE byte stream into validated [`ChatChunk`]s.
///
/// Each `data:` line holds one JSON chunk; blank lines, comments, and the
/// `event:`/`id:`/`retry:` fields are skipped, and the terminal `[DONE]`
/// sentinel - which is not JSON - ends the stream without being yielded and
/// without ever reaching the malformed-chunk log. Every chunk's model is
/// rewritten to `requested` (the caller's model name, never the backend's).
/// A chunk that is undecodable or fails the minimal shape check is logged
/// and skipped, so one bad chunk never ends an otherwise healthy stream. A
/// transport failure mid-stream surfaces as an `Err` item and ends the
/// stream, so a caller never mistakes a truncated stream for a complete one.
/// Dropping the returned stream drops the upstream response, which aborts
/// the upstream connection: that Drop chain is the entire client-disconnect
/// cancellation mechanism.
pub fn sse_chunks(response: reqwest::Response, requested: String) -> StreamedChunks {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cache_control = response
        .headers()
        .get(reqwest::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let chunks = futures_util::stream::unfold(
        (
            response.bytes_stream().boxed(),
            Vec::new(),
            requested,
            false,
        ),
        |(mut bytes, mut buffer, requested, terminated)| async move {
            if terminated {
                return None;
            }
            loop {
                if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=end).collect();
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim_end_matches(['\r', '\n']);
                    if line.is_empty() || line.starts_with(':') {
                        continue;
                    }
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim_start();
                    if data == "[DONE]" {
                        return None;
                    }
                    let mut chunk = match serde_json::from_str::<ChatChunk>(data) {
                        Ok(chunk) => chunk,
                        Err(error) => {
                            tracing::warn!(%error, "skipping undecodable upstream chunk");
                            continue;
                        }
                    };
                    if let Err(reason) = chunk.validate() {
                        tracing::warn!(%reason, "skipping malformed upstream chunk");
                        continue;
                    }
                    chunk.model.clone_from(&requested);
                    return Some((Ok(chunk), (bytes, buffer, requested, false)));
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                    Some(Err(error)) => {
                        return Some((
                            Err(ProtocolError::upstream_transport(error)),
                            (bytes, buffer, requested, true),
                        ));
                    }
                    None => return None,
                }
            }
        },
    )
    .boxed();
    StreamedChunks {
        content_type,
        cache_control,
        chunks,
    }
}

#[cfg(test)]
mod tests;
