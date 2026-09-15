//! Meta provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.meta.ai/v1` - Bearer auth over the
//! OpenAI-compatible list shape. The response schema is not fully
//! enumerated in the official docs, so normalization treats it as the
//! IDs-only shape: every entry is the conservative base entry.
//!
//! Docs: <https://ai.developer.meta.com/docs>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "META_API_KEY";

/// The Meta provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "meta",
    display_name: "Meta",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.meta.ai/v1",
    openai_base_url: None,
    env_vars: &[],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize Meta's model list in a single request.
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
    let models: Vec<WireModel> =
        fetch_list(client, &format!("{base_url}{MODELS_PATH}"), key).await?;
    Ok(models.iter().map(normalize_model).collect())
}

/// One model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
}

/// Normalize one wire model: the response schema is not fully enumerated,
/// so the entry is the conservative base.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape: OpenAI-compatible entries.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "llama-4-maverick",
      "object": "model",
      "created": 1782864000,
      "owned_by": "meta"
    },
    {
      "id": "llama-4-scout",
      "object": "model",
      "created": 1782864000,
      "owned_by": "meta"
    }
  ]
}"#;

    #[test]
    fn ids_only_entries_are_conservative() {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        let entries: Vec<ModelEntry> = page.data.iter().map(normalize_model).collect();
        assert_eq!(entries.len(), 2, "both listed models normalize");
        let entry = &entries[0];
        assert_eq!(entry.id, "llama-4-maverick");
        assert_eq!(entry.display_name, "llama-4-maverick");
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
    }
}
