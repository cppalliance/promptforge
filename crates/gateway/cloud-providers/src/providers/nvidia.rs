//! NVIDIA provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://integrate.api.nvidia.com` - no auth
//! on the listing endpoint (verified live 2026-09-14) and the OpenAI
//! list envelope carrying IDs only: namespaced slugs like
//! `meta/llama-3.1-8b-instruct`, with `created` a constant placeholder
//! on every entry, so it never becomes `released_at`. No pagination, no
//! context window, no capability or pricing fields.
//!
//! Docs: <https://build.nvidia.com/llms.txt>

use gateway_api::{ModelEntry, Tier};
use serde::Deserialize;

use crate::providers::openai_shape::{ListResponse, base_entry};
use crate::{FetchError, Provider};

/// The NVIDIA provider descriptor: keyless - the model-list endpoint
/// needs no credential.
pub const PROVIDER: Provider = Provider {
    name: "nvidia",
    display_name: "NVIDIA",
    tier: Tier::Subprime,
    key_env: None,
    base_url: "https://integrate.api.nvidia.com/v1",
    openai_base_url: Some("https://integrate.api.nvidia.com/v1"),
    env_vars: &[],
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetches and normalizes NVIDIA's model list in a single request; the
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
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// One model as the wire reports it. Only the id carries sheet meaning:
/// `created` is a constant placeholder on every entry and is never
/// parsed, so it cannot leak into `released_at`; `owned_by`,
/// `permission`, `root`, and `parent` carry no sheet meaning.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
}

/// Normalizes one wire model: the endpoint is IDs-only, so the entry is
/// the conservative base with no release date.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, None)
}

/// Sets every entry's family to the vendor prefix of its
/// `vendor/model` id (`meta`, `nvidia`, `google`, ...), and to the
/// whole id when there is no slash. There is no snapshot or SKU
/// collapse: the catalog carries neither.
pub(crate) fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = crate::taxonomy::vendor_prefix(&entry.id)
            .map_or_else(|| entry.id.clone(), |(vendor, _)| vendor.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The verified 2026-09-14 live payload shape: namespaced IDs and
    /// the same constant placeholder `created` on every entry.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "meta/llama-3.1-8b-instruct",
      "object": "model",
      "created": 1750000000,
      "owned_by": "meta",
      "permission": [],
      "root": "meta/llama-3.1-8b-instruct",
      "parent": null
    },
    {
      "id": "nvidia/llama-3.3-nemotron-super-49b",
      "object": "model",
      "created": 1750000000,
      "owned_by": "nvidia",
      "permission": [],
      "root": "nvidia/llama-3.3-nemotron-super-49b",
      "parent": null
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn namespaced_ids_normalize_to_conservative_entries() {
        let entries = entries();
        assert_eq!(entries.len(), 2);
        let entry = &entries[0];
        assert_eq!(entry.id, "meta/llama-3.1-8b-instruct");
        assert_eq!(
            entry.display_name, "meta/llama-3.1-8b-instruct",
            "the id doubles as the display name"
        );
        assert_eq!(entry.kind, gateway_api::ModelKind::Chat);
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn placeholder_created_never_becomes_released_at() {
        for entry in entries() {
            assert_eq!(
                entry.released_at, None,
                "the constant placeholder `created` must not publish a release date: {}",
                entry.id
            );
        }
    }

    /// Trimmed 2026-09-14 sheet excerpt: real NVIDIA ids across vendors.
    const FIXTURE: &str = include_str!("../../tests/fixtures/2026-09-14-nvidia.json");

    #[test]
    fn fixture_ids_classify_into_vendor_families() {
        let mut entries = crate::taxonomy::fixture::entries(FIXTURE);
        apply_taxonomy(&mut entries);
        let by_id: std::collections::BTreeMap<String, ModelEntry> = entries
            .into_iter()
            .map(|entry| (entry.id.clone(), entry))
            .collect();
        let table: &[(&str, &str)] = &[
            ("meta/llama-3.2-11b-vision-instruct", "meta"),
            ("nvidia/llama-3.1-nemotron-70b-instruct", "nvidia"),
            ("nvidia/nvclip", "nvidia"),
            ("google/gemma-3-12b-it", "google"),
            ("deepseek-ai/deepseek-v4-flash-0731", "deepseek-ai"),
            ("moonshotai/kimi-k3", "moonshotai"),
            ("openai/gpt-oss-20b", "openai"),
            ("mistralai/mistral-large", "mistralai"),
            ("z-ai/glm-5.3-flash", "z-ai"),
            ("01-ai/yi-large", "01-ai"),
            ("snowflake/arctic-embed-l", "snowflake"),
            ("writer/palmyra-med-70b", "writer"),
        ];
        for &(id, family) in table {
            assert_eq!(by_id[id].family, family, "{id}");
        }
        for entry in by_id.values() {
            assert!(
                entry.variant_of.is_none(),
                "no variant collapse for the aggregator: {}",
                entry.id
            );
        }
    }

    #[test]
    fn descriptor_publishes_the_chat_base_and_stays_keyless() {
        assert_eq!(
            PROVIDER.openai_base_url,
            Some("https://integrate.api.nvidia.com/v1")
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
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "meta/llama-3.1-8b-instruct");
    }
}
