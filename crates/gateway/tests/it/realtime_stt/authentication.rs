#[tokio::test]
async fn gateway_auth_origin_query_and_final_speech_surfaces_precede_upgrade() {
    let service = speech(&ScriptedDecoder::new(), Some(&ScriptedDecoder::new()));
    let strict = server(true, &service).await;

    assert_eq!(
        rejected(
            strict.addr,
            "intent=transcription&intent=transcription",
            Some("wrong"),
            None,
        )
        .await,
        401,
        "Gateway auth runs before Realtime query validation"
    );
    assert_eq!(
        rejected(
            strict.addr,
            "intent=transcription&intent=transcription",
            Some("test-token"),
            None,
        )
        .await,
        400
    );
    assert_eq!(
        rejected(
            strict.addr,
            "intent=transcription",
            Some("test-token"),
            Some("http://evil.example"),
        )
        .await,
        403
    );
    let mut duplicate_origin = request(
        strict.addr,
        "intent=transcription",
        Some("test-token"),
        None,
        None,
    );
    duplicate_origin
        .headers_mut()
        .append("origin", HeaderValue::from_static("http://localhost:8080"));
    duplicate_origin
        .headers_mut()
        .append("origin", HeaderValue::from_static("http://localhost:8080"));
    assert_eq!(rejected_request(duplicate_origin).await, 403);

    for origin in [None, Some("http://localhost:8080")] {
        let mut socket = connect(strict.addr, Some("test-token"), None, origin).await;
        expect_type(&mut socket, "session.created").await;
        socket.close(None).await.expect("socket closes");
        drop(socket);
    }

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client builds");
    let handoff =
        send_within(http.get(format!("http://{}/auth?key=test-token", strict.addr))).await;
    let cookie = handoff
        .headers()
        .get("set-cookie")
        .expect("handoff sets a cookie")
        .to_str()
        .expect("cookie is text")
        .split(';')
        .next()
        .expect("cookie has a pair")
        .to_owned();
    let mut cookie_socket = connect(strict.addr, None, Some(&cookie), None).await;
    expect_type(&mut cookie_socket, "session.created").await;
    cookie_socket.close(None).await.expect("socket closes");
    drop(cookie_socket);

    assert_final_speech_route_surface(&http, strict.addr).await;
    strict.shutdown().await;

    let trusted = server(false, &service).await;
    let mut socket = connect(trusted.addr, None, None, None).await;
    expect_type(&mut socket, "session.created").await;
    socket.close(None).await.expect("socket closes");
    drop(socket);
    trusted.shutdown().await;
}
