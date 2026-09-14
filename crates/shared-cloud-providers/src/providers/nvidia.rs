//! NVIDIA provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://integrate.api.nvidia.com` - no auth
//! on the listing endpoint (verified live 2026-09-14) and the OpenAI
//! list envelope carrying IDs only: namespaced slugs like
//! `meta/llama-3.1-8b-instruct`, with `created` a constant placeholder
//! on every entry, so it never becomes `released_at`. No pagination, no
//! context window, no capability or pricing fields.
//!
//! Docs: <https://build.nvidia.com/llms.txt>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

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
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize NVIDIA's model list in a single request; the
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
    Ok(response.data.iter().map(normalize_model).collect())
}

/// One model as the wire reports it. Only the id carries sheet meaning:
/// `created` is a constant placeholder on every entry and is never
/// parsed, so it cannot leak into `released_at`; `owned_by`,
/// `permission`, `root`, and `parent` carry no sheet meaning.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
}

/// Normalize one wire model: the endpoint is IDs-only, so the entry is
/// the conservative base with no release date.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, None)
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
        assert_eq!(entry.kind, shared_gateway_api::ModelKind::Chat);
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
