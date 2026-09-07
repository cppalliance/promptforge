#[tokio::test]
async fn realtime_relay_is_authenticated_fixed_and_payload_opaque() {
    let (gateway, probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?ignored=browser");
    let mut request = request_with(&url, None, None);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer browser-secret"
            .parse()
            .expect("the browser credential is a header"),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the Workshop Realtime socket upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    let opaque = "not JSON: \u{00e9}\u{65e5}\u{1f40d}";
    socket
        .send(ClientMessage::Text(opaque.into()))
        .await
        .expect("opaque text sends");
    assert_eq!(recv(&mut socket).await, ClientMessage::Text(opaque.into()));

    socket
        .send(ClientMessage::Ping(vec![2, 4, 6, 8].into()))
        .await
        .expect("browser ping sends");
    assert_eq!(
        recv(&mut socket).await,
        ClientMessage::Pong(vec![2, 4, 6, 8].into()),
        "the Workshop hop owns exactly one matching browser pong"
    );
    assert_no_frame(&mut socket).await;

    socket
        .send(ClientMessage::Pong(vec![1, 3, 5, 7].into()))
        .await
        .expect("caller-owned pong sends");

    let binary = vec![0, 255, 1, 128, 2];
    socket
        .send(ClientMessage::Binary(binary.clone().into()))
        .await
        .expect("opaque binary sends");
    assert_eq!(
        recv(&mut socket).await,
        ClientMessage::Binary(binary.into())
    );
    tokio::time::timeout(RECV_TIMEOUT, async {
        loop {
            let notified = probe.control_seen.notified();
            if !probe.pongs().is_empty() {
                break;
            }
            notified.await;
        }
    })
    .await
    .expect("the Gateway hop receives its automatic pong");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        probe.pongs(),
        vec![vec![9, 8, 7]],
        "the Gateway hop owns exactly one matching pong and receives no browser pong"
    );
    assert!(
        probe.pings().is_empty(),
        "the browser ping terminates at Workshop"
    );
    assert_no_frame(&mut socket).await;
    socket.close(None).await.expect("the browser socket closes");

    assert_eq!(
        probe.request(),
        UpstreamRequest {
            path: "/v1/realtime".to_owned(),
            query: "intent=transcription".to_owned(),
            has_origin: false,
            has_subprotocol: false,
        },
        "the connector fixes the upstream target and forwards no browser policy headers"
    );
}

#[tokio::test]
async fn realtime_relay_enforces_same_origin_authority_and_no_subprotocol() {
    let (gateway, _probe) = spawn_probe().await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?intent=transcription");
    let parsed = url::Url::parse(&url).expect("the Workshop URL parses");
    let authority = parsed
        .socket_addrs(|| None)
        .expect("the Workshop authority resolves")
        .into_iter()
        .next()
        .expect("the Workshop authority has an address");
    let same_origin = format!("http://{authority}");

    let (socket, response) =
        tokio_tungstenite::connect_async(request_with(&url, Some(&same_origin), None))
            .await
            .expect("the exact same origin upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    drop(socket);

    assert_eq!(
        rejected_status(request_with(&url, Some("http://localhost:9"), None)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        rejected_status(request_with(&url, Some(&same_origin), Some("realtime"))).await,
        StatusCode::BAD_REQUEST
    );
}
