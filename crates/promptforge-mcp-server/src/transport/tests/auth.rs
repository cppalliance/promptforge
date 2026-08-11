//! The bearer matrix: what the auth layer in front of `/mcp` lets through and
//! what it refuses, and that `/healthz` sits outside it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::config::Secret;
use crate::transport::{HEALTHZ_PATH, MCP_PATH};

use super::{TOKEN, initialize, router, router_with};

#[tokio::test]
async fn a_call_with_no_authorization_header_is_refused() {
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(None))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer"),
        "a refusal names the scheme the caller should have used"
    );
}

#[tokio::test]
async fn a_call_using_the_wrong_scheme_is_refused() {
    // The token is right; the scheme is not. Nothing but `Bearer` is accepted,
    // so a basic-auth client cannot smuggle the same secret through.
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some(&format!("Basic {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_bearer_layer_authenticates_nobody_without_a_bearer_header() {
    // `Secret` now refuses a blank value at the type boundary, so an empty
    // configured token is unrepresentable and this layer cannot be built from
    // one. It is built directly here to show the bearer defence standing on its
    // own: a caller presenting nothing, or a scheme that is not `Bearer`, is
    // refused before the comparison it would otherwise reach.
    let (_dir, router) =
        router_with(Secret::try_from("layer-only").expect("the layer token is non-blank"));

    let missing = router
        .clone()
        .oneshot(initialize(None))
        .await
        .expect("the router answers");
    assert_eq!(
        missing.status(),
        StatusCode::UNAUTHORIZED,
        "no header never compares equal, whatever the configured token is"
    );

    let scheme = router
        .oneshot(initialize(Some("Basic ")))
        .await
        .expect("the router answers");
    assert_eq!(
        scheme.status(),
        StatusCode::UNAUTHORIZED,
        "a scheme that is not Bearer is refused before anything is compared"
    );
}

#[tokio::test]
async fn a_call_with_the_wrong_token_is_refused() {
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some("Bearer not-the-token")))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_bearer_with_no_credential_is_refused() {
    // `Bearer ` names the scheme but presents nothing. It must not fall through
    // as the empty string, which an empty configured token would then match.
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some("Bearer ")))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_bearer_scheme_is_matched_case_insensitively() {
    // RFC 7235 makes the scheme case-insensitive, so a client that spells it
    // `bearer` is presenting the same credential and must be let through.
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some(&format!("bearer {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_call_with_the_right_token_reaches_the_mcp_endpoint() {
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some(&format!("Bearer {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().contains_key("mcp-session-id"),
        "the handshake reached the transport and opened a session"
    );
}

#[tokio::test]
async fn a_rotated_token_refuses_the_next_request_on_an_established_session() {
    // The check is per HTTP request, not per MCP session, so possessing a
    // session id buys nothing once the token no longer matches.
    let (_dir, router) = router();
    let opened = router
        .clone()
        .oneshot(initialize(Some(&format!("Bearer {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(opened.status(), StatusCode::OK);
    let session = opened
        .headers()
        .get("mcp-session-id")
        .expect("the handshake opened a session")
        .clone();

    let request = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header("host", "127.0.0.1")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("authorization", "Bearer rotated-away")
        .header("mcp-session-id", session)
        .body(Body::from(
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
        ))
        .expect("build the follow-up request");
    let response = router.oneshot(request).await.expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn healthz_answers_without_a_token() {
    let (_dir, router) = router();
    let request = Request::builder()
        .uri(HEALTHZ_PATH)
        .body(Body::empty())
        .expect("build the liveness request");
    let response = router.oneshot(request).await.expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
}
