//! Catalog transport: fetching and decoding gateway `GET /v1/models`.

use std::num::NonZeroU32;

use promptforge::model::{CompletionError, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use serde::Deserialize;

use crate::failure::{malformed, transport_failure};
use crate::wire::classify::classify_http_failure;
use crate::wire::stream::escape_controls;

/// Wire shape of one entry from gateway `GET /v1/models`.
///
/// The list mixes inference models with the gateway's speech-to-text models,
/// which have only `id`, `object`, and `kind` because they answer no
/// completion request. The inference fields are therefore optional at the
/// wire, and an entry without a context window is skipped rather than
/// failing the whole catalog.
#[derive(Debug, Deserialize)]
struct ModelsListEntry {
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    context: Option<u32>,
    #[serde(default)]
    thinking: Option<ThinkingMode>,
}

/// Wire shape of gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelsListEntry>,
}

/// The largest gateway error body kept for a catalog-fetch diagnostic, in bytes.
const MAX_CATALOG_ERROR_BODY: usize = 2000;

/// The largest success-path model-catalog body accepted before decoding, in
/// bytes. A gateway that returns more than this is refused rather than buffered
/// unbounded, mirroring the bound the error path already applies. Sized well
/// above any realistic model list (16 MiB) so legitimate catalogs are unaffected.
const MAX_CATALOG_BODY: u64 = 16 * 1024 * 1024;

/// Reads a success-path response body, refusing it once it would exceed `cap`
/// bytes so a decode cannot buffer an unbounded body first.
///
/// The advertised `Content-Length` short-circuits an oversize body, and the
/// streamed chunks are bounded so a gateway that omits or lies about the length
/// still cannot force an unbounded allocation.
async fn read_catalog_body_capped(
    mut response: reqwest::Response,
    cap: u64,
) -> std::result::Result<Vec<u8>, CompletionError> {
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(malformed(format!(
            "model list body of {len} bytes exceeds the {cap}-byte limit"
        )));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_failure)? {
        if body.len() as u64 + chunk.len() as u64 > cap {
            return Err(malformed(format!(
                "model list body exceeds the {cap}-byte limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads at most `limit` bytes of a non-success response body, stopping early so
/// an oversized error body cannot exhaust memory.
///
/// A read failure is returned as the concrete [`reqwest::Error`] (MODEL-010) so
/// the caller can retain it as an error-chain `#[source]`, rather than being
/// flattened into display text that severs the cause.
async fn read_error_body_bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> std::result::Result<String, reqwest::Error> {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < limit {
        match response.chunk().await? {
            Some(chunk) => {
                let take = (limit - buffer.len()).min(chunk.len());
                buffer.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            None => break,
        }
    }
    if buffer.is_empty() {
        return Ok("(empty body)".to_owned());
    }
    // F5: escape control characters so a hostile catalog error body cannot forge
    // log lines or smuggle terminal control sequences into a diagnostic.
    let lossy = String::from_utf8_lossy(&buffer);
    let mut escaped = String::with_capacity(lossy.len());
    for ch in lossy.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    Ok(escaped)
}

/// Returns the process-wide catalog HTTP client, building it once on first use.
///
/// A single reusable client (MODEL-018) lets catalog fetches share one
/// connection pool and transport configuration rather than each constructing a
/// throwaway client with its own pool. The returned handle is a cheap clone of
/// the shared client (its state is reference-counted internally).
fn catalog_client() -> reqwest::Client {
    static CATALOG_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CATALOG_CLIENT.get_or_init(reqwest::Client::new).clone()
}

/// Sends a bearer-authed GET through the shared client (MODEL-018) and
/// returns the success response, classifying every failure the same way for
/// each gateway endpoint: `Transport` (or `Timeout`) when the send fails, the
/// classified kind with a bounded, control-escaped body as its detail on a
/// non-success status (MODEL-010: no unbounded buffering), and `Transport`
/// (or `Timeout`) when that error body cannot be read, keeping the
/// [`reqwest::Error`] as a typed source under the same timeout marking as a
/// send failure.
async fn get_authed(
    url: String,
    token: &str,
) -> std::result::Result<reqwest::Response, CompletionError> {
    let response = catalog_client()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(transport_failure)?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = match read_error_body_bounded(response, MAX_CATALOG_ERROR_BODY).await {
        Ok(body) => body,
        Err(source) => return Err(transport_failure(source)),
    };
    Err(classify_http_failure(status.as_u16(), &body))
}

/// Fetches a [`ModelCatalog`] from the Gateway's `/models` endpoint.
///
/// `base_url` is the root of the Gateway's OpenAI-compatible API, for
/// example `http://127.0.0.1:8081/v1`. The request sends `token` as a
/// bearer token.
///
/// # Errors
/// Returns a [`CompletionError`]. Its [`kind`](CompletionError::kind) is:
///
/// - `Transport` or `Timeout` when the HTTP request or a response read fails;
/// - the kind [`classify_http_failure`] picks for the response when the
///   Gateway returns a non-success status;
/// - `MalformedResponse` when the body is not a valid model list.
pub async fn fetch_model_catalog(
    base_url: &str,
    token: &str,
) -> std::result::Result<ModelCatalog, CompletionError> {
    let base = base_url.trim_end_matches('/');
    let response = get_authed(format!("{base}/models"), token).await?;
    // Bound the success body BEFORE decoding so an oversized (or unbounded)
    // model list cannot exhaust memory, matching the bound the error path applies.
    let body = read_catalog_body_capped(response, MAX_CATALOG_BODY).await?;
    // A body that does not decode as a model list is a malformed response, not a
    // transport failure - matching this function's documented error contract.
    let list: ModelsListResponse = serde_json::from_slice(&body).map_err(|error| {
        // MODEL-009: keep the decode error as a private `#[source]` cause instead
        // of flattening it into the message, while the classification stays
        // `MalformedResponse`.
        malformed("model list response was not valid JSON").with_source(error)
    })?;
    let mut descriptors = Vec::with_capacity(list.data.len());
    for entry in list.data {
        // An entry with no context window is not an inference model (the
        // gateway lists its transcription models here too); it is not a
        // descriptor and must not fail the catalog.
        let Some(context) = entry.context else {
            continue;
        };
        let id = ModelId::gateway(entry.id).map_err(|error| {
            malformed(format!("model catalog entry has an invalid id: {error}"))
        })?;
        let context = NonZeroU32::new(context).ok_or_else(|| {
            malformed("a model declares a zero-token context window")
                .with_detail(escape_controls(id.name(), MAX_CATALOG_ERROR_BODY))
        })?;
        let thinking = entry.thinking.ok_or_else(|| {
            malformed("a model declares a context window but no thinking mode")
                .with_detail(escape_controls(id.name(), MAX_CATALOG_ERROR_BODY))
        })?;
        descriptors.push(ModelDescriptor::new(
            id,
            entry.description,
            context,
            thinking,
        ));
    }
    ModelCatalog::new(descriptors).map_err(|error| {
        malformed("gateway returned an inconsistent model catalog")
            .with_detail(escape_controls(&error.to_string(), MAX_CATALOG_ERROR_BODY))
    })
}

#[cfg(test)]
#[path = "catalog-tests.rs"]
mod tests;
