use super::*;

#[tokio::test]
async fn origin_and_config_proxy_follow_one_replacement_snapshot() {
    let gateway = axum::Router::new().route(
        "/admin/status",
        axum_get(|headers: axum::http::HeaderMap| async move {
            if headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                == Some("Bearer replacement-key")
            {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        }),
    );
    let replacement = spawn_gateway(gateway).await;
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    state
        .gateway_binding()
        .replace(&replacement, "replacement-key")
        .expect("the replacement publishes");
    let app = router(state);

    let origin = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/gateway/origin")
                .body(Body::empty())
                .expect("the origin request builds"),
        )
        .await
        .expect("the router is infallible");
    let json: serde_json::Value =
        serde_json::from_slice(&body_bytes(origin).await).expect("the origin body is JSON");
    assert_eq!(json["origin"], replacement);

    let proxied = app
        .oneshot(
            Request::builder()
                .uri("/gateway/api/admin/status")
                .body(Body::empty())
                .expect("the proxy request builds"),
        )
        .await
        .expect("the router is infallible");
    assert_eq!(
        proxied.status(),
        StatusCode::OK,
        "the proxy uses the replacement bearer with the replacement URL"
    );
}
