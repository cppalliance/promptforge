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
