//! The opaque wire error every HTTP failure answers with.
//!
//! [`AppError`] is the boundary between zone-two failures and the HTTP
//! response: one variant per wire failure that exists today, each mapped to
//! exactly one status code by the central [`IntoResponse`] impl, so the
//! same failure is built in one place no matter which handler hits it.
//! Conversions in are explicit - handler seams name a variant constructor;
//! no `#[from]` derive exists on this side of the boundary. The extracted
//! feature crates map their own error types at their own route boundaries
//! (`workshop_workspace::WorkspaceError`, the sessions relay's gateway
//! envelope); this shell type covers the shell's own routes.
//! Internal failure detail (the source chain) reaches the response body in
//! debug builds only; production bodies stay at each variant's own message,
//! close to the status text. Rich construction-time errors live elsewhere
//! ([`workshop_support::ConfigError`], [`crate::serve::SpawnError`]) and
//! never cross the wire.

use std::fmt::Write as _;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use workshop_protocol::ErrorEnvelope;

use crate::gateway::GatewayError;

/// Whether wire bodies carry internal failure detail. Debug builds append
/// the source chain to the envelope message; production bodies stay at the
/// variant's own message.
const LEAK_DETAIL: bool = cfg!(debug_assertions);

/// A failure answered over the HTTP wire.
///
/// Every variant renders as exactly one status code. Variants carrying a
/// source keep it out of `Display`; [`render_message`] appends the chain to
/// the response body in debug builds only.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum AppError {
    /// An attempted gateway call failed in transport. Transparent so the
    /// wire message stays the [`GatewayError`]'s own summary line, as it
    /// was before the error split.
    #[error(transparent)]
    Gateway(GatewayError),

    /// The request arrived from a cross-site browser context: a
    /// `Sec-Fetch-Site: cross-site` marking, a non-loopback `Host`
    /// (DNS rebinding), or a foreign WebSocket `Origin` (see
    /// [`crate::cross_site`]).
    #[error("cross-site request refused")]
    CrossSite,

    /// A body-bearing request did not declare an `application/json` body.
    #[error("request body is not application/json")]
    NotJson,

    /// The config-panel proxy refused to forward a path outside its
    /// allowlist (see [`crate::routes::gateway_config`]).
    #[error("path is not forwardable to the gateway")]
    ForwardDenied,

    /// An embedded UI asset is missing from the bundle.
    #[error("ui asset not found: {0}")]
    AssetMissing(String),
}

impl AppError {
    /// The one HTTP status this failure answers with.
    fn status(&self) -> StatusCode {
        match self {
            Self::Gateway(_) => StatusCode::BAD_GATEWAY,
            Self::CrossSite | Self::ForwardDenied => StatusCode::FORBIDDEN,
            Self::NotJson => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::AssetMissing(_) => StatusCode::NOT_FOUND,
        }
    }

    /// The machine-readable code of the JSON error envelope, or `None` for
    /// the failures rendered as plain text instead of the envelope.
    fn code(&self) -> Option<&'static str> {
        match self {
            Self::Gateway(_) => Some("gateway_unreachable"),
            Self::CrossSite => Some("cross_site"),
            Self::NotJson => Some("not_json"),
            Self::ForwardDenied => Some("forward_denied"),
            Self::AssetMissing(_) => None,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        match self.code() {
            Some(code) => {
                let envelope = ErrorEnvelope::new(render_message(&self, LEAK_DETAIL), code);
                // Serializing the envelope cannot fail: two strings only.
                // A body that somehow cannot serialize degrades to the
                // status line's own text.
                let body = serde_json::to_string(&envelope)
                    .unwrap_or_else(|_| status.canonical_reason().unwrap_or("error").to_string());
                (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
            }
            None => (
                status,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                self.to_string(),
            )
                .into_response(),
        }
    }
}

/// Renders the envelope message for `error`: its own `Display` text, with
/// the source chain appended as `: cause` segments when `leak_detail` is
/// set.
fn render_message(error: &AppError, leak_detail: bool) -> String {
    let mut message = error.to_string();
    if leak_detail {
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::io;

    use crate::app::fixtures::body_bytes;

    /// A distinctive injected cause for leak-boundary assertions.
    fn injected_io() -> io::Error {
        io::Error::other("injected disk failure")
    }

    #[test]
    fn gateway_failures_map_to_bad_gateway() {
        let transport =
            AppError::Gateway(GatewayError::transport_for_test(Box::new(injected_io())));
        assert_eq!(transport.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(transport.code(), Some("gateway_unreachable"));
    }

    #[test]
    fn the_asset_miss_maps_to_not_found_with_no_envelope_code() {
        let miss = AppError::AssetMissing("app.js".to_string());
        assert_eq!(miss.status(), StatusCode::NOT_FOUND);
        assert_eq!(miss.code(), None, "the asset 404 is plain text, not JSON");
    }

    #[tokio::test]
    async fn the_json_envelope_carries_message_code_and_content_type() {
        let response = AppError::CrossSite.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the envelope sets content-type");
        assert_eq!(content_type, "application/json");
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
        assert_eq!(
            json["error"]["message"], "cross-site request refused",
            "the pinned user-visible message is identical in every build"
        );
        assert_eq!(json["error"]["code"], "cross_site");
    }

    #[tokio::test]
    async fn the_asset_miss_renders_the_plain_text_diagnostic() {
        let response = AppError::AssetMissing("app.js".to_string()).into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the diagnostic sets content-type");
        assert_eq!(content_type, "text/plain; charset=utf-8");
        assert_eq!(
            &body_bytes(response).await[..],
            b"ui asset not found: app.js"
        );
    }

    #[test]
    fn production_messages_stay_at_the_variant_text() {
        let gateway = AppError::Gateway(GatewayError::transport_for_test(Box::new(injected_io())));
        assert_eq!(
            render_message(&gateway, false),
            "gateway transport error",
            "the transport failure keeps its pre-split production message"
        );
    }

    #[test]
    fn debug_messages_append_the_source_chain() {
        let gateway = AppError::Gateway(GatewayError::transport_for_test(Box::new(injected_io())));
        assert_eq!(
            render_message(&gateway, true),
            "gateway transport error: injected disk failure"
        );
    }

    /// Tests run under debug assertions, so the live envelope must carry
    /// the detail the debug side of the boundary promises - in the exact
    /// pre-split message format.
    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn debug_builds_leak_detail_into_the_live_envelope() {
        let response = AppError::Gateway(GatewayError::transport_for_test(Box::new(injected_io())))
            .into_response();
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("the envelope is JSON");
        assert_eq!(
            json["error"]["message"],
            "gateway transport error: injected disk failure"
        );
    }
}
