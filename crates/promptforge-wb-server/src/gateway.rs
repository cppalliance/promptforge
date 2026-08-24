//! HTTP client for the PromptForge gateway's OpenAI-compatible API.
//!
//! [`GatewayClient`] wraps `reqwest` with bearer authentication and returns
//! responses as raw bytes so the workbench routes can relay them to the
//! caller byte-for-byte. A non-success status from the gateway is *not* an
//! error here: it is part of the relayed response.

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
}
