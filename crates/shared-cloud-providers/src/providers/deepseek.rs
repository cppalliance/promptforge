//! DeepSeek provider: the public descriptor plus the private variance of
//! `GET /models` under `https://api.deepseek.com` - Bearer auth and the
//! IDs-only OpenAI response shape: no pagination, no token limits, and no
//! capability reporting, so every entry is the conservative base entry.
//!
//! Docs: <https://api-docs.deepseek.com>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "DEEPSEEK_API_KEY";

/// The DeepSeek provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "deepseek",
    display_name: "DeepSeek",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.deepseek.com",
    openai_base_url: None,
    env_vars: &[],
};

/// The list path under the base URL: DeepSeek mounts it at `/models`,
/// not `/v1/models`.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize DeepSeek's model list in a single request.
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

/// Normalize one wire model: the endpoint is IDs-only, so the entry is
/// the conservative base.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape: the two flagship models.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "deepseek-chat",
      "object": "model",
      "created": 1782864000,
      "owned_by": "deepseek"
    },
    {
      "id": "deepseek-reasoner",
      "object": "model",
      "created": 1782864000,
      "owned_by": "deepseek"
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
        assert_eq!(entry.id, "deepseek-chat");
        assert_eq!(entry.display_name, "deepseek-chat");
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
    }
}
