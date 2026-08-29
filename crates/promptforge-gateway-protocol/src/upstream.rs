//! The backend-facing side: the [`Upstream`] trait and its OpenAI passthrough.
//!
//! The trait is the seam where per-vendor translation will live. v0 ships one
//! implementation, [`OpenAiUpstream`], which forwards the OpenAI shape
//! unchanged. Adding an Anthropic or pack upstream later is a new implementation
//! behind this same trait, with no change to routing or the request handler.

use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use promptforge_gateway_config::Secret;

use crate::error::{ProtocolError, ShutdownError};
use crate::wire::{
    ChatChunk, ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, RerankRequest,
    RerankResponse,
};

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

/// A backend the gateway can forward a chat completion to.
#[async_trait]
pub trait Upstream: Send + Sync {
    /// Forward `req` to the backend, substituting `upstream_model` for the
    /// caller's model name, and return the response.
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

    /// Forward an embeddings `req` to the backend, substituting
    /// `upstream_model` for the caller's model name, and return the response.
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

    /// Forward a rerank `req` to the backend, substituting `upstream_model`
    /// for the caller's model name, and return the response.
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

    /// Open a streaming chat completion for `req`, substituting
    /// `upstream_model` for the caller's model name, and return the chunk
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

    /// Explicitly release any owned resources (for example a child process) and
    /// disable further recovery, surfacing any teardown failure.
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
}

impl OpenAiUpstream {
    /// Build an upstream for `base_url` (a trailing slash is trimmed).
    #[must_use]
    pub fn new(base_url: &str, api_key: Secret) -> OpenAiUpstream {
        OpenAiUpstream {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http: crate::http_util::bounded_client(),
            http_stream: crate::http_util::streaming_client(),
        }
    }

    /// Build an upstream with a caller-supplied HTTP client (test seam for
    /// exercising request deadlines against a stalled server).
    #[cfg(test)]
    pub(crate) fn with_client(
        base_url: &str,
        api_key: Secret,
        http: reqwest::Client,
    ) -> OpenAiUpstream {
        OpenAiUpstream {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http: http.clone(),
            http_stream: http,
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

/// Parse an upstream SSE byte stream into validated [`ChatChunk`]s.
///
/// Each `data:` line carries one JSON chunk; blank lines, comments, and the
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
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use serde_json::Map;

    use super::*;

    /// A one-shot mock backend: serves a single canned `(status, body)` and
    /// returns its base URL plus the captured raw request for assertions.
    fn serve_once(status_line: &str, body: &str) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock backend");
        let addr = listener.local_addr().expect("addr");
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || -> String {
            let (mut stream, _) = listener.accept().expect("accept");
            // A short read timeout bounds request capture without a sleep: once
            // the client has sent its request and is awaiting a response, the
            // next read simply times out and we reply.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut request = Vec::new();
            let mut buf = [0_u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{addr}"), handle)
    }

    fn request(model: &str) -> ChatRequest {
        ChatRequest {
            model: model.to_owned(),
            messages: vec![serde_json::json!({ "role": "user", "content": "hi" })],
            stream: false,
            rest: Map::new(),
        }
    }

    fn embedding_request(model: &str) -> EmbeddingRequest {
        EmbeddingRequest {
            model: model.to_owned(),
            input: crate::wire::EmbeddingInput::One("embed me".to_owned()),
            encoding_format: None,
            rest: Map::new(),
        }
    }

    #[tokio::test]
    async fn embeddings_rewrites_caller_model_and_posts_to_embeddings() {
        // UP-008: same contract as chat - the caller's model name is restored
        // on the response while the upstream model is what the backend sees.
        let (base, handle) = serve_once(
            "200 OK",
            r#"{"object":"list","model":"backend-embed","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2]}],"usage":{"prompt_tokens":2,"total_tokens":2}}"#,
        );
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let response = upstream
            .send_embeddings(embedding_request("caller-model"), "backend-embed")
            .await
            .expect("send ok");
        assert_eq!(response.model, "caller-model");
        assert_eq!(response.data.len(), 1);
        assert!(response.rest.contains_key("usage"));
        let sent = handle.join().expect("join");
        assert!(sent.contains("POST /embeddings"), "{sent}");
        assert!(sent.contains("backend-embed"), "forwarded body: {sent}");
        assert!(
            !sent.contains("caller-model"),
            "caller model leaked: {sent}"
        );
    }

    #[tokio::test]
    async fn embeddings_non_success_status_is_upstream_status() {
        let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream
            .send_embeddings(embedding_request("m"), "u")
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, ProtocolError::UpstreamStatus { status: 500, .. }),
            "expected UpstreamStatus 500, got {err:?}"
        );
        let _ = handle.join();
    }

    #[tokio::test]
    async fn default_send_embeddings_is_model_unavailable() {
        // Upstreams without an embeddings implementation (a local chat server)
        // decline the workload with ModelUnavailable naming the caller's model.
        struct ChatOnly;

        #[async_trait]
        impl Upstream for ChatOnly {
            async fn send(
                &self,
                _req: ChatRequest,
                _upstream_model: &str,
            ) -> Result<ChatResponse, ProtocolError> {
                unreachable!("not under test")
            }
        }

        let err = ChatOnly
            .send_embeddings(embedding_request("local-chat"), "ignored-alias")
            .await
            .expect_err("default must decline");
        match err {
            ProtocolError::ModelUnavailable(model) => assert_eq!(model, "local-chat"),
            other => panic!("expected ModelUnavailable, got {other:?}"),
        }
    }

    fn rerank_request(model: &str) -> RerankRequest {
        RerankRequest {
            model: model.to_owned(),
            query: "what is rust".to_owned(),
            documents: vec!["a systems language".to_owned()],
            top_n: None,
            rest: Map::new(),
        }
    }

    #[tokio::test]
    async fn rerank_rewrites_caller_model_and_posts_to_rerank() {
        // UP-008: same contract as chat - the caller's model name is restored
        // on the response while the upstream model is what the backend sees.
        let (base, handle) = serve_once(
            "200 OK",
            r#"{"model":"backend-rerank","results":[{"index":0,"relevance_score":0.9}],"usage":{"total_tokens":5}}"#,
        );
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let response = upstream
            .send_rerank(rerank_request("caller-model"), "backend-rerank")
            .await
            .expect("send ok");
        assert_eq!(response.model, "caller-model");
        assert_eq!(response.results.len(), 1);
        assert!(response.rest.contains_key("usage"));
        let sent = handle.join().expect("join");
        assert!(sent.contains("POST /rerank"), "{sent}");
        assert!(sent.contains("backend-rerank"), "forwarded body: {sent}");
        assert!(
            !sent.contains("caller-model"),
            "caller model leaked: {sent}"
        );
    }

    #[tokio::test]
    async fn rerank_non_success_status_is_upstream_status() {
        let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream
            .send_rerank(rerank_request("m"), "u")
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, ProtocolError::UpstreamStatus { status: 500, .. }),
            "expected UpstreamStatus 500, got {err:?}"
        );
        let _ = handle.join();
    }

    #[tokio::test]
    async fn default_send_rerank_is_model_unavailable() {
        // Upstreams without a rerank implementation (a local chat server, for
        // example) decline the workload with ModelUnavailable naming the
        // caller's model.
        struct ChatOnly;

        #[async_trait]
        impl Upstream for ChatOnly {
            async fn send(
                &self,
                _req: ChatRequest,
                _upstream_model: &str,
            ) -> Result<ChatResponse, ProtocolError> {
                unreachable!("not under test")
            }
        }

        let err = ChatOnly
            .send_rerank(rerank_request("local-classifier"), "ignored-alias")
            .await
            .expect_err("default must decline");
        match err {
            ProtocolError::ModelUnavailable(model) => assert_eq!(model, "local-classifier"),
            other => panic!("expected ModelUnavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn default_stream_is_model_unavailable_and_object_safe() {
        // Upstreams without a streaming implementation decline the workload
        // with ModelUnavailable naming the caller's model. The call goes
        // through `Arc<dyn Upstream>` to prove the boxed-stream signature
        // stays object-safe.
        struct ChatOnly;

        #[async_trait]
        impl Upstream for ChatOnly {
            async fn send(
                &self,
                _req: ChatRequest,
                _upstream_model: &str,
            ) -> Result<ChatResponse, ProtocolError> {
                unreachable!("not under test")
            }
        }

        let upstream: std::sync::Arc<dyn Upstream> = std::sync::Arc::new(ChatOnly);
        match upstream
            .stream(request("local-chat"), "ignored-alias")
            .await
        {
            Err(ProtocolError::ModelUnavailable(model)) => assert_eq!(model, "local-chat"),
            Err(other) => panic!("expected ModelUnavailable, got {other:?}"),
            Ok(_) => panic!("default must decline"),
        }
    }

    fn chunk_line(model: &str, content: &str) -> String {
        format!(
            "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\n"
        )
    }

    #[tokio::test]
    async fn stream_parses_chunks_rewrites_model_and_stops_at_done() {
        // UP-008: same contract as `send` - the caller's model name is restored
        // on every chunk while the backend sees the upstream model, and the
        // upstream's [DONE] sentinel ends the stream without being yielded.
        let body = format!(
            "{}{}{}data: [DONE]\n\n",
            chunk_line("backend-model", "Hel"),
            chunk_line("backend-model", "lo"),
            chunk_line("backend-model", "!"),
        );
        let (base, handle) = serve_once("200 OK", &body);
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let mut streamed = upstream
            .stream(request("caller-model"), "backend-model")
            .await
            .expect("stream opens");
        let mut chunks = Vec::new();
        while let Some(item) = streamed.chunks.next().await {
            chunks.push(item.expect("chunk ok"));
        }
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.model == "caller-model"));
        let text: String = chunks
            .iter()
            .filter_map(|chunk| chunk.choices[0].delta.get("content"))
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert_eq!(text, "Hello!");
        let sent = handle.join().expect("join");
        assert!(
            sent.contains("\"stream\":true"),
            "stream flag forwarded: {sent}"
        );
        assert!(sent.contains("backend-model"), "forwarded body: {sent}");
        assert!(
            !sent.contains("caller-model"),
            "caller model leaked: {sent}"
        );
    }

    #[tokio::test]
    async fn stream_non_success_status_is_upstream_status_before_any_chunk() {
        // A non-2xx is consumed as a normal error before the stream starts;
        // the caller never sees a chunk stream that dies mid-flight.
        let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream
            .stream(request("m"), "u")
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, ProtocolError::UpstreamStatus { status: 500, .. }),
            "expected UpstreamStatus 500, got {err:?}"
        );
        let _ = handle.join();
    }

    /// A shared buffer that captures what the parser logs, so tests can
    /// assert on malformed-chunk warnings.
    #[derive(Clone, Default)]
    struct LogBuffer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl LogBuffer {
        fn contents(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
        }
    }

    impl std::io::Write for LogBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("log buffer").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
        type Writer = LogBuffer;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Install a WARN-level subscriber writing to a fresh capture buffer for
    /// the current thread (tokio's current-thread test runtime keeps every
    /// poll on this thread, so the parser's warnings land in the buffer).
    fn capture_warnings() -> (LogBuffer, tracing::subscriber::DefaultGuard) {
        let buffer = LogBuffer::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(buffer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        let guard = tracing::subscriber::set_default(subscriber);
        (buffer, guard)
    }

    #[tokio::test]
    async fn stream_malformed_chunks_are_logged_and_skipped() {
        // An undecodable or shape-invalid chunk is logged and skipped; the
        // stream continues with the next good chunk instead of ending.
        let (logs, _guard) = capture_warnings();
        let body = format!(
            "{}data: not json\n\ndata: {{\"model\":\"m\",\"choices\":[]}}\n\n{}data: [DONE]\n\n",
            chunk_line("m", "a"),
            chunk_line("m", "b"),
        );
        let (base, handle) = serve_once("200 OK", &body);
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let mut streamed = upstream
            .stream(request("m"), "u")
            .await
            .expect("stream opens");
        let mut chunks = Vec::new();
        while let Some(item) = streamed.chunks.next().await {
            chunks.push(item.expect("malformed chunks never surface as items"));
        }
        let text: String = chunks
            .iter()
            .filter_map(|chunk| chunk.choices[0].delta.get("content"))
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert_eq!(text, "ab", "both good chunks survive the malformed ones");
        let logs = logs.contents();
        assert_eq!(
            logs.matches("skipping").count(),
            2,
            "each malformed chunk is logged once: {logs}"
        );
        let _ = handle.join();
    }

    #[tokio::test]
    async fn stream_done_sentinel_is_never_logged_as_malformed() {
        // [DONE] is not JSON; it is recognized before parsing, so a healthy
        // stream ends without a spurious malformed-chunk warning.
        let (logs, _guard) = capture_warnings();
        let body = format!("{}data: [DONE]\n\n", chunk_line("m", "a"));
        let (base, handle) = serve_once("200 OK", &body);
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let mut streamed = upstream
            .stream(request("m"), "u")
            .await
            .expect("stream opens");
        let mut count = 0;
        while let Some(item) = streamed.chunks.next().await {
            item.expect("chunk ok");
            count += 1;
        }
        assert_eq!(count, 1);
        assert!(
            logs.contents().is_empty(),
            "a healthy stream logs no warnings: {}",
            logs.contents()
        );
        let _ = handle.join();
    }

    /// A mock streaming backend: answers with one chunk and no
    /// `Content-Length` (so the body stays open until close), then reports
    /// whether the client hung up.
    fn serve_one_chunk_then_watch() -> (String, JoinHandle<bool>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock backend");
        let addr = listener.local_addr().expect("addr");
        let body = chunk_line("backend-model", "po");
        let handle = thread::spawn(move || -> bool {
            let Ok((mut stream, _)) = listener.accept() else {
                return false;
            };
            let mut buf = [0_u8; 4096];
            // Consume the request head; the read timeout ends the wait once
            // the client is awaiting a response (same pattern as serve_once).
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
            if stream
                .write_all(head.as_bytes())
                .and_then(|()| stream.write_all(body.as_bytes()))
                .and_then(|()| stream.flush())
                .is_err()
            {
                return false;
            }
            // Watch for the client hanging up: a clean EOF or a reset both
            // mean the connection is gone; a timeout means it is still open.
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => return true,
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        return false;
                    }
                    Err(_) => return true,
                }
            }
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn dropping_the_stream_aborts_the_upstream_connection() {
        // Client-disconnect cancellation is Drop all the way down: dropping
        // the chunk stream drops the upstream response, which aborts the
        // upstream connection. The watch is joined off the runtime thread so
        // the client connection task can run the close.
        let (base, handle) = serve_one_chunk_then_watch();
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let mut streamed = upstream
            .stream(request("m"), "u")
            .await
            .expect("stream opens");
        let first = streamed.chunks.next().await.expect("first item");
        assert!(first.is_ok(), "first chunk parses: {first:?}");
        drop(streamed);
        let closed = tokio::task::spawn_blocking(move || handle.join().expect("join"))
            .await
            .expect("watch task");
        assert!(
            closed,
            "dropping the stream must abort the upstream connection"
        );
    }

    #[test]
    fn new_trims_a_trailing_slash_from_base_url() {
        // UP-008: the base URL is normalized so the joined path is well-formed.
        let upstream = OpenAiUpstream::new("http://host:1234/v1/", Secret::new(String::new()));
        assert_eq!(upstream.base_url, "http://host:1234/v1");
    }

    #[tokio::test]
    async fn rewrites_caller_model_and_forwards_upstream_model() {
        // UP-008: the caller's model name is restored on the response, while the
        // upstream (backend) model is what is actually sent to the backend.
        let (base, handle) = serve_once(
            "200 OK",
            r#"{"model":"backend-model","choices":[{"index":0,"message":{"role":"assistant","content":"ok"}}]}"#,
        );
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let response = upstream
            .send(request("caller-model"), "backend-model")
            .await
            .expect("send ok");
        assert_eq!(response.model, "caller-model");
        let sent = handle.join().expect("join");
        assert!(sent.contains("POST /chat/completions"), "{sent}");
        assert!(sent.contains("backend-model"), "forwarded body: {sent}");
        assert!(
            !sent.contains("caller-model"),
            "caller model leaked: {sent}"
        );
    }

    #[tokio::test]
    async fn non_success_status_is_upstream_status_with_capped_body() {
        // UP-008: a backend error status surfaces as UpstreamStatus.
        let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream
            .send(request("m"), "u")
            .await
            .expect_err("should fail");
        match err {
            ProtocolError::UpstreamStatus { status, body } => {
                assert_eq!(status, 500);
                assert_eq!(body, "backend exploded");
            }
            other => panic!("expected UpstreamStatus, got {other:?}"),
        }
        let _ = handle.join();
    }

    /// A server that accepts the connection and then never sends a response, so
    /// the client's request deadline (not an idle read) is what must fire.
    fn serve_stalled() -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalled backend");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            // A read timeout bounds the server thread: the client's request
            // deadline fires first; this only stops the thread from blocking
            // forever if the client keeps the socket open in its pool.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
            let mut buf = [0_u8; 1024];
            let _ = stream.read(&mut buf); // consume request head
            // Never write a response; the client must fail on its own deadline.
            let _ = stream.read(&mut buf);
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn connect_refused_is_upstream_connect_not_transport() {
        // A refused connection means the request never left the gateway:
        // nothing was billed upstream and a retry is safe, so the error must
        // classify as `upstream_connect`, distinct from a mid-flight death.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let upstream = OpenAiUpstream::new(&format!("http://{addr}"), Secret::new(String::new()));
        let err = upstream
            .send(request("m"), "u")
            .await
            .expect_err("connect refused must fail");
        assert!(
            matches!(err, ProtocolError::UpstreamConnect(_)),
            "expected UpstreamConnect, got {err:?}"
        );
        assert_eq!(err.envelope()["error"]["code"], "upstream_connect");
    }

    #[tokio::test]
    async fn send_times_out_on_a_stalled_server() {
        // UP-008: a backend that accepts and then stalls must fail on the
        // request deadline as a transport error, never hang the caller.
        // A timeout is NEVER connect: the request may have reached the
        // provider, so it stays `upstream_transport`.
        let (base, handle) = serve_stalled();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(300))
            .build()
            .expect("client");
        let upstream = OpenAiUpstream::with_client(&base, Secret::new(String::new()), client);
        let err = upstream
            .send(request("m"), "u")
            .await
            .expect_err("stalled server must time out");
        assert!(
            matches!(err, ProtocolError::UpstreamTransport(_)),
            "expected UpstreamTransport, got {err:?}"
        );
        assert_eq!(err.envelope()["error"]["code"], "upstream_transport");
        let _ = handle.join();
    }

    #[tokio::test]
    async fn error_body_is_capped_at_the_boundary() {
        // UP-008: an over-limit error body is bounded (the handler additionally
        // caps to 2000 chars); an exact-size small body is preserved whole.
        let exact = "x".repeat(64);
        let (base, handle) = serve_once("503 Service Unavailable", &exact);
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream.send(request("m"), "u").await.expect_err("error");
        match err {
            ProtocolError::UpstreamStatus { status, body } => {
                assert_eq!(status, 503);
                assert_eq!(body, exact);
            }
            other => panic!("expected UpstreamStatus, got {other:?}"),
        }
        let _ = handle.join();

        // An over-2000-char error body is truncated by the handler's char cap.
        let huge = "y".repeat(5000);
        let (base, handle) = serve_once("500 Internal Server Error", &huge);
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream.send(request("m"), "u").await.expect_err("error");
        match err {
            ProtocolError::UpstreamStatus { body, .. } => {
                assert_eq!(body.chars().count(), 2000, "error body char-capped");
            }
            other => panic!("expected UpstreamStatus, got {other:?}"),
        }
        let _ = handle.join();
    }

    #[tokio::test]
    async fn malformed_success_body_is_a_protocol_error_not_transport() {
        // UP-008: a 200 with a non-JSON body is a protocol/decode failure, not a
        // transport death (so it never triggers a spurious recovery).
        let (base, handle) = serve_once("200 OK", "definitely not json");
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream
            .send(request("m"), "u")
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, ProtocolError::UpstreamProtocol(_)),
            "expected UpstreamProtocol, got {err:?}"
        );
        let _ = handle.join();
    }
}
