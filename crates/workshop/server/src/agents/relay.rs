//! The buffered gateway relay: the `/v1/models` catalog passthrough and
//! the helpers that shape gateway responses for the wire.

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use workshop_gateway::{GatewayError, GatewayResponse};
use workshop_protocol::{Activity, ErrorEnvelope};
use workshop_registry::Push;

use super::state::SessionsState;

/// Whether wire bodies carry internal failure detail. Debug builds append
/// the source chain to the envelope message; production bodies stay at
/// the failure's own message.
const LEAK_DETAIL: bool = cfg!(debug_assertions);

/// Relays the gateway's model catalog to the caller verbatim.
///
/// While the heartbeat reports the gateway down, the catalog is not
/// attempted: the route answers 502 with a user-visible message instead.
pub(crate) async fn models(State(state): State<SessionsState>) -> Response {
    // An unregistered gateway subsystem reads as the health flag's
    // optimistic default; a registered one short-circuits while down.
    if state.health().is_some_and(|health| !health.is_reachable()) {
        return envelope(StatusCode::BAD_GATEWAY, "Gateway unreachable".to_string());
    }
    let push = state.push();
    push.push_status_update(
        "Loading models...",
        "fetching the gateway model catalog",
        Activity::General,
    );
    let Some(gateway) = state.gateway_snapshot() else {
        return envelope(StatusCode::BAD_GATEWAY, "Gateway unreachable".to_string());
    };
    let result = gateway.client().list_models().await;
    report_gateway_outcome(&push, &result, "GET /v1/models");
    relay(result)
}

/// Reports a gateway call's outcome on the status bus: back to idle on
/// success, otherwise the error label matching the failure shape.
fn report_gateway_outcome(
    push: &Push,
    result: &Result<GatewayResponse, GatewayError>,
    route: &str,
) {
    match result {
        Ok(upstream) if upstream.status.is_success() => push.push_idle(),
        Ok(upstream) => push.push_failure(
            format!("Gateway error: {}", upstream.status),
            format!("{route} answered a non-success status"),
            Activity::General,
        ),
        Err(error) => push.push_failure("Connection lost", error.to_string(), Activity::General),
    }
}

/// Parses a gateway body as JSON, falling back to a plain string.
pub(crate) fn value_from_bytes(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(body).into_owned()))
}

/// Turns a gateway call outcome into the workshop's HTTP response.
///
/// Success (any status) is relayed byte-for-byte; a transport failure
/// becomes `502 Bad Gateway` in the `gateway_unreachable` wire envelope.
pub(crate) fn relay(result: Result<GatewayResponse, GatewayError>) -> Response {
    match result {
        Ok(upstream) => (
            upstream.status,
            [(header::CONTENT_TYPE, "application/json")],
            upstream.body,
        )
            .into_response(),
        Err(error) => envelope(StatusCode::BAD_GATEWAY, gateway_message(&error)),
    }
}

/// The wire message of a gateway transport failure: the error's own
/// summary line, with the source chain appended in debug builds only.
fn gateway_message(error: &GatewayError) -> String {
    use std::fmt::Write as _;
    let mut message = error.to_string();
    if LEAK_DETAIL {
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            // fmt::Write to a String cannot fail; the Result is a trait
            // artifact.
            let _ = write!(message, ": {cause}");
            source = cause.source();
        }
    }
    message
}

/// Renders one `gateway_unreachable` envelope at `status`.
fn envelope(status: StatusCode, message: String) -> Response {
    let envelope = ErrorEnvelope::new(message, "gateway_unreachable");
    // Serializing the envelope cannot fail: two strings only. A body
    // that somehow cannot serialize degrades to the status line's text.
    let body = serde_json::to_string(&envelope)
        .unwrap_or_else(|_| status.canonical_reason().unwrap_or("error").to_string());
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
#[path = "relay-tests.rs"]
mod tests;
