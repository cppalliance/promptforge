#[tokio::test]
async fn admission_is_bounded_and_replacement_closes_with_1012() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    final_decoder.push_text("too late");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut sockets = Vec::new();
    for _ in 0..8 {
        let mut socket = connect(server.addr, Some("test-token"), None, None).await;
        expect_type(&mut socket, "session.created").await;
        sockets.push(socket);
    }
    assert_eq!(
        rejected(
            server.addr,
            "intent=transcription",
            Some("test-token"),
            None
        )
        .await,
        429
    );
    for mut socket in sockets.drain(1..) {
        socket.close(None).await.expect("socket closes");
    }
    for _ in 0..5 {
        send(
            &mut sockets[0],
            serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": audio()
            }),
        )
        .await;
    }
    send(
        &mut sockets[0],
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    let committed = expect_type(&mut sockets[0], "input_audio_buffer.committed").await;
    let item_id = committed["item_id"].as_str().expect("item ID").to_owned();
    expect_type(&mut sockets[0], "conversation.item.created").await;
    let parked = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("park observer joins"),
        "committed item owns its final decode"
    );

    let replacement = ScriptedDecoder::new();
    let replacement_final = ScriptedDecoder::new();
    let replacement_service = service.clone();
    let replacement_task = tokio::task::spawn_blocking(move || {
        begin_scripted_replacement(
            &replacement_service,
            ScriptedModelFactory::new(replacement).with_final(replacement_final),
            true,
            PHASE_TIMEOUT,
        )
    });
    let replaced = expect_type(
        &mut sockets[0],
        "conversation.item.input_audio_transcription.failed",
    )
    .await;
    assert_eq!(replaced["item_id"], item_id);
    assert_eq!(replaced["error"]["code"], "engine_replaced");
    let message = tokio::time::timeout(PHASE_TIMEOUT, sockets[0].next())
        .await
        .expect("replacement closes the socket before its deadline")
        .expect("socket emits a close frame")
        .expect("close frame is valid");
    let Message::Close(Some(close)) = message else {
        panic!("replacement emits a close frame, got {message:?}");
    };
    assert_eq!(u16::from(close.code), 1012);
    assert_eq!(close.reason, "engine_replaced");
    drop(sockets);
    final_decoder.release();

    let staged = replacement_task
        .await
        .expect("replacement task joins")
        .expect("replacement stages after session ownership drains");
    service
        .commit_replacement(staged)
        .expect("replacement commits");
    let mut replacement_socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut replacement_socket, "session.created").await;
    replacement_socket
        .close(None)
        .await
        .expect("replacement socket closes");
    drop(replacement_socket);
    server.shutdown().await;
}
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
