#[tokio::test]
async fn browser_realtime_retry_reaches_the_new_port_and_key_without_workshop_reload() {
    let server = TestServer::spawn("http://127.0.0.1:1");
    let url = server.ws_url("/v1/realtime?intent=transcription");
    assert_eq!(
        rejected_status(request_with(&url, None, None)).await,
        StatusCode::BAD_GATEWAY,
        "the dead original sidecar produces the recoverable handshake failure"
    );

    let replacement =
        spawn_gateway(Router::new().route("/v1/realtime", get(recovered_upstream))).await;
    server.replace_gateway(&replacement, "replacement-key");

    let (mut socket, response) = tokio_tungstenite::connect_async(request_with(&url, None, None))
        .await
        .expect("the browser retry upgrades through the same Workshop server");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    let ClientMessage::Text(created) = recv(&mut socket).await else {
        panic!("the replacement readiness frame stays text");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&created).expect("readiness parses")["type"],
        "session.created"
    );
    let ClientMessage::Text(updated) = recv(&mut socket).await else {
        panic!("the replacement negotiation frame stays text");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&updated).expect("negotiation parses")["type"],
        "session.updated"
    );
}
