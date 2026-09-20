//! MiniMax provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.minimax.io/v1` - Bearer auth and
//! the IDs-only OpenAI response shape: no pagination, no token limits,
//! and no capability reporting, so every entry is the conservative base
//! entry. The global `.io` host is used; mainland-China keys against
//! `api.minimaxi.com` are not interchangeable with it.
//!
//! Docs: <https://platform.minimax.io/docs/api-reference/models/openai/list-models>

use gateway_api_types::{EnvRole, ModelEntry, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

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
    openai_base_url: Some("https://api.minimax.io/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetches and normalizes MiniMax's model list in a single request.
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

/// The entry's family: `MiniMax-<line>` for the flagship-prefixed ids
/// (`MiniMax-M3`, `MiniMax-Text-01`), the first segment for the media
/// lines (`speech`, `music`, `image`, `video`), and the whole id
/// otherwise. The catalog carries no snapshot suffixes, so there is no
/// collapse pass.
fn family_of(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("MiniMax-") {
        let line = rest.split('-').next().unwrap_or(rest);
        if !line.is_empty() {
            return format!("MiniMax-{line}");
        }
    }
    for line in ["speech", "music", "image", "video"] {
        if id == line || id.starts_with(&format!("{line}-")) {
            return (*line).to_owned();
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
        assert_eq!(entry.kind, gateway_api_types::ModelKind::Chat);
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

    #[test]
    fn catalog_ids_classify_into_line_families() {
        // MiniMax was unprovisioned for the 2026-09-14 sheet, so the
        // table follows the documented catalog.
        let mut entries: Vec<ModelEntry> = [
            "MiniMax-M3",
            "MiniMax-M2.7",
            "MiniMax-Text-01",
            "MiniMax-VL-01",
            "speech-02-hd",
            "speech-02-turbo",
            "music-01",
            "image-01",
            "video-01",
            "hailuo-02",
        ]
        .iter()
        .map(|id| crate::taxonomy::fixture::entry(id))
        .collect();
        apply_taxonomy(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[
            ("MiniMax-M3", "MiniMax-M3"),
            ("MiniMax-M2.7", "MiniMax-M2.7"),
            ("MiniMax-Text-01", "MiniMax-Text"),
            ("MiniMax-VL-01", "MiniMax-VL"),
            ("speech-02-hd", "speech"),
            ("speech-02-turbo", "speech"),
            ("music-01", "music"),
            ("image-01", "image"),
            ("video-01", "video"),
            ("hailuo-02", "hailuo-02"),
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

    #[test]
    fn descriptor_publishes_the_chat_base_and_key() {
        assert_eq!(PROVIDER.openai_base_url, Some("https://api.minimax.io/v1"));
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api_types::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
