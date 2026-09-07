#[tokio::test]
async fn stalled_browser_cleanup_is_bounded_after_gateway_disconnect() {
    let probe = StalledPeerProbe::default();
    let gateway = spawn_gateway(
        Router::new()
            .route("/v1/realtime", get(send_large_frame_then_disconnect))
            .with_state(probe.clone()),
    )
    .await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    tokio::time::timeout(RECV_TIMEOUT, probe.wait_for_frame())
        .await
        .expect("the Gateway fills the relay's browser send");

    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    let first = tokio::time::timeout(RECV_TIMEOUT, socket.next())
        .await
        .expect("bounded relay cleanup releases the stalled browser");
    assert!(
        !matches!(first, Some(Ok(ClientMessage::Binary(_)))),
        "the stalled send is canceled before peer reads can release it"
    );
}
