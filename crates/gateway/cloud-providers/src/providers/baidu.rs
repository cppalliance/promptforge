//! Baidu Qianfan provider: the public descriptor plus the private
//! variance of `GET /v2/models` under `https://qianfan.baidubce.com` -
//! Bearer auth (console-issued `bce-v3/ALTAK-...` key; the legacy V1
//! AK/SK OAuth exchange does not apply) and the OpenAI list envelope
//! around Qianfan's rich card: `type`, `context_length`,
//! `max_tokens`, an `architecture` modality object, and `pricing` in
//! CNY per thousand tokens, normalized to per-million-token units with
//! `currency: "CNY"`. Tiered pricing normalizes to its base (first)
//! tier. No pagination, no release-date or deprecation fields beyond
//! the unix `created`.
//!
//! Docs: <https://cloud.baidu.com/doc/qianfan-api/s/Dmba8k71y>

use gateway_api_types::{EnvRole, ModelEntry, ModelKind, Pricing, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "BAIDU_API_KEY";

/// The Baidu provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "baidu",
    display_name: "Baidu",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://qianfan.baidubce.com",
    openai_base_url: Some("https://qianfan.baidubce.com/v2"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v2/models";

/// Fetches and normalizes Baidu's model list in a single request.
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

/// One model as the wire reports it. Every field is documented as
/// optional. `max_completions_tokens` (the reasoning-plus-output
/// ceiling) and `prompt_tokens` (the settable input ceiling) duplicate
/// what `context_length` already states and are not parsed.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
    #[serde(rename = "type")]
    model_type: Option<String>,
    context_length: Option<u32>,
    max_tokens: Option<u32>,
    architecture: Option<WireArchitecture>,
    pricing: Option<WirePricing>,
}

/// The modality object: `modality` plus the input/output modality
/// lists.
#[derive(Debug, Deserialize)]
struct WireArchitecture {
    input_modalities: Option<Vec<String>>,
}

/// The pricing object: prompt and completion prices in CNY per
/// thousand tokens, each either a single price string or a tiered
/// array of `{up_to, price}` rows. The per-image `image` price has no
/// sheet field and is not parsed.
#[derive(Debug, Deserialize)]
struct WirePricing {
    prompt: Option<WirePrice>,
    completion: Option<WirePrice>,
}

/// One price: a flat string or a tiered table.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WirePrice {
    /// A flat price, CNY per thousand tokens.
    Flat(String),
    /// Tiered rows; the base (first) tier represents the model.
    Tiered(Vec<WireTier>),
}

/// One tier of a tiered price.
#[derive(Debug, Deserialize)]
struct WireTier {
    price: String,
}

/// The workload, from the wire `type` value.
fn model_kind(model_type: Option<&str>) -> ModelKind {
    match model_type {
        Some("embeddings") => ModelKind::Embedding,
        Some("rerank") => ModelKind::Classifier,
        Some("text2image" | "image2image") => ModelKind::Image,
        Some("text2video") => ModelKind::Video,
        _ => ModelKind::Chat,
    }
}

/// One price in CNY per million tokens: the wire reports CNY per
/// thousand tokens, so the flat value (or the base tier of a tiered
/// table) scales by a thousand. An unparseable price drops the whole
/// pricing object rather than publishing a wrong number.
fn price_per_mtok(price: &WirePrice) -> Option<f64> {
    let flat = match price {
        WirePrice::Flat(value) => value,
        WirePrice::Tiered(tiers) => &tiers.first()?.price,
    };
    flat.parse::<f64>().ok().map(|per_1k| per_1k * 1000.0)
}

/// Normalizes one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    entry.kind = model_kind(model.model_type.as_deref());
    entry.context_window = model.context_length;
    entry.max_output = model.max_tokens;
    entry.images = model
        .architecture
        .as_ref()
        .and_then(|a| a.input_modalities.as_deref())
        .unwrap_or_default()
        .iter()
        .any(|m| m == "image");
    if let Some(pricing) = &model.pricing {
        // The endpoint reports no completion price for models without
        // completions (embeddings, rerankers); that absence is zero,
        // not a reason to drop the prompt price.
        entry.pricing = pricing
            .prompt
            .as_ref()
            .and_then(price_per_mtok)
            .map(|prompt_per_mtok| Pricing {
                currency: "CNY".to_owned(),
                prompt_per_mtok,
                completion_per_mtok: pricing
                    .completion
                    .as_ref()
                    .and_then(price_per_mtok)
                    .unwrap_or(0.0),
            });
    }
    entry
}

/// The entry's family: the `ernie-<version>` prefix for the numbered
/// lines (`ernie-5.0`, `ernie-4.5-turbo-128k`, the `x`-prefixed
/// `ernie-x1.1`), and the whole id otherwise. The catalog has no
/// snapshot suffixes, so there is no collapse pass.
fn family_of(id: &str) -> String {
    if let Some(rest) = id.strip_prefix("ernie-") {
        let (x, rest) = match rest.strip_prefix('x') {
            Some(rest) => ("x", rest),
            None => ("", rest),
        };
        let token: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if token.bytes().any(|b| b.is_ascii_digit()) {
            return format!("ernie-{x}{token}");
        }
    }
    id.to_owned()
}

/// Sets every entry's family.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = family_of(&entry.id);
    }
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented response shape (2026-09-14 research extraction):
    /// a multimodal chat model with flat pricing, an embedding model
    /// with tiered pricing, and an image-generation model with no
    /// pricing.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "ernie-5.0",
      "object": "model",
      "owned_by": "Baidu",
      "created": 1767225600,
      "type": "chat",
      "context_length": 131072,
      "max_completions_tokens": 65536,
      "max_tokens": 32768,
      "prompt_tokens": 98304,
      "architecture": {
        "modality": "text+image->text",
        "input_modalities": ["text", "image"],
        "output_modalities": ["text"]
      },
      "pricing": {
        "prompt": "0.004",
        "completion": "0.012",
        "image": null
      }
    },
    {
      "id": "ernie-embedding-v2",
      "object": "model",
      "owned_by": "Baidu",
      "created": 1751500800,
      "type": "embeddings",
      "context_length": 8192,
      "max_completions_tokens": null,
      "max_tokens": null,
      "prompt_tokens": null,
      "architecture": {
        "modality": "text->embedding",
        "input_modalities": ["text"],
        "output_modalities": ["embedding"]
      },
      "pricing": {
        "prompt": [
          { "up_to": 1000000, "price": "0.0005" },
          { "up_to": null, "price": "0.001" }
        ],
        "completion": null,
        "image": null
      }
    },
    {
      "id": "ernie-irag",
      "object": "model",
      "owned_by": "Baidu",
      "created": null,
      "type": "text2image",
      "context_length": null,
      "max_completions_tokens": null,
      "max_tokens": null,
      "prompt_tokens": null,
      "architecture": {
        "modality": "text->image",
        "input_modalities": ["text"],
        "output_modalities": ["image"]
      },
      "pricing": null
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn chat_model_maps_limits_modality_and_cny_pricing() {
        let entry = &entries()[0];
        assert_eq!(entry.id, "ernie-5.0");
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::January, 1).ok(),
            "1767225600 is 2026-01-01T00:00:00Z"
        );
        assert_eq!(
            entry.context_window,
            Some(131_072),
            "context_length maps to context_window"
        );
        assert_eq!(
            entry.max_output,
            Some(32_768),
            "max_tokens maps to max_output"
        );
        assert!(entry.images, "an image input modality maps to images");
        let pricing = entry.pricing.as_ref().expect("pricing must map");
        assert_eq!(pricing.currency, "CNY");
        assert_eq!(
            pricing.prompt_per_mtok.to_bits(),
            4.0f64.to_bits(),
            "0.004 CNY per 1k tokens is 4.0 per million"
        );
        assert_eq!(
            pricing.completion_per_mtok.to_bits(),
            12.0f64.to_bits(),
            "0.012 CNY per 1k tokens is 12.0 per million"
        );
    }

    #[test]
    fn tiered_pricing_normalizes_to_the_base_tier() {
        let entry = &entries()[1];
        assert_eq!(
            entry.kind,
            ModelKind::Embedding,
            "the wire type maps the kind"
        );
        assert!(!entry.images, "text-only input modalities");
        let pricing = entry.pricing.as_ref().expect("tiered pricing must map");
        assert_eq!(pricing.currency, "CNY");
        assert_eq!(
            pricing.prompt_per_mtok.to_bits(),
            0.5f64.to_bits(),
            "the base tier 0.0005 CNY per 1k tokens is 0.5 per million"
        );
        assert_eq!(
            pricing.completion_per_mtok.to_bits(),
            0.0f64.to_bits(),
            "an absent completion price is zero, not a dropped pricing object"
        );
    }

    #[test]
    fn wire_type_maps_media_kinds() {
        let entry = &entries()[2];
        assert_eq!(entry.kind, ModelKind::Image, "text2image is an image model");
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.max_output, None);
        assert_eq!(entry.released_at, None, "a null created maps to no date");
        assert!(
            entry.pricing.is_none(),
            "a null pricing object maps to none"
        );
    }

    #[test]
    fn unparseable_price_drops_the_pricing_object() {
        let model: WireModel = serde_json::from_str(
            r#"{
              "id": "ernie-weird",
              "type": "chat",
              "pricing": { "prompt": "contact-sales", "completion": "0.01" }
            }"#,
        )
        .expect("fixture must parse");
        let entry = normalize_model(&model);
        assert!(
            entry.pricing.is_none(),
            "an unparseable price must not publish a wrong number"
        );
    }

    /// The documented catalog ids with the provider's taxonomy applied,
    /// by id. Baidu was unprovisioned for the 2026-09-14 sheet, so the
    /// table follows the documented catalog.
    fn classified(ids: &[&str]) -> std::collections::BTreeMap<String, ModelEntry> {
        let mut entries: Vec<ModelEntry> = ids
            .iter()
            .map(|id| crate::taxonomy::fixture::entry(id))
            .collect();
        apply_taxonomy(&mut entries);
        entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect()
    }

    #[test]
    fn catalog_ids_classify_into_version_and_fallback_families() {
        let by_id = classified(&[
            "ernie-5.0",
            "ernie-4.5-turbo-128k",
            "ernie-x1.1",
            "ernie-embedding-v2",
            "ernie-irag",
            "ernie-novel-8k",
            "deepseek-r1",
        ]);
        let table: &[(&str, &str)] = &[
            ("ernie-5.0", "ernie-5.0"),
            ("ernie-4.5-turbo-128k", "ernie-4.5"),
            ("ernie-x1.1", "ernie-x1.1"),
            ("ernie-embedding-v2", "ernie-embedding-v2"),
            ("ernie-irag", "ernie-irag"),
            ("ernie-novel-8k", "ernie-novel-8k"),
            ("deepseek-r1", "deepseek-r1"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
        for entry in by_id.values() {
            assert!(
                entry.variant_of.is_none(),
                "the catalog carries no snapshot suffixes: {}",
                entry.id
            );
        }
    }

    #[test]
    fn descriptor_publishes_the_v2_base_and_key() {
        assert_eq!(
            PROVIDER.openai_base_url,
            Some("https://qianfan.baidubce.com/v2")
        );
        assert_eq!(PROVIDER.env_vars.len(), 1);
        assert_eq!(PROVIDER.env_vars[0].name, KEY_ENV);
        assert_eq!(PROVIDER.env_vars[0].role, gateway_api_types::EnvRole::Key);
        assert_eq!(PROVIDER.env_vars[0].default, None);
    }
}
