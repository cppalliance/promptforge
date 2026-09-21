//! Moonshot AI (Kimi) provider: the public descriptor plus the private
//! variance of `GET /v1/models` - Bearer auth over the OpenAI list shape,
//! enriched with `context_length` and image-input, video-input, and
//! reasoning flags. No pagination. The global `.ai` host is used; `.cn`
//! keys are not interchangeable with it.
//!
//! Docs: <https://platform.moonshot.ai/docs>

use gateway_api_types::{EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "MOONSHOT_API_KEY";

/// The Moonshot AI provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "moonshot",
    display_name: "Moonshot AI",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.moonshot.ai/v1",
    openai_base_url: Some("https://api.moonshot.ai/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetches and normalizes Moonshot's model list in a single request.
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

/// Normalizes one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    entry.kind = kind_of(&model.id);
    entry.context_window = model.context_length;
    entry.images = model.supports_image_input.unwrap_or(false);
    entry.video_input = model.supports_video_input.unwrap_or(false);
    entry.thinking.supported = model.supports_reasoning.unwrap_or(false);
    entry
}

/// The workload, inferred from the name: the endpoint's flags report
/// capabilities, not kinds. The current catalog is all chat; the
/// segment rules cover the line names Moonshot documents for media
/// models, the same treatment as the other dialect providers.
fn kind_of(id: &str) -> ModelKind {
    let segments: Vec<&str> = id.split('-').collect();
    let has = |names: &[&str]| segments.iter().any(|segment| names.contains(segment));
    if has(&["tts"]) {
        ModelKind::Speech
    } else if has(&["asr"]) {
        ModelKind::Transcription
    } else if has(&["image"]) {
        ModelKind::Image
    } else if has(&["embedding"]) {
        ModelKind::Embedding
    } else {
        ModelKind::Chat
    }
}

/// The entry's family: the `kimi-k<version>` prefix for the numbered
/// Kimi line, the `moonshot-v<N>` prefix for the legacy line, and the
/// whole id otherwise. The catalog has no snapshot suffixes, so
/// there is no collapse pass.
fn family_of(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("kimi-k") {
        let token: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if token.bytes().any(|b| b.is_ascii_digit()) {
            return format!("kimi-k{token}");
        }
    }
    if let Some(rest) = id.strip_prefix("moonshot-v") {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            return format!("moonshot-v{digits}");
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

    /// Trimmed 2026-09-14 sheet excerpt: the real Moonshot ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-moonshot.json");

    #[test]
    fn fixture_ids_classify_into_version_families() {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply_taxonomy(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[("kimi-k2.6", "kimi-k2.6"), ("kimi-k2.7-code", "kimi-k2.7")];
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
    fn family_covers_the_documented_lines() {
        let table: &[(&str, &str)] = &[
            ("moonshot-v1-8k", "moonshot-v1"),
            ("moonshot-v1-128k", "moonshot-v1"),
            ("kimi-latest", "kimi-latest"),
            ("kimi-k2.5", "kimi-k2.5"),
        ];
        for &(id, family) in table {
            assert_eq!(family_of(id), family, "{id}");
        }
    }

    #[test]
    fn name_patterns_infer_the_kind() {
        let table: &[(&str, gateway_api_types::ModelKind)] = &[
            ("kimi-k2.6", gateway_api_types::ModelKind::Chat),
            ("moonshot-v1-8k", gateway_api_types::ModelKind::Chat),
            ("kimi-tts-1", gateway_api_types::ModelKind::Speech),
            ("kimi-asr-1", gateway_api_types::ModelKind::Transcription),
            ("kimi-image-1", gateway_api_types::ModelKind::Image),
        ];
        for &(id, kind) in table {
            assert_eq!(kind_of(id), kind, "{id}");
        }
    }

    #[test]
    fn normalization_applies_the_inferred_kind() {
        let entries = entries(
            r#"{
              "object": "list",
              "data": [
                {
                  "id": "kimi-tts-1",
                  "object": "model",
                  "created": 1782864000,
                  "owned_by": "moonshot"
                }
              ]
            }"#,
        );
        assert_eq!(
            entries[0].kind,
            gateway_api_types::ModelKind::Speech,
            "the endpoint reports no kind; the name rule supplies it"
        );
    }

    #[test]
    fn descriptor_publishes_the_chat_base_and_key() {
        assert_eq!(PROVIDER.openai_base_url, Some("https://api.moonshot.ai/v1"));
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api_types::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
