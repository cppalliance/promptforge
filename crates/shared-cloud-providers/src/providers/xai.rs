//! xAI provider: the public descriptor plus the private variance of
//! `GET /v1/models` - Bearer auth over the OpenAI list shape, extended
//! with `aliases` (carried on the wire; the sheet schema has no aliases
//! field, so they are deliberately dropped), `context_length`, and
//! per-token pricing reported as USD cents per 100M tokens.
//!
//! Docs: <https://docs.x.ai>

use serde::Deserialize;
use shared_gateway_api::{EnvRole, ModelEntry, Pricing, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "XAI_API_KEY";

/// The xAI provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "xai",
    display_name: "xAI",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.x.ai",
    openai_base_url: Some("https://api.x.ai/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// USD cents per 100M tokens to USD per million tokens: 100M to 1M is a
/// factor of 100, cents to dollars another.
const CENTS_PER_100M_TO_USD_PER_MTOK: f64 = 10_000.0;

/// Fetch and normalize xAI's model list in a single request.
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

/// One model as the wire reports it: the OpenAI shape plus xAI's
/// extensions. `aliases` parses but is dropped - the sheet schema has no
/// aliases field.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
    context_length: Option<u32>,
    prompt_text_token_price: Option<f64>,
    completion_text_token_price: Option<f64>,
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    entry.context_window = model.context_length;
    entry.pricing = pricing(model);
    entry
}

/// Convert a wire price (USD cents per 100M tokens) to USD per million
/// tokens.
fn usd_per_mtok(cents_per_100m: f64) -> f64 {
    cents_per_100m / CENTS_PER_100M_TO_USD_PER_MTOK
}

/// Pricing is emitted only when the wire reports both directions; a
/// half-known price is worse than an absent one.
fn pricing(model: &WireModel) -> Option<Pricing> {
    Some(Pricing {
        currency: "USD".to_owned(),
        prompt_per_mtok: usd_per_mtok(model.prompt_text_token_price?),
        completion_per_mtok: usd_per_mtok(model.completion_text_token_price?),
    })
}

/// The entry's family: `grok-<version>` for the numbered line, the
/// product line for the imagine and build ids, and the whole id
/// otherwise.
fn family_of(id: &str) -> String {
    const LINES: &[&str] = &["grok-imagine-image", "grok-imagine-video", "grok-build"];
    if let Some(rest) = id.strip_prefix("grok-") {
        let token = rest.split('-').next().unwrap_or(rest);
        if crate::taxonomy::is_version_token(token) {
            return format!("grok-{token}");
        }
    }
    for line in LINES {
        if id == *line || id.starts_with(&format!("{line}-")) {
            return (*line).to_owned();
        }
    }
    id.to_owned()
}

/// Set every entry's family, then collapse `-MMDD` snapshot suffixes
/// onto their canonical entries. Ids carrying the date as an infix
/// (`grok-4.20-0309-reasoning`) are not suffixes and stay canonical.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
    crate::taxonomy::collapse_variants(entries, |id| {
        crate::taxonomy::strip_snapshot(id, crate::taxonomy::SnapshotStyle::MonthDay)
    });
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape: one fully extended entry
    /// and one entry carrying neither pricing nor a context length.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "grok-4",
      "object": "model",
      "created": 1782864000,
      "owned_by": "xai",
      "aliases": ["grok-4-latest"],
      "context_length": 256000,
      "prompt_text_token_price": 30000,
      "completion_text_token_price": 150000
    },
    {
      "id": "grok-3-mini",
      "object": "model",
      "created": 1782864000,
      "owned_by": "xai",
      "aliases": []
    }
  ]
}"#;

    fn entries(json: &str) -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(json).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn normalizes_context_length_and_pricing() {
        let entries = entries(LIST);
        let entry = &entries[0];
        assert_eq!(entry.id, "grok-4");
        assert_eq!(
            entry.context_window,
            Some(256_000),
            "context_length maps to context_window"
        );
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::July, 1).ok()
        );
        let pricing = entry.pricing.as_ref().expect("pricing must be present");
        assert_eq!(pricing.currency, "USD");
        assert_eq!(
            pricing.prompt_per_mtok.to_bits(),
            3.0f64.to_bits(),
            "30000 cents per 100M tokens is $3 per million"
        );
        assert_eq!(
            pricing.completion_per_mtok.to_bits(),
            15.0f64.to_bits(),
            "150000 cents per 100M tokens is $15 per million"
        );
    }

    #[test]
    fn absent_extensions_stay_absent() {
        let entries = entries(LIST);
        let entry = &entries[1];
        assert_eq!(entry.id, "grok-3-mini");
        assert_eq!(entry.context_window, None);
        assert!(
            entry.pricing.is_none(),
            "pricing is emitted only when both directions are reported"
        );
    }

    #[test]
    fn aliases_are_tolerated_and_dropped() {
        let entries = entries(LIST);
        assert_eq!(
            entries.len(),
            2,
            "the aliases extension must not break parsing"
        );
    }

    #[test]
    fn pricing_requires_both_directions() {
        let half = WireModel {
            id: "m".to_owned(),
            created: None,
            context_length: None,
            prompt_text_token_price: Some(30000.0),
            completion_text_token_price: None,
        };
        assert!(
            pricing(&half).is_none(),
            "a half-known price is worse than an absent one"
        );
    }

    /// Trimmed 2026-09-14 sheet excerpt: the real xAI ids.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-xai.json");

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
            ("grok-4.20-0309-non-reasoning", "grok-4.20"),
            ("grok-4.20-0309-reasoning", "grok-4.20"),
            ("grok-4.20-multi-agent-0309", "grok-4.20"),
            ("grok-4.3", "grok-4.3"),
            ("grok-4.5", "grok-4.5"),
            ("grok-4.6", "grok-4.6"),
            ("grok-build-0.1", "grok-build"),
            ("grok-imagine-image", "grok-imagine-image"),
            ("grok-imagine-image-2.0", "grok-imagine-image"),
            ("grok-imagine-video", "grok-imagine-video"),
            ("grok-imagine-video-1.5", "grok-imagine-video"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
    }

    #[test]
    fn month_day_snapshots_without_a_canonical_stay_canonical() {
        let by_id = classified();
        // `grok-4.20-multi-agent-0309` is the only `-MMDD` suffix in the
        // 2026-09-14 sheet and its base id is absent, so nothing
        // collapses and every id keeps a non-empty family.
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
    fn a_month_day_snapshot_collapses_onto_its_canonical() {
        let mut entries = vec![
            crate::taxonomy::fixture::entry("grok-4.20-multi-agent"),
            crate::taxonomy::fixture::entry("grok-4.20-multi-agent-0309"),
        ];
        apply_taxonomy(&mut entries);
        let variant = &entries[1];
        assert_eq!(variant.variant_of.as_deref(), Some("grok-4.20-multi-agent"));
        assert_eq!(variant.variant.as_deref(), Some("0309"));
        assert_eq!(
            variant.family, "grok-4.20",
            "the variant inherits the canonical's family"
        );
    }
}
