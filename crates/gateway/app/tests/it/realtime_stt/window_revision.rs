async fn mounted_hypothesis_session(transcripts: &[&str]) -> (TestServer, Socket, ScriptedDecoder) {
    let interim = ScriptedDecoder::new();
    for transcript in transcripts {
        interim.push_text(*transcript);
    }
    let service = speech_with_policy(&interim, Some(&ScriptedDecoder::new()), 1, 500);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "include": ["item.input_audio_transcription.hypothesis"]
            }
        }),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;
    (server, socket, interim)
}

async fn schedule_hypothesis(socket: &mut Socket) -> serde_json::Value {
    for _ in 0..5 {
        append_audio(socket, audio()).await;
    }
    expect_type(
        socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await
}

#[tokio::test]
async fn mounted_advancing_window_accepts_two_normalized_leading_tokens() {
    let (server, mut socket, _interim) =
        mounted_hypothesis_session(&["Why, IS it", "Why, IS it", "why is this"]).await;

    schedule_hypothesis(&mut socket).await;
    schedule_hypothesis(&mut socket).await;
    let revision = schedule_hypothesis(&mut socket).await;

    assert_eq!(revision["transcript"], "why is this");
    assert_eq!(revision["audio_start_ms"], 500);
    assert_eq!(revision["audio_end_ms"], 1_500);
    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}

#[tokio::test]
async fn mounted_advancing_window_suppresses_one_generic_normalized_token() {
    let (server, mut socket, interim) =
        mounted_hypothesis_session(&["And this stays", "And this stays", "and unrelated"]).await;

    schedule_hypothesis(&mut socket).await;
    schedule_hypothesis(&mut socket).await;
    for _ in 0..5 {
        append_audio(&mut socket, audio()).await;
    }
    let completed = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || completed.wait_for_completed(3, PHASE_TIMEOUT))
            .await
            .expect("decode observer joins")
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), socket.next())
            .await
            .is_err(),
        "one generic normalized token cannot replace the live hypothesis"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
