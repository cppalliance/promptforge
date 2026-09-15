//! Gemini provider: the public descriptor plus the private variance of
//! `GET /v1beta/models` - `x-goog-api-key` header auth, `pageToken`
//! pagination, and normalization of `inputTokenLimit`, `outputTokenLimit`,
//! `supportedGenerationMethods`, and the `thinking` flag into
//! `ModelEntry`.
//!
//! Docs: <https://ai.google.dev/api/models>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, ModelKind, Thinking, Tier};

use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "GEMINI_API_KEY";

/// The Gemini provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "gemini",
    display_name: "Google Gemini",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://generativelanguage.googleapis.com",
    openai_base_url: None,
    env_vars: &[],
};

/// Page size for the list request: generous, so the full catalog arrives
/// in one page today and pagination only engages as the lineup grows.
const PAGE_SIZE: u32 = 1000;

/// Fetch and normalize Gemini's model list, following `nextPageToken`
/// until the final page.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    let Some(key) = key else {
        return Err(FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: KEY_ENV,
        });
    };
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = client
            .get(format!("{base_url}/v1beta/models"))
            .header("x-goog-api-key", key)
            .query(&[("pageSize", PAGE_SIZE)]);
        if let Some(page_token) = &token {
            request = request.query(&[("pageToken", page_token)]);
        }
        let page: Page = request.send().await?.error_for_status()?.json().await?;
        entries.extend(page.models.iter().map(normalize_model));
        let Some(next) = next_token(&page) else {
            break;
        };
        token = Some(next);
    }
    Ok(entries)
}

/// One page of the list response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    models: Vec<WireModel>,
    next_page_token: Option<String>,
}

/// One model as the wire reports it. `displayName` and the token limits
/// are optional in the wire schema; the generation-method list and the
/// thinking flag default to empty and false.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireModel {
    name: String,
    display_name: Option<String>,
    input_token_limit: Option<u32>,
    output_token_limit: Option<u32>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
    #[serde(default)]
    thinking: bool,
}

/// The token for the next page. An absent or empty `nextPageToken` ends
/// traversal, so a malformed page can never loop the fetch forever.
fn next_token(page: &Page) -> Option<String> {
    page.next_page_token
        .clone()
        .filter(|token| !token.is_empty())
}

/// The upstream model slug: the wire `name` carries a `models/` prefix
/// that the sheet id drops.
fn model_id(name: &str) -> &str {
    name.strip_prefix("models/").unwrap_or(name)
}

/// The workload: an embedding-only method list means an embedding model;
/// everything else is chat.
fn model_kind(methods: &[String]) -> ModelKind {
    if methods.iter().any(|m| m == "embedContent")
        && !methods.iter().any(|m| m == "generateContent")
    {
        ModelKind::Embedding
    } else {
        ModelKind::Chat
    }
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let methods = &model.supported_generation_methods;
    let generates = methods.iter().any(|m| m == "generateContent");
    ModelEntry {
        id: model_id(&model.name).to_owned(),
        display_name: model
            .display_name
            .clone()
            .unwrap_or_else(|| model_id(&model.name).to_owned()),
        family: String::new(),
        variant_of: None,
        variant: None,
        languages: Vec::new(),
        kind: model_kind(methods),
        released_at: None,
        context_window: model.input_token_limit,
        max_output: model.output_token_limit,
        // The list endpoint reports no modality, citation, code-execution,
        // or structured-output flags.
        images: false,
        pdf_input: false,
        video_input: false,
        audio_input: false,
        citations: false,
        code_execution: false,
        structured_outputs: false,
        batch: methods.iter().any(|m| m == "batchGenerateContent"),
        // Curated: function calling is part of `generateContent`; the
        // endpoint reports no separate flag.
        tool_calling: generates,
        thinking: Thinking {
            supported: model.thinking,
            // The bare reasoning flag says nothing about budget modes
            // (the contract's normalization principle; see moonshot.rs).
            enabled: false,
            // The endpoint does not report model-chosen thinking depth.
            adaptive: false,
        },
        // The endpoint reports no effort levels, pricing, release date,
        // or deprecation status.
        effort_levels: Vec::new(),
        default_effort: None,
        pricing: None,
        deprecation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First page of the documented example response shape: a fully
    /// capable thinking chat model, with `nextPageToken` set so
    /// pagination continues.
    const PAGE_1: &str = r#"{
  "models": [
    {
      "name": "models/gemini-2.5-pro",
      "version": "2.5",
      "displayName": "Gemini 2.5 Pro",
      "description": "Stable release of Gemini 2.5 Pro.",
      "inputTokenLimit": 1048576,
      "outputTokenLimit": 65536,
      "supportedGenerationMethods": [
        "generateContent",
        "countTokens",
        "createCachedContent",
        "batchGenerateContent"
      ],
      "temperature": 1,
      "topP": 0.95,
      "topK": 64,
      "maxTemperature": 2,
      "thinking": true
    }
  ],
  "nextPageToken": "page-2"
}"#;

    /// Second and final page: an embedding-only model with no thinking
    /// flag, exercising the generation-method to kind mapping.
    const PAGE_2: &str = r#"{
  "models": [
    {
      "name": "models/gemini-embedding-001",
      "version": "001",
      "displayName": "Gemini Embedding",
      "description": "Text embedding model.",
      "inputTokenLimit": 2048,
      "outputTokenLimit": 1,
      "supportedGenerationMethods": ["embedContent"]
    }
  ]
}"#;

    fn page(json: &str) -> Page {
        serde_json::from_str(json).expect("fixture must parse as a page")
    }

    fn only_model(json: &str) -> ModelEntry {
        let page = page(json);
        assert_eq!(page.models.len(), 1, "fixture holds exactly one model");
        normalize_model(&page.models[0])
    }

    #[test]
    fn normalizes_full_chat_entry() {
        let entry = only_model(PAGE_1);
        assert_eq!(entry.id, "gemini-2.5-pro", "the models/ prefix drops");
        assert_eq!(entry.display_name, "Gemini 2.5 Pro");
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(
            entry.context_window,
            Some(1_048_576),
            "inputTokenLimit maps to context_window"
        );
        assert_eq!(
            entry.max_output,
            Some(65_536),
            "outputTokenLimit maps to max_output"
        );
        assert!(
            entry.batch,
            "batchGenerateContent maps to the batch capability"
        );
        assert!(
            entry.tool_calling,
            "curated: function calling is part of generateContent"
        );
        assert!(
            entry.thinking.supported && !entry.thinking.enabled,
            "the thinking flag maps to supported only; the bare flag says \
             nothing about budget modes: {:?}",
            entry.thinking
        );
        assert!(
            !entry.thinking.adaptive,
            "the endpoint does not report model-chosen thinking depth"
        );
        assert!(!entry.images && !entry.pdf_input);
        assert!(!entry.video_input && !entry.audio_input);
        assert!(!entry.citations && !entry.code_execution && !entry.structured_outputs);
        assert_eq!(
            entry.released_at, None,
            "the endpoint reports no release date"
        );
        assert!(entry.effort_levels.is_empty());
        assert_eq!(entry.default_effort, None);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn embedding_only_methods_map_to_embedding_kind() {
        let entry = only_model(PAGE_2);
        assert_eq!(entry.id, "gemini-embedding-001");
        assert_eq!(
            entry.kind,
            ModelKind::Embedding,
            "embedContent without generateContent is an embedding model"
        );
        assert!(!entry.tool_calling, "no generateContent, no tool calling");
        assert!(!entry.batch, "no batchGenerateContent, no batch");
        assert!(
            !entry.thinking.supported && !entry.thinking.enabled,
            "an absent thinking flag normalizes to false"
        );
        assert_eq!(entry.context_window, Some(2048));
        assert_eq!(entry.max_output, Some(1));
    }

    #[test]
    fn missing_display_name_and_limits_fall_back_conservatively() {
        let entry = only_model(
            r#"{
              "models": [
                {
                  "name": "models/gemini-legacy",
                  "version": "1.0",
                  "supportedGenerationMethods": ["generateContent", "countTokens"]
                }
              ]
            }"#,
        );
        assert_eq!(
            entry.display_name, "gemini-legacy",
            "a missing displayName falls back to the stripped id"
        );
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.max_output, None);
        assert!(!entry.thinking.supported);
        assert!(!entry.batch);
        assert_eq!(entry.kind, ModelKind::Chat);
    }

    #[test]
    fn pagination_follows_next_page_token() {
        let first = page(PAGE_1);
        let token = next_token(&first).expect("a page with nextPageToken must yield a token");
        assert_eq!(token, "page-2", "the token passes through verbatim");
        let second = page(PAGE_2);
        assert_eq!(next_token(&second), None, "the final page ends traversal");
    }

    #[test]
    fn pagination_stops_on_empty_token() {
        let page = page(r#"{ "models": [], "nextPageToken": "" }"#);
        assert_eq!(
            next_token(&page),
            None,
            "an empty token must not loop the fetch forever"
        );
    }
}
