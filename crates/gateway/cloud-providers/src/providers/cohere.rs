//! Cohere provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.cohere.com` - Bearer auth,
//! `page_token` pagination, and normalization of `context_length`,
//! `endpoints`, `features`, and `is_deprecated` into `ModelEntry`. No
//! pricing, no max-output field, no release date; `tokenizer_url`,
//! `finetuned`, `default_endpoints`, and `sampling_defaults` are
//! ignored.
//!
//! Docs: <https://docs.cohere.com/reference/list-models>

use gateway_api_types::{Deprecation, EnvRole, ModelEntry, ModelKind, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::base_entry;
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "COHERE_API_KEY";

/// The Cohere provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "cohere",
    display_name: "Cohere",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.cohere.com",
    openai_base_url: Some("https://api.cohere.com/compatibility/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// Page size for the list request: the endpoint maximum, so the full
/// catalog arrives in one page today and pagination only engages as the
/// lineup grows.
const PAGE_SIZE: u32 = 1000;

/// Fetches and normalizes Cohere's model list, following `next_page_token`
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
            .get(format!("{base_url}/v1/models"))
            .bearer_auth(key)
            .query(&[("page_size", PAGE_SIZE)]);
        if let Some(page_token) = &token {
            request = request.query(&[("page_token", page_token)]);
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
struct Page {
    models: Vec<WireModel>,
    next_page_token: Option<String>,
}

/// One model as the wire reports it; every field is optional in the
/// wire schema.
#[derive(Debug, Deserialize)]
struct WireModel {
    name: String,
    is_deprecated: Option<bool>,
    endpoints: Option<Vec<String>>,
    context_length: Option<f64>,
    features: Option<Vec<String>>,
}

/// The token for the next page. An absent or empty `next_page_token`
/// ends traversal, so a malformed page can never loop the fetch
/// forever.
fn next_token(page: &Page) -> Option<String> {
    page.next_page_token
        .clone()
        .filter(|token| !token.is_empty())
}

/// The workload: an endpoint list without `chat` is an embedding model
/// for `embed`, a classifier for `rerank`/`classify`; everything else
/// is chat.
fn model_kind(endpoints: &[String]) -> ModelKind {
    if endpoints.iter().any(|e| e == "chat") {
        ModelKind::Chat
    } else if endpoints.iter().any(|e| e == "embed") {
        ModelKind::Embedding
    } else if endpoints.iter().any(|e| e == "rerank" || e == "classify") {
        ModelKind::Classifier
    } else {
        ModelKind::Chat
    }
}

/// Normalizes one wire model into a sheet entry.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the wire reports context_length as a double; the sheet field is a whole-token count"
)]
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.name, None);
    let endpoints = model.endpoints.as_deref().unwrap_or_default();
    let features = model.features.as_deref().unwrap_or_default();
    entry.kind = model_kind(endpoints);
    entry.context_window = model.context_length.map(|length| length.round() as u32);
    // The known feature flags; the rest of the free-form list is ignored.
    entry.tool_calling = features.iter().any(|f| f == "tools");
    entry.images = features.iter().any(|f| f == "vision");
    if model.is_deprecated.unwrap_or(false) {
        entry.deprecation = Some(Deprecation {
            status: "deprecated".to_owned(),
            date: None,
            replacement: None,
        });
    }
    entry
}

/// The entry's family: the `command-a` and `command-r` lines by prefix
/// (`command-r7b` belongs to the `command-r` line), `aya` for any id
/// containing the segment (the `c4ai-aya-*` and `tiny-aya-*` spellings),
/// and the whole id otherwise.
fn family_of(id: &str) -> String {
    for line in ["command-a", "command-r"] {
        if id.starts_with(line) {
            return (*line).to_owned();
        }
    }
    if id.split('-').any(|segment| segment == "aya") {
        return "aya".to_owned();
    }
    id.to_owned()
}

/// Sets every entry's family, then collapses `-MM-YYYY` snapshot suffixes
/// onto their canonical entries.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    crate::taxonomy::collapse_variants(entries, |id| {
        crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::MonthYear)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First page of the documented response shape (2026-09-14 research
    /// extraction): a chat model with the `tools` and `vision` feature
    /// flags, with `next_page_token` set so pagination continues.
    const PAGE_1: &str = r#"{
  "models": [
    {
      "name": "command-a-03-2025",
      "is_deprecated": false,
      "endpoints": ["chat"],
      "finetuned": false,
      "context_length": 256000.0,
      "tokenizer_url": "https://storage.googleapis.com/cohere-public/tokenizers/command-a.json",
      "default_endpoints": ["chat"],
      "features": ["tools", "vision", "json_mode", "json_object_mode"]
    }
  ],
  "next_page_token": "page-2"
}"#;

    /// Second and final page: an embedding-only model, a rerank-only
    /// model, and a deprecated chat model.
    const PAGE_2: &str = r#"{
  "models": [
    {
      "name": "embed-v4.0",
      "is_deprecated": false,
      "endpoints": ["embed"],
      "context_length": 512.0,
      "features": []
    },
    {
      "name": "rerank-v3.5",
      "is_deprecated": false,
      "endpoints": ["rerank"],
      "context_length": 4096.0,
      "features": []
    },
    {
      "name": "command",
      "is_deprecated": true,
      "endpoints": ["chat", "generate"],
      "context_length": 4096.0,
      "features": []
    }
  ]
}"#;

    fn page(json: &str) -> Page {
        serde_json::from_str(json).expect("fixture must parse as a page")
    }

    fn entries(json: &str) -> Vec<ModelEntry> {
        page(json).models.iter().map(normalize_model).collect()
    }

    #[test]
    fn chat_model_maps_context_and_features() {
        let entry = &entries(PAGE_1)[0];
        assert_eq!(entry.id, "command-a-03-2025");
        assert_eq!(
            entry.display_name, "command-a-03-2025",
            "the endpoint reports no display name; the id doubles"
        );
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(
            entry.context_window,
            Some(256_000),
            "context_length (a wire double) maps to context_window"
        );
        assert!(
            entry.tool_calling,
            "the `tools` feature flag maps to tool_calling"
        );
        assert!(entry.images, "the `vision` feature flag maps to images");
        assert_eq!(entry.max_output, None, "the endpoint reports no max output");
        assert_eq!(
            entry.released_at, None,
            "the endpoint reports no release date"
        );
        assert!(entry.pricing.is_none(), "the endpoint reports no pricing");
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn endpoint_lists_map_to_model_kinds() {
        let entries = entries(PAGE_2);
        assert_eq!(
            entries[0].kind,
            ModelKind::Embedding,
            "embed without chat is an embedding model"
        );
        assert_eq!(
            entries[1].kind,
            ModelKind::Classifier,
            "rerank without chat or embed is a classifier"
        );
        assert_eq!(
            entries[2].kind,
            ModelKind::Chat,
            "a chat endpoint dominates the other endpoint values"
        );
        assert!(!entries[0].tool_calling && !entries[0].images);
        assert_eq!(entries[0].context_window, Some(512));
    }

    #[test]
    fn is_deprecated_maps_to_deprecation() {
        let entry = &entries(PAGE_2)[2];
        let deprecation = entry.deprecation.as_ref().expect("deprecation must map");
        assert_eq!(deprecation.status, "deprecated");
        assert_eq!(
            deprecation.date, None,
            "the boolean flag reports no sunset date"
        );
        assert_eq!(
            deprecation.replacement, None,
            "the boolean flag names no successor"
        );
    }

    #[test]
    fn pagination_follows_next_page_token() {
        let first = page(PAGE_1);
        let token = next_token(&first).expect("a page with next_page_token must yield a token");
        assert_eq!(token, "page-2", "the token passes through verbatim");
        let second = page(PAGE_2);
        assert_eq!(next_token(&second), None, "the final page ends traversal");
    }

    #[test]
    fn pagination_stops_on_empty_token() {
        let page = page(r#"{ "models": [], "next_page_token": "" }"#);
        assert_eq!(
            next_token(&page),
            None,
            "an empty token must not loop the fetch forever"
        );
    }

    /// Trimmed 2026-09-14 sheet excerpt: the real Cohere ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-cohere.json");

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
    fn fixture_ids_classify_into_line_families() {
        let by_id = classified();
        let table: &[(&str, &str)] = &[
            ("command-a-03-2025", "command-a"),
            ("command-a-plus-05-2026", "command-a"),
            ("command-a-reasoning-08-2025", "command-a"),
            ("command-a-translate-08-2025", "command-a"),
            ("command-r-08-2024", "command-r"),
            ("command-r-plus-08-2024", "command-r"),
            ("command-r7b-12-2024", "command-r"),
            ("command-r7b-arabic-02-2025", "command-r"),
            ("c4ai-aya-expanse-32b", "aya"),
            ("c4ai-aya-vision-32b", "aya"),
            ("tiny-aya-earth", "aya"),
            ("tiny-aya-global", "aya"),
            ("embed-v4.0", "embed-v4.0"),
            ("rerank-v3.5", "rerank-v3.5"),
            ("parse-v5.0", "parse-v5.0"),
            ("north-mini-code-1-0", "north-mini-code-1-0"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn month_year_snapshots_without_a_canonical_stay_canonical() {
        let by_id = classified();
        // Every `-MM-YYYY` suffix in the 2026-09-14 sheet (command-a-03-2025,
        // cohere-transcribe-03-2026, ...) lacks its base id in the list, so
        // nothing collapses and every id keeps a non-empty family.
        for entry in by_id.values() {
            assert!(!entry.family.is_empty(), "{} has an empty family", entry.id);
            assert!(
                entry.variant_of.is_none(),
                "{} must stay canonical: its base id is not in the list",
                entry.id
            );
        }
    }

    #[test]
    fn a_month_year_snapshot_collapses_onto_its_canonical() {
        let mut entries = vec![
            crate::taxonomy::fixture::entry("command-a"),
            crate::taxonomy::fixture::entry("command-a-03-2025"),
        ];
        apply_taxonomy(&mut entries);
        let variant = &entries[1];
        assert_eq!(variant.variant_of.as_deref(), Some("command-a"));
        assert_eq!(variant.variant.as_deref(), Some("03-2025"));
        assert_eq!(
            variant.family, "command-a",
            "the variant inherits the canonical's family"
        );
    }

    #[test]
    fn descriptor_publishes_the_compatibility_base_and_key() {
        assert_eq!(
            PROVIDER.openai_base_url,
            Some("https://api.cohere.com/compatibility/v1")
        );
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api_types::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
