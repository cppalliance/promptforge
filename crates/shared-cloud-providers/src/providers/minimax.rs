//! MiniMax provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.minimax.io/v1` - Bearer auth and
//! the IDs-only OpenAI response shape: no pagination, no token limits,
//! and no capability reporting, so every entry is the conservative base
//! entry. The global `.io` host is used; mainland-China keys against
//! `api.minimaxi.com` are not interchangeable with it.
//!
//! Docs: <https://platform.minimax.io/docs/api-reference/models/openai/list-models>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "MINIMAX_API_KEY";

/// The MiniMax provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "minimax",
    display_name: "MiniMax",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.minimax.io/v1",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize MiniMax's model list in a single request.
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
    use time::{Date, Month};

    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape (2026-09-14 research
    /// extraction): `MiniMax-M3` plus two earlier flagships.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "MiniMax-M3",
      "object": "model",
      "created": 1780272000,
      "owned_by": "minimax"
    },
    {
      "id": "MiniMax-M2.7",
      "object": "model",
      "created": 1772064000,
      "owned_by": "minimax"
    },
    {
      "id": "MiniMax-M2.5",
      "object": "model",
      "created": 1767225600,
      "owned_by": "minimax"
    }
  ]
}"#;

    #[test]
    fn ids_only_entries_are_conservative() {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        let entries: Vec<ModelEntry> = page.data.iter().map(normalize_model).collect();
        assert_eq!(entries.len(), 3, "all listed models normalize");
        let entry = &entries[0];
        assert_eq!(entry.id, "MiniMax-M3");
        assert_eq!(entry.display_name, "MiniMax-M3");
        assert_eq!(entry.kind, shared_gateway_api::ModelKind::Chat);
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn created_unix_seconds_map_to_a_calendar_date() {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        let entry = normalize_model(&page.data[0]);
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::June, 1).ok(),
            "1780272000 is 2026-06-01T00:00:00Z"
        );
    }
}
