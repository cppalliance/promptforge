#[tokio::test]
async fn workshop_exposes_only_the_realtime_speech_route() {
    let (gateway, _probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let client = reqwest::Client::new();
    for path in ["/stt", "/stt/capability"] {
        let response = client
            .get(server.http_url(path))
            .send()
            .await
            .expect("the Workshop route answers");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "GET {path} is retired"
        );
    }
}

#[tokio::test]
async fn gateway_close_code_and_reason_reach_the_browser() {
    let gateway = spawn_gateway(Router::new().route("/v1/realtime", get(upstream_close))).await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    let ClientMessage::Close(Some(close)) = recv(&mut socket).await else {
        panic!("the upstream close frame is relayed");
    };
    assert_eq!(u16::from(close.code), 4101);
    assert_eq!(close.reason, "upstream finished");
}

#[tokio::test]
async fn browser_close_code_and_reason_reach_the_gateway() {
    let (gateway, probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    socket
        .send(ClientMessage::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: 4201.into(),
                reason: "browser finished".into(),
            },
        )))
        .await
        .expect("the browser close sends");
    tokio::time::timeout(RECV_TIMEOUT, probe.close_seen.notified())
        .await
        .expect("the gateway receives the close");
    assert_eq!(
        probe.browser_close(),
        Some((4201, "browser finished".to_owned()))
    );
}
