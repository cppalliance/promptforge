//! Alibaba Qwen provider: the public descriptor plus the private variance
//! of the DashScope compatible-mode endpoint - `GET /models` under
//! `https://dashscope-intl.aliyuncs.com/compatible-mode/v1`, Bearer auth, and
//! the plain OpenAI response shape. The native `/api/v1/models` endpoint
//! adds pagination, pricing, and context length; the compatible-mode
//! endpoint is IDs-only, so every entry is the conservative base entry.
//!
//! Docs: <https://www.alibabacloud.com/help/en/model-studio>

use serde::Deserialize;
use shared_gateway_api::{EnvRole, ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "DASHSCOPE_API_KEY";

/// The Alibaba Qwen provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "qwen",
    display_name: "Alibaba Qwen",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    openai_base_url: Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize Qwen's model list in a single request.
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

/// Normalize one wire model: the compatible-mode endpoint is IDs-only,
/// so the entry is the conservative base plus the name-based kind.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    entry.kind = kind_of(&model.id);
    entry
}

/// The workload, inferred from the name: the compatible-mode endpoint
/// reports no kinds. Segment matches keep `tts` and `asr` from matching
/// inside unrelated words.
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

/// The entry's family: a product line for the known line prefixes
/// (checked first - `qwen3-tts` and `qwen3-asr` would otherwise read as
/// the `qwen3` version prefix), the vendor for resold models (the
/// slash-namespaced form lowercased, or a bare vendor prefix), the
/// `qwen<version>` prefix for the numbered lines, and the whole id
/// otherwise.
fn family_of(id: &str) -> String {
    const LINES: &[&str] = &[
        "qwen3-tts",
        "qwen3-asr",
        "qwen-image",
        "qwen-mt",
        "qwen-audio",
    ];
    for line in LINES {
        if id == *line || id.starts_with(&format!("{line}-")) {
            return (*line).to_owned();
        }
    }
    if let Some((vendor, _)) = crate::taxonomy::vendor_prefix(id) {
        return vendor.to_ascii_lowercase();
    }
    for vendor in ["deepseek", "kimi", "glm", "wan"] {
        if id.starts_with(vendor) {
            return (*vendor).to_owned();
        }
    }
    if let Some(rest) = id.strip_prefix("qwen") {
        let token: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if token.bytes().any(|b| b.is_ascii_digit()) {
            return format!("qwen{token}");
        }
    }
    id.to_owned()
}

/// Set every entry's family, then collapse dated snapshots onto their
/// canonical entries. DashScope uses `-YYYY-MM-DD`, `-MMDD`, and
/// `-YYMM` suffixes; the four-digit ambiguity resolves as month-day
/// first, then year-month.
fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    crate::taxonomy::collapse_variants(entries, |id| {
        crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::DashedDate)
            .or_else(|| {
                crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::MonthDay)
            })
            .or_else(|| {
                crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::YearMonth)
            })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape from the compatible-mode
    /// endpoint.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "qwen3-max",
      "object": "model",
      "created": 1782864000,
      "owned_by": "alibaba"
    },
    {
      "id": "qwen3-coder-plus",
      "object": "model",
      "created": 1782864000,
      "owned_by": "alibaba"
    }
  ]
}"#;

    #[test]
    fn ids_only_entries_are_conservative() {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        let entries: Vec<ModelEntry> = page.data.iter().map(normalize_model).collect();
        assert_eq!(entries.len(), 2, "both listed models normalize");
        let entry = &entries[0];
        assert_eq!(entry.id, "qwen3-max");
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
    }

    /// Trimmed 2026-09-14 sheet excerpt: representative real DashScope
    /// ids across the version prefixes, product lines, resold vendors,
    /// and snapshot styles.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-qwen.json");

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
    fn fixture_ids_classify_into_version_line_and_vendor_families() {
        let by_id = classified();
        let table: &[(&str, &str)] = &[
            ("qwen3-max", "qwen3"),
            ("qwen3-14b", "qwen3"),
            ("qwen3-coder-plus", "qwen3"),
            ("qwen3.5-flash", "qwen3.5"),
            ("qwen3.6-flash", "qwen3.6"),
            ("qwen3.7-max", "qwen3.7"),
            ("qwen3.8-max", "qwen3.8"),
            ("qwen3.8-2.4t-a95b", "qwen3.8"),
            ("qwen2-7b-instruct", "qwen2"),
            ("qwen3-tts-flash", "qwen3-tts"),
            ("qwen3-asr-flash-realtime", "qwen3-asr"),
            ("qwen-image-2.0", "qwen-image"),
            ("qwen-image-edit", "qwen-image"),
            ("qwen-mt-flash", "qwen-mt"),
            ("qwen-audio-3.0-asr-flash", "qwen-audio"),
            ("deepseek-v3.2", "deepseek"),
            ("deepseek-v4-flash", "deepseek"),
            ("glm-5.1", "glm"),
            ("glm-5.2-fast-preview", "glm"),
            ("kimi-k2.7-code", "kimi"),
            ("kimi/kimi-k3", "kimi"),
            ("ZHIPU/GLM-5.3", "zhipu"),
            ("wan2.7-image", "wan"),
            ("wan2.7-image-pro", "wan"),
            ("qwen-max", "qwen-max"),
            ("qwen-plus", "qwen-plus"),
            ("qwen-vl-max", "qwen-vl-max"),
            ("qwq-plus", "qwq-plus"),
            ("text-embedding-v4", "text-embedding-v4"),
            ("tongyi-tingwu-slp", "tongyi-tingwu-slp"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn dated_snapshots_collapse_onto_present_aliases() {
        let by_id = classified();
        let table: &[(&str, &str, &str)] = &[
            ("qwen-plus-2025-01-25", "qwen-plus", "2025-01-25"),
            (
                "qwen3-coder-plus-2025-07-22",
                "qwen3-coder-plus",
                "2025-07-22",
            ),
            ("qwen3-max-2025-09-23", "qwen3-max", "2025-09-23"),
            ("qwen-image-2.0-2026-03-03", "qwen-image-2.0", "2026-03-03"),
            ("qwen3.5-plus-2026-02-15", "qwen3.5-plus", "2026-02-15"),
            ("qwen3.7-max-2026-05-17", "qwen3.7-max", "2026-05-17"),
            (
                "qwen3-tts-flash-2025-09-18",
                "qwen3-tts-flash",
                "2025-09-18",
            ),
            ("qwq-plus-2025-03-05", "qwq-plus", "2025-03-05"),
            ("deepseek-v4-flash-0731", "deepseek-v4-flash", "0731"),
            ("deepseek-v4-pro-0813", "deepseek-v4-pro", "0813"),
            ("qwen3.8-max-0902", "qwen3.8-max", "0902"),
        ];
        for &(id, base, suffix) in table {
            let entry = &by_id[id];
            assert_eq!(entry.variant_of.as_deref(), Some(base), "{id}");
            assert_eq!(entry.variant.as_deref(), Some(suffix), "{id}");
            assert_eq!(
                entry.family, by_id[base].family,
                "{id} inherits its canonical's family"
            );
        }
    }

    #[test]
    fn snapshots_without_a_canonical_stay_canonical() {
        let by_id = classified();
        // `-2507` reads as `-YYMM`; the base id is absent from the list.
        assert!(
            by_id["qwen3-235b-a22b-instruct-2507"].variant_of.is_none(),
            "the base id is not in the list"
        );
        // `qwen-vl-ocr-2025-11-20` has no `qwen-vl-ocr` alias.
        assert!(by_id["qwen-vl-ocr-2025-11-20"].variant_of.is_none());
        for entry in by_id.values() {
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
        }
    }

    #[test]
    fn name_patterns_infer_the_kind() {
        let table: &[(&str, shared_gateway_api::ModelKind)] = &[
            ("qwen3-tts-flash", shared_gateway_api::ModelKind::Speech),
            (
                "qwen3-asr-flash-realtime",
                shared_gateway_api::ModelKind::Transcription,
            ),
            (
                "qwen-audio-3.0-asr-flash",
                shared_gateway_api::ModelKind::Transcription,
            ),
            ("qwen-image-2.0", shared_gateway_api::ModelKind::Image),
            ("wan2.7-image", shared_gateway_api::ModelKind::Image),
            ("z-image-turbo", shared_gateway_api::ModelKind::Image),
            (
                "text-embedding-v4",
                shared_gateway_api::ModelKind::Embedding,
            ),
            (
                "qwen3.7-text-embedding",
                shared_gateway_api::ModelKind::Embedding,
            ),
            ("qwen3-max", shared_gateway_api::ModelKind::Chat),
            (
                "qwen-vl-ocr-2025-11-20",
                shared_gateway_api::ModelKind::Chat,
            ),
            ("qwen-mt-flash", shared_gateway_api::ModelKind::Chat),
        ];
        for &(id, kind) in table {
            assert_eq!(kind_of(id), kind, "{id}");
        }
    }

    #[test]
    fn normalization_applies_the_inferred_kind() {
        let page: ListResponse<WireModel> = serde_json::from_str(
            r#"{
              "object": "list",
              "data": [
                { "id": "qwen3-tts-flash", "object": "model", "created": null,
                  "owned_by": "alibaba" }
              ]
            }"#,
        )
        .expect("fixture must parse as a list");
        let entry = normalize_model(&page.data[0]);
        assert_eq!(
            entry.kind,
            shared_gateway_api::ModelKind::Speech,
            "the compatible-mode endpoint reports no kind; the name rule supplies it"
        );
    }

    #[test]
    fn descriptor_publishes_the_compatible_mode_base_and_key() {
        assert_eq!(
            PROVIDER.openai_base_url,
            Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1")
        );
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, shared_gateway_api::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
