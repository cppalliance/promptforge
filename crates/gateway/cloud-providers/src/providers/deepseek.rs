//! DeepSeek provider: the public descriptor plus the private variance of
//! `GET /models` under `https://api.deepseek.com` - Bearer auth and the
//! IDs-only OpenAI response shape: no pagination, no token limits, and no
//! capability reporting, so every entry is the conservative base entry.
//!
//! Docs: <https://api-docs.deepseek.com>

use gateway_api_types::{EnvRole, ModelEntry, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

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
    openai_base_url: Some("https://api.deepseek.com/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL: DeepSeek mounts it at `/models`,
/// not `/v1/models`.
const MODELS_PATH: &str = "/models";

/// Fetches and normalizes DeepSeek's model list in a single request.
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
    let mut entries: Vec<ModelEntry> = models.iter().map(normalize_model).collect();
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// One model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
}

/// Normalizes one wire model: the endpoint is IDs-only, so the entry is
/// the conservative base.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

/// The entry's family: the `deepseek-v<N>` version prefix for the
/// numbered line, and the whole id otherwise. The catalog has no
/// snapshot suffixes, so there is no collapse pass.
fn family_of(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("deepseek-v") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return format!("deepseek-v{digits}");
        }
    }
    id.to_owned()
}

/// Sets every entry's family.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
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

    /// Trimmed 2026-09-14 sheet excerpt: the real DeepSeek ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-deepseek.json");

    #[test]
    fn fixture_ids_classify_into_families() {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply_taxonomy(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[
            ("deepseek-flash", "deepseek-flash"),
            ("deepseek-v4-pro", "deepseek-v4"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
        for entry in by_id.values() {
            assert!(
                entry.variant_of.is_none(),
                "the catalog carries no snapshot suffixes: {}",
                entry.id
            );
        }
    }
}
