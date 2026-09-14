//! ElevenLabs provider: the public descriptor plus the private variance
//! of `GET /v1/models` - the `xi-api-key` header over a rich list
//! response (per-model languages, capability flags, character rates). No
//! pagination. Languages and character-cost rates have no sheet field and
//! are dropped; the capability flags select the entry kind.
//!
//! Docs: <https://elevenlabs.io/docs/api-reference/models/list>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::base_entry;
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "ELEVENLABS_API_KEY";

/// The ElevenLabs provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "elevenlabs",
    display_name: "ElevenLabs",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.elevenlabs.io",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// Fetch and normalize ElevenLabs' model list in a single request.
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
    let models: Vec<WireModel> = client
        .get(format!("{base_url}{MODELS_PATH}"))
        .header("xi-api-key", key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(models.iter().map(normalize_model).collect())
}

/// One model as the wire reports it. The response is a bare array, not
/// an envelope; languages, rates, and fine-tuning flags are dropped.
#[derive(Debug, Deserialize)]
struct WireModel {
    model_id: String,
    name: Option<String>,
    can_do_text_to_speech: Option<bool>,
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.model_id, None);
    if let Some(name) = &model.name {
        entry.display_name.clone_from(name);
    }
    entry.kind = if model.can_do_text_to_speech.unwrap_or(false) {
        ModelKind::Speech
    } else {
        ModelKind::Transcription
    };
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented example response shape: one TTS model and the
    /// Scribe STT model, with the fields normalization consumes.
    const LIST: &str = r#"[
  {
    "model_id": "eleven_multilingual_v2",
    "name": "Eleven Multilingual v2",
    "can_be_finetuned": true,
    "can_do_text_to_speech": true,
    "can_do_voice_conversion": true,
    "can_use_style": true,
    "can_use_speaker_boost": true,
    "serves_pro_voices": false,
    "token_cost_factor": 1.0,
    "description": "Cutting-edge multilingual model",
    "requires_alpha_access": false,
    "max_characters_request_free_user": 2500,
    "max_characters_request_subscribed_user": 5000,
    "maximum_text_length_per_request": 5000,
    "languages": [
      { "language_id": "en", "name": "English" },
      { "language_id": "ja", "name": "Japanese" }
    ],
    "model_rates": { "character_cost_multiplier": 1.0 },
    "concurrency_group": "standard"
  },
  {
    "model_id": "scribe_v1",
    "name": "Scribe v1",
    "can_be_finetuned": false,
    "can_do_text_to_speech": false,
    "can_do_voice_conversion": false,
    "can_use_style": false,
    "can_use_speaker_boost": false,
    "serves_pro_voices": false,
    "token_cost_factor": 1.0,
    "description": "Speech-to-text model",
    "requires_alpha_access": false,
    "max_characters_request_free_user": 0,
    "max_characters_request_subscribed_user": 0,
    "maximum_text_length_per_request": 0,
    "languages": [
      { "language_id": "en", "name": "English" }
    ],
    "model_rates": { "character_cost_multiplier": 1.0 },
    "concurrency_group": "standard"
  }
]"#;

    fn entries(json: &str) -> Vec<ModelEntry> {
        let models: Vec<WireModel> =
            serde_json::from_str(json).expect("fixture must parse as a list");
        models.iter().map(normalize_model).collect()
    }

    #[test]
    fn tts_flag_selects_speech_kind() {
        let entries = entries(LIST);
        let entry = &entries[0];
        assert_eq!(entry.id, "eleven_multilingual_v2");
        assert_eq!(
            entry.display_name, "Eleven Multilingual v2",
            "the wire name is the display name"
        );
        assert_eq!(
            entry.kind,
            ModelKind::Speech,
            "can_do_text_to_speech maps to the speech kind"
        );
    }

    #[test]
    fn absent_tts_flag_selects_transcription_kind() {
        let entries = entries(LIST);
        let entry = &entries[1];
        assert_eq!(entry.id, "scribe_v1");
        assert_eq!(
            entry.kind,
            ModelKind::Transcription,
            "Scribe cannot do TTS, so it is a transcription model"
        );
    }

    #[test]
    fn missing_flags_and_name_are_conservative() {
        let entries = entries(r#"[{ "model_id": "eleven_legacy" }]"#);
        let entry = &entries[0];
        assert_eq!(
            entry.display_name, "eleven_legacy",
            "the id doubles as the display name"
        );
        assert_eq!(entry.kind, ModelKind::Transcription);
        assert!(!entry.images && !entry.tool_calling);
        assert!(
            entry.pricing.is_none(),
            "character rates are not token pricing"
        );
    }
}
