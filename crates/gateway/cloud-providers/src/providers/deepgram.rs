//! Deepgram provider: the public descriptor plus the private variance of
//! `GET /v1/models` - the `Authorization: Token` prefix over one payload
//! that contains STT models and a TTS array, split here into separate
//! entries with distinct kinds. No pagination. The wire repeats each
//! model once per language: normalization groups rows by id and collects
//! the languages. Architectures and tags have no sheet field and are
//! dropped.
//!
//! Docs: <https://developers.deepgram.com/reference/get-models>

use gateway_api_types::{EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::base_entry;
use crate::{EnvVarSpec, FetchError, Provider};

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
    openai_base_url: None,
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// Fetches and normalizes Deepgram's model list in a single request.
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
    let mut entries = normalize_list(&response);
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// The list envelope: STT models and TTS models in separate arrays.
#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    stt: Vec<WireStt>,
    #[serde(default)]
    tts: Vec<WireTts>,
}

/// One STT model as the wire reports it. The wire repeats each model
/// once per language; the row's languages are retained for the grouping
/// pass.
#[derive(Debug, Deserialize)]
struct WireStt {
    name: String,
    canonical_name: String,
    batch: Option<bool>,
    #[serde(default)]
    languages: Vec<String>,
}

/// One TTS model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireTts {
    name: String,
    canonical_name: String,
    #[serde(default)]
    languages: Vec<String>,
}

/// Splits one payload into STT and TTS entries with distinct kinds, one
/// entry per distinct canonical name: the wire repeats each model once
/// per language, so rows group by id and their languages collect in
/// first-seen order.
fn normalize_list(response: &ListResponse) -> Vec<ModelEntry> {
    let mut entries: Vec<ModelEntry> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for model in &response.stt {
        let mut entry = base_entry(&model.canonical_name, None);
        entry.display_name.clone_from(&model.name);
        entry.kind = ModelKind::Transcription;
        entry.batch = model.batch.unwrap_or(false);
        absorb(&mut entries, &mut index, entry, &model.languages);
    }
    for model in &response.tts {
        let mut entry = base_entry(&model.canonical_name, None);
        entry.display_name.clone_from(&model.name);
        entry.kind = ModelKind::Speech;
        absorb(&mut entries, &mut index, entry, &model.languages);
    }
    entries
}

/// Merges one row into the grouped list: the first row for an id pushes
/// the entry; later rows for the same id contribute only the languages
/// the entry does not already list.
fn absorb(
    entries: &mut Vec<ModelEntry>,
    index: &mut std::collections::HashMap<String, usize>,
    base: ModelEntry,
    languages: &[String],
) {
    let slot = if let Some(&slot) = index.get(&base.id) {
        slot
    } else {
        let slot = entries.len();
        index.insert(base.id.clone(), slot);
        entries.push(base);
        slot
    };
    let entry = &mut entries[slot];
    for language in languages {
        if !entry.languages.contains(language) {
            entry.languages.push(language.clone());
        }
    }
}

/// The entry's family: the model line for a known line prefix
/// (`nova-3`, `nova-2`, `enhanced`, `base`, `phoneme` for STT; `aura-2`,
/// `aura` for TTS; the resold `flux` and `whisper` lines), and the
/// whole id otherwise - the legacy unprefixed tiers (`general`,
/// `meeting`, ...) are their own families.
fn family_of(id: &str) -> String {
    const LINES: &[&str] = &[
        "nova-3", "nova-2", "enhanced", "base", "phoneme", "aura-2", "aura", "flux", "whisper",
    ];
    for line in LINES {
        if id == *line || id.starts_with(&format!("{line}-")) {
            return (*line).to_owned();
        }
    }
    id.to_owned()
}

/// Sets every entry's family. Deepgram's catalog has no snapshot
/// suffixes, so there is no collapse pass.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
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

    /// The 2026-09-14 sheet shape reduced: the wire repeats each model
    /// once per language, and normalization emits one entry per
    /// distinct id with the languages collected.
    const DEDUP: &str = r#"{
  "stt": [
    {
      "name": "Nova-3 General",
      "canonical_name": "nova-3-general",
      "languages": ["en"],
      "batch": true,
      "streaming": true
    },
    {
      "name": "Nova-3 General",
      "canonical_name": "nova-3-general",
      "languages": ["es"],
      "batch": true,
      "streaming": true
    },
    {
      "name": "Nova-3 General",
      "canonical_name": "nova-3-general",
      "languages": ["en", "de"],
      "batch": true,
      "streaming": true
    },
    {
      "name": "Nova-2 General",
      "canonical_name": "nova-2-general",
      "languages": ["en"],
      "batch": true,
      "streaming": true
    }
  ],
  "tts": [
    {
      "name": "Aura-2 Thalia English",
      "canonical_name": "aura-2-thalia-en",
      "languages": ["en"]
    },
    {
      "name": "Aura-2 Thalia English",
      "canonical_name": "aura-2-thalia-en",
      "languages": ["en"]
    }
  ]
}"#;

    #[test]
    fn rows_group_by_id_and_collect_languages() {
        let entries = entries(DEDUP);
        assert_eq!(
            entries.len(),
            3,
            "six rows over three distinct ids emit three entries"
        );
        assert_eq!(entries[0].id, "nova-3-general");
        assert_eq!(
            entries[0].languages,
            ["en", "es", "de"],
            "languages collect in first-seen order, deduplicated"
        );
        assert_eq!(entries[1].id, "nova-2-general");
        assert_eq!(entries[1].languages, ["en"]);
        assert_eq!(entries[2].id, "aura-2-thalia-en");
        assert_eq!(
            entries[2].languages,
            ["en"],
            "a repeated row never duplicates a language"
        );
        assert_eq!(entries[0].kind, ModelKind::Transcription);
        assert_eq!(entries[2].kind, ModelKind::Speech);
        assert!(entries[0].batch, "the first row's flags survive grouping");
    }

    #[test]
    fn wire_languages_survive_normalization() {
        let entries = entries(LIST);
        assert_eq!(entries[0].languages, ["en", "en-US", "es"]);
        assert_eq!(entries[1].languages, ["en"]);
    }

    /// Trimmed 2026-09-14 sheet excerpt: representative real Deepgram
    /// ids across the STT lines, the TTS lines, and the resold models.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-deepgram.json");

    #[test]
    fn fixture_ids_classify_into_model_line_families() {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply_taxonomy(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[
            ("nova-3-general", "nova-3"),
            ("nova-3-medical", "nova-3"),
            ("nova-2-conversationalai", "nova-2"),
            ("enhanced-meeting", "enhanced"),
            ("base-video", "base"),
            ("phoneme-general", "phoneme"),
            ("whisper-large", "whisper"),
            ("flux-general", "flux"),
            ("aura-2-thalia-en", "aura-2"),
            ("aura-asteria-en", "aura"),
            ("general", "general"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
        for entry in by_id.values() {
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
            assert!(
                entry.variant_of.is_none(),
                "the catalog carries no snapshot suffixes: {}",
                entry.id
            );
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
