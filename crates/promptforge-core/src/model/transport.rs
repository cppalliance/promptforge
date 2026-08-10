//! Catalog transport: fetching and decoding gateway `GET /v1/models`.

use std::num::NonZeroU32;

use serde::Deserialize;

use super::{CompletionError, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use crate::Error;
use crate::dialects::{ToolDialectId, ToolsMode};

/// Wire shape of one entry from gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListEntry {
    id: String,
    description: String,
    context: u32,
    thinking: ThinkingMode,
    #[serde(default = "default_tool_dialect")]
    tool_dialect: ToolDialectId,
    /// Legacy wire field. Read only to validate against the dialect-derived
    /// mode; never retained, since [`ToolDialectId`] is the sole source of
    /// truth for the tools mode.
    #[serde(default)]
    tools_mode: Option<ToolsMode>,
}

fn default_tool_dialect() -> ToolDialectId {
    ToolDialectId::OpenAi
}

/// Wire shape of gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelsListEntry>,
}

/// The largest gateway error body kept for a catalog-fetch diagnostic, in bytes.
pub(crate) const MAX_CATALOG_ERROR_BODY: usize = 2000;

/// The largest success-path model-catalog body accepted before decoding, in
/// bytes. A gateway that returns more than this is refused rather than buffered
/// unbounded, mirroring the bound the error path already applies. Sized well
/// above any realistic model list (16 MiB) so legitimate catalogs are unaffected.
pub(crate) const MAX_CATALOG_BODY: u64 = 16 * 1024 * 1024;

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
        return Err(CompletionError::from(Error::MalformedResponse(format!(
            "model list body of {len} bytes exceeds the {cap}-byte limit"
        ))));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::http)? {
        if body.len() as u64 + chunk.len() as u64 > cap {
            return Err(CompletionError::from(Error::MalformedResponse(format!(
                "model list body exceeds the {cap}-byte limit"
            ))));
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

/// Fetches a [`ModelCatalog`] from a bearer-authed gateway `/models` endpoint.
///
/// `base_url` is the OpenAI-shaped API root (for example `http://127.0.0.1:8081/v1`).
///
/// # Errors
/// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
/// `Transport` on transport failure, `Backend` on a non-success status, and
/// `MalformedResponse` when the body is not a model list.
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> Result<(), promptforge_core::model::CompletionError> {
/// use promptforge_core::model::fetch_model_catalog;
///
/// let catalog = fetch_model_catalog("http://127.0.0.1:8081/v1", "secret-token").await?;
/// println!("gateway offers {} models", catalog.models().len());
/// # Ok(())
/// # }
/// ```
pub async fn fetch_model_catalog(
    base_url: &str,
    token: &str,
) -> std::result::Result<ModelCatalog, CompletionError> {
    let base = base_url.trim_end_matches('/');
    // MODEL-018: reuse one process-wide catalog client so its connection pool
    // and TLS/transport state are shared across fetches, instead of building a
    // fresh `reqwest::Client` (and a fresh pool) on every call.
    let http = catalog_client();
    let response = http
        .get(format!("{base}/models"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(Error::http)?;
    let status = response.status();
    if !status.is_success() {
        // The error body is external, so bound the read (MODEL-010: no unbounded
        // buffering) and preserve a read failure as a typed source rather than
        // flattening it into display text.
        let body = match read_error_body_bounded(response, MAX_CATALOG_ERROR_BODY).await {
            Ok(body) => body,
            Err(source) => {
                return Err(CompletionError::from(Error::BackendBodyRead {
                    status: status.as_u16(),
                    source: Box::new(source),
                }));
            }
        };
        return Err(CompletionError::from(Error::Backend {
            status: status.as_u16(),
            body,
        }));
    }
    // Bound the success body BEFORE decoding so an oversized (or unbounded)
    // model list cannot exhaust memory, matching the bound the error path applies.
    let body = read_catalog_body_capped(response, MAX_CATALOG_BODY).await?;
    // A body that does not decode as a model list is a malformed response, not a
    // transport failure - matching this function's documented error contract.
    let list: ModelsListResponse = serde_json::from_slice(&body).map_err(|error| {
        // MODEL-009: keep the decode error as a private `#[source]` cause instead
        // of flattening it into the message, while the classification stays
        // `MalformedResponse`.
        CompletionError::from(Error::MalformedResponseSource {
            message: "model list response was not valid JSON".to_owned(),
            source: Box::new(error),
        })
    })?;
    let mut descriptors = Vec::with_capacity(list.data.len());
    for entry in list.data {
        let id = ModelId::gateway(entry.id).map_err(|error| {
            CompletionError::from(Error::MalformedResponse(format!(
                "model catalog entry has an invalid id: {error}"
            )))
        })?;
        let context = NonZeroU32::new(entry.context).ok_or_else(|| {
            CompletionError::from(Error::MalformedResponse(format!(
                "model {} declares a zero-token context window",
                id.name()
            )))
        })?;
        // A legacy `tools_mode` on the wire is validated against the mode the
        // dialect derives, then discarded. A contradiction is a malformed
        // catalog, not a second stored value that could drift.
        if let Some(wire_mode) = entry.tools_mode {
            let derived = entry.tool_dialect.tools_mode();
            if wire_mode != derived {
                return Err(CompletionError::from(Error::MalformedResponse(format!(
                    "model {} wire tools_mode {wire_mode} contradicts dialect-derived {derived}",
                    id.name()
                ))));
            }
        }
        descriptors.push(
            ModelDescriptor::new(id, entry.description, context, entry.thinking)
                .with_dialect(entry.tool_dialect),
        );
    }
    ModelCatalog::new(descriptors).map_err(|error| {
        CompletionError::from(Error::MalformedResponse(format!(
            "gateway returned an inconsistent model catalog: {error}"
        )))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CompletionErrorKind;

    #[test]
    fn models_list_entry_parses_dialect_fields() {
        let json = serde_json::json!({
            "id": "gemma-local",
            "description": "A Gemma model",
            "context": 32768,
            "thinking": "never",
            "tool_dialect": "gemma3_tool_code",
            "tools_mode": "emulated"
        });
        let entry: ModelsListEntry = serde_json::from_value(json).unwrap();
        assert_eq!(entry.tool_dialect, ToolDialectId::Gemma3ToolCode);
        // tools_mode is derived from the dialect at runtime, not read from the wire.
        assert_eq!(entry.tool_dialect.tools_mode(), ToolsMode::Emulated);
    }

    #[test]
    fn models_list_entry_defaults_to_openai_native() {
        let json = serde_json::json!({
            "id": "remote",
            "description": "A remote model",
            "context": 8192,
            "thinking": "never"
        });
        let entry: ModelsListEntry = serde_json::from_value(json).unwrap();
        assert_eq!(entry.tool_dialect, ToolDialectId::OpenAi);
        assert_eq!(entry.tool_dialect.tools_mode(), ToolsMode::Native);
    }

    #[tokio::test]
    async fn fetch_model_catalog_rejects_a_wire_tools_mode_that_contradicts_the_dialect() {
        use axum::Router;
        use axum::routing::get;

        // MODEL-008: a wire `tools_mode` is validated against the mode derived
        // from `tool_dialect`. An OpenAI (native) dialect paired with an
        // `emulated` wire mode is contradictory and must be refused as malformed
        // rather than silently keeping one of the two.
        async fn models() -> axum::Json<serde_json::Value> {
            axum::Json(serde_json::json!({
                "data": [{
                    "id": "remote",
                    "description": "a remote model",
                    "context": 8192,
                    "thinking": "never",
                    "tool_dialect": "openai",
                    "tools_mode": "emulated"
                }]
            }))
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a contradictory wire tools_mode must be rejected");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            err.to_string().contains("contradicts"),
            "the rejection must name the contradiction, got {err}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_and_reports_non_success_body() {
        use axum::Router;
        use axum::routing::get;

        async fn models() -> (axum::http::StatusCode, String) {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "e".repeat(MAX_CATALOG_ERROR_BODY * 4),
            )
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a 500 response must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::Backend);
        let msg = err.to_string();
        assert!(
            msg.len() < MAX_CATALOG_ERROR_BODY + 128,
            "the error-path body must be bounded, got {} bytes",
            msg.len()
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_an_oversized_success_body() {
        use axum::Router;
        use axum::routing::get;

        // A 200 response whose body exceeds the success cap must be refused
        // BEFORE decoding, not buffered unbounded. The body is deliberately not
        // valid JSON: the bound must trip first, regardless of contents.
        async fn models() -> (axum::http::StatusCode, String) {
            let oversized = usize::try_from(MAX_CATALOG_BODY).unwrap() + 1;
            (axum::http::StatusCode::OK, "e".repeat(oversized))
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("an oversized success body must be refused");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            err.to_string().contains("exceeds"),
            "the bound must report the size limit, got {err}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_preserves_the_json_decode_source() {
        use axum::Router;
        use axum::routing::get;

        // MODEL-009: a 200 body that is not a valid model list is classified as
        // MalformedResponse, and the underlying `serde_json::Error` survives as
        // the error-chain `#[source]` rather than being flattened into the text.
        async fn models() -> (axum::http::StatusCode, String) {
            (axum::http::StatusCode::OK, "{ this is not json".to_owned())
        }
        let app = Router::new().route("/models", get(models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("an undecodable body must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        let source =
            std::error::Error::source(&err).expect("the decode error must be a preserved source");
        assert!(
            source.downcast_ref::<serde_json::Error>().is_some(),
            "the preserved source must be the JSON decode error, got {source}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_preserves_a_body_read_failure_source() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // MODEL-010: a non-success response whose body cannot be fully read
        // (the server promises a large body then drops the connection) must
        // surface as a typed transport failure that keeps the `reqwest::Error`
        // as its `#[source]`, not display text.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = "HTTP/1.1 500 Internal Server Error\r\n\
                     Content-Length: 1000000\r\n\r\n";
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(b"abc").await;
                // Socket drops here: the promised body never completes.
            }
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a truncated error body must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::Transport);
        assert_eq!(err.status(), Some(500));
        let source =
            std::error::Error::source(&err).expect("the read failure must be a preserved source");
        assert!(
            source.downcast_ref::<reqwest::Error>().is_some(),
            "the preserved source must be the reqwest read error, got {source}"
        );
    }
}
