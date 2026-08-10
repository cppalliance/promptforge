//! Supervised local `llama-server` upstream with lazy respawn on child death.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::error::GatewayError;
use crate::local::error::LocalError;
use crate::local::server::{LaunchOptions, ServerGuard};
use crate::upstream::Upstream;
use crate::wire::{ChatRequest, ChatResponse};

/// Minimum gap between respawn attempts for one local child.
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(3);

struct LocalInner {
    executable: PathBuf,
    model_path: PathBuf,
    options: LaunchOptions,
    model_name: String,
    guard: Mutex<ServerGuard>,
    last_respawn: Mutex<Option<Instant>>,
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
            }),
            http: crate::http_util::bounded_client(),
        }
    }

    /// If the child has exited, respawn it once (honoring cooldown).
    ///
    /// Returns `true` when a respawn completed successfully.
    fn recover_if_dead(inner: &LocalInner) -> Result<bool, LocalError> {
        let mut guard = inner
            .guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            return Err(LocalError::Server(format!(
                "llama-server for {} exited; respawn cooldown active",
                inner.model_name
            )));
        }

        tracing::warn!(
            model = %inner.model_name,
            port = guard.port(),
            "llama-server exited after readiness; respawning"
        );
        match guard.respawn(&inner.executable, &inner.model_path, &inner.options) {
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
                    "llama-server respawn failed"
                );
                Err(error)
            }
        }
    }

    async fn forward(
        &self,
        mut req: ChatRequest,
        upstream_model: &str,
    ) -> Result<ChatResponse, GatewayError> {
        let requested = std::mem::replace(&mut req.model, upstream_model.to_string());
        let (base_url, api_key) = {
            let guard = self
                .inner
                .guard
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (guard.base_url(), guard.api_key().to_owned())
        };

        let mut builder = self
            .http
            .post(format!(
                "{}/chat/completions",
                base_url.trim_end_matches('/')
            ))
            .json(&req);
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
                crate::http_util::read_body_capped(response, crate::http_util::MAX_ERROR_BODY).await;
            let body: String = body.chars().take(2000).collect();
            return Err(GatewayError::UpstreamStatus {
                status: status.as_u16(),
                body,
            });
        }

        let mut parsed: ChatResponse = response
            .json()
            .await
            .map_err(GatewayError::upstream_transport)?;
        parsed.model = requested;
        Ok(parsed)
    }
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
            Err(error) if matches!(error, GatewayError::UpstreamTransport(_)) => {
                // Recover on a plain OS thread so reqwest::blocking readiness
                // (used by ServerGuard::respawn) never nests a Tokio runtime
                // inside the gateway's async runtime.
                let inner = Arc::clone(&self.inner);
                let (tx, rx) = tokio::sync::oneshot::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(LocalUpstream::recover_if_dead(&inner));
                });
                let recover_result = rx.await.map_err(|_| {
                    GatewayError::UpstreamTransport(Box::new(std::io::Error::other(
                        "llama-server respawn thread dropped before reporting",
                    )))
                })?;
                match recover_result {
                    Ok(true) => self.forward(req, upstream_model).await,
                    Ok(false) => Err(error),
                    Err(local) => Err(GatewayError::UpstreamTransport(Box::new(local))),
                }
            }
            Err(error) => Err(error),
        }
    }
}
