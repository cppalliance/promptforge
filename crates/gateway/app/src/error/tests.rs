//! Tests for error classification, the conversions into `GatewayError`, and cause rendering.

use axum::http::StatusCode;

use super::*;
use std::error::Error as _;

#[test]
fn gateway_error_classify_is_table_driven() {
    let cases: Vec<(GatewayError, (StatusCode, &str, &str))> = vec![
        (
            GatewayError::Unauthorized,
            (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "unauthorized",
            ),
        ),
        (
            GatewayError::UnknownModel("m".to_owned()),
            (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "model_not_found",
            ),
        ),
        (
            GatewayError::KindMismatch {
                model: "m".to_owned(),
                expected: ModelKind::Chat,
                actual: ModelKind::Embedding,
            },
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "kind_mismatch",
            ),
        ),
        (
            GatewayError::QueueFull,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "queue_full",
            ),
        ),
        (
            GatewayError::QueueRejected,
            (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "queue_rejected",
            ),
        ),
        (
            GatewayError::InvalidVoice {
                voice: "coral".to_owned(),
                valid: vec!["alloy".to_owned(), "nova".to_owned()],
            },
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "invalid_voice",
            ),
        ),
        (
            GatewayError::UpstreamRateLimited,
            (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "upstream_rate_limited",
            ),
        ),
        (
            GatewayError::UpstreamUnavailable,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "upstream_unavailable",
            ),
        ),
        (
            GatewayError::switch_failed("build-routing", std::io::Error::other("x")),
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "switch_failed",
            ),
        ),
        (
            GatewayError::ModelProvisioning("load-profile: main".to_owned()),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_provisioning",
            ),
        ),
        (
            GatewayError::ModelLoading("local-model".to_owned()),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_loading",
            ),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.classify(), expected);
    }
}

#[test]
fn only_a_loading_model_sets_retry_after() {
    // A loading model is a 503 the client should wait out, so its
    // response names the wait; the other 503s (a full queue, a model
    // still provisioning) promise nothing about when they clear and
    // set none.
    let response = GatewayError::ModelLoading("local-model".to_owned()).into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("5")
    );
    for error in [
        GatewayError::QueueFull,
        GatewayError::CommandCancelled("load-profile: main".to_owned()),
        GatewayError::ModelProvisioning("load-profile: main".to_owned()),
        GatewayError::UnknownModel("ghost".to_owned()),
    ] {
        let response = error.into_response();
        assert!(
            response
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .is_none(),
            "{} omits Retry-After",
            response.status()
        );
    }
}

#[test]
fn shadow_route_errors_classify_is_table_driven() {
    let cases: Vec<(GatewayError, (StatusCode, &str, &str))> = vec![
        (
            GatewayError::ConfigWriteRejected("invalid config: bad".to_owned()),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_error",
                "config_write_rejected",
            ),
        ),
        (
            GatewayError::ConfigWriteIo(Box::new(std::io::Error::other("disk"))),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "config_write_error",
            ),
        ),
        (
            GatewayError::ConfigPathUnavailable,
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "config_path_unavailable",
            ),
        ),
        (
            GatewayError::EnvFile(Box::new(std::io::Error::other("bad line"))),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "env_file_error",
            ),
        ),
        (
            GatewayError::PendingConfig("corrupt shadow".to_owned()),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "pending_config_error",
            ),
        ),
        (
            GatewayError::ApplyReloadFailed("ghost endpoint".to_owned()),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "apply_reload_failed",
            ),
        ),
        (
            GatewayError::ApplyCancelled,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "apply_cancelled",
            ),
        ),
        (
            GatewayError::CloudModelsLoading,
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "cloud_models_loading",
            ),
        ),
        (
            GatewayError::CloudModelsUnavailable("offline".to_owned()),
            (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_unavailable",
            ),
        ),
        (
            GatewayError::CloudModelsBodyTooLarge {
                announced: 5_000_000,
                cap: 4_194_304,
            },
            (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_body_too_large",
            ),
        ),
        (
            GatewayError::CloudModelsSchemaVersion {
                found: 2,
                accepted: 1,
            },
            (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_schema_version",
            ),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.classify(), expected);
    }
}

#[test]
fn reveal_errors_classify_is_table_driven() {
    let cases: Vec<(GatewayError, (StatusCode, &str, &str))> = vec![
        (
            GatewayError::RevealPathNotFound("C:/ghost.gguf".to_owned()),
            (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "reveal_path_not_found",
            ),
        ),
        (
            GatewayError::RevealFailed(Box::new(std::io::Error::other("spawn"))),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "reveal_error",
            ),
        ),
        (
            GatewayError::BlockingTask(Box::new(std::io::Error::other("panicked"))),
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "blocking_task_failed",
            ),
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.classify(), expected);
    }
}

#[test]
fn protocol_error_delegates_classify_and_display() {
    // The protocol crate owns the transport/protocol variants and their
    // envelope mapping; the gateway wrapper delegates both and stays
    // transparent in the source chain.
    let error = GatewayError::upstream_protocol(std::io::Error::other("bad json"));
    assert!(matches!(error, GatewayError::Protocol(_)));
    assert_eq!(
        error.classify(),
        (StatusCode::BAD_GATEWAY, "server_error", "upstream_protocol")
    );
    assert_eq!(error.to_string(), "upstream protocol error");
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("bad json")
    );
}

#[test]
fn switch_failed_preserves_its_cause() {
    let error = GatewayError::switch_failed("load-profile", std::io::Error::other("disk"));
    assert!(error.source().is_some());
    assert!(error.to_string().contains("load-profile"));
    assert!(!error.to_string().contains("disk"));
}

#[test]
fn admit_error_maps_to_queue_errors() {
    for admit in [
        crate::queue::AdmitError::QueueFull,
        crate::queue::AdmitError::Unavailable,
    ] {
        assert!(matches!(GatewayError::from(admit), GatewayError::QueueFull));
    }
    assert!(matches!(
        GatewayError::from(crate::queue::AdmitError::Rejected),
        GatewayError::QueueRejected
    ));
}

#[cfg(feature = "web-search")]
#[test]
fn web_search_error_maps_to_gateway_error() {
    use gateway_web_search::WebSearchError;
    // The malformed-request arm preserves the message verbatim, so the
    // wire envelope is unchanged by the crate boundary.
    let err = GatewayError::from(WebSearchError::MalformedRequest(
        "web_search: empty query".to_string(),
    ));
    assert!(matches!(&err, GatewayError::MalformedRequest(m) if m == "web_search: empty query"));
    assert_eq!(
        err.classify(),
        (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "malformed_request"
        )
    );
    // The protocol arm is transparent: same variant, same display.
    let err = GatewayError::from(WebSearchError::from(ProtocolError::upstream_status(
        502,
        "bad gateway".to_string(),
    )));
    assert!(matches!(err, GatewayError::Protocol(_)));
    assert_eq!(err.to_string(), "upstream returned 502");
}

/// A leaf cause with its own text.
#[derive(Debug, thiserror::Error)]
#[error("disk full")]
struct Leaf;

/// An outer error that copies its cause's text into its own message,
/// the shape of the variants that embed an [`error_chain`] rendering
/// in their own string.
#[derive(Debug, thiserror::Error)]
#[error("config write rejected: {message}")]
struct Copying {
    message: String,
    #[source]
    source: Leaf,
}

/// A cause that renders as nothing.
#[derive(Debug, thiserror::Error)]
#[error("")]
struct Silent;

/// An outer error whose cause renders as nothing.
#[derive(Debug, thiserror::Error)]
#[error("config write rejected")]
struct OverSilent(#[source] Silent);

#[test]
fn a_cause_the_outer_message_already_includes_renders_once() {
    let error = Copying {
        message: "disk full".to_owned(),
        source: Leaf,
    };
    let rendered = error_chain(&error);
    assert_eq!(
        rendered, "config write rejected: disk full",
        "a cause whose text the outer message already includes is skipped"
    );
    assert_eq!(
        rendered.matches("disk full").count(),
        1,
        "the cause text appears exactly once"
    );
    assert_eq!(
        error_chain(&OverSilent(Silent)),
        "config write rejected",
        "a cause that renders as nothing adds no trailing separator"
    );
}
