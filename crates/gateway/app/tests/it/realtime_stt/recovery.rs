#[tokio::test]
async fn blocked_server_send_expires_and_releases_admission() {
    let interim = ScriptedDecoder::new();
    interim.push_text("blocked transcript");
    let mut service = speech(&interim, Some(&ScriptedDecoder::new()));
    service.block_realtime_send_after(8);
    let server = server(true, &service).await;

    let mut blocked = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut blocked, "session.created").await;
    let mut occupants = Vec::new();
    for _ in 0..7 {
        let mut socket = connect(server.addr, Some("test-token"), None, None).await;
        expect_type(&mut socket, "session.created").await;
        occupants.push(socket);
    }
    send(
        &mut blocked,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": audio()
        }),
    )
    .await;
    send(
        &mut blocked,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    assert_eq!(
        rejected(
            server.addr,
            "intent=transcription",
            Some("test-token"),
            None
        )
        .await,
        429,
        "the blocked send initially retains its session"
    );

    tokio::time::sleep(Duration::from_secs(2)).await;
    let admitted = connect(server.addr, Some("test-token"), None, None).await;

    drop(admitted);
    for mut socket in occupants {
        socket.close(None).await.expect("socket closes");
    }
    drop(blocked);
    server.shutdown().await;
}

#[tokio::test]
async fn mounted_gateway_failure_ends_the_session_with_an_error_event_then_a_1011_close() {
    let mut service = speech(&ScriptedDecoder::new(), Some(&ScriptedDecoder::new()));
    service.fail_realtime_finish_ready();
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;
    append_audio(&mut socket, audio()).await;
    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.committed").await;
    expect_type(&mut socket, "conversation.item.created").await;

    let ended = expect_type(&mut socket, "error").await;
    assert_eq!(ended["error"]["type"], "server_error", "{ended}");
    assert_eq!(ended["error"]["code"], "internal_error", "{ended}");
    let message = ended["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("no active finalization"),
        "the message names the cause: {ended}"
    );
    let closing = tokio::time::timeout(PHASE_TIMEOUT, socket.next())
        .await
        .expect("the close frame arrives before the deadline")
        .expect("the server closes the socket with a frame")
        .expect("the close frame is valid");
    let Message::Close(Some(frame)) = closing else {
        panic!("the error event is followed by a close frame with a code: {closing:?}");
    };
    assert_eq!(u16::from(frame.code), 1011, "{frame:?}");

    drop(socket);
    server.shutdown().await;
}
