//! The OpenAI passthrough surface: chat completions, embeddings, and
//! rerank, plus the typed SSE relay and the live-table model resolver
//! they share.

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};

use crate::AppState;
use crate::auth::{Caller, check_auth};
use crate::error::GatewayError;
use crate::wire::{
    ChatRequest, EmbeddingRequest, EmbeddingResponse, RerankRequest, RerankResponse,
};
use gateway_config::ModelKind;

/// Header naming the caller for fair queue scheduling. Absent → `"default"`.
pub(crate) const CLIENT_HEADER: &str = "X-PromptForge-Client";

/// Resolves a request's model name against the live routing table.
///
/// A local model the boot load is spawning (one in [`LiveState::loading`])
/// earns [`GatewayError::ModelLoading`]: a 503 with `Retry-After`, because
/// the load will land it. A configured but not-yet-loaded model - one the
/// catalog names while the boot load is still downloading - earns a 503
/// naming the active queue command rather than a bare 404, so the caller
/// knows to retry once the command completes. With no command active the
/// miss is [`GatewayError::UnknownModel`], exactly as before the queue
/// existed.
pub(crate) async fn resolve_routed_model(
    state: &AppState,
    name: &str,
) -> Result<Arc<crate::routing::Model>, GatewayError> {
    let live = state.live.read().await;
    match live.routing.model(name) {
        Ok(model) => Ok(model),
        Err(unknown) => {
            if live.loading.contains(name) {
                return Err(GatewayError::ModelLoading(name.to_owned()));
            }
            let configured = live
                .config
                .models()
                .iter()
                .any(|model| model.name() == name)
                || live
                    .config
                    .catalog_local_models()
                    .iter()
                    .any(|model| model.name() == name);
            if configured && let Some(active) = state.commands.active_command() {
                return Err(GatewayError::ModelProvisioning(active.name));
            }
            Err(unknown)
        }
    }
}

/// The chat route to a backend.
pub(crate) async fn chat_completions(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<ChatRequest>,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Chat)?;
    let client_id = crate::queue::ClientId::from_header(
        caller
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    // Emulated dialects rewrite the request (guide injection, tool stripping)
    // and parse the reply's content fences. The fence parse needs the whole
    // reply, so the emulated streaming path buffers one non-streaming
    // upstream round trip and re-emits the rewritten response as synthetic
    // chunks; without it an always-streaming caller would silently lose
    // tool calling on this dialect.
    let emulated = model.tool_dialect == crate::dialect::GEMMA3_TOOL_CODE;
    let request = if emulated {
        let mut request = request;
        crate::dialect::prepare_request(&mut request)?;
        request
    } else {
        request
    };
    if request.stream {
        if emulated {
            let mut buffered = request;
            buffered.stream = false;
            // Streaming-only options must not reach a non-streaming upstream
            // call; the synthetic summary chunk restores the usage the
            // caller asked `stream_options.include_usage` for.
            buffered.rest.remove("stream_options");
            let response = model
                .endpoint
                .upstream
                .send(buffered, &model.upstream_name)
                .await?;
            response
                .validate()
                .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
            let mut response = response;
            crate::dialect::apply_response(&mut response, &model.name);
            return Ok(relay_sse(crate::dialect::response_chunks(response), permit));
        }
        // A failure here is before the SSE response starts, so it is
        // consumed as a normal JSON error, never a stream that dies
        // mid-flight.
        let streamed = model
            .endpoint
            .upstream
            .stream(request, &model.upstream_name)
            .await?;
        return Ok(relay_sse(streamed, permit));
    }
    let response = model
        .endpoint
        .upstream
        .send(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
    let mut response = response;
    if emulated {
        crate::dialect::apply_response(&mut response, &model.name);
    }
    Ok(Json(response).into_response())
}

/// Re-emit a validated upstream chunk stream as an SSE response, holding the
/// dominion queue permit for the stream's lifetime.
///
/// The relay is typed: each upstream chunk is validated and re-serialized per
/// chunk rather than splicing upstream bytes through. A mid-stream failure is
/// emitted as an error-envelope `data:` event before the stream ends, and a
/// clean end is marked with the `data: [DONE]` sentinel. The response
/// forwards the upstream `Content-Type`/`Cache-Control` when present,
/// defaulting to `text/event-stream`/`no-cache`.
///
/// Client-disconnect cancellation is Drop all the way down: when the client
/// goes away the response body is dropped, which drops the chunk stream,
/// which drops the upstream response and aborts the upstream connection,
/// releasing the permit in the same unwind. There is no explicit cancel path.
pub(crate) fn relay_sse(
    streamed: crate::upstream::StreamedChunks,
    permit: crate::queue::Permit,
) -> Response {
    use futures_util::StreamExt as _;

    let relayed = futures_util::stream::unfold(
        (streamed.chunks, false, permit),
        |(mut chunks, failed, permit)| async move {
            if failed {
                return None;
            }
            let (line, failed) = match chunks.next().await? {
                Ok(chunk) => match serde_json::to_string(&chunk) {
                    Ok(json) => (format!("data: {json}\n\n"), false),
                    Err(error) => (
                        format!(
                            "data: {}\n\n",
                            GatewayError::upstream_protocol(error).envelope()
                        ),
                        true,
                    ),
                },
                Err(error) => (format!("data: {}\n\n", error.envelope()), true),
            };
            Some((
                Ok::<String, std::convert::Infallible>(line),
                (chunks, failed, permit),
            ))
        },
    );
    let done = futures_util::stream::once(async { Ok("data: [DONE]\n\n".to_owned()) });
    let mut response = Response::new(Body::from_stream(relayed.chain(done)));
    let headers = response.headers_mut();
    let content_type = streamed
        .content_type
        .and_then(|value| HeaderValue::from_str(&value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_TYPE, content_type);
    let cache_control = streamed
        .cache_control
        .and_then(|value| HeaderValue::from_str(&value).ok())
        .unwrap_or_else(|| HeaderValue::from_static("no-cache"));
    headers.insert(CACHE_CONTROL, cache_control);
    response
}

/// The embeddings route to a backend: the same auth, routing, kind guard, and
/// dominion queue admission as chat, for `kind = "embedding"` models.
pub(crate) async fn embeddings(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<EmbeddingRequest>,
) -> Result<Json<EmbeddingResponse>, GatewayError> {
    check_auth(&state, &caller).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Embedding)?;
    let client_id = crate::queue::ClientId::from_header(
        caller
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    let response = model
        .endpoint
        .upstream
        .send_embeddings(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
    Ok(Json(response))
}

/// The rerank route to a backend: the same auth, routing, kind guard, and
/// dominion queue admission as chat, for `kind = "classifier"` models.
pub(crate) async fn rerank(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<RerankRequest>,
) -> Result<Json<RerankResponse>, GatewayError> {
    check_auth(&state, &caller).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Classifier)?;
    let client_id = crate::queue::ClientId::from_header(
        caller
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let _permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    let response = model
        .endpoint
        .upstream
        .send_rerank(request, &model.upstream_name)
        .await?;
    response
        .validate()
        .map_err(|reason| GatewayError::upstream_protocol(std::io::Error::other(reason)))?;
    Ok(Json(response))
}
