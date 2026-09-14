//! Moonshot AI (Kimi) provider: the public descriptor plus the private
//! variance of `GET /v1/models` - Bearer auth over the OpenAI list shape,
//! enriched with `context_length` and image-input, video-input, and
//! reasoning flags. No pagination. The global `.ai` host is used; `.cn`
//! keys are not interchangeable with it.
//!
//! Docs: <https://platform.moonshot.ai/docs>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// The Moonshot AI provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "moonshot",
    display_name: "Moonshot AI",
    tier: Tier::Prime,
    key_env: "MOONSHOT_API_KEY",
    base_url: "https://api.moonshot.ai/v1",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize Moonshot's model list in a single request.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> Result<Vec<ModelEntry>, FetchError> {
    let models: Vec<WireModel> =
        fetch_list(client, &format!("{base_url}{MODELS_PATH}"), key).await?;
    Ok(models.iter().map(normalize_model).collect())
}

/// One model as the wire reports it: the OpenAI shape plus Moonshot's
/// enrichment fields.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
    context_length: Option<u32>,
    supports_image_input: Option<bool>,
    supports_video_input: Option<bool>,
    supports_reasoning: Option<bool>,
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    entry.context_window = model.context_length;
    entry.images = model.supports_image_input.unwrap_or(false);
    entry.video_input = model.supports_video_input.unwrap_or(false);
    entry.thinking.supported = model.supports_reasoning.unwrap_or(false);
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape: one fully enriched entry
    /// and one text-only non-reasoning entry.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "kimi-k2.5",
      "object": "model",
      "created": 1782864000,
      "owned_by": "moonshot",
      "context_length": 262144,
      "supports_image_input": true,
      "supports_video_input": true,
      "supports_reasoning": true
    },
    {
      "id": "kimi-k2-instruct",
      "object": "model",
      "created": 1782864000,
      "owned_by": "moonshot",
      "context_length": 131072,
      "supports_image_input": false,
      "supports_video_input": false,
      "supports_reasoning": false
    }
  ]
}"#;

    fn entries(json: &str) -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(json).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn maps_context_length_and_capability_flags() {
        let entries = entries(LIST);
        let entry = &entries[0];
        assert_eq!(entry.id, "kimi-k2.5");
        assert_eq!(
            entry.context_window,
            Some(262_144),
            "context_length maps to context_window"
        );
        assert!(entry.images, "supports_image_input maps to images");
        assert!(
            entry.video_input,
            "supports_video_input maps to video_input"
        );
        assert!(
            entry.thinking.supported,
            "supports_reasoning maps to thinking.supported"
        );
        assert!(
            !entry.thinking.enabled && !entry.thinking.adaptive,
            "the reasoning flag says nothing about budget modes"
        );
    }

    #[test]
    fn text_only_entry_is_conservative() {
        let entries = entries(LIST);
        let entry = &entries[1];
        assert_eq!(entry.id, "kimi-k2-instruct");
        assert_eq!(entry.context_window, Some(131_072));
        assert!(!entry.images && !entry.video_input);
        assert!(!entry.thinking.supported);
    }

    #[test]
    fn absent_flags_are_conservative() {
        let entries = entries(
            r#"{
              "object": "list",
              "data": [
                {
                  "id": "kimi-legacy",
                  "object": "model",
                  "created": 1782864000,
                  "owned_by": "moonshot"
                }
              ]
            }"#,
        );
        let entry = &entries[0];
        assert_eq!(entry.context_window, None);
        assert!(!entry.images && !entry.video_input && !entry.thinking.supported);
    }
}
