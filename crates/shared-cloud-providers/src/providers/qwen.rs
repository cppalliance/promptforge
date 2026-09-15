//! Alibaba Qwen provider: the public descriptor plus the private variance
//! of the DashScope compatible-mode endpoint - `GET /models` under
//! `https://dashscope-intl.aliyuncs.com/compatible-mode/v1`, Bearer auth, and
//! the plain OpenAI response shape. The native `/api/v1/models` endpoint
//! adds pagination, pricing, and context length; the compatible-mode
//! endpoint is IDs-only, so every entry is the conservative base entry.
//!
//! Docs: <https://www.alibabacloud.com/help/en/model-studio>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "DASHSCOPE_API_KEY";

/// The Alibaba Qwen provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "qwen",
    display_name: "Alibaba Qwen",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    openai_base_url: None,
    env_vars: &[],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize Qwen's model list in a single request.
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

/// Normalize one wire model: the compatible-mode endpoint is IDs-only,
/// so the entry is the conservative base.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape from the compatible-mode
    /// endpoint.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "qwen3-max",
      "object": "model",
      "created": 1782864000,
      "owned_by": "alibaba"
    },
    {
      "id": "qwen3-coder-plus",
      "object": "model",
      "created": 1782864000,
      "owned_by": "alibaba"
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
        assert_eq!(entry.id, "qwen3-max");
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
    }
}
