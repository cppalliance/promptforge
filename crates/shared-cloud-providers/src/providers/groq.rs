//! Groq provider: the public descriptor plus the private variance of
//! `GET /models` under `https://api.groq.com/openai/v1` - Bearer auth and
//! the IDs-only OpenAI response shape: no pagination, no token limits,
//! and no capability reporting, so every entry is the conservative base
//! entry. The one enrichment: Groq's speech-to-text models (the
//! `whisper-*` family) are transcription models, so their kind is set.
//!
//! Docs: <https://console.groq.com/docs/models>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

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
    Ok(models.iter().map(normalize_model).collect())
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
}
