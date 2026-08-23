//! The backend-facing side: the [`Upstream`] trait and its OpenAI passthrough.
//!
//! The trait is the seam where per-vendor translation will live. v0 ships one
//! implementation, [`OpenAiUpstream`], which forwards the OpenAI shape
//! unchanged. Adding an Anthropic or pack upstream later is a new implementation
//! behind this same trait, with no change to routing or the request handler.

use async_trait::async_trait;
use promptforge_gateway_config::Secret;

use crate::error::GatewayError;
use crate::wire::{ChatRequest, ChatResponse};

/// A backend the gateway can forward a chat completion to.
#[async_trait]
pub(crate) trait Upstream: Send + Sync {
    /// Forward `req` to the backend, substituting `upstream_model` for the
    /// caller's model name, and return the response.
    ///
    /// # Errors
    /// Returns [`GatewayError::UpstreamTransport`] on a transport failure and
    /// [`GatewayError::UpstreamStatus`] on a non-success backend status.
    async fn send(
        &self,
        req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, GatewayError>;

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
    /// Returns a [`LocalError`](crate::local::LocalError) when a child kill/reap
    /// or capture-reader teardown fails, so a caller can refuse to proceed
    /// rather than start replacements while an old child may survive.
    fn shutdown(&self) -> Result<(), crate::local::LocalError> {
        Ok(())
    }
}

/// An OpenAI-compatible backend reached over HTTP.
#[derive(Debug)]
pub(crate) struct OpenAiUpstream {
    base_url: String,
    api_key: Secret,
    http: reqwest::Client,
}

impl OpenAiUpstream {
    /// Build an upstream for `base_url` (a trailing slash is trimmed).
    #[must_use]
    pub(crate) fn new(base_url: &str, api_key: Secret) -> OpenAiUpstream {
        OpenAiUpstream {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http: crate::http_util::bounded_client(),
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
            http,
        }
    }
}

#[async_trait]
impl Upstream for OpenAiUpstream {
    async fn send(
        &self,
        mut req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, GatewayError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());

        let mut builder = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&req);
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(self.api_key.expose());
        }

        let response = builder
            .send()
            .await
            .map_err(GatewayError::upstream_transport)?;

        let status = response.status();
        if !status.is_success() {
            let body =
                crate::http_util::read_body_capped(response, crate::http_util::MAX_ERROR_BODY)
                    .await;
            let body: String = body.chars().take(2000).collect();
            return Err(GatewayError::UpstreamStatus {
                status: status.as_u16(),
                body,
            });
        }

        // Read a byte-bounded body, then decode. A chunk read failure is a
        // transport error; a decode failure is a protocol error (never a
        // transport death) so it cannot trigger a spurious recovery upstream
        // (UP-003, UP-004).
        let bytes = crate::http_util::read_bytes_capped(response, crate::http_util::MAX_JSON_BODY)
            .await
            .map_err(GatewayError::upstream_transport)?;
        let mut parsed: ChatResponse =
            serde_json::from_slice(&bytes).map_err(GatewayError::upstream_protocol)?;
        // Return the caller's model name, never the backend's.
        parsed.model = requested;
        Ok(parsed)
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
            rest: Map::new(),
        }
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
            GatewayError::UpstreamStatus { status, body } => {
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
    async fn send_times_out_on_a_stalled_server() {
        // UP-008: a backend that accepts and then stalls must fail on the
        // request deadline as a transport error, never hang the caller.
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
            matches!(err, GatewayError::UpstreamTransport(_)),
            "expected UpstreamTransport, got {err:?}"
        );
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
            GatewayError::UpstreamStatus { status, body } => {
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
            GatewayError::UpstreamStatus { body, .. } => {
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
            matches!(err, GatewayError::UpstreamProtocol(_)),
            "expected UpstreamProtocol, got {err:?}"
        );
        let _ = handle.join();
    }
}
