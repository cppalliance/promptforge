#[tokio::test]
async fn canonical_fixture_drives_hypothesis_completion_and_clear() {
    let fixtures = canonical_sequences();
    let interim = ScriptedDecoder::new();
    interim.push_text("Hello");
    interim.push_text("Hello!");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("Hello");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;

    let created = expect_type(&mut socket, "session.created").await;
    assert_eq!(
        created["type"],
        canonical_server(&fixtures, "first_event_readiness", "session.created")["type"]
    );
    send(
        &mut socket,
        canonical_client(&fixtures, "hypothesis_negotiation", "session.update"),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;

    let mut hypotheses = Vec::new();
    for _ in 0..2 {
        for _ in 0..5 {
            let mut append = canonical_client(
                &fixtures,
                "hypothesis_negotiation",
                "input_audio_buffer.append",
            );
            append["audio"] = serde_json::json!(audio());
            send(&mut socket, append).await;
        }
        hypotheses.push(
            expect_type(
                &mut socket,
                "conversation.item.input_audio_transcription.hypothesis",
            )
            .await,
        );
    }
    let first = &hypotheses[0];
    let second = &hypotheses[1];
    assert_eq!(first["revision"], 1);
    assert_eq!(first["transcript"], "Hello");
    assert_eq!(second["revision"], 2);
    assert_eq!(second["transcript"], "Hello!");

    send(
        &mut socket,
        canonical_client(
            &fixtures,
            "immediate_commit_and_provisional_promotion",
            "input_audio_buffer.commit",
        ),
    )
    .await;
    let committed = expect_type(&mut socket, "input_audio_buffer.committed").await;
    let item_id = committed["item_id"].clone();
    assert_eq!(
        expect_type(&mut socket, "conversation.item.created").await["item"]["id"],
        item_id
    );
    let completed = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(completed["item_id"], item_id);
    assert_eq!(
        completed["transcript"],
        canonical_server(
            &fixtures,
            "hypothesis_negotiation",
            "conversation.item.input_audio_transcription.completed",
        )["transcript"]
    );

    let mut append = canonical_client(
        &fixtures,
        "clear_retires_only_uncommitted_input",
        "input_audio_buffer.append",
    );
    append["audio"] = serde_json::json!(audio());
    send(&mut socket, append).await;
    send(
        &mut socket,
        canonical_client(
            &fixtures,
            "clear_retires_only_uncommitted_input",
            "input_audio_buffer.clear",
        ),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.cleared").await;

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
