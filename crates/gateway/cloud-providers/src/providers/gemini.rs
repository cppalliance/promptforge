//! Gemini provider: the public descriptor plus the private variance of
//! `GET /v1beta/models` - `x-goog-api-key` header auth, `pageToken`
//! pagination, and normalization of `inputTokenLimit`, `outputTokenLimit`,
//! `supportedGenerationMethods`, and the `thinking` flag into
//! `ModelEntry`.
//!
//! Docs: <https://ai.google.dev/api/models>

use gateway_api::{EnvRole, ModelEntry, ModelKind, Thinking, Tier};
use serde::Deserialize;

use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "GEMINI_API_KEY";

/// The Gemini provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "gemini",
    display_name: "Google Gemini",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://generativelanguage.googleapis.com",
    openai_base_url: Some("https://generativelanguage.googleapis.com/v1beta/openai"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// Page size for the list request: generous, so the full catalog arrives
/// in one page today and pagination only engages as the lineup grows.
const PAGE_SIZE: u32 = 1000;

/// Fetch and normalize Gemini's model list, following `nextPageToken`
/// until the final page.
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
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = client
            .get(format!("{base_url}/v1beta/models"))
            .header("x-goog-api-key", key)
            .query(&[("pageSize", PAGE_SIZE)]);
        if let Some(page_token) = &token {
            request = request.query(&[("pageToken", page_token)]);
        }
        let page: Page = request.send().await?.error_for_status()?.json().await?;
        entries.extend(page.models.iter().map(normalize_model));
        let Some(next) = next_token(&page) else {
            break;
        };
        token = Some(next);
    }
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// One page of the list response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    models: Vec<WireModel>,
    next_page_token: Option<String>,
}

/// One model as the wire reports it. `displayName` and the token limits
/// are optional in the wire schema; the generation-method list and the
/// thinking flag default to empty and false.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireModel {
    name: String,
    display_name: Option<String>,
    input_token_limit: Option<u32>,
    output_token_limit: Option<u32>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
    #[serde(default)]
    thinking: bool,
}

/// The token for the next page. An absent or empty `nextPageToken` ends
/// traversal, so a malformed page can never loop the fetch forever.
fn next_token(page: &Page) -> Option<String> {
    page.next_page_token
        .clone()
        .filter(|token| !token.is_empty())
}

/// The upstream model slug: the wire `name` carries a `models/` prefix
/// that the sheet id drops.
fn model_id(name: &str) -> &str {
    name.strip_prefix("models/").unwrap_or(name)
}

/// The workload: an embedding-only method list means an embedding model;
/// everything else is chat.
fn model_kind(methods: &[String]) -> ModelKind {
    if methods.iter().any(|m| m == "embedContent")
        && !methods.iter().any(|m| m == "generateContent")
    {
        ModelKind::Embedding
    } else {
        ModelKind::Chat
    }
}

/// The entry's family: a product line when the id opens with one
/// (`gemma`, `veo`, `lyria`, `deep-research`, `nano-banana`,
/// `antigravity`, the embedding line), the `gemini-<version>` prefix for
/// the numbered line, `gemini` for unversioned gemini ids, and the first
/// segment otherwise.
fn family_of(id: &str) -> String {
    const LINES: &[&str] = &[
        "gemma",
        "veo",
        "lyria",
        "deep-research",
        "nano-banana",
        "antigravity",
    ];
    for line in LINES {
        if id == *line || id.starts_with(&format!("{line}-")) {
            return (*line).to_owned();
        }
    }
    if id == "embedding" || id.starts_with("embedding-") || id.starts_with("gemini-embedding") {
        return "embedding".to_owned();
    }
    if let Some(rest) = id.strip_prefix("gemini-") {
        let token = rest.split('-').next().unwrap_or(rest);
        if crate::taxonomy::is_version_token(token) {
            return format!("gemini-{token}");
        }
        return "gemini".to_owned();
    }
    id.split('-').next().unwrap_or(id).to_owned()
}

/// Set every entry's family, then collapse `-MM-YYYY` preview snapshots
/// onto their canonical entries.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    crate::taxonomy::collapse_variants(entries, |id| {
        crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::MonthYear)
    });
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let methods = &model.supported_generation_methods;
    let generates = methods.iter().any(|m| m == "generateContent");
    ModelEntry {
        id: model_id(&model.name).to_owned(),
        display_name: model
            .display_name
            .clone()
            .unwrap_or_else(|| model_id(&model.name).to_owned()),
        family: String::new(),
        variant_of: None,
        variant: None,
        languages: Vec::new(),
        kind: model_kind(methods),
        released_at: None,
        context_window: model.input_token_limit,
        max_output: model.output_token_limit,
        // The list endpoint reports no modality, citation, code-execution,
        // or structured-output flags.
        images: false,
        pdf_input: false,
        video_input: false,
        audio_input: false,
        citations: false,
        code_execution: false,
        structured_outputs: false,
        batch: methods.iter().any(|m| m == "batchGenerateContent"),
        // Curated: function calling is part of `generateContent`; the
        // endpoint reports no separate flag.
        tool_calling: generates,
        thinking: Thinking {
            supported: model.thinking,
            // The bare reasoning flag says nothing about budget modes
            // (the contract's normalization principle; see moonshot.rs).
            enabled: false,
            // The endpoint does not report model-chosen thinking depth.
            adaptive: false,
        },
        // The endpoint reports no effort levels, pricing, release date,
        // or deprecation status.
        effort_levels: Vec::new(),
        default_effort: None,
        pricing: None,
        deprecation: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First page of the documented example response shape: a fully
    /// capable thinking chat model, with `nextPageToken` set so
    /// pagination continues.
    const PAGE_1: &str = r#"{
  "models": [
    {
      "name": "models/gemini-2.5-pro",
      "version": "2.5",
      "displayName": "Gemini 2.5 Pro",
      "description": "Stable release of Gemini 2.5 Pro.",
      "inputTokenLimit": 1048576,
      "outputTokenLimit": 65536,
      "supportedGenerationMethods": [
        "generateContent",
        "countTokens",
        "createCachedContent",
        "batchGenerateContent"
      ],
      "temperature": 1,
      "topP": 0.95,
      "topK": 64,
      "maxTemperature": 2,
      "thinking": true
    }
  ],
  "nextPageToken": "page-2"
}"#;

    /// Second and final page: an embedding-only model with no thinking
    /// flag, exercising the generation-method to kind mapping.
    const PAGE_2: &str = r#"{
  "models": [
    {
      "name": "models/gemini-embedding-001",
      "version": "001",
      "displayName": "Gemini Embedding",
      "description": "Text embedding model.",
      "inputTokenLimit": 2048,
      "outputTokenLimit": 1,
      "supportedGenerationMethods": ["embedContent"]
    }
  ]
}"#;

    fn page(json: &str) -> Page {
        serde_json::from_str(json).expect("fixture must parse as a page")
    }

    fn only_model(json: &str) -> ModelEntry {
        let page = page(json);
        assert_eq!(page.models.len(), 1, "fixture holds exactly one model");
        normalize_model(&page.models[0])
    }

    #[test]
    fn normalizes_full_chat_entry() {
        let entry = only_model(PAGE_1);
        assert_eq!(entry.id, "gemini-2.5-pro", "the models/ prefix drops");
        assert_eq!(entry.display_name, "Gemini 2.5 Pro");
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(
            entry.context_window,
            Some(1_048_576),
            "inputTokenLimit maps to context_window"
        );
        assert_eq!(
            entry.max_output,
            Some(65_536),
            "outputTokenLimit maps to max_output"
        );
        assert!(
            entry.batch,
            "batchGenerateContent maps to the batch capability"
        );
        assert!(
            entry.tool_calling,
            "curated: function calling is part of generateContent"
        );
        assert!(
            entry.thinking.supported && !entry.thinking.enabled,
            "the thinking flag maps to supported only; the bare flag says \
             nothing about budget modes: {:?}",
            entry.thinking
        );
        assert!(
            !entry.thinking.adaptive,
            "the endpoint does not report model-chosen thinking depth"
        );
        assert!(!entry.images && !entry.pdf_input);
        assert!(!entry.video_input && !entry.audio_input);
        assert!(!entry.citations && !entry.code_execution && !entry.structured_outputs);
        assert_eq!(
            entry.released_at, None,
            "the endpoint reports no release date"
        );
        assert!(entry.effort_levels.is_empty());
        assert_eq!(entry.default_effort, None);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn embedding_only_methods_map_to_embedding_kind() {
        let entry = only_model(PAGE_2);
        assert_eq!(entry.id, "gemini-embedding-001");
        assert_eq!(
            entry.kind,
            ModelKind::Embedding,
            "embedContent without generateContent is an embedding model"
        );
        assert!(!entry.tool_calling, "no generateContent, no tool calling");
        assert!(!entry.batch, "no batchGenerateContent, no batch");
        assert!(
            !entry.thinking.supported && !entry.thinking.enabled,
            "an absent thinking flag normalizes to false"
        );
        assert_eq!(entry.context_window, Some(2048));
        assert_eq!(entry.max_output, Some(1));
    }

    #[test]
    fn missing_display_name_and_limits_fall_back_conservatively() {
        let entry = only_model(
            r#"{
              "models": [
                {
                  "name": "models/gemini-legacy",
                  "version": "1.0",
                  "supportedGenerationMethods": ["generateContent", "countTokens"]
                }
              ]
            }"#,
        );
        assert_eq!(
            entry.display_name, "gemini-legacy",
            "a missing displayName falls back to the stripped id"
        );
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.max_output, None);
        assert!(!entry.thinking.supported);
        assert!(!entry.batch);
        assert_eq!(entry.kind, ModelKind::Chat);
    }

    #[test]
    fn pagination_follows_next_page_token() {
        let first = page(PAGE_1);
        let token = next_token(&first).expect("a page with nextPageToken must yield a token");
        assert_eq!(token, "page-2", "the token passes through verbatim");
        let second = page(PAGE_2);
        assert_eq!(next_token(&second), None, "the final page ends traversal");
    }

    #[test]
    fn pagination_stops_on_empty_token() {
        let page = page(r#"{ "models": [], "nextPageToken": "" }"#);
        assert_eq!(
            next_token(&page),
            None,
            "an empty token must not loop the fetch forever"
        );
    }

    /// Trimmed 2026-09-14 sheet excerpt: the real Gemini ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-gemini.json");

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
    fn fixture_ids_classify_into_version_and_product_line_families() {
        let by_id = classified();
        let table: &[(&str, &str)] = &[
            ("gemini-2.5-flash", "gemini-2.5"),
            ("gemini-2.5-pro", "gemini-2.5"),
            ("gemini-3-flash-preview", "gemini-3"),
            ("gemini-3.1-pro-preview", "gemini-3.1"),
            ("gemini-3.8-flash", "gemini-3.8"),
            ("gemma-4-26b-a4b-it", "gemma"),
            ("gemma-4-31b-it", "gemma"),
            ("veo-3.1-generate-preview", "veo"),
            ("lyria-3-clip-preview", "lyria"),
            ("lyria-realtime-exp", "lyria"),
            ("deep-research-max-preview-04-2026", "deep-research"),
            ("gemini-embedding-001", "embedding"),
            ("gemini-embedding-2-preview", "embedding"),
            ("nano-banana-pro-preview", "nano-banana"),
            ("antigravity-preview-05-2026", "antigravity"),
            ("aqa", "aqa"),
            ("gemini-flash-latest", "gemini"),
            ("gemini-robotics-er-2-preview", "gemini"),
            ("gemini-omni-1.1-flash", "gemini"),
            ("gemini-2.5-computer-use-preview-10-2025", "gemini-2.5"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn month_year_snapshots_without_a_canonical_stay_canonical() {
        let by_id = classified();
        // The 2026-09-14 sheet carries `-MM-YYYY` preview snapshots but
        // not their base ids, so nothing collapses and every id keeps a
        // non-empty family.
        for entry in by_id.values() {
            assert!(
                entry.variant_of.is_none(),
                "{} must stay canonical: its base id is not in the list",
                entry.id
            );
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
        }
    }

    #[test]
    fn a_month_year_snapshot_collapses_onto_its_canonical() {
        let mut entries = vec![
            crate::taxonomy::fixture::entry("deep-research-pro-preview"),
            crate::taxonomy::fixture::entry("deep-research-pro-preview-12-2025"),
        ];
        apply_taxonomy(&mut entries);
        let variant = &entries[1];
        assert_eq!(
            variant.variant_of.as_deref(),
            Some("deep-research-pro-preview")
        );
        assert_eq!(variant.variant.as_deref(), Some("12-2025"));
        assert_eq!(
            variant.family, "deep-research",
            "the variant inherits the canonical's family"
        );
    }
}
