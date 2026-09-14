//! Amazon Bedrock provider: the public descriptor plus the private
//! variance of `GET /foundation-models` under
//! `https://bedrock.{region}.amazonaws.com` - hand-assembled SigV4
//! HMAC-SHA256 auth - and normalization of `modelSummaries`:
//! modalities into kind and input flags, `modelLifecycle` into
//! `Deprecation`. The endpoint reports no context window, no
//! max-output field, no pricing, and no release date.
//!
//! Extra environment variables beyond the descriptor's
//! `AWS_ACCESS_KEY_ID`: `AWS_SECRET_ACCESS_KEY` (required - the SigV4
//! signing key; absent records `unavailable`) and `AWS_REGION`
//! (optional, default `us-east-1`).
//!
//! Docs: <https://docs.aws.amazon.com/bedrock/latest/APIReference/API_ListFoundationModels.html>

use hmac::{Hmac, KeyInit, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use shared_gateway_api::{Deprecation, ModelEntry, ModelKind, Tier};
use time::OffsetDateTime;

use crate::providers::openai_shape::base_entry;
use crate::{FetchError, Provider};

/// Environment variable the access key id arrives under; matches the
/// GitHub secret name.
const KEY_ENV: &str = "AWS_ACCESS_KEY_ID";

/// Environment variable carrying the SigV4 signing key.
const SECRET_ENV: &str = "AWS_SECRET_ACCESS_KEY";

/// Environment variable carrying the signing and endpoint region.
const REGION_ENV: &str = "AWS_REGION";

/// The region used when `AWS_REGION` is unset.
const DEFAULT_REGION: &str = "us-east-1";

/// The SigV4 service name.
const SERVICE: &str = "bedrock";

/// The list path under the base URL.
const MODELS_PATH: &str = "/foundation-models";

/// The Amazon Bedrock provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "bedrock",
    display_name: "Amazon Bedrock",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://bedrock.us-east-1.amazonaws.com",
};

/// Fetch and normalize Bedrock's foundation-model list with a
/// SigV4-signed request; the secret key and region are private env reads.
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
    let Some(secret) = std::env::var(SECRET_ENV)
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return Err(FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: SECRET_ENV,
        });
    };
    let region = region_from(std::env::var(REGION_ENV).ok());
    fetch_signed(client, base_url, key, &secret, &region).await
}

/// The signing and endpoint region: `AWS_REGION` when set and
/// non-blank, else the default.
fn region_from(env_value: Option<String>) -> String {
    env_value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_owned())
}

/// The endpoint base for the region: the descriptor's default-region
/// base URL with the region segment swapped. A base URL without the
/// default region (a fixture server) is used unchanged.
fn endpoint_base(base_url: &str, region: &str) -> String {
    if region == DEFAULT_REGION {
        base_url.to_owned()
    } else {
        base_url.replacen(DEFAULT_REGION, region, 1)
    }
}

/// The signed list request and normalization, with credentials and
/// region as parameters so tests need no environment.
async fn fetch_signed(
    client: &reqwest::Client,
    base_url: &str,
    key: &str,
    secret: &str,
    region: &str,
) -> Result<Vec<ModelEntry>, FetchError> {
    let url = format!("{}{MODELS_PATH}", endpoint_base(base_url, region));
    let amz_date = amz_date(OffsetDateTime::now_utc());
    let authorization = sigv4_sign_get(
        host_of(&url),
        MODELS_PATH,
        "",
        region,
        SERVICE,
        key,
        secret,
        &amz_date,
    );
    let list: ListResponse = client
        .get(&url)
        .header("x-amz-date", &amz_date)
        .header(reqwest::header::AUTHORIZATION, authorization)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(list.model_summaries.iter().map(normalize_model).collect())
}

/// The SigV4 basic-format amz date; the signer re-derives the date
/// stamp from its first eight characters.
fn amz_date(now: OffsetDateTime) -> String {
    let date = now.date();
    let time = now.time();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year(),
        u8::from(date.month()),
        date.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

/// The host (with any non-default port) a request to `url` carries.
fn host_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    after_scheme.split('/').next().unwrap_or(after_scheme)
}

/// Sign a GET request per AWS Signature Version 4, returning the
/// `Authorization` header value. `query` is the canonical query string
/// (name-sorted, URI-encoded); the Bedrock list endpoint takes none.
#[allow(clippy::too_many_arguments)]
fn sigv4_sign_get(
    host: &str,
    path: &str,
    query: &str,
    region: &str,
    service: &str,
    access_key: &str,
    secret_key: &str,
    amz_date: &str,
) -> String {
    let payload_hash = hex_lower(&Sha256::digest(b""));
    let canonical_request = format!(
        "GET\n{path}\n{query}\nhost:{host}\nx-amz-date:{amz_date}\n\nhost;x-amz-date\n{payload_hash}"
    );
    let date_stamp = &amz_date[..8];
    let scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex_lower(&Sha256::digest(canonical_request.as_bytes()))
    );
    let k_date = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    let signature = hex_lower(&hmac_sha256(&k_signing, string_to_sign.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders=host;x-amz-date, Signature={signature}"
    )
}

/// HMAC-SHA256 over `data` under `key`.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC-SHA256 accepts a key of any length"));
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// Lowercase hex, as SigV4 renders hashes and signatures.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

/// The list envelope.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListResponse {
    model_summaries: Vec<WireModel>,
}

/// One model summary as the wire reports it. `modelArn`,
/// `providerName`, `responseStreamingSupported`,
/// `customizationsSupported`, and `inferenceTypesSupported` carry no
/// sheet meaning and are not parsed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireModel {
    model_id: String,
    model_name: Option<String>,
    input_modalities: Option<Vec<String>>,
    output_modalities: Option<Vec<String>>,
    model_lifecycle: Option<WireLifecycle>,
}

/// The lifecycle block: `ACTIVE`, `LEGACY`, or `DEPRECATED`.
#[derive(Debug, Deserialize)]
struct WireLifecycle {
    status: String,
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.model_id, None);
    if let Some(name) = &model.model_name {
        entry.display_name.clone_from(name);
    }
    let inputs = model.input_modalities.as_deref().unwrap_or_default();
    let outputs = model.output_modalities.as_deref().unwrap_or_default();
    entry.kind = model_kind(outputs);
    entry.images = inputs.iter().any(|modality| modality == "IMAGE");
    entry.video_input = inputs.iter().any(|modality| modality == "VIDEO");
    if let Some(lifecycle) = &model.model_lifecycle
        && lifecycle.status != "ACTIVE"
    {
        entry.deprecation = Some(Deprecation {
            status: lifecycle.status.clone(),
            date: None,
            replacement: None,
        });
    }
    entry
}

/// The workload from the output modality: `EMBEDDING` is an embedding
/// model, `IMAGE` an image model; everything else is chat.
fn model_kind(output_modalities: &[String]) -> ModelKind {
    if output_modalities.iter().any(|m| m == "EMBEDDING") {
        ModelKind::Embedding
    } else if output_modalities.iter().any(|m| m == "IMAGE") {
        ModelKind::Image
    } else {
        ModelKind::Chat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented `modelSummaries` shape (2026-09-14 research
    /// extraction, AWS ListFoundationModels docs): an active multimodal
    /// Nova, an embedding model, and a legacy model.
    const LIST: &str = r#"{
  "modelSummaries": [
    {
      "modelId": "amazon.nova-pro-v1:0",
      "modelArn": "arn:aws:bedrock:us-east-1::foundation-model/amazon.nova-pro-v1:0",
      "modelName": "Nova Pro",
      "providerName": "Amazon",
      "inputModalities": ["TEXT", "IMAGE", "VIDEO"],
      "outputModalities": ["TEXT"],
      "responseStreamingSupported": true,
      "modelLifecycle": { "status": "ACTIVE" }
    },
    {
      "modelId": "amazon.titan-embed-text-v2:0",
      "modelName": "Titan Embeddings G1 - Text",
      "providerName": "Amazon",
      "inputModalities": ["TEXT"],
      "outputModalities": ["EMBEDDING"],
      "modelLifecycle": { "status": "ACTIVE" }
    },
    {
      "modelId": "anthropic.claude-v2:1",
      "modelName": "Claude",
      "providerName": "Anthropic",
      "inputModalities": ["TEXT"],
      "outputModalities": ["TEXT"],
      "modelLifecycle": { "status": "LEGACY" }
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let list: ListResponse = serde_json::from_str(LIST).expect("fixture must parse as a list");
        list.model_summaries.iter().map(normalize_model).collect()
    }

    /// Sign with the AWS SigV4 test-suite credentials, host, and date.
    fn sign_vector(path: &str, query: &str) -> String {
        sigv4_sign_get(
            "example.amazonaws.com",
            path,
            query,
            "us-east-1",
            "service",
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830T123600Z",
        )
    }

    /// AWS's published SigV4 test-suite vector `get-vanilla`: an empty
    /// path-less GET against the example host.
    #[test]
    fn sigv4_matches_aws_get_vanilla_vector() {
        assert_eq!(
            sign_vector("/", ""),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    /// AWS's published SigV4 test-suite vector
    /// `get-vanilla-query-order-key-case`: the canonical query string is
    /// name-sorted regardless of wire order.
    #[test]
    fn sigv4_matches_aws_sorted_query_vector() {
        assert_eq!(
            sign_vector("/", "Param1=value1&Param2=value2"),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
        );
    }

    #[test]
    fn region_defaults_to_us_east_1() {
        assert_eq!(region_from(None), "us-east-1");
        assert_eq!(
            region_from(Some(String::new())),
            "us-east-1",
            "blank falls back"
        );
        assert_eq!(region_from(Some("eu-west-1".to_owned())), "eu-west-1");
    }

    #[test]
    fn endpoint_base_swaps_only_the_region_segment() {
        let base = PROVIDER.base_url;
        assert_eq!(
            endpoint_base(base, "eu-west-1"),
            "https://bedrock.eu-west-1.amazonaws.com"
        );
        assert_eq!(
            endpoint_base(base, "us-east-1"),
            base,
            "default region keeps the URL"
        );
        assert_eq!(
            endpoint_base("http://127.0.0.1:8080", "eu-west-1"),
            "http://127.0.0.1:8080",
            "no region segment to swap"
        );
    }

    #[test]
    fn active_chat_model_maps_modalities() {
        let entry = &entries()[0];
        assert_eq!(entry.id, "amazon.nova-pro-v1:0");
        assert_eq!(
            entry.display_name, "Nova Pro",
            "modelName is the display name"
        );
        assert_eq!(entry.kind, ModelKind::Chat);
        assert!(entry.images, "IMAGE in inputModalities maps to images");
        assert!(
            entry.video_input,
            "VIDEO in inputModalities maps to video_input"
        );
        assert_eq!(entry.context_window, None, "the endpoint reports no limits");
        assert_eq!(entry.max_output, None);
        assert_eq!(entry.released_at, None, "the endpoint reports no date");
        assert!(entry.pricing.is_none());
        assert!(
            entry.deprecation.is_none(),
            "ACTIVE lifecycle records no deprecation"
        );
    }

    #[test]
    fn embedding_output_maps_to_embedding_kind() {
        let entry = &entries()[1];
        assert_eq!(entry.kind, ModelKind::Embedding);
        assert!(!entry.images && !entry.video_input);
    }

    #[test]
    fn non_active_lifecycle_maps_to_deprecation() {
        let entry = &entries()[2];
        let deprecation = entry.deprecation.as_ref().expect("LEGACY must map");
        assert_eq!(
            deprecation.status, "LEGACY",
            "the provider's own lifecycle label is kept"
        );
        assert_eq!(
            deprecation.date, None,
            "the endpoint reports no sunset date"
        );
        assert_eq!(deprecation.replacement, None);
    }

    #[test]
    fn absent_model_name_falls_back_to_id() {
        let list: ListResponse = serde_json::from_str(
            r#"{ "modelSummaries": [ { "modelId": "amazon.nova-lite-v1:0" } ] }"#,
        )
        .expect("fixture must parse");
        let entry = normalize_model(&list.model_summaries[0]);
        assert_eq!(entry.display_name, "amazon.nova-lite-v1:0");
        assert_eq!(entry.kind, ModelKind::Chat);
        assert!(entry.deprecation.is_none());
    }

    #[tokio::test]
    async fn fetch_sends_a_signed_request() {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            // Read the request first, as in the sheet.rs fixture server.
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
        let entries = fetch_signed(
            &client,
            &format!("http://{addr}"),
            "test-access-key",
            "test-secret-key",
            "us-east-1",
        )
        .await
        .expect("a signed fetch against the fixture must succeed");
        let request = String::from_utf8(server.join().expect("fixture server must finish"))
            .expect("the request is ASCII");
        assert!(
            request.contains("authorization: AWS4-HMAC-SHA256 Credential=test-access-key/"),
            "the request carries a SigV4 authorization header: {request}"
        );
        assert!(
            request.contains("SignedHeaders=host;x-amz-date"),
            "the signature covers host and the amz date: {request}"
        );
        assert!(
            request.contains("x-amz-date:"),
            "the request carries the signing date: {request}"
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, "amazon.nova-pro-v1:0");
    }
}
