//! The backend-facing side: the [`Upstream`] trait and its OpenAI passthrough.
//!
//! The trait is the seam where per-vendor translation will live. v0 ships one
//! implementation, [`OpenAiUpstream`], which forwards the OpenAI shape
//! unchanged. Adding an Anthropic or pack upstream later is a new implementation
//! behind this same trait, with no change to routing or the request handler.

use async_trait::async_trait;

use crate::config::Secret;
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
        // Return the caller's model name, never the backend's.
        parsed.model = requested;
        Ok(parsed)
    }
}
