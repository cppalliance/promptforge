use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request, Response, StatusCode};
use gateway_config::Config;
use tower::ServiceExt;

use crate::shutdown::ShutdownReason;
use crate::test_support::app_state;
use crate::{AppState, build_router};

fn state(supervised: bool) -> AppState {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
    )
    .expect("config parses");

    let mut state = app_state(config, None);
    state.supervised = supervised;

    state
}

/// Sends `POST /admin/restart` through the router from a loopback peer
/// with the bearer key, as the walled route requires.
async fn send_restart(state: &AppState) -> Response<Body> {
    let mut request = Request::builder()
        .method(Method::POST)
        .uri("/admin/restart")
        .header(AUTHORIZATION, "Bearer test-token")
        .body(Body::empty())
        .expect("request builds");

    let peer: SocketAddr = "127.0.0.1:50000".parse().expect("a socket address");
    request.extensions_mut().insert(ConnectInfo(peer));

    build_router(state.clone(), None)
        .oneshot(request)
        .await
        .expect("the router should be infallible")
}

#[tokio::test]
async fn a_supervised_restart_answers_202_and_fires_a_restart() {
    let state = state(true);

    let response = send_restart(&state).await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(5), state.shutdown.fired())
        .await
        .expect("the route should have fired the shutdown signal");
    assert_eq!(state.shutdown.reason(), Some(ShutdownReason::Restart));
}

#[tokio::test]
async fn an_unsupervised_restart_is_refused_and_leaves_the_gateway_up() {
    let state = state(false);

    let response = send_restart(&state).await;

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("should be able to read the response body");
    let error: serde_json::Value =
        serde_json::from_slice(&body).expect("should be able to build an error envelope");
    assert_eq!(error["error"]["code"], "restart_unavailable");

    assert!(
        !state.shutdown.is_fired(),
        "a refused restart must leave the gateway up"
    );
}
