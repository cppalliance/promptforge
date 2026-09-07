#[tokio::test]
async fn browser_disconnect_releases_the_gateway_peer() {
    let (gateway, probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let (mut socket, _) =
        tokio_tungstenite::connect_async(server.ws_url("/v1/realtime?intent=transcription"))
            .await
            .expect("the Workshop Realtime socket upgrades");
    let close_seen = probe.close_seen.notified();
    let disconnected = probe.disconnected.notified();
    let tokio_tungstenite::MaybeTlsStream::Plain(transport) = socket.get_mut() else {
        panic!("the loopback Workshop test uses a plain transport");
    };
    transport
        .shutdown()
        .await
        .expect("the browser transport disconnects");
    drop(socket);
    tokio::time::timeout(RECV_TIMEOUT, async {
        tokio::select! {
            () = close_seen => {}
            () = disconnected => {}
        }
    })
    .await
    .expect("an abrupt browser disconnect closes the Gateway hop");
}
