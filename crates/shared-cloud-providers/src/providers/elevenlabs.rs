//! ElevenLabs provider: the public descriptor plus the private variance
//! of `GET /v1/models` - the `xi-api-key` header over a rich list
//! response (per-model languages, capability flags, character rates). No
//! pagination. The per-model languages collect into the entry;
//! character-cost rates have no sheet field and are dropped; the
//! capability flags select the entry kind.
//!
//! Docs: <https://elevenlabs.io/docs/api-reference/models/list>

use serde::Deserialize;
use shared_gateway_api::{EnvRole, ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::base_entry;
use crate::{EnvVarSpec, FetchError, Provider};

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
    openai_base_url: None,
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
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
    let mut entries: Vec<ModelEntry> = models.iter().map(normalize_model).collect();
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// One model as the wire reports it. The response is a bare array, not
/// an envelope; rates and fine-tuning flags are dropped.
#[derive(Debug, Deserialize)]
struct WireModel {
    model_id: String,
    name: Option<String>,
    can_do_text_to_speech: Option<bool>,
    #[serde(default)]
    languages: Vec<WireLanguage>,
}

/// One language as the wire reports it; only the code has sheet
/// meaning, the display name is dropped.
#[derive(Debug, Deserialize)]
struct WireLanguage {
    language_id: String,
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
    entry.languages = model
        .languages
        .iter()
        .map(|language| language.language_id.clone())
        .collect();
    entry
}

/// The entry's family: the id without its trailing `_v<version>` run
/// (`eleven_multilingual_v2` -> `eleven_multilingual`,
/// `eleven_turbo_v2_5` -> `eleven_turbo`), and the whole id when it
/// carries no version suffix.
fn family_of(id: &str) -> String {
    let segments: Vec<&str> = id.split('_').collect();
    for (index, segment) in segments.iter().enumerate().skip(1) {
        let versioned = segment.len() > 1
            && segment.starts_with('v')
            && segment[1..].bytes().all(|b| b.is_ascii_digit())
            && segments[index + 1..]
                .iter()
                .all(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()));
        if versioned {
            return segments[..index].join("_");
        }
    }
    id.to_owned()
}

/// Set every entry's family. ElevenLabs' catalog carries no snapshot
/// suffixes, so there is no collapse pass.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
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
            entry.languages.is_empty(),
            "absent languages default to empty"
        );
        assert!(
            entry.pricing.is_none(),
            "character rates are not token pricing"
        );
    }

    #[test]
    fn wire_languages_survive_normalization() {
        let entries = entries(LIST);
        assert_eq!(
            entries[0].languages,
            ["en", "ja"],
            "the per-model language ids collect into the entry"
        );
        assert_eq!(entries[1].languages, ["en"]);
    }

    #[test]
    fn family_drops_the_trailing_version_run() {
        let table: &[(&str, &str)] = &[
            ("eleven_multilingual_v2", "eleven_multilingual"),
            ("eleven_turbo_v2_5", "eleven_turbo"),
            ("eleven_flash_v2_5", "eleven_flash"),
            ("eleven_v3", "eleven"),
            ("scribe_v1", "scribe"),
            ("scribe_v2", "scribe"),
            ("eleven_legacy", "eleven_legacy"),
        ];
        for &(id, family) in table {
            assert_eq!(family_of(id), family, "{id}");
        }
    }

    #[test]
    fn taxonomy_sets_every_family() {
        let mut entries = entries(LIST);
        apply_taxonomy(&mut entries);
        assert_eq!(entries[0].family, "eleven_multilingual");
        assert_eq!(entries[1].family, "scribe");
    }

    #[test]
    fn descriptor_publishes_the_key_and_no_chat_base() {
        assert_eq!(PROVIDER.openai_base_url, None);
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, shared_gateway_api::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
