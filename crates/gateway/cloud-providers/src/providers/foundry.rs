//! Azure AI Foundry provider: the public descriptor plus the private
//! variance of `POST /asset-gallery/v1.0/models` under
//! `https://api.catalog.azureml.ms` - no auth, a JSON body carrying
//! filters, ordering, and paging, and a card richer than most keyed
//! endpoints: context window, max output tokens, modalities, capability
//! labels, languages, lifecycle, and the inference retirement date. It
//! reports no pricing.
//!
//! Keyless and global, unlike Foundry's inference surface, which is
//! per-resource (`https://<resource>.services.ai.azure.com`) and lists
//! only the deployments an operator created there - so a subscription
//! with nothing deployed yields an empty list. Cataloguing what Foundry
//! offers needs no subscription, no deployed resource, and no
//! credential, so the descriptor declares no environment variables.
//!
//! Two filters keep the response to the models Azure hosts itself:
//! `Labels=latest` drops superseded versions and
//! `AzureOffers=standard-paygo` drops the mirrored HuggingFace registry,
//! the bulk of the 15k-row unfiltered catalog. Ordering is by name, not
//! the default popularity, which is a float that drifts between runs
//! and would reorder the sheet.
//!
//! This is the catalog API behind the Foundry portal's model browser
//! rather than a documented public REST contract; the shape below came
//! from live responses on 2026-09-15. A request naming an unknown
//! filter field gets the valid ones back in the error body.

use gateway_api::{ModelEntry, Tier};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::providers::openai_shape::base_entry;
use crate::{FetchError, Provider};

#[path = "foundry-taxonomy.rs"]
pub(crate) mod taxonomy;

/// The Azure AI Foundry provider descriptor: keyless - the catalog
/// endpoint needs no credential. `openai_base_url` stays `None`
/// because inference remains per-resource, so there is no fixed chat
/// URL to publish even though the catalog has one.
pub const PROVIDER: Provider = Provider {
    name: "foundry",
    display_name: "Azure AI Foundry",
    tier: Tier::Subprime,
    key_env: None,
    base_url: "https://api.catalog.azureml.ms",
    openai_base_url: None,
    env_vars: &[],
};

/// The catalog path under the base URL.
const MODELS_PATH: &str = "/asset-gallery/v1.0/models";

/// Rows per request. The filtered catalog runs to a few hundred rows,
/// so this bounds a full fetch to a handful of pages.
const PAGE_SIZE: u32 = 100;

/// The offer label marking a model Azure hosts and bills itself, as
/// opposed to a mirrored registry entry that is merely listed.
const HOSTED_OFFER: &str = "standard-paygo";

/// Fetch and normalize Foundry's catalog, following the continuation
/// token until the final page; the endpoint is keyless, so no
/// credential is read or sent.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    _key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    let url = format!("{base_url}{MODELS_PATH}");
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page: Page = client
            .post(&url)
            .json(&request_body(token.as_deref()))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        entries.extend(page.summaries.iter().map(normalize_model));
        let Some(next) = next_token(&page, token.as_deref()) else {
            break;
        };
        token = Some(next);
    }
    taxonomy::apply(&mut entries);
    Ok(entries)
}

/// The request body for one page. The continuation token is omitted
/// rather than sent as null on the first request, since the endpoint
/// documents no null handling.
fn request_body(continuation_token: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "filters": [
            { "field": "Labels", "values": ["latest"], "operator": "eq" },
            { "field": "AzureOffers", "values": [HOSTED_OFFER], "operator": "eq" }
        ],
        "order": [{ "field": "Name", "direction": "asc" }],
        "pageSize": PAGE_SIZE
    });
    if let Some(token) = continuation_token {
        body["continuationToken"] = serde_json::Value::String(token.to_owned());
    }
    body
}

/// The next page's token. An absent, empty, or unchanged token ends
/// traversal, so neither a malformed page nor a server repeating the
/// token it was given can loop the fetch forever.
fn next_token(page: &Page, sent: Option<&str>) -> Option<String> {
    page.continuation_token
        .clone()
        .filter(|token| !token.is_empty() && Some(token.as_str()) != sent)
}

/// One page of the catalog response.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    summaries: Vec<WireModel>,
    continuation_token: Option<String>,
}

/// One model card as the wire reports it. `assetId`, `registryName`,
/// `version`, `summary`, `keywords`, `license`, `popularity`,
/// `deploymentOptions`, `playgroundLimits`, `fineTuningTasks`, and the
/// variant and quota blocks carry no sheet meaning and are not parsed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireModel {
    /// The catalog slug; the id an operator deploys under.
    name: String,
    display_name: Option<String>,
    publisher: Option<String>,
    created_time: Option<String>,
    inference_tasks: Option<Vec<String>>,
    model_capabilities: Option<Vec<String>>,
    model_limits: Option<WireLimits>,
    lifecycle: Option<String>,
    deprecation: Option<WireDeprecation>,
}

/// The limits block: token limits, modalities, and languages.
/// `otherLimits` is an untyped bag with no sheet field.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLimits {
    text_limits: Option<WireTextLimits>,
    supported_languages: Option<Vec<String>>,
    supported_input_modalities: Option<Vec<String>>,
    supported_output_modalities: Option<Vec<String>>,
}

/// The token limits: the context window and the completion ceiling.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTextLimits {
    input_context_window: Option<u32>,
    max_output_tokens: Option<u32>,
}

/// The deprecation block: the sunset date, when the catalog sets one.
/// The endpoint reports a blank string for cards with no date.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDeprecation {
    inference_retirement_date: Option<String>,
}

/// Normalize one wire card into a sheet entry. The catalog reports no
/// pricing, so that field stays empty.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.name, None);
    if let Some(display_name) = &model.display_name {
        entry.display_name.clone_from(display_name);
    }
    entry.released_at = model.created_time.as_deref().and_then(parse_wire_date);
    let limits = model.model_limits.as_ref();
    let text_limits = limits.and_then(|limits| limits.text_limits.as_ref());
    let inputs = limits
        .and_then(|limits| limits.supported_input_modalities.as_deref())
        .unwrap_or_default();
    let outputs = limits
        .and_then(|limits| limits.supported_output_modalities.as_deref())
        .unwrap_or_default();
    let tasks = model.inference_tasks.as_deref().unwrap_or_default();
    let capabilities = model.model_capabilities.as_deref().unwrap_or_default();
    entry.kind = taxonomy::model_kind(tasks, outputs);
    entry.context_window = text_limits.and_then(|limits| limits.input_context_window);
    entry.max_output = text_limits.and_then(|limits| limits.max_output_tokens);
    entry.images = inputs.iter().any(|modality| modality == "image");
    entry.pdf_input = inputs.iter().any(|modality| modality == "pdf");
    entry.video_input = inputs.iter().any(|modality| modality == "video");
    entry.audio_input = inputs.iter().any(|modality| modality == "audio");
    entry.tool_calling = capabilities.iter().any(|label| label == "tool-calling");
    entry.thinking.supported = capabilities.iter().any(|label| label == "reasoning");
    entry.languages = limits
        .and_then(|limits| limits.supported_languages.clone())
        .unwrap_or_default();
    // The catalog reports a blank string, not null, for a card with no
    // retirement date.
    let retirement = model
        .deprecation
        .as_ref()
        .and_then(|block| block.inference_retirement_date.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(parse_wire_date);
    entry.deprecation = taxonomy::deprecation_of(model.lifecycle.as_deref(), retirement);
    if let Some(family) = taxonomy::family_of(model.publisher.as_deref()) {
        entry.family = family;
    }
    entry
}

/// Parse a wire timestamp into a calendar date; an unparseable value
/// keeps no date.
fn parse_wire_date(value: &str) -> Option<Date> {
    OffsetDateTime::parse(value, &Rfc3339)
        .ok()
        .map(OffsetDateTime::date)
}

#[cfg(test)]
mod tests {
    use gateway_api::ModelKind;
    use time::Month;

    use super::*;

    /// A trimmed excerpt of the verified 2026-09-15 live payload: a
    /// reasoning chat model, a retired card carrying a retirement date,
    /// an image model, and an embedding model whose task vocabulary the
    /// mapping does not know.
    const PAGE_1: &str = r#"{
  "summaries": [
    { "name": "claude-mythos-5-1", "displayName": "Claude Mythos 5.1 (gated)",
      "publisher": "Anthropic", "version": "1", "registryName": "azureml-anthropic",
      "createdTime": "2026-08-27T21:16:05.9760599+00:00", "inferenceTasks": ["messages"],
      "modelCapabilities": ["streaming", "reasoning", "tool-calling"],
      "azureOffers": ["standard-paygo"], "lifecycle": "Preview",
      "deprecation": { "inferenceRetirementDate": null },
      "modelLimits": { "otherLimits": {}, "supportedLanguages": ["en", "fr", "ja"],
        "textLimits": { "inputContextWindow": 1000000, "maxOutputTokens": 128000 },
        "supportedInputModalities": ["text", "image", "code"],
        "supportedOutputModalities": ["text"] } },
    { "name": "Phi-3-small-8k-instruct", "displayName": "Phi-3-small instruct (8k)",
      "publisher": "Microsoft", "createdTime": null, "inferenceTasks": ["chat-completion"],
      "modelCapabilities": [], "lifecycle": null,
      "deprecation": { "inferenceRetirementDate": "2025-08-30T00:00:00+00:00" },
      "modelLimits": { "supportedLanguages": ["en"],
        "textLimits": { "inputContextWindow": 131072, "maxOutputTokens": 4096 },
        "supportedInputModalities": ["text"], "supportedOutputModalities": ["text"] } },
    { "name": "FLUX.1-Kontext-pro", "displayName": "FLUX.1 Kontext pro",
      "publisher": "Black Forest Labs", "lifecycle": "Generally available",
      "inferenceTasks": ["text-to-image", "image-to-image"],
      "deprecation": { "inferenceRetirementDate": " " },
      "modelLimits": { "supportedInputModalities": ["text", "image"],
        "supportedOutputModalities": ["image"] } },
    { "name": "cohere-embed-v4", "publisher": null, "lifecycle": "Retired",
      "inferenceTasks": ["some-future-task"],
      "modelLimits": { "supportedInputModalities": ["text"],
        "supportedOutputModalities": ["embeddings"] } }
  ],
  "continuationToken": null
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page: Page = serde_json::from_str(PAGE_1).expect("fixture must parse as a page");
        let mut entries: Vec<ModelEntry> = page.summaries.iter().map(normalize_model).collect();
        taxonomy::apply(&mut entries);
        entries
    }

    #[test]
    fn chat_card_maps_limits_modalities_capabilities_and_languages() {
        let entry = &entries()[0];
        assert_eq!(entry.id, "claude-mythos-5-1", "the catalog name is the id");
        assert_eq!(entry.display_name, "Claude Mythos 5.1 (gated)");
        assert_eq!(entry.kind, ModelKind::Chat, "messages is a chat task");
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::August, 27).ok(),
            "createdTime maps to released_at"
        );
        assert_eq!(entry.context_window, Some(1_000_000));
        assert_eq!(entry.max_output, Some(128_000));
        assert!(entry.images, "an image input modality maps to images");
        assert!(!entry.pdf_input && !entry.video_input && !entry.audio_input);
        assert!(entry.tool_calling, "tool-calling maps to tool_calling");
        assert!(entry.thinking.supported, "reasoning maps to thinking");
        assert!(
            !entry.thinking.enabled,
            "the catalog reports no budget mode"
        );
        assert!(!entry.thinking.adaptive);
        assert_eq!(entry.languages, ["en", "fr", "ja"]);
        assert_eq!(entry.family, "anthropic", "the publisher is the family");
        assert!(entry.pricing.is_none(), "the catalog reports no pricing");
        assert!(
            entry.deprecation.is_none(),
            "a preview card with no retirement date is not deprecated"
        );
    }

    #[test]
    fn retirement_date_without_a_sunset_label_still_deprecates() {
        let entry = &entries()[1];
        assert_eq!(entry.family, "microsoft");
        assert_eq!(entry.released_at, None, "a null createdTime maps to none");
        let deprecation = entry.deprecation.as_ref().expect("the date must map");
        assert_eq!(
            deprecation.status, "retires",
            "a card with a date but no sunset label gets a neutral status"
        );
        assert_eq!(
            deprecation.date,
            Date::from_calendar_date(2025, Month::August, 30).ok()
        );
        assert_eq!(deprecation.replacement, None);
    }

    #[test]
    fn image_card_maps_its_kind_and_ignores_a_blank_retirement_date() {
        let entry = &entries()[2];
        assert_eq!(entry.kind, ModelKind::Image);
        assert!(entry.images);
        assert_eq!(
            entry.context_window, None,
            "an image card reports no limits"
        );
        assert_eq!(entry.family, "black forest labs");
        assert!(
            entry.deprecation.is_none(),
            "a blank retirement date on a live card is no deprecation"
        );
    }

    #[test]
    fn a_publisherless_retired_card_keeps_its_id_and_its_status() {
        let entry = &entries()[3];
        assert_eq!(
            entry.family, "cohere-embed-v4",
            "a card with no publisher keeps its id as the family"
        );
        let deprecation = entry.deprecation.as_ref().expect("Retired must map");
        assert_eq!(deprecation.status, "retired");
        assert_eq!(
            deprecation.date, None,
            "a sunset label with no date keeps the status alone"
        );
    }

    #[test]
    fn request_body_omits_the_token_on_the_first_page_and_carries_it_after() {
        let first = request_body(None);
        assert!(
            first.get("continuationToken").is_none(),
            "the first page sends no token: {first}"
        );
        assert_eq!(first["pageSize"], serde_json::json!(PAGE_SIZE));
        assert_eq!(
            first["filters"][1]["values"][0],
            serde_json::json!(HOSTED_OFFER),
            "the hosted-offer filter drops the mirrored registry"
        );
        assert_eq!(
            first["order"][0]["field"], "Name",
            "ordering by name keeps the sheet stable across runs"
        );
        let next = request_body(Some("token-a"));
        assert_eq!(next["continuationToken"], serde_json::json!("token-a"));
    }

    #[test]
    fn traversal_stops_on_an_absent_empty_or_repeated_token() {
        let page = |token: &str| Page {
            summaries: Vec::new(),
            continuation_token: (!token.is_empty()).then(|| token.to_owned()),
        };
        assert_eq!(
            next_token(&page("token-b"), None).as_deref(),
            Some("token-b")
        );
        assert_eq!(next_token(&page(""), None), None, "an empty token ends it");
        assert_eq!(
            next_token(&page("token-b"), Some("token-b")),
            None,
            "a server repeating the token it was given must not loop the fetch"
        );
    }

    #[test]
    fn descriptor_is_keyless_with_no_variables_and_a_global_catalog() {
        assert_eq!(PROVIDER.key_env, None, "the catalog needs no credential");
        assert!(
            PROVIDER.env_vars.is_empty(),
            "no endpoint or key variable is read any more"
        );
        assert_eq!(PROVIDER.base_url, "https://api.catalog.azureml.ms");
        assert_eq!(
            PROVIDER.openai_base_url, None,
            "inference stays per-resource, so no chat base is published"
        );
    }

    #[tokio::test]
    async fn fetch_posts_without_a_credential_and_follows_the_token() {
        use std::io::{Read as _, Write as _};

        const PAGE_2: &str = r#"{ "summaries": [ { "name": "gpt-5.4", "publisher": "OpenAI",
          "inferenceTasks": ["responses"] } ], "continuationToken": null }"#;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in [
                PAGE_1.replace(
                    "\"continuationToken\": null",
                    "\"continuationToken\": \"page-2\"",
                ),
                PAGE_2.to_owned(),
            ] {
                let (mut stream, _) = listener.accept().expect("accept fixture client");
                // Read the request before replying, as in the sheet.rs
                // fixture server: answering early is a protocol error.
                // The read timeout bounds the capture.
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
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write fixture response");
                requests.push(String::from_utf8_lossy(&request).into_owned());
            }
            requests
        });
        let client = reqwest::Client::new();
        let entries = fetch(&client, &format!("http://{addr}"), None)
            .await
            .expect("a keyless fetch with no credential must succeed");
        let requests = server.join().expect("fixture server must finish");
        assert_eq!(
            requests.len(),
            2,
            "the fetch follows the continuation token"
        );
        let (first, second) = (&requests[0], &requests[1]);
        assert!(
            first.starts_with("POST /asset-gallery/v1.0/models"),
            "the catalog is a POST: {first}"
        );
        for request in &requests {
            assert!(
                !request.to_lowercase().contains("authorization"),
                "a keyless provider sends no credential: {request}"
            );
        }
        assert!(
            second.contains("page-2"),
            "the second request carries the token: {second}"
        );
        assert_eq!(entries.len(), 5, "both pages land in one list");
        assert_eq!(entries[4].id, "gpt-5.4");
        assert_eq!(entries[4].family, "openai");
    }
}
