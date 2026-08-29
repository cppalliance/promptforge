//! The web-search service error type.
//!
//! [`WebSearchError`] is what [`crate::WebSearchState::search`] returns; the
//! gateway adapts it into its own route-level error type so the envelope and
//! status mapping stay in one place.

use promptforge_gateway_protocol::ProtocolError;

/// A request-time failure of the web-search service.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WebSearchError {
    /// The request body could not be accepted.
    #[error("malformed request: {0}")]
    MalformedRequest(String),

    /// A transport- or protocol-level failure from the provider call. The
    /// variants live in [`ProtocolError`]; the service propagates them so the
    /// gateway renders one envelope shape.
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_request_display_matches_the_gateway_envelope() {
        // The gateway maps this variant one-to-one, so the message text is
        // part of the wire contract.
        let err = WebSearchError::MalformedRequest("web_search: empty query".to_string());
        assert_eq!(
            err.to_string(),
            "malformed request: web_search: empty query"
        );
    }

    #[test]
    fn protocol_error_is_transparent() {
        let err = WebSearchError::from(ProtocolError::upstream_status(
            502,
            "bad gateway".to_string(),
        ));
        assert_eq!(err.to_string(), "upstream returned 502");
    }
}
