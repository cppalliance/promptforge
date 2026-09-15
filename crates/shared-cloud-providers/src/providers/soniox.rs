//! Soniox provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.soniox.com` - Bearer auth and a
//! single-page `models` envelope of STT models. Every listed model is
//! speech-to-text, so the transcription kind is set; the per-model
//! `languages` list (code + name), `aliased_model_id`,
//! `context_version`, `transcription_mode`, and the `supports_*`
//! capability flags have no sheet fields and are not parsed. No
//! pagination, no token limits, no pricing.
//!
//! Docs: <https://soniox.com/docs/stt/models> (OpenAPI:
//! <https://soniox.com/docs/openapi.yaml>)

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::base_entry;
use crate::{FetchError, Provider};

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
    env_vars: &[],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// Fetch and normalize Soniox's model list in a single request.
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
    Ok(response.models.iter().map(normalize_model).collect())
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
}

/// Normalize one wire model: every Soniox model is speech-to-text, so
/// the entry is the conservative base with the transcription kind.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, None);
    if let Some(name) = &model.name {
        entry.display_name.clone_from(name);
    }
    entry.kind = ModelKind::Transcription;
    entry
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
}
