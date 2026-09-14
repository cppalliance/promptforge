//! xAI provider: the public descriptor plus the private variance of
//! `GET /v1/models` - Bearer auth over the OpenAI list shape, extended
//! with `aliases` (carried on the wire; the sheet schema has no aliases
//! field, so they are deliberately dropped), `context_length`, and
//! per-token pricing reported as USD cents per 100M tokens.
//!
//! Docs: <https://docs.x.ai>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Pricing, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// The xAI provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "xai",
    display_name: "xAI",
    tier: Tier::Prime,
    key_env: "XAI_API_KEY",
    base_url: "https://api.x.ai",
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
    key: &str,
) -> Result<Vec<ModelEntry>, FetchError> {
    let models: Vec<WireModel> =
        fetch_list(client, &format!("{base_url}{MODELS_PATH}"), key).await?;
    Ok(models.iter().map(normalize_model).collect())
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
}
