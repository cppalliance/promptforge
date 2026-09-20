//! The speech surface: the `audio/speech` byte-passthrough relay with its
//! bounded background forwarding task, the voices catalog, and the relay's
//! named bounds.

use std::collections::BTreeSet;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{FromRequest, Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, Method};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use gateway_config::ModelKind;
use gateway_protocol::ProtocolError;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::{GatewayError, WireJson};
use crate::registry::RouteInfo;
use crate::relay::{CLIENT_HEADER, resolve_routed_model};
use crate::wire::{SpeechRequest, SpeechResponseFormat, SpeechStreamFormat, SpeechVoice};

const AUDIO_SPEECH: RouteInfo = RouteInfo::open("/v1/audio/speech", &[Method::POST]);
const AUDIO_VOICES: RouteInfo = RouteInfo::open("/v1/audio/voices", &[Method::GET]);

/// The speech routes, as the registry sees them.
pub(crate) const ROUTES: &[RouteInfo] = &[AUDIO_SPEECH, AUDIO_VOICES];

/// The speech routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route(AUDIO_SPEECH.path, post(audio_speech))
        .route(AUDIO_VOICES.path, get(audio_voices))
}

/// The speech route to a backend: the same auth, routing, kind guard, and
/// dominion queue admission as chat, for `kind = "speech"` models.
///
/// Two deliberate departures from the other routes. First, auth runs before
/// body extraction: the handler takes the [`AuthedCaller`] parts extractor,
/// which runs the auth rules while the parts are extracted, and only then
/// extracts `WireJson<SpeechRequest>` from the raw [`Request`] by hand, so
/// an unauthorized caller never makes the gateway parse a body. Second, the
/// reply is a byte passthrough, not a typed relay: audio frames are opaque
/// bytes the gateway cannot re-validate per chunk, so the upstream body is
/// forwarded unread - the one departure from the gateway's typed-relay
/// norm, the same trade [`crate::relay::relay_sse`] documents for its own
/// design. Because the response is a long-lived byte stream, this route
/// must never sit under a `CompressionLayer` or a whole-request
/// `TimeoutLayer`: both buffer or kill long-lived streams. The stream runs
/// under the bounded background relay [`relay_audio`] documents, so every
/// early end - a tripped bound or an upstream failure - fails the client's
/// body read rather than truncating it.
pub(crate) async fn audio_speech(
    State(state): State<AppState>,
    caller: AuthedCaller,
    request: Request,
) -> Result<Response, GatewayError> {
    let WireJson(request) = WireJson::<SpeechRequest>::from_request(request, &state).await?;
    request
        .validate()
        .map_err(|reason| GatewayError::MalformedRequest(reason.to_owned()))?;
    let model = resolve_routed_model(&state, &request.model).await?;
    crate::routing::require_kind(&model, ModelKind::Speech)?;
    // A voice the model does not offer is a client error, so it is judged
    // before queue admission: a 400 never burns a queue slot.
    let voices = model.capabilities.voices();
    if !voices.is_empty() {
        let requested = match &request.voice {
            SpeechVoice::Name(name) => name.as_str(),
            SpeechVoice::Id { id } => id.as_str(),
            // `SpeechVoice` is `#[non_exhaustive]` in `gateway-protocol`; a
            // form this route cannot name is refused as malformed.
            _ => {
                return Err(GatewayError::MalformedRequest(
                    "voice must be a name string or an object with `id`".to_owned(),
                ));
            }
        };
        if !voices.iter().any(|voice| voice == requested) {
            return Err(GatewayError::InvalidVoice {
                voice: requested.to_owned(),
                valid: voices.to_vec(),
            });
        }
    }
    let format = request.response_format;
    let stream_format = request.stream_format;
    let client_id = crate::queue::ClientId::from_header(
        caller
            .get(CLIENT_HEADER)
            .and_then(|value| value.to_str().ok()),
    );
    let permit = model.endpoint.queue.admit(client_id.as_str()).await?;
    // A failure here is before the response starts, so it is consumed as a
    // normal JSON error, never a stream that dies mid-flight. The speech
    // path maps upstream 429/503 to its own envelope codes; every other
    // error keeps the shared protocol mapping.
    let streamed = model
        .endpoint
        .upstream
        .send_speech(request, &model.upstream_name)
        .await
        .map_err(|error| match error {
            ProtocolError::UpstreamStatus { status: 429, .. } => GatewayError::UpstreamRateLimited,
            ProtocolError::UpstreamStatus { status: 503, .. } => GatewayError::UpstreamUnavailable,
            other => GatewayError::Protocol(other),
        })?;
    Ok(relay_audio(streamed, format, stream_format, permit))
}

/// Total lifetime of one speech relay, headers to terminal end: a bound on
/// streams that would otherwise outlive every other budget by dripping.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_TOTAL_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_TOTAL_LIFETIME: Duration = Duration::from_secs(2);

/// Ceiling on the response bytes one speech relay forwards.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_BYTE_CEILING: u64 = 1 << 30;
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_BYTE_CEILING: u64 = 16 * 1024 * 1024;

/// Per-read idle budget on the opened upstream body: a silent upstream ends
/// the stream within this window. Time-to-headers is never governed here;
/// it is the upstream layer's first-response budget.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_UPSTREAM_IDLE: Duration = Duration::from_secs(30);
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_UPSTREAM_IDLE: Duration = Duration::from_millis(200);

/// Budget for one blocked channel send: a downstream that stops reading
/// backpressures the bounded channel, and the relay ends the stream rather
/// than holding the permit forever.
#[cfg(not(any(test, feature = "test-fixtures")))]
const SPEECH_RELAY_DOWNSTREAM_BLOCKED: Duration = Duration::from_secs(60);
/// Test-scaled so the relay boundary tests run in milliseconds.
#[cfg(any(test, feature = "test-fixtures"))]
const SPEECH_RELAY_DOWNSTREAM_BLOCKED: Duration = Duration::from_millis(400);

/// Data chunks buffered between the relay task and the HTTP body. The
/// channel is built one slot larger: the extra slot is reserved up front
/// for the terminal error item, so delivering it never waits on the
/// downstream.
const SPEECH_RELAY_CHANNEL_CAPACITY: usize = 4;

/// Re-emits an upstream audio byte stream as the response body, holding the
/// dominion queue permit for the stream's lifetime.
///
/// The relay is untyped on purpose: audio frames are opaque bytes, so the
/// chunks pass through unread rather than being validated and re-serialized
/// the way [`crate::relay::relay_sse`] re-emits chat chunks. The response
/// forwards the upstream `Content-Type` when present and otherwise falls
/// back to the requested format's MIME mapping (or `text/event-stream`
/// when the framing selector is `sse`); `Content-Length` is never set, so
/// hyper emits `Transfer-Encoding: chunked`.
///
/// The forwarding runs in a spawned task that owns the upstream body and
/// the permit, feeding a small bounded channel the HTTP body consumes, so
/// the permit's lifetime never depends on downstream polling. Four named
/// bounds cap the stream: [`SPEECH_RELAY_TOTAL_LIFETIME`],
/// [`SPEECH_RELAY_BYTE_CEILING`], [`SPEECH_RELAY_UPSTREAM_IDLE`], and
/// [`SPEECH_RELAY_DOWNSTREAM_BLOCKED`].
///
/// No error envelope can follow 200 plus audio bytes, so every terminal
/// path - a bound tripped, an upstream body error, the downstream gone -
/// emits exactly one `Err` item into the channel, then drops the permit:
/// the client's body read fails rather than seeing a clean EOF, the same
/// fail-rather-than-truncate trade [`crate::relay::relay_sse`] makes with
/// its mid-stream error envelope. Over the wire a body-stream error aborts
/// the response, so the item's message is server-side diagnostics; the
/// client observes a failed read. Only a stream that ran to a clean
/// upstream end inside every bound ends the body without an error item.
fn relay_audio(
    streamed: crate::upstream::StreamedAudio,
    format: SpeechResponseFormat,
    stream_format: Option<SpeechStreamFormat>,
    permit: crate::queue::Permit,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel(SPEECH_RELAY_CHANNEL_CAPACITY + 1);
    tokio::spawn(relay_speech_stream(streamed.body, tx, permit));
    let relayed = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let mut response = Response::new(Body::from_stream(relayed));
    let content_type = if streamed.content_type.is_empty() {
        speech_fallback_mime(format, stream_format)
    } else {
        HeaderValue::from_str(&streamed.content_type)
            .unwrap_or_else(|_| speech_fallback_mime(format, stream_format))
    };
    response.headers_mut().insert(CONTENT_TYPE, content_type);
    response
}

/// The relay task behind [`relay_audio`]: reads the upstream body under the
/// idle and total-lifetime budgets, forwards each chunk under the
/// blocked-delivery budget, and on every terminal path emits exactly one
/// `Err` item through the reserved channel slot before returning, which
/// drops the upstream body and the permit together. A clean upstream end
/// is the one exit with no error item.
async fn relay_speech_stream(
    mut body: futures_util::stream::BoxStream<'static, Result<Bytes, ProtocolError>>,
    tx: tokio::sync::mpsc::Sender<Result<Bytes, ProtocolError>>,
    permit: crate::queue::Permit,
) {
    use futures_util::StreamExt as _;

    // Reserve the terminal slot before the first send competes for the
    // channel: the terminal error item is delivered even when every data
    // slot is full.
    let error_slot = tx
        .clone()
        .try_reserve_owned()
        .unwrap_or_else(|_| unreachable!("a fresh channel always has capacity"));
    let deadline = tokio::time::Instant::now() + SPEECH_RELAY_TOTAL_LIFETIME;
    let mut total_bytes: u64 = 0;
    let terminal: Option<ProtocolError> = 'relay: loop {
        let item = tokio::select! {
            item = tokio::time::timeout(SPEECH_RELAY_UPSTREAM_IDLE, body.next()) => item,
            () = tokio::time::sleep_until(deadline) => {
                break 'relay Some(relay_terminal("speech relay total lifetime exceeded"));
            }
            () = tx.closed() => {
                break 'relay Some(relay_terminal("speech relay downstream gone"));
            }
        };
        let chunk = match item {
            Ok(Some(Ok(chunk))) => chunk,
            // A clean upstream end inside every bound: the one exit with no
            // error item.
            Ok(None) => break 'relay None,
            // The upstream's own mid-stream failure is the terminal item.
            Ok(Some(Err(error))) => break 'relay Some(error),
            Err(_idle) => {
                break 'relay Some(relay_terminal("speech relay upstream idle"));
            }
        };
        total_bytes += u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if total_bytes > SPEECH_RELAY_BYTE_CEILING {
            break 'relay Some(relay_terminal("speech relay byte ceiling exceeded"));
        }
        let delivered = tokio::select! {
            result = tokio::time::timeout(SPEECH_RELAY_DOWNSTREAM_BLOCKED, tx.send(Ok(chunk))) => {
                result
            }
            () = tokio::time::sleep_until(deadline) => {
                break 'relay Some(relay_terminal("speech relay total lifetime exceeded"));
            }
            () = tx.closed() => {
                break 'relay Some(relay_terminal("speech relay downstream gone"));
            }
        };
        match delivered {
            Ok(Ok(())) => {}
            // The downstream is gone mid-send or past the blocked budget.
            Ok(Err(_closed)) => {
                break 'relay Some(relay_terminal("speech relay downstream gone"));
            }
            Err(_elapsed) => {
                break 'relay Some(relay_terminal("speech relay downstream blocked"));
            }
        }
    };
    if let Some(error) = terminal {
        // The reserved slot makes this send immediate; when the downstream
        // is already gone the item is simply discarded.
        let _ = error_slot.send(Err(error));
    }
    // Explicit about the ownership the task exists for: the upstream body
    // and the dominion permit are released together on every exit, so no
    // path can end the stream while holding the slot.
    drop((body, permit));
}

/// The relay's terminal condition as a transport-classified protocol error:
/// the request may have reached the provider, and the message is
/// server-side diagnostics (a body-stream error aborts the response, so the
/// client observes a failed read, never this text).
fn relay_terminal(message: &'static str) -> ProtocolError {
    ProtocolError::transport(std::io::Error::other(message))
}

/// The `Content-Type` a speech response falls back to when the upstream
/// omits it or sends an invalid one: the framing selector first, so an SSE
/// stream is labeled `text/event-stream` and never an audio type, then the
/// requested format's MIME type (the OpenAI spellings).
fn speech_fallback_mime(
    format: SpeechResponseFormat,
    stream_format: Option<SpeechStreamFormat>,
) -> HeaderValue {
    if matches!(stream_format, Some(SpeechStreamFormat::Sse)) {
        return HeaderValue::from_static("text/event-stream");
    }
    HeaderValue::from_static(match format {
        SpeechResponseFormat::Mp3 => "audio/mpeg",
        SpeechResponseFormat::Opus => "audio/ogg",
        SpeechResponseFormat::Aac => "audio/aac",
        SpeechResponseFormat::Flac => "audio/flac",
        SpeechResponseFormat::Wav => "audio/wav",
        SpeechResponseFormat::Pcm => "audio/pcm",
        // `SpeechResponseFormat` is `#[non_exhaustive]` in `gateway-protocol`;
        // an encoding without a spelling here is labeled as opaque bytes.
        _ => "application/octet-stream",
    })
}

/// Bearer-authed union of the active profile's speech voices for host
/// bind: every `kind = "speech"` model's configured `voices`, deduplicated
/// and sorted, as id-first `{"id", "name"}` entries under
/// `{"voices": [...]}`.
///
/// OpenAI has no voice-list route; the OpenAI-compatible ecosystem
/// (Kokoro-FastAPI, vLLM-Omni, Fish Audio) converged on this one, and
/// clients such as Open WebUI read the `id` key, so the entry shape is a
/// compatibility surface pinned by the integration suite. `name` mirrors
/// `id`: the catalog configures voices as bare strings with no separate
/// display name.
pub(crate) async fn audio_voices(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let routing = state.routing().await;
    let voices = routing
        .models()
        .iter()
        .filter(|model| model.kind == ModelKind::Speech)
        .flat_map(|model| model.capabilities.voices().iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|voice| serde_json::json!({ "id": voice, "name": voice }))
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({ "voices": voices })))
}

/// Generic speech lifecycle facts included in Gateway operational status.
#[cfg(feature = "stt")]
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub(crate) struct SpeechSnapshot {
    configured: bool,
    ready: bool,
    gpu: bool,
}

#[cfg(feature = "stt")]
impl From<gateway_stt::SpeechStatus> for SpeechSnapshot {
    fn from(status: gateway_stt::SpeechStatus) -> Self {
        Self {
            configured: status.configured(),
            ready: status.ready(),
            gpu: status.gpu(),
        }
    }
}

#[cfg(test)]
#[path = "speech-tests.rs"]
mod tests;
