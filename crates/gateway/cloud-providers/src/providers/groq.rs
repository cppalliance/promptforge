//! Groq provider: the public descriptor plus the private variance of
//! `GET /models` under `https://api.groq.com/openai/v1` - Bearer auth and
//! the IDs-only OpenAI response shape: no pagination, no token limits,
//! and no capability reporting, so every entry is the conservative base
//! entry. The one enrichment: Groq's speech-to-text models (the
//! `whisper-*` family) are transcription models, so their kind is set.
//!
//! Docs: <https://console.groq.com/docs/models>

use gateway_api::{EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "GROQ_API_KEY";

/// The Groq provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "groq",
    display_name: "Groq",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.groq.com/openai/v1",
    openai_base_url: Some("https://api.groq.com/openai/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize Groq's model list in a single request.
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

/// Normalize one wire model: the endpoint is IDs-only, so the entry is
/// the conservative base; the `whisper-*` family is speech-to-text.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    if model.id.starts_with("whisper-") {
        entry.kind = ModelKind::Transcription;
    }
    entry
}

/// The entry's family: the vendor for slash-namespaced resold ids
/// (`meta-llama/...`, `qwen/...`), the `whisper` line, the
/// `llama-<version>` prefix for the bare Llama ids, and the whole id
/// otherwise. The catalog carries no snapshot suffixes, so there is no
/// collapse pass.
fn family_of(id: &str) -> String {
    if let Some((vendor, _)) = crate::taxonomy::vendor_prefix(id) {
        return vendor.to_owned();
    }
    if id == "whisper" || id.starts_with("whisper-") {
        return "whisper".to_owned();
    }
    if let Some(rest) = id.strip_prefix("llama-") {
        let token = rest.split('-').next().unwrap_or(rest);
        if crate::taxonomy::is_version_token(token) {
            return format!("llama-{token}");
        }
    }
    id.to_owned()
}

/// Set every entry's family.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented response shape (2026-09-14 research extraction):
    /// one chat model and Groq's two STT models.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "llama-3.3-70b-versatile",
      "object": "model",
      "created": 1733097600,
      "owned_by": "Meta"
    },
    {
      "id": "whisper-large-v3",
      "object": "model",
      "created": 1693728000,
      "owned_by": "OpenAI"
    },
    {
      "id": "whisper-large-v3-turbo",
      "object": "model",
      "created": 1727740800,
      "owned_by": "OpenAI"
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn chat_entry_is_conservative() {
        let entries = entries();
        let entry = &entries[0];
        assert_eq!(entry.id, "llama-3.3-70b-versatile");
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
    }

    #[test]
    fn whisper_models_are_transcription_kind() {
        let entries = entries();
        assert_eq!(
            entries[1].kind,
            ModelKind::Transcription,
            "whisper-large-v3 is speech-to-text"
        );
        assert_eq!(
            entries[2].kind,
            ModelKind::Transcription,
            "whisper-large-v3-turbo is speech-to-text"
        );
    }

    #[test]
    fn catalog_ids_classify_into_vendor_and_line_families() {
        // Groq was unprovisioned for the 2026-09-14 sheet, so the table
        // follows the documented catalog: namespaced ids family by
        // vendor, bare ids by line.
        let mut entries: Vec<ModelEntry> = [
            "llama-3.3-70b-versatile",
            "whisper-large-v3",
            "whisper-large-v3-turbo",
            "meta-llama/llama-4-scout-17b-16e-instruct",
            "qwen/qwen3-32b",
            "moonshotai/kimi-k2-instruct",
            "openai/gpt-oss-120b",
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
            ("llama-3.3-70b-versatile", "llama-3.3"),
            ("whisper-large-v3", "whisper"),
            ("whisper-large-v3-turbo", "whisper"),
            ("meta-llama/llama-4-scout-17b-16e-instruct", "meta-llama"),
            ("qwen/qwen3-32b", "qwen"),
            ("moonshotai/kimi-k2-instruct", "moonshotai"),
            ("openai/gpt-oss-120b", "openai"),
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
        assert_eq!(
            PROVIDER.openai_base_url,
            Some("https://api.groq.com/openai/v1")
        );
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
