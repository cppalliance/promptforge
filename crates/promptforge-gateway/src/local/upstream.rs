//! Supervised local `llama-server` upstream with lazy respawn on child death.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::error::GatewayError;
use crate::local::error::LocalError;
use crate::local::server::{LaunchOptions, ServerGuard};
use crate::upstream::Upstream;
use crate::wire::{
    ChatRequest, ChatResponse, EmbeddingRequest, EmbeddingResponse, RerankRequest, RerankResponse,
};

/// Minimum gap between respawn attempts for one local child.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(3);

struct LocalInner {
    executable: PathBuf,
    model_path: PathBuf,
    options: LaunchOptions,
    model_name: String,
    guard: Mutex<ServerGuard>,
    last_respawn: Mutex<Option<Instant>>,
    /// Set by an explicit [`LocalUpstream::shutdown`] to permanently disable
    /// respawn so a torn-down profile's child is never resurrected.
    shut_down: AtomicBool,
}

/// OpenAI-compatible upstream that owns one `llama-server` child and respawns it
/// after a post-ready death.
///
/// Port, `--alias`, and API key stay fixed across respawns so catalog
/// `upstream_name` and routing stay valid without rewriting `Arc<Model>`.
#[derive(Clone)]
pub(crate) struct LocalUpstream {
    inner: Arc<LocalInner>,
    http: reqwest::Client,
    /// Connect-timeout-only client for the streaming path: reqwest's
    /// whole-request timeout covers the body read and would kill any
    /// long-lived SSE stream, so streams never use `http`.
    http_stream: reqwest::Client,
}

impl std::fmt::Debug for LocalUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalUpstream")
            .field("executable", &self.inner.executable)
            .field("model_path", &self.inner.model_path)
            .field("options", &self.inner.options)
            .field("model_name", &self.inner.model_name)
            .field("guard", &self.inner.guard)
            .finish_non_exhaustive()
    }
}

impl LocalUpstream {
    /// Takes ownership of a ready [`ServerGuard`] and the recipe needed to respawn it.
    #[must_use]
    pub(crate) fn new(
        guard: ServerGuard,
        executable: PathBuf,
        model_path: PathBuf,
        options: LaunchOptions,
        model_name: String,
    ) -> LocalUpstream {
        LocalUpstream {
            inner: Arc::new(LocalInner {
                executable,
                model_path,
                options,
                model_name,
                guard: Mutex::new(guard),
                last_respawn: Mutex::new(None),
                shut_down: AtomicBool::new(false),
            }),
            http: crate::http_util::bounded_client(),
            http_stream: crate::http_util::streaming_client(),
        }
    }

    /// Terminate the owned child and permanently disable respawn, returning any
    /// teardown failure to the caller.
    ///
    /// Called at profile-switch teardown so the old child is freed
    /// deterministically even while the outgoing routing table still holds an
    /// `Arc<dyn Upstream>` clone (dropping [`crate::local::LocalRuntime`] alone
    /// cannot guarantee this - PFGL-MOD-001/PF-GW-SERVER-004).
    ///
    /// The `shut_down` flag is set *before* acquiring the guard, so an in-flight
    /// recovery worker's readiness wait is cancelled (via `respawn`'s `cancel`
    /// argument) and any post-lock recovery re-check aborts. After this returns
    /// the child is dead and no in-flight or future transport failure can
    /// resurrect it.
    ///
    /// # Errors
    /// Returns the underlying [`LocalError`] when the child kill/reap or a
    /// capture-reader join fails.
    fn teardown(&self) -> Result<(), LocalError> {
        self.inner.shut_down.store(true, Ordering::Release);
        let mut guard = self
            .inner
            .guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.shutdown()
    }

    /// If the child has exited, respawn it once (honoring cooldown).
    ///
    /// Returns `true` when a respawn completed successfully.
    fn recover_if_dead(inner: &LocalInner) -> Result<bool, LocalError> {
        // An explicit teardown wins over recovery: never resurrect a child that
        // was deliberately shut down at profile-switch time (PFGL-MOD-001).
        if inner.shut_down.load(Ordering::Acquire) {
            return Ok(false);
        }
        let mut guard = inner
            .guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Re-check after acquiring the guard: a teardown may have set the flag
        // and killed the child while we waited for the lock. Racing past the
        // pre-lock check must not respawn a torn-down child (PF-GW-SERVER-004).
        if inner.shut_down.load(Ordering::Acquire) {
            return Ok(false);
        }
        if guard.is_running()? {
            return Ok(false);
        }

        let mut last = inner
            .last_respawn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(at) = *last
            && at.elapsed() < RESPAWN_COOLDOWN
        {
            tracing::warn!(
                model = %inner.model_name,
                "llama-server dead but respawn cooldown active"
            );
            return Err(LocalError::RespawnCooldown {
                model: inner.model_name.clone(),
            });
        }

        tracing::warn!(
            model = %inner.model_name,
            port = guard.port(),
            "llama-server exited after readiness; respawning"
        );
        match guard.respawn(
            &inner.executable,
            &inner.model_path,
            &inner.options,
            &inner.shut_down,
        ) {
            Ok(()) => {
                *last = Some(Instant::now());
                tracing::info!(
                    model = %inner.model_name,
                    port = guard.port(),
                    "llama-server respawned"
                );
                Ok(true)
            }
            Err(error) => {
                *last = Some(Instant::now());
                tracing::error!(
                    model = %inner.model_name,
                    error = %error,
                    retryable = error.is_retryable(),
                    "llama-server respawn failed"
                );
                Err(error)
            }
        }
    }

    /// Test-only seam: run the recovery decision directly against `inner`.
    #[cfg(test)]
    pub(crate) fn test_recover(&self) -> Result<bool, LocalError> {
        Self::recover_if_dead(&self.inner)
    }

    /// POST `body` to the child's `{base_url}/{path}` with the per-attempt
    /// loopback credential and return the success response.
    ///
    /// A non-success status fails before the body is consumed as anything but
    /// diagnostics, so a streaming caller never sees an error response as the
    /// start of a chunk stream.
    ///
    /// # Errors
    /// Returns [`GatewayError::UpstreamConnect`] when the connection itself
    /// fails, [`GatewayError::UpstreamTransport`] on a mid-flight transport
    /// failure, and [`GatewayError::UpstreamStatus`] with a truncated body on
    /// a non-success child status.
    async fn post(
        &self,
        client: &reqwest::Client,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<reqwest::Response, GatewayError> {
        let (base_url, api_key) = {
            let guard = self
                .inner
                .guard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (guard.base_url(), guard.api_key().to_owned())
        };

        let mut builder = client
            .post(format!("{}/{path}", base_url.trim_end_matches('/')))
            .json(body);
        if !api_key.is_empty() {
            builder = builder.bearer_auth(&api_key);
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
        Ok(response)
    }

    /// POST `body` to the child's `{base_url}/{path}` and return the success
    /// body bytes.
    ///
    /// The body read is byte-bounded; decoding is left to the caller so a
    /// decode failure surfaces as a protocol error, never a transport death
    /// that would trigger a spurious child respawn (UPSTREAM-003).
    ///
    /// # Errors
    /// Returns [`GatewayError::UpstreamConnect`] when the connection itself
    /// fails, [`GatewayError::UpstreamTransport`] on a mid-flight transport
    /// failure, and [`GatewayError::UpstreamStatus`] with a truncated body on
    /// a non-success child status.
    async fn post_json(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Result<Vec<u8>, GatewayError> {
        let response = self.post(&self.http, path, body).await?;
        crate::http_util::read_bytes_capped(response, crate::http_util::MAX_JSON_BODY)
            .await
            .map_err(GatewayError::upstream_transport)
    }

    async fn forward(
        &self,
        mut req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, GatewayError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let bytes = self.post_json("chat/completions", &req).await?;
        let mut parsed: ChatResponse =
            serde_json::from_slice(&bytes).map_err(GatewayError::upstream_protocol)?;
        parsed.model = requested;
        Ok(parsed)
    }

    async fn forward_embeddings(
        &self,
        mut req: EmbeddingRequest,
        upstream_model: &str,
    ) -> Result<EmbeddingResponse, GatewayError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let bytes = self.post_json("embeddings", &req).await?;
        let mut parsed: EmbeddingResponse =
            serde_json::from_slice(&bytes).map_err(GatewayError::upstream_protocol)?;
        parsed.model = requested;
        Ok(parsed)
    }

    async fn forward_rerank(
        &self,
        mut req: RerankRequest,
        upstream_model: &str,
    ) -> Result<RerankResponse, GatewayError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let bytes = self.post_json("rerank", &req).await?;
        let mut parsed: RerankResponse =
            serde_json::from_slice(&bytes).map_err(GatewayError::upstream_protocol)?;
        parsed.model = requested;
        Ok(parsed)
    }

    async fn forward_stream(
        &self,
        mut req: ChatRequest,
        upstream_model: &str,
    ) -> Result<crate::upstream::StreamedChunks, GatewayError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        req.stream = true;
        let response = self
            .post(&self.http_stream, "chat/completions", &req)
            .await?;
        Ok(crate::upstream::sse_chunks(response, requested))
    }

    /// Run the dead-child recovery after a transport failure.
    ///
    /// Recovery runs on a plain OS thread so reqwest::blocking readiness (used
    /// by [`ServerGuard::respawn`]) never nests a Tokio runtime inside the
    /// gateway's async runtime.
    async fn recover_on_transport(&self, error: GatewayError) -> RecoveryOutcome {
        let inner = Arc::clone(&self.inner);
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(LocalUpstream::recover_if_dead(&inner));
        });
        map_recovery_reply(rx.await, error)
    }
}

/// True when a forward failure is a transport-layer death - connect or
/// mid-flight - that a child respawn might cure. A protocol or status
/// failure means the child answered, so respawning would not help.
fn is_transport_failure(error: &GatewayError) -> bool {
    matches!(
        error,
        GatewayError::UpstreamTransport(_) | GatewayError::UpstreamConnect(_)
    )
}

#[async_trait]
impl Upstream for LocalUpstream {
    async fn send(
        &self,
        req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, GatewayError> {
        match self.forward(req.clone(), upstream_model).await {
            Ok(response) => Ok(response),
            Err(error) if is_transport_failure(&error) => {
                match self.recover_on_transport(error).await {
                    RecoveryOutcome::Retry => self.forward(req, upstream_model).await,
                    RecoveryOutcome::Failed(err) => Err(err),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn send_embeddings(
        &self,
        req: EmbeddingRequest,
        upstream_model: &str,
    ) -> Result<EmbeddingResponse, GatewayError> {
        match self.forward_embeddings(req.clone(), upstream_model).await {
            Ok(response) => Ok(response),
            Err(error) if is_transport_failure(&error) => {
                match self.recover_on_transport(error).await {
                    RecoveryOutcome::Retry => self.forward_embeddings(req, upstream_model).await,
                    RecoveryOutcome::Failed(err) => Err(err),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn send_rerank(
        &self,
        req: RerankRequest,
        upstream_model: &str,
    ) -> Result<RerankResponse, GatewayError> {
        match self.forward_rerank(req.clone(), upstream_model).await {
            Ok(response) => Ok(response),
            Err(error) if is_transport_failure(&error) => {
                match self.recover_on_transport(error).await {
                    RecoveryOutcome::Retry => self.forward_rerank(req, upstream_model).await,
                    RecoveryOutcome::Failed(err) => Err(err),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn stream(
        &self,
        req: ChatRequest,
        upstream_model: &str,
    ) -> Result<crate::upstream::StreamedChunks, GatewayError> {
        // Recovery applies only to a pre-stream transport failure: once the
        // chunk stream is open, a mid-stream death surfaces as an `Err` item
        // rather than triggering a respawn under a live response.
        match self.forward_stream(req.clone(), upstream_model).await {
            Ok(streamed) => Ok(streamed),
            Err(error) if is_transport_failure(&error) => {
                match self.recover_on_transport(error).await {
                    RecoveryOutcome::Retry => self.forward_stream(req, upstream_model).await,
                    RecoveryOutcome::Failed(err) => Err(err),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn shutdown(&self) -> Result<(), LocalError> {
        self.teardown()
    }
}

/// The decision produced from a recovery worker's reply.
enum RecoveryOutcome {
    /// The child was respawned; retry the forward.
    Retry,
    /// Recovery did not (or could not) restore the child; surface this error.
    Failed(GatewayError),
}

/// Maps a recovery worker's reply (or a dropped reply) to a [`RecoveryOutcome`].
///
/// Pure so every branch - respawned, still-alive, recovery error, and a dropped
/// recovery reply (the worker thread vanished before answering) - is unit-tested
/// without a live child (UPSTREAM-005).
fn map_recovery_reply(
    reply: Result<Result<bool, LocalError>, tokio::sync::oneshot::error::RecvError>,
    original: GatewayError,
) -> RecoveryOutcome {
    match reply {
        Ok(Ok(true)) => RecoveryOutcome::Retry,
        Ok(Ok(false)) => RecoveryOutcome::Failed(original),
        Ok(Err(local)) => RecoveryOutcome::Failed(GatewayError::UpstreamTransport(Box::new(local))),
        Err(_) => RecoveryOutcome::Failed(GatewayError::UpstreamTransport(Box::new(
            std::io::Error::other("llama-server respawn thread dropped before reporting"),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport_err() -> GatewayError {
        GatewayError::UpstreamTransport(Box::new(std::io::Error::other(
            "original transport failure",
        )))
    }

    #[test]
    fn recovery_triggers_on_connect_and_transport_but_not_protocol() {
        // A dead child looks the same whether the connection was refused or
        // died mid-flight: both transport variants trigger recovery, while a
        // protocol failure means the child answered and must not respawn.
        let connect = GatewayError::UpstreamConnect(Box::new(std::io::Error::other("refused")));
        assert!(is_transport_failure(&connect));
        assert!(is_transport_failure(&transport_err()));
        let protocol = GatewayError::upstream_protocol(std::io::Error::other("bad json"));
        assert!(!is_transport_failure(&protocol));
    }

    #[tokio::test]
    async fn map_recovery_reply_covers_every_branch() {
        // Respawned -> retry.
        assert!(matches!(
            map_recovery_reply(Ok(Ok(true)), transport_err()),
            RecoveryOutcome::Retry
        ));
        // Still-alive child (no respawn) -> return the original transport error.
        assert!(matches!(
            map_recovery_reply(Ok(Ok(false)), transport_err()),
            RecoveryOutcome::Failed(GatewayError::UpstreamTransport(_))
        ));
        // Recovery error -> wrapped as a transport error.
        assert!(matches!(
            map_recovery_reply(Ok(Err(LocalError::TeardownTimeout)), transport_err()),
            RecoveryOutcome::Failed(GatewayError::UpstreamTransport(_))
        ));
        // Dropped recovery reply -> synthesized transport error, never a hang.
        let (tx, rx) = tokio::sync::oneshot::channel::<std::result::Result<bool, LocalError>>();
        drop(tx);
        let dropped = rx.await;
        match map_recovery_reply(dropped, transport_err()) {
            RecoveryOutcome::Failed(GatewayError::UpstreamTransport(source)) => {
                assert!(
                    source.to_string().contains("dropped before reporting"),
                    "unexpected message: {source}"
                );
            }
            _ => panic!("dropped recovery reply must yield a transport failure"),
        }
    }
}
