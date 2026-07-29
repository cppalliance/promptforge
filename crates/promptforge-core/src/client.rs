//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! Tranche 1 speaks the plain `/chat/completions` shape: a list of messages in,
//! one text reply out. No tools, no streaming. The client holds only the
//! gateway's URL and the shared token; the vendor credential lives in the
//! gateway, so the executor never sees it. Point `PROMPTFORGE_BASE_URL` at a
//! local server or another gateway to retarget it.

use crate::{Error, Result};

/// Default backend base URL (the local development gateway).
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8081/v1";
/// Default model used when `PROMPTFORGE_MODEL` is unset.
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

/// A single chat message.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Message {
    /// The role: `system`, `user`, or `assistant`.
    pub role: String,
    /// The message text.
    pub content: String,
}

impl Message {
    /// Construct a `user` message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: "user".into(),
            content: content.into(),
        }
    }
}

/// A chat completions client bound to one gateway, token, and model.
#[derive(Debug, Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    model: String,
}

impl GatewayClient {
    /// Build a client from explicit parts (used by tests and by
    /// [`GatewayClient::from_env`]). A trailing slash on `base_url` is trimmed.
    #[must_use]
    pub fn new(
        base_url: &str,
        token: impl Into<String>,
        model: impl Into<String>,
    ) -> GatewayClient {
        GatewayClient {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.into(),
            model: model.into(),
        }
    }

    /// Build a client from the environment.
    ///
    /// - Base URL: `PROMPTFORGE_BASE_URL`, else the local gateway.
    /// - Model: `PROMPTFORGE_MODEL`, else a sane default.
    /// - Token: `PROMPTFORGE_TOKEN`, the gateway's shared bearer. Required.
    ///
    /// # Errors
    /// Returns [`Error::MissingEnv`] when `PROMPTFORGE_TOKEN` is not set.
    pub fn from_env() -> Result<GatewayClient> {
        let token = std::env::var("PROMPTFORGE_TOKEN")
            .map_err(|_| Error::MissingEnv("PROMPTFORGE_TOKEN".into()))?;
        let base_url =
            std::env::var("PROMPTFORGE_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        let model = std::env::var("PROMPTFORGE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Ok(GatewayClient::new(&base_url, token, model))
    }

    /// The model this client will call.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Send a list of messages and return the assistant's text reply.
    ///
    /// # Errors
    /// Returns [`Error::Http`] on a transport failure, [`Error::Backend`] when
    /// the gateway responds with a non-success status, and
    /// [`Error::MalformedResponse`] when the response carries no usable content.
    pub async fn complete(&self, messages: &[Message]) -> Result<String> {
        let request = ChatRequest {
            model: &self.model,
            messages,
        };
        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.token)
            .json(&request)
            .send()
            .await
            .map_err(Error::http)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let body: String = body.chars().take(2000).collect();
            let body = if body.is_empty() {
                "(empty body)".to_string()
            } else {
                body
            };
            return Err(Error::Backend {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: ChatResponse = response.json().await.map_err(Error::http)?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::MalformedResponse("no choices in response".into()))?;
        choice
            .message
            .content
            .ok_or_else(|| Error::MalformedResponse("choice had no content".into()))
    }
}

/// The chat completions request body.
#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
}

/// The chat completions response body (only the fields we use).
#[derive(serde::Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

/// One completion choice.
#[derive(serde::Deserialize)]
struct Choice {
    message: ResponseMessage,
}

/// The assistant message inside a choice.
#[derive(serde::Deserialize)]
struct ResponseMessage {
    content: Option<String>,
}
