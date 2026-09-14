//! OpenAI provider: the public descriptor plus the private variance of
//! `GET /v1/models` - Bearer auth and the IDs-only response shape (`id`,
//! `created`, `owned_by`): no pagination, no token limits, and no
//! capability reporting, so every entry is the conservative base entry.
//!
//! Docs: <https://developers.openai.com/api/reference>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// The OpenAI provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "openai",
    display_name: "OpenAI",
    tier: Tier::Prime,
    key_env: "OPENAI_API_KEY",
    base_url: "https://api.openai.com/v1",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize OpenAI's model list in a single request.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> Result<Vec<ModelEntry>, FetchError> {
    let models: Vec<WireModel> =
        fetch_list(client, &format!("{base_url}{MODELS_PATH}"), key).await?;
    Ok(models.iter().map(normalize_model).collect())
}

/// One model as the wire reports it; `owned_by` is carried on the wire
/// but has no sheet field.
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
    use time::{Date, Month};

    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape: two IDs-only entries.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "gpt-5.2",
      "object": "model",
      "created": 1782864000,
      "owned_by": "openai"
    },
    {
      "id": "gpt-5-mini",
      "object": "model",
      "created": 1782864000,
      "owned_by": "openai"
    }
  ]
}"#;

    fn entries(json: &str) -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(json).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn ids_only_entries_are_conservative() {
        let entries = entries(LIST);
        assert_eq!(entries.len(), 2, "both listed models normalize");
        let entry = &entries[0];
        assert_eq!(entry.id, "gpt-5.2");
        assert_eq!(
            entry.display_name, "gpt-5.2",
            "the id doubles as the display name"
        );
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.effort_levels.is_empty());
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn created_maps_to_released_at() {
        let entries = entries(LIST);
        assert_eq!(
            entries[0].released_at,
            Date::from_calendar_date(2026, Month::July, 1).ok(),
            "the unix `created` timestamp maps to a calendar date"
        );
    }
}
