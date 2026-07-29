//! An `OpenAI`-compatible chat completions client.
//!
//! Tranche 1 speaks the plain `/chat/completions` shape: a list of messages in,
//! one text reply out. No tools, no streaming. The base URL defaults to
//! Anthropic's `OpenAI`-compatible endpoint but can be repointed at a local
//! server or the future gateway with one environment variable.

use crate::{Error, Result};

/// Default backend base URL (Anthropic's `OpenAI`-compatible endpoint).
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
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

/// A chat completions client bound to one backend, key, and model.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl Client {
    /// Build a client from the environment.
    ///
    /// - Base URL: `PROMPTFORGE_BASE_URL`, else Anthropic's endpoint.
    /// - Model: `PROMPTFORGE_MODEL`, else a sane default.
    /// - API key: `PROMPTFORGE_API_KEY`, else `ANTHROPIC_API_KEY`. Required.
    ///
    /// # Errors
    /// Returns [`Error::MissingEnv`] when neither `PROMPTFORGE_API_KEY` nor
    /// `ANTHROPIC_API_KEY` is set.
    pub fn from_env() -> Result<Client> {
        let api_key = std::env::var("PROMPTFORGE_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .map_err(|_| Error::MissingEnv("ANTHROPIC_API_KEY".into()))?;
        let base_url =
            std::env::var("PROMPTFORGE_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        let model = std::env::var("PROMPTFORGE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Ok(Client {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
        })
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
    /// the backend responds with a non-success status, and
    /// [`Error::MalformedResponse`] when the response carries no usable content.
    pub async fn complete(&self, messages: &[Message]) -> Result<String> {
        let request = ChatRequest {
            model: &self.model,
            messages,
        };
        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
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
