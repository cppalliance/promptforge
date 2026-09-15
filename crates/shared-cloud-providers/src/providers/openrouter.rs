//! OpenRouter provider: the public descriptor plus the private variance
//! of `GET /api/v1/models` under `https://openrouter.ai` - no auth on
//! the listing endpoint (verified live 2026-09-14, a 718 KB payload)
//! and the richest card of any surveyed provider: `context_length`,
//! `architecture` input/output modalities, `pricing` in USD per token
//! (normalized to per-million-token), `top_provider.max_completion_tokens`,
//! and `expiration_date` as the sunset signal. The default response is
//! the full catalog, so the `links.next` pagination is not engaged.
//! Tier: aggregator.
//!
//! Docs: <https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties>

use serde::Deserialize;
use shared_gateway_api::{Deprecation, ModelEntry, ModelKind, Pricing, Tier};
use time::{Date, Month};

use crate::providers::openai_shape::{ListResponse, base_entry};
use crate::{FetchError, Provider};

mod taxonomy;

/// The OpenRouter provider descriptor: keyless - the model-list
/// endpoint needs no credential.
pub const PROVIDER: Provider = Provider {
    name: "openrouter",
    display_name: "OpenRouter",
    tier: Tier::Aggregator,
    key_env: None,
    base_url: "https://openrouter.ai",
    openai_base_url: Some("https://openrouter.ai/api/v1"),
    env_vars: &[],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/api/v1/models";

/// Fetch and normalize OpenRouter's model list in a single request; the
/// endpoint is keyless, so no credential is read or sent.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    _key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    let response: ListResponse<WireModel> = client
        .get(format!("{base_url}{MODELS_PATH}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut entries: Vec<ModelEntry> = response.data.iter().map(normalize_model).collect();
    taxonomy::apply(&mut entries);
    Ok(entries)
}

/// One model as the wire reports it. `canonical_slug`, `description`,
/// `per_request_limits`, `supported_parameters`, `default_parameters`,
/// `supported_voices`, and `links` carry no sheet meaning and are not
/// parsed.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    name: Option<String>,
    created: Option<i64>,
    context_length: Option<u32>,
    architecture: Option<WireArchitecture>,
    pricing: Option<WirePricing>,
    top_provider: Option<WireTopProvider>,
    expiration_date: Option<String>,
}

/// The modality object: the input/output modality lists. `modality`,
/// `instruct_type`, and `tokenizer` duplicate the lists and are not
/// parsed.
#[derive(Debug, Deserialize)]
struct WireArchitecture {
    input_modalities: Option<Vec<String>>,
    output_modalities: Option<Vec<String>>,
}

/// The pricing object: prompt and completion prices in USD per token,
/// as strings. The cache, image, audio, and request price variants have
/// no sheet field and are not parsed.
#[derive(Debug, Deserialize)]
struct WirePricing {
    prompt: Option<String>,
    completion: Option<String>,
}

/// The top-provider object: only `max_completion_tokens` has a sheet
/// field. Its `context_length` duplicates the top-level field and
/// `is_moderated` has no sheet field.
#[derive(Debug, Deserialize)]
struct WireTopProvider {
    max_completion_tokens: Option<u32>,
}

/// The workload, from the output modalities: the first non-text output
/// is the model's product; a text-only model is chat.
fn model_kind(output_modalities: &[String]) -> ModelKind {
    for modality in output_modalities {
        match modality.as_str() {
            "embeddings" => return ModelKind::Embedding,
            "image" => return ModelKind::Image,
            "video" => return ModelKind::Video,
            "audio" | "speech" => return ModelKind::Speech,
            "transcription" => return ModelKind::Transcription,
            "rerank" => return ModelKind::Classifier,
            _ => {}
        }
    }
    ModelKind::Chat
}

/// One price in USD per million tokens: the wire reports USD per token
/// as a string, so the value scales by a million. An unparseable price
/// drops the whole pricing object rather than publishing a wrong number.
fn price_per_mtok(per_token: &str) -> Option<f64> {
    per_token
        .parse::<f64>()
        .ok()
        .map(|price| price * 1_000_000.0)
}

/// Parse the `YYYY-MM-DD` expiration date; an unparseable value keeps
/// the status with no date.
fn parse_expiration_date(value: &str) -> Option<Date> {
    let mut parts = value.split('-');
    let (Ok(year), Ok(month), Ok(day)) = (
        parts.next().unwrap_or_default().parse::<i32>(),
        parts.next().unwrap_or_default().parse::<u8>(),
        parts.next().unwrap_or_default().parse::<u8>(),
    ) else {
        return None;
    };
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    if let Some(name) = &model.name {
        entry.display_name.clone_from(name);
    }
    let inputs = model
        .architecture
        .as_ref()
        .and_then(|a| a.input_modalities.as_deref())
        .unwrap_or_default();
    let outputs = model
        .architecture
        .as_ref()
        .and_then(|a| a.output_modalities.as_deref())
        .unwrap_or_default();
    entry.kind = model_kind(outputs);
    entry.context_window = model.context_length;
    entry.max_output = model
        .top_provider
        .as_ref()
        .and_then(|provider| provider.max_completion_tokens);
    entry.images = inputs.iter().any(|m| m == "image");
    entry.pdf_input = inputs.iter().any(|m| m == "file");
    entry.video_input = inputs.iter().any(|m| m == "video");
    entry.audio_input = inputs.iter().any(|m| m == "audio");
    if let Some(pricing) = &model.pricing {
        // The endpoint reports no completion price for models without
        // completions (embeddings); that absence is zero, not a reason
        // to drop the prompt price.
        entry.pricing = pricing
            .prompt
            .as_deref()
            .and_then(price_per_mtok)
            .map(|prompt_per_mtok| Pricing {
                currency: "USD".to_owned(),
                prompt_per_mtok,
                completion_per_mtok: pricing
                    .completion
                    .as_deref()
                    .and_then(price_per_mtok)
                    .unwrap_or(0.0),
            });
    }
    entry.deprecation = model.expiration_date.as_ref().map(|date| Deprecation {
        status: "expires".to_owned(),
        date: parse_expiration_date(date),
        replacement: None,
    });
    entry
}

#[cfg(test)]
mod tests {
    use time::{Date, Month};

    use super::*;

    /// A trimmed excerpt of the verified 2026-09-14 live payload (718 KB
    /// in full): a multimodal chat model with pricing, an embedding
    /// model, and an expiring model with video and audio inputs.
    const LIST: &str = r#"{
  "data": [
    {
      "id": "openai/gpt-5.2",
      "canonical_slug": "openai/gpt-5.2",
      "name": "OpenAI: GPT-5.2",
      "created": 1767225600,
      "description": "Flagship multimodal model.",
      "context_length": 400000,
      "architecture": {
        "modality": "text+image->text",
        "input_modalities": ["text", "image", "file"],
        "output_modalities": ["text"],
        "instruct_type": null,
        "tokenizer": "GPT"
      },
      "pricing": {
        "prompt": "0.0000025",
        "completion": "0.00001",
        "input_cache_read": "0.00000025"
      },
      "top_provider": {
        "context_length": 400000,
        "max_completion_tokens": 128000,
        "is_moderated": false
      },
      "per_request_limits": null,
      "supported_parameters": ["tools", "structured_outputs", "max_tokens"],
      "expiration_date": null
    },
    {
      "id": "qwen/qwen3-embedding-8b",
      "canonical_slug": "qwen/qwen3-embedding-8b",
      "name": "Qwen: Qwen3 Embedding 8B",
      "created": 1751500800,
      "description": "Embedding model.",
      "context_length": 32768,
      "architecture": {
        "modality": "text->embeddings",
        "input_modalities": ["text"],
        "output_modalities": ["embeddings"],
        "instruct_type": null,
        "tokenizer": "Qwen"
      },
      "pricing": {
        "prompt": "0.0000005",
        "completion": "0"
      },
      "top_provider": {
        "context_length": 32768,
        "max_completion_tokens": null,
        "is_moderated": false
      },
      "per_request_limits": null,
      "supported_parameters": [],
      "expiration_date": null
    },
    {
      "id": "google/gemini-2.0-flash-001",
      "canonical_slug": "google/gemini-2.0-flash-001",
      "name": "Google: Gemini 2.0 Flash",
      "created": 1735689600,
      "description": "Expiring multimodal model.",
      "context_length": 1048576,
      "architecture": {
        "modality": "text+image+video+audio->text",
        "input_modalities": ["text", "image", "video", "audio"],
        "output_modalities": ["text"],
        "instruct_type": "gemini",
        "tokenizer": "Gemini"
      },
      "pricing": {
        "prompt": "0.000001",
        "completion": "0.000002"
      },
      "top_provider": {
        "context_length": 1048576,
        "max_completion_tokens": 8192,
        "is_moderated": true
      },
      "per_request_limits": null,
      "supported_parameters": ["tools"],
      "expiration_date": "2026-12-31"
    }
  ],
  "total_count": 3,
  "links": { "next": null }
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page =
            serde_json::from_str::<ListResponse<WireModel>>(LIST).expect("fixture must parse");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn chat_model_maps_limits_modalities_and_usd_pricing() {
        let entry = &entries()[0];
        assert_eq!(entry.id, "openai/gpt-5.2");
        assert_eq!(
            entry.display_name, "OpenAI: GPT-5.2",
            "the card's name is the display name"
        );
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::January, 1).ok(),
            "1767225600 is 2026-01-01T00:00:00Z"
        );
        assert_eq!(
            entry.context_window,
            Some(400_000),
            "context_length maps to context_window"
        );
        assert_eq!(
            entry.max_output,
            Some(128_000),
            "top_provider.max_completion_tokens maps to max_output"
        );
        assert!(entry.images, "an image input modality maps to images");
        assert!(entry.pdf_input, "a file input modality maps to pdf_input");
        assert!(!entry.video_input && !entry.audio_input);
        let pricing = entry.pricing.as_ref().expect("pricing must map");
        assert_eq!(pricing.currency, "USD");
        assert_eq!(
            pricing.prompt_per_mtok.to_bits(),
            2.5f64.to_bits(),
            "0.0000025 USD per token is 2.50 per million"
        );
        assert_eq!(
            pricing.completion_per_mtok.to_bits(),
            10.0f64.to_bits(),
            "0.00001 USD per token is 10.00 per million"
        );
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn output_modalities_map_the_kind() {
        let entry = &entries()[1];
        assert_eq!(
            entry.kind,
            ModelKind::Embedding,
            "an embeddings output modality is an embedding model"
        );
        assert!(!entry.images);
        assert_eq!(
            entry.max_output, None,
            "a null max_completion_tokens maps to none"
        );
        let pricing = entry.pricing.as_ref().expect("pricing must map");
        assert_eq!(
            pricing.prompt_per_mtok.to_bits(),
            0.5f64.to_bits(),
            "0.0000005 USD per token is 0.50 per million"
        );
        assert_eq!(
            pricing.completion_per_mtok.to_bits(),
            0.0f64.to_bits(),
            "a zero completion price stays zero"
        );
    }

    #[test]
    fn every_output_modality_maps_to_its_kind() {
        let cases: &[(&[&str], ModelKind)] = &[
            (&["embeddings"], ModelKind::Embedding),
            (&["image"], ModelKind::Image),
            (&["video"], ModelKind::Video),
            (&["audio"], ModelKind::Speech),
            (&["speech"], ModelKind::Speech),
            (&["transcription"], ModelKind::Transcription),
            (&["rerank"], ModelKind::Classifier),
            (&["text"], ModelKind::Chat),
        ];
        for (modalities, kind) in cases {
            let owned: Vec<String> = modalities.iter().map(|m| (*m).to_owned()).collect();
            assert_eq!(model_kind(&owned), *kind, "{modalities:?}");
        }
    }

    #[test]
    fn expiration_date_maps_to_deprecation() {
        let entry = &entries()[2];
        assert!(entry.video_input && entry.audio_input);
        let deprecation = entry
            .deprecation
            .as_ref()
            .expect("expiration_date must map");
        assert_eq!(deprecation.status, "expires");
        assert_eq!(
            deprecation.date,
            Date::from_calendar_date(2026, Month::December, 31).ok(),
            "the YYYY-MM-DD expiration date maps to a calendar date"
        );
        assert_eq!(
            deprecation.replacement, None,
            "the endpoint names no successor"
        );
        let pricing = entry.pricing.as_ref().expect("pricing must map");
        assert_eq!(pricing.prompt_per_mtok.to_bits(), 1.0f64.to_bits());
        assert_eq!(pricing.completion_per_mtok.to_bits(), 2.0f64.to_bits());
    }

    #[test]
    fn absent_fields_are_conservative() {
        let page: ListResponse<WireModel> = serde_json::from_str(
            r#"{
              "data": [
                {
                  "id": "acme/legacy-model",
                  "name": null, "created": null, "context_length": null,
                  "architecture": null, "pricing": null,
                  "top_provider": null, "expiration_date": null
                }
              ]
            }"#,
        )
        .expect("fixture must parse");
        let entry = normalize_model(&page.data[0]);
        assert_eq!(
            entry.display_name, "acme/legacy-model",
            "a null name falls back to the id"
        );
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.max_output, None);
        assert_eq!(entry.released_at, None);
        assert!(!entry.images && !entry.pdf_input && !entry.video_input && !entry.audio_input);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn unparseable_price_drops_the_pricing_object() {
        let model: WireModel = serde_json::from_str(
            r#"{
              "id": "acme/weird-model",
              "pricing": { "prompt": "contact-sales", "completion": "0.000001" }
            }"#,
        )
        .expect("fixture must parse");
        let entry = normalize_model(&model);
        assert!(
            entry.pricing.is_none(),
            "an unparseable price must not publish a wrong number"
        );
    }

    #[test]
    fn unparseable_expiration_date_keeps_status_without_a_date() {
        assert_eq!(
            parse_expiration_date("2027-06-30"),
            Date::from_calendar_date(2027, Month::June, 30).ok(),
            "a bare YYYY-MM-DD expiration date parses"
        );
        assert_eq!(
            parse_expiration_date("eventually"),
            None,
            "an unparseable expiration value keeps no date"
        );
    }

    #[test]
    fn descriptor_publishes_the_chat_base_and_stays_keyless() {
        assert_eq!(
            PROVIDER.openai_base_url,
            Some("https://openrouter.ai/api/v1")
        );
        assert!(
            PROVIDER.env_vars.is_empty(),
            "the keyless provider declares no variables"
        );
    }

    #[tokio::test]
    async fn fetch_sends_no_credential() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            // Read the request first, as in the sheet.rs fixture server:
            // replying before the client finishes sending is an HTTP
            // protocol error. The read timeout bounds the capture.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let mut request = Vec::new();
            let mut buf = [0_u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{LIST}",
                LIST.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
            request
        });
        let client = reqwest::Client::new();
        let entries = fetch(&client, &format!("http://{addr}"), None)
            .await
            .expect("a keyless fetch with no credential must succeed");
        let request = String::from_utf8(server.join().expect("fixture server must finish"))
            .expect("the request is ASCII");
        assert!(
            !request.to_lowercase().contains("authorization"),
            "a keyless provider sends no credential: {request}"
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "openai/gpt-5.2");
    }
}
