#[tokio::test]
async fn canonical_sequences_cross_the_fake_upstream_unchanged_without_browser_bearer() {
    let fixture = FixtureUpstream {
        frames: Arc::new(canonical_server_frames()),
        ..FixtureUpstream::default()
    };
    let gateway = spawn_gateway(
        Router::new()
            .route("/v1/realtime", get(fixture_upstream))
            .with_state(fixture.clone()),
    )
    .await;
    let server = TestServer::spawn(&gateway);
    let url = server.ws_url("/v1/realtime?browser=query");
    let mut request = request_with(&url, None, None);
    request.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer browser-secret"
            .parse()
            .expect("browser bearer is a header"),
    );
    let (mut socket, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("Workshop fixture relay upgrades");
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

    for expected in fixture.frames.iter() {
        let ClientMessage::Text(actual) = recv(&mut socket).await else {
            panic!("canonical fixture remains a text payload");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual).expect("relayed event parses"),
            serde_json::from_str::<serde_json::Value>(expected).expect("fixture event parses")
        );
    }
    let opaque = "opaque: not JSON, not speech state";
    socket
        .send(ClientMessage::Text(opaque.into()))
        .await
        .expect("opaque browser text sends");
    assert_eq!(recv(&mut socket).await, ClientMessage::Text(opaque.into()));

    assert!(fixture.gateway_bearer_seen.load(Ordering::Acquire));
    assert!(
        !fixture.browser_bearer_seen.load(Ordering::Acquire),
        "the browser bearer never reaches the fake Gateway"
    );
    socket.close(None).await.expect("fixture socket closes");
}
