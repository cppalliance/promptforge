//! Meta provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.meta.ai/v1` - Bearer auth over the
//! OpenAI-compatible list shape. The response schema is not fully
//! enumerated in the official docs, so normalization treats it as the
//! IDs-only shape: every entry is the conservative base entry.
//!
//! Docs: <https://ai.developer.meta.com/docs>

use gateway_api::{EnvRole, ModelEntry, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

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
    openai_base_url: Some("https://api.meta.ai/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetches and normalizes Meta's model list in a single request.
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

/// Normalizes one wire model: the response schema is not fully enumerated,
/// so the entry is the conservative base.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

/// The entry's family: the product line, taken as the first two segments
/// (`muse-spark`, `muse-voice`, `muse-image`). The catalog carries no
/// snapshot suffixes, so there is no collapse pass.
fn family_of(id: &str) -> String {
    let mut segments = id.split('-');
    let Some(first) = segments.next() else {
        return id.to_owned();
    };
    let Some(second) = segments.next() else {
        return id.to_owned();
    };
    format!("{first}-{second}")
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

    /// Trimmed 2026-09-14 sheet excerpt: the real Meta ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-meta.json");

    #[test]
    fn fixture_ids_classify_into_product_line_families() {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply_taxonomy(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[
            ("muse-spark-1.1", "muse-spark"),
            ("muse-spark-1.2", "muse-spark"),
            ("muse-spark-1.2-contributor", "muse-spark"),
            ("muse-spark-1.3", "muse-spark"),
            ("muse-spark-1.3-contributor", "muse-spark"),
            ("muse-voice-transcribe-1.0", "muse-voice"),
            ("muse-image-1.0", "muse-image"),
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
