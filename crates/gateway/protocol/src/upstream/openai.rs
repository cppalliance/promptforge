//! The `OpenAiUpstream` passthrough: its HTTP calls, error diagnostics, and `Upstream` impl.

use std::fmt::Write as _;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use gateway_config::Secret;

use super::{OpenAiUpstream, StreamedAudio, StreamedChunks, Upstream, sse_chunks};
use crate::error::ProtocolError;
use crate::wire::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, RerankRequest, RerankResponse,
    SpeechRequest,
};

impl OpenAiUpstream {
    /// Builds an upstream for `base_url` (a trailing slash is trimmed).
    #[must_use]
    pub fn new(base_url: &str, api_key: Secret) -> OpenAiUpstream {
        OpenAiUpstream {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http: crate::http_util::bounded_client(),
            http_stream: crate::http_util::streaming_client(),
            http_audio: crate::http_util::audio_streaming_client(),
        }
    }

    /// Builds an upstream with a caller-supplied HTTP client (test seam for
    /// exercising request deadlines against a stalled server).
    #[cfg(test)]
    pub(super) fn with_client(
        base_url: &str,
        api_key: Secret,
        http: reqwest::Client,
    ) -> OpenAiUpstream {
        OpenAiUpstream {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http: http.clone(),
            http_stream: http.clone(),
            http_audio: http,
        }
    }

    /// POST `body` to `{base_url}/{path}` with the endpoint credential and
    /// return the success response.
    ///
    /// A non-success status fails before the body is consumed as anything
    /// but diagnostics, so a streaming caller never sees an error response
    /// as the start of a chunk stream.
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure, and [`ProtocolError::UpstreamStatus`] with a truncated body on
    /// a non-success backend status.
    async fn post(
        &self,
        client: &reqwest::Client,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<reqwest::Response, ProtocolError> {
        let mut builder = client.post(format!("{}/{path}", self.base_url)).json(body);
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(self.api_key.expose());
        }

        let response = builder
            .send()
            .await
            .map_err(ProtocolError::upstream_transport)?;

        let status = response.status();
        if !status.is_success() {
            let body =
                crate::http_util::read_body_capped(response, crate::http_util::MAX_ERROR_BODY)
                    .await;
            // F5: the raw body may echo prompt content or credentials, so only
            // its OpenAI error envelope reaches the log, bounded and escaped.
            let diagnostics = UpstreamErrorDiagnostics::from_body(&body);
            let code = diagnostics.code.as_str();
            let kind = diagnostics.kind.as_str();
            let message = diagnostics.message.as_str();
            if status.is_server_error() {
                tracing::warn!(
                    status = status.as_u16(),
                    code = %code,
                    r#type = %kind,
                    error.message = %message,
                    "upstream returned a server error"
                );
            } else {
                tracing::info!(
                    status = status.as_u16(),
                    code = %code,
                    r#type = %kind,
                    error.message = %message,
                    "upstream returned a client error"
                );
            }
            let body: String = body.chars().take(2000).collect();
            return Err(ProtocolError::UpstreamStatus {
                status: status.as_u16(),
                body,
            });
        }
        Ok(response)
    }

    /// POST `body` to `{base_url}/{path}` and return the success body bytes.
    ///
    /// The body read is byte-bounded: a chunk read failure is a transport
    /// error, while decoding the returned bytes is left to the caller so a
    /// decode failure surfaces as a protocol error (never a transport death)
    /// and cannot trigger a spurious recovery upstream (UP-003, UP-004).
    ///
    /// # Errors
    /// Returns [`ProtocolError::UpstreamConnect`] when the connection itself
    /// fails, [`ProtocolError::UpstreamTransport`] on a mid-flight transport
    /// failure, and [`ProtocolError::UpstreamStatus`] with a truncated body on
    /// a non-success backend status.
    async fn post_json(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<Vec<u8>, ProtocolError> {
        let response = self.post(&self.http, path, body).await?;
        crate::http_util::read_bytes_capped(response, crate::http_util::MAX_JSON_BODY)
            .await
            .map_err(ProtocolError::upstream_transport)
    }
}

/// Maximum characters retained from an upstream error field for diagnostics.
/// Applied per field on the decoded string, before control escaping can expand
/// it, so one event cannot flood the log with an oversized upstream message.
pub(super) const MAX_ERROR_MESSAGE_CHARS: usize = 512;

/// The bounded, safe diagnostics extracted from a non-success upstream body.
///
/// The raw body is never retained or logged: only the OpenAI error envelope's
/// `code`, `type`, and `message` are read, and each string is control-escaped
/// and bounded. A body outside that shape (or a non-string field) yields an
/// empty value rather than a fallback that could leak the body.
#[derive(Debug, Default)]
struct UpstreamErrorDiagnostics {
    code: String,
    kind: String,
    message: String,
}

impl UpstreamErrorDiagnostics {
    fn from_body(body: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return Self::default();
        };
        match value.get("error") {
            Some(serde_json::Value::Object(error)) => Self {
                code: bounded_error_field(error.get("code")),
                kind: bounded_error_field(error.get("type")),
                message: bounded_error_field(error.get("message")),
            },
            Some(serde_json::Value::String(message)) => Self {
                message: escape_control(message, MAX_ERROR_MESSAGE_CHARS),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }
}

/// Renders a string-valued error field, control-escaped and bounded; a
/// missing or non-string field renders as the empty string.
fn bounded_error_field(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(serde_json::Value::as_str)
        .map_or_else(String::new, |text| {
            escape_control(text, MAX_ERROR_MESSAGE_CHARS)
        })
}

/// Escapes every control character so a crafted upstream message cannot forge
/// log lines or inject terminal control sequences, keeping at most `max_chars`
/// input characters.
fn escape_control(text: &str, max_chars: usize) -> String {
    let mut escaped = String::new();
    for character in text.chars().take(max_chars) {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(escaped, "\\u{{{:x}}}", control as u32);
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// Deadline for the upstream's first response bytes (headers) on the speech
/// path: a maximum-length batch generation can legitimately take longer to
/// first byte than the per-read body idle budget, so time-to-headers has
/// its own, larger budget. reqwest's `read_timeout` cannot express the split
/// because it also governs the header wait, so the deadline is applied here
/// rather than on the client.
#[cfg(not(test))]
const FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
/// Test-scaled so the deadline-separation tests run in milliseconds: the
/// stalled-headers arm waits on this budget, so it cannot stay at 120 s.
#[cfg(test)]
const FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_millis(1000);

/// Per-read idle deadline on an opened audio body: a stalled upstream is
/// detected within this window without capping the stream's total length.
#[cfg(not(test))]
const AUDIO_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Test-scaled so the deadline-separation tests run in milliseconds.
#[cfg(test)]
const AUDIO_READ_TIMEOUT: Duration = Duration::from_millis(200);

#[async_trait]
impl Upstream for OpenAiUpstream {
    async fn send(
        &self,
        mut req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, ProtocolError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let bytes = self.post_json("chat/completions", &req).await?;
        let mut parsed: ChatResponse =
            serde_json::from_slice(&bytes).map_err(ProtocolError::upstream_protocol)?;
        // Return the caller's model name, never the backend's.
        parsed.model = requested;
        Ok(parsed)
    }

    async fn send_embeddings(
        &self,
        mut req: EmbeddingRequest,
        upstream_model: &str,
    ) -> Result<EmbeddingResponse, ProtocolError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let bytes = self.post_json("embeddings", &req).await?;
        let mut parsed: EmbeddingResponse =
            serde_json::from_slice(&bytes).map_err(ProtocolError::upstream_protocol)?;
        // Return the caller's model name, never the backend's.
        parsed.model = requested;
        Ok(parsed)
    }

    async fn send_rerank(
        &self,
        mut req: RerankRequest,
        upstream_model: &str,
    ) -> Result<RerankResponse, ProtocolError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let bytes = self.post_json("rerank", &req).await?;
        let mut parsed: RerankResponse =
            serde_json::from_slice(&bytes).map_err(ProtocolError::upstream_protocol)?;
        // Return the caller's model name, never the backend's.
        parsed.model = requested;
        Ok(parsed)
    }

    async fn stream(
        &self,
        mut req: ChatRequest,
        upstream_model: &str,
    ) -> Result<StreamedChunks, ProtocolError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        req.stream = true;
        let response = self
            .post(&self.http_stream, "chat/completions", &req)
            .await?;
        Ok(sse_chunks(response, requested))
    }

    async fn send_speech(
        &self,
        mut req: SpeechRequest,
        upstream_model: &str,
    ) -> Result<StreamedAudio, ProtocolError> {
        req.model = upstream_model.to_string();
        // Time-to-headers and per-read body idle are separate budgets: the
        // send await is bounded by FIRST_RESPONSE_TIMEOUT, each body read by
        // AUDIO_READ_TIMEOUT.
        let response = tokio::time::timeout(
            FIRST_RESPONSE_TIMEOUT,
            self.post(&self.http_audio, "audio/speech", &req),
        )
        .await
        .map_err(ProtocolError::transport)??;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .unwrap_or_default();
        let body = futures_util::stream::unfold(
            (response.bytes_stream().boxed(), false),
            |(mut bytes, terminated)| async move {
                if terminated {
                    return None;
                }
                match tokio::time::timeout(AUDIO_READ_TIMEOUT, bytes.next()).await {
                    Ok(Some(Ok(chunk))) => Some((Ok(chunk), (bytes, false))),
                    Ok(None) => None,
                    // A mid-stream transport failure or a read idle timeout
                    // surfaces as one Err item and ends the stream, so a
                    // caller never mistakes a truncated stream for a
                    // complete one.
                    Ok(Some(Err(error))) => {
                        Some((Err(ProtocolError::upstream_transport(error)), (bytes, true)))
                    }
                    Err(elapsed) => Some((Err(ProtocolError::transport(elapsed)), (bytes, true))),
                }
            },
        )
        .boxed();
        Ok(StreamedAudio { content_type, body })
    }
}
