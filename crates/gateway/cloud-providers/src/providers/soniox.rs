//! Soniox provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.soniox.com` - Bearer auth and a
//! single-page `models` envelope of STT models. Every listed model is
//! speech-to-text, so the transcription kind is set; the per-model
//! `languages` list contributes its codes to the entry, while
//! `aliased_model_id`, `context_version`, `transcription_mode`, and the
//! `supports_*` capability flags have no sheet fields and are not
//! parsed. No pagination, no token limits, no pricing.
//!
//! Docs: <https://soniox.com/docs/stt/models> (OpenAPI:
//! <https://soniox.com/docs/openapi.yaml>)

use gateway_api_types::{EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::base_entry;
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "SONIOX_API_KEY";

/// The Soniox provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "soniox",
    display_name: "Soniox",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.soniox.com",
    openai_base_url: None,
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// Fetches and normalizes Soniox's model list in a single request.
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
    let response: ListResponse = client
        .get(format!("{base_url}{MODELS_PATH}"))
        .bearer_auth(key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut entries: Vec<ModelEntry> = response.models.iter().map(normalize_model).collect();
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// The list envelope.
#[derive(Debug, Deserialize)]
struct ListResponse {
    models: Vec<WireModel>,
}

/// One model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    name: Option<String>,
    #[serde(default)]
    languages: Vec<WireLanguage>,
}

/// One language as the wire reports it; only the code has sheet
/// meaning, the display name is dropped.
#[derive(Debug, Deserialize)]
struct WireLanguage {
    code: String,
}

/// Normalizes one wire model: every Soniox model is speech-to-text, so
/// the entry is the conservative base with the transcription kind and
/// the wire's language codes.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, None);
    if let Some(name) = &model.name {
        entry.display_name.clone_from(name);
    }
    entry.kind = ModelKind::Transcription;
    entry.languages = model
        .languages
        .iter()
        .map(|language| language.code.clone())
        .collect();
    entry
}

/// The entry's family: the id without its trailing `-v<version>`
/// (`stt-rt-v5` -> `stt-rt`), and the whole id otherwise.
fn family_of(id: &str) -> String {
    if let Some((base, last)) = id.rsplit_once('-') {
        let versioned = last.len() > 1
            && last.starts_with('v')
            && last[1..].bytes().all(|b| b.is_ascii_digit());
        if versioned && !base.is_empty() {
            return base.to_owned();
        }
    }
    id.to_owned()
}

/// Sets every entry's family. Soniox's catalog carries no snapshot
/// suffixes, so there is no collapse pass.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented response shape (2026-09-14 research extraction
    /// and the published OpenAPI schema): the active real-time model
    /// and the v4 alias pointing at it.
    const LIST: &str = r#"{
  "models": [
    {
      "id": "stt-rt-v5",
      "aliased_model_id": null,
      "name": "Soniox Real-Time v5",
      "context_version": 2,
      "transcription_mode": "realtime",
      "languages": [
        { "code": "en", "name": "English" },
        { "code": "zh", "name": "Chinese" }
      ],
      "supports_language_hints_strict": true,
      "supports_max_endpoint_delay": true
    },
    {
      "id": "stt-rt-v4",
      "aliased_model_id": "stt-rt-v5",
      "name": null,
      "context_version": 2,
      "transcription_mode": "realtime",
      "languages": [
        { "code": "en", "name": "English" }
      ],
      "supports_language_hints_strict": true,
      "supports_max_endpoint_delay": true
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let response: ListResponse =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        response.models.iter().map(normalize_model).collect()
    }

    #[test]
    fn models_normalize_to_transcription_kind() {
        let entries = entries();
        assert_eq!(entries.len(), 2, "aliases list alongside models");
        assert_eq!(entries[0].id, "stt-rt-v5");
        assert_eq!(
            entries[0].display_name, "Soniox Real-Time v5",
            "the wire name is the display name"
        );
        for entry in &entries {
            assert_eq!(
                entry.kind,
                ModelKind::Transcription,
                "every Soniox model is speech-to-text"
            );
            assert_eq!(entry.context_window, None, "the endpoint reports no limits");
            assert!(entry.pricing.is_none());
            assert!(entry.deprecation.is_none());
        }
    }

    #[test]
    fn missing_name_falls_back_to_the_id() {
        let entry = &entries()[1];
        assert_eq!(entry.id, "stt-rt-v4");
        assert_eq!(
            entry.display_name, "stt-rt-v4",
            "a null name falls back to the id"
        );
    }

    #[test]
    fn wire_languages_survive_normalization() {
        let entries = entries();
        assert_eq!(
            entries[0].languages,
            ["en", "zh"],
            "the per-model language codes collect into the entry"
        );
        assert_eq!(entries[1].languages, ["en"]);
    }

    #[test]
    fn family_drops_the_trailing_version() {
        let table: &[(&str, &str)] = &[
            ("stt-rt-v5", "stt-rt"),
            ("stt-rt-v4", "stt-rt"),
            ("stt-rt", "stt-rt"),
        ];
        for &(id, family) in table {
            assert_eq!(family_of(id), family, "{id}");
        }
    }

    #[test]
    fn taxonomy_sets_every_family() {
        let mut entries = entries();
        apply_taxonomy(&mut entries);
        for entry in &entries {
            assert_eq!(entry.family, "stt-rt", "{}", entry.id);
        }
    }

    #[test]
    fn descriptor_publishes_the_key_and_no_chat_base() {
        assert_eq!(PROVIDER.openai_base_url, None);
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api_types::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
