//! Deepgram provider: the public descriptor plus the private variance of
//! `GET /v1/models` - the `Authorization: Token` prefix over one payload
//! that carries STT models and a TTS array, split here into separate
//! entries with distinct kinds. No pagination. Languages, architectures,
//! and tags have no sheet field and are dropped.
//!
//! Docs: <https://developers.deepgram.com/reference/get-models>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::base_entry;
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "DEEPGRAM_API_KEY";

/// The Deepgram provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "deepgram",
    display_name: "Deepgram",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.deepgram.com",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// Fetch and normalize Deepgram's model list in a single request.
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
        .header("Authorization", format!("Token {key}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(normalize_list(&response))
}

/// The list envelope: STT models and TTS models in separate arrays.
#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    stt: Vec<WireStt>,
    #[serde(default)]
    tts: Vec<WireTts>,
}

/// One STT model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireStt {
    name: String,
    canonical_name: String,
    batch: Option<bool>,
}

/// One TTS model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireTts {
    name: String,
    canonical_name: String,
}

/// Split one payload into STT and TTS entries with distinct kinds.
fn normalize_list(response: &ListResponse) -> Vec<ModelEntry> {
    let stt = response.stt.iter().map(|model| {
        let mut entry = base_entry(&model.canonical_name, None);
        entry.display_name.clone_from(&model.name);
        entry.kind = ModelKind::Transcription;
        entry.batch = model.batch.unwrap_or(false);
        entry
    });
    let tts = response.tts.iter().map(|model| {
        let mut entry = base_entry(&model.canonical_name, None);
        entry.display_name.clone_from(&model.name);
        entry.kind = ModelKind::Speech;
        entry
    });
    stt.chain(tts).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented example response shape: one Nova STT model with
    /// batch and streaming flags, and one Aura TTS model with tags.
    const LIST: &str = r#"{
  "stt": [
    {
      "name": "Nova-3 General",
      "canonical_name": "nova-3-general",
      "architecture": "nova-3",
      "languages": ["en", "en-US", "es"],
      "version": "2024-01-01.0",
      "uuid": "1b1b1b1b-0000-0000-0000-000000000000",
      "batch": true,
      "streaming": true,
      "formatted_output": true
    }
  ],
  "tts": [
    {
      "name": "Aura-2 Thalia English",
      "canonical_name": "aura-2-thalia-en",
      "architecture": "aura-2",
      "languages": ["en"],
      "version": "2025-04-01.0",
      "uuid": "2c2c2c2c-0000-0000-0000-000000000000",
      "tags": ["general", "female"]
    }
  ]
}"#;

    fn entries(json: &str) -> Vec<ModelEntry> {
        let response: ListResponse =
            serde_json::from_str(json).expect("fixture must parse as a list");
        normalize_list(&response)
    }

    #[test]
    fn one_payload_splits_into_distinct_kinds() {
        let entries = entries(LIST);
        assert_eq!(entries.len(), 2, "STT and TTS arrays both contribute");
        let stt = &entries[0];
        assert_eq!(stt.id, "nova-3-general");
        assert_eq!(stt.display_name, "Nova-3 General");
        assert_eq!(stt.kind, ModelKind::Transcription);
        let tts = &entries[1];
        assert_eq!(tts.id, "aura-2-thalia-en");
        assert_eq!(tts.display_name, "Aura-2 Thalia English");
        assert_eq!(tts.kind, ModelKind::Speech);
    }

    #[test]
    fn stt_batch_flag_maps_to_batch_capability() {
        let entries = entries(LIST);
        assert!(entries[0].batch, "the wire batch flag maps to batch");
        assert!(!entries[1].batch, "TTS entries report no batch capability");
    }

    #[test]
    fn empty_arrays_yield_no_entries() {
        let result = entries(r#"{ "stt": [], "tts": [] }"#);
        assert!(result.is_empty());
        let result = entries(r"{}");
        assert!(result.is_empty(), "absent arrays default to empty");
    }
}
