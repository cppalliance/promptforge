//! OpenAI provider: the public descriptor plus the private variance of
//! `GET /v1/models` - Bearer auth and the IDs-only response shape (`id`,
//! `created`, `owned_by`): no pagination, no token limits, and no
//! capability reporting, so every entry is the conservative base entry.
//!
//! Docs: <https://developers.openai.com/api/reference>

use gateway_api::{EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "OPENAI_API_KEY";

/// The OpenAI provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "openai",
    display_name: "OpenAI",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.openai.com/v1",
    openai_base_url: Some("https://api.openai.com/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetches and normalizes OpenAI's model list in a single request.
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

/// One model as the wire reports it; `owned_by` is carried on the wire
/// but has no sheet field.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
}

/// Normalizes one wire model: the endpoint reports no capabilities, so
/// the entry is the conservative base plus the name-based kind.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    entry.kind = kind_of(&model.id);
    entry
}

/// The workload, inferred from the name: the endpoint reports no kinds.
/// Segment matches keep `tts` and `audio` from matching inside unrelated
/// words.
fn kind_of(id: &str) -> ModelKind {
    let segments: Vec<&str> = id.split('-').collect();
    let has = |names: &[&str]| segments.iter().any(|segment| names.contains(segment));
    if has(&["whisper", "transcribe"]) {
        ModelKind::Transcription
    } else if has(&["tts", "audio"]) {
        ModelKind::Speech
    } else if has(&["embedding"]) {
        ModelKind::Embedding
    } else if has(&["image"]) || id.contains("dall-e") {
        ModelKind::Image
    } else if has(&["sora"]) {
        ModelKind::Video
    } else if has(&["moderation"]) {
        ModelKind::Classifier
    } else {
        ModelKind::Chat
    }
}

/// The entry's family: `o-series` for the reasoning line, the
/// `gpt-<version>` prefix for the numbered lines, a product line when
/// the id opens with one, and the whole id otherwise.
fn family_of(id: &str) -> String {
    const LINES: &[(&str, &str)] = &[
        ("gpt-image", "image"),
        ("chatgpt-image", "image"),
        ("dall-e", "image"),
        ("gpt-realtime", "realtime"),
        ("gpt-audio", "audio"),
        ("gpt-transcribe", "transcribe"),
        ("gpt-live", "live"),
        ("tts", "tts"),
        ("whisper", "whisper"),
        ("text-embedding", "embedding"),
        ("sora", "sora"),
        ("omni-moderation", "moderation"),
        ("davinci", "legacy"),
        ("babbage", "legacy"),
    ];
    let mut chars = id.chars();
    if chars.next() == Some('o') && chars.next().is_some_and(|c| c.is_ascii_digit()) {
        return "o-series".to_owned();
    }
    if let Some(rest) = id.strip_prefix("gpt-") {
        let token = rest.split('-').next().unwrap_or(rest);
        if crate::taxonomy::is_version_token_o(token) {
            return format!("gpt-{token}");
        }
    }
    for (prefix, family) in LINES {
        if id == *prefix || id.starts_with(&format!("{prefix}-")) {
            return (*family).to_owned();
        }
    }
    id.to_owned()
}

/// Sets every entry's family, then collapses `-YYYY-MM-DD` and `-MMDD`
/// dated snapshots onto their canonical entries.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    crate::taxonomy::collapse_variants(entries, |id| {
        crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::DashedDate).or_else(
            || crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::MonthDay),
        )
    });
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape: two IDs-only entries.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "gpt-5.2",
      "object": "model",
      "created": 1782864000,
      "owned_by": "openai"
    },
    {
      "id": "gpt-5-mini",
      "object": "model",
      "created": 1782864000,
      "owned_by": "openai"
    }
  ]
}"#;

    fn entries(json: &str) -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(json).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn ids_only_entries_are_conservative() {
        let entries = entries(LIST);
        assert_eq!(entries.len(), 2, "both listed models normalize");
        let entry = &entries[0];
        assert_eq!(entry.id, "gpt-5.2");
        assert_eq!(
            entry.display_name, "gpt-5.2",
            "the id doubles as the display name"
        );
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.effort_levels.is_empty());
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn created_maps_to_released_at() {
        let entries = entries(LIST);
        assert_eq!(
            entries[0].released_at,
            Date::from_calendar_date(2026, Month::July, 1).ok(),
            "the unix `created` timestamp maps to a calendar date"
        );
    }

    /// Trimmed 2026-09-14 sheet excerpt: the real OpenAI ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-openai.json");

    /// The fixture ids with the provider's taxonomy applied, by id.
    fn classified() -> std::collections::BTreeMap<String, ModelEntry> {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply_taxonomy(&mut entries);
        entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect()
    }

    #[test]
    fn fixture_ids_map_to_kinds() {
        let table: &[(&str, ModelKind)] = &[
            ("whisper-1", ModelKind::Transcription),
            ("gpt-4o-transcribe", ModelKind::Transcription),
            ("gpt-transcribe", ModelKind::Transcription),
            ("tts-1", ModelKind::Speech),
            ("gpt-4o-mini-tts", ModelKind::Speech),
            ("gpt-audio", ModelKind::Speech),
            ("text-embedding-3-small", ModelKind::Embedding),
            ("text-embedding-ada-002", ModelKind::Embedding),
            ("gpt-image-1", ModelKind::Image),
            ("chatgpt-image-latest", ModelKind::Image),
            ("sora-2", ModelKind::Video),
            ("omni-moderation-latest", ModelKind::Classifier),
            ("gpt-5.4", ModelKind::Chat),
            ("o3", ModelKind::Chat),
            ("gpt-realtime", ModelKind::Chat),
            ("davinci-002", ModelKind::Chat),
        ];
        for &(id, kind) in table {
            assert_eq!(kind_of(id), kind, "{id}");
        }
    }

    #[test]
    fn fixture_ids_classify_into_families_and_variants() {
        let by_id = classified();
        // (id, family, variant_of, variant)
        let table: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            ("gpt-5.4", "gpt-5.4", None, None),
            (
                "gpt-5.4-2026-03-05",
                "gpt-5.4",
                Some("gpt-5.4"),
                Some("2026-03-05"),
            ),
            (
                "gpt-5.4-pro-2026-03-05",
                "gpt-5.4",
                Some("gpt-5.4-pro"),
                Some("2026-03-05"),
            ),
            ("gpt-4o", "gpt-4o", None, None),
            (
                "gpt-4o-2024-08-06",
                "gpt-4o",
                Some("gpt-4o"),
                Some("2024-08-06"),
            ),
            (
                "gpt-4o-mini-search-preview-2025-03-11",
                "gpt-4o",
                Some("gpt-4o-mini-search-preview"),
                Some("2025-03-11"),
            ),
            ("o1", "o-series", None, None),
            ("o1-2024-12-17", "o-series", Some("o1"), Some("2024-12-17")),
            (
                "o3-mini-2025-01-31",
                "o-series",
                Some("o3-mini"),
                Some("2025-01-31"),
            ),
            ("gpt-3.5-turbo", "gpt-3.5", None, None),
            (
                "gpt-3.5-turbo-0125",
                "gpt-3.5",
                Some("gpt-3.5-turbo"),
                Some("0125"),
            ),
            // `16k` is a size suffix, not a date: no collapse.
            ("gpt-3.5-turbo-16k", "gpt-3.5", None, None),
            (
                "gpt-3.5-turbo-instruct-0914",
                "gpt-3.5",
                Some("gpt-3.5-turbo-instruct"),
                Some("0914"),
            ),
            ("tts-1-1106", "tts", Some("tts-1"), Some("1106")),
            ("tts-1-hd-1106", "tts", Some("tts-1-hd"), Some("1106")),
            ("whisper-1", "whisper", None, None),
            ("text-embedding-3-small", "embedding", None, None),
            ("gpt-image-1", "image", None, None),
            (
                "gpt-image-2.5-flare-2026-09-08",
                "image",
                Some("gpt-image-2.5-flare"),
                Some("2026-09-08"),
            ),
            ("sora-2", "sora", None, None),
            // The base id `omni-moderation` is absent: no collapse.
            ("omni-moderation-2024-09-26", "moderation", None, None),
            (
                "gpt-realtime-2025-08-28",
                "realtime",
                Some("gpt-realtime"),
                Some("2025-08-28"),
            ),
            (
                "gpt-audio-mini-2025-10-06",
                "audio",
                Some("gpt-audio-mini"),
                Some("2025-10-06"),
            ),
            ("gpt-6-astra", "gpt-6", None, None),
            ("gpt-5.6-sol", "gpt-5.6", None, None),
            ("davinci-002", "legacy", None, None),
            ("chatgpt-image-latest", "image", None, None),
            ("chat-latest", "chat-latest", None, None),
        ];
        for &(id, family, variant_of, variant) in table {
            let entry = &by_id[id];
            assert_eq!(entry.family, family, "{id} family");
            assert_eq!(entry.variant_of.as_deref(), variant_of, "{id} variant_of");
            assert_eq!(entry.variant.as_deref(), variant, "{id} variant");
        }
    }

    #[test]
    fn every_fixture_id_has_a_family_and_every_variant_pointer_resolves() {
        let by_id = classified();
        for entry in by_id.values() {
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
            if let Some(base) = &entry.variant_of {
                assert!(
                    by_id.contains_key(base),
                    "{} points at {base}, which is not in the list",
                    entry.id
                );
            }
        }
    }
}
