const LIVE_PREFIX_STRIDE_SAMPLES: usize = 24_000 * 10;

fn live_prefix_text(start: usize, end: usize) -> String {
    (start..end)
        .map(|second| format!("word{second:04}"))
        .collect::<Vec<_>>()
        .join(" ")
}

async fn wait_for_live_prefix_decodes(decoder: &ScriptedDecoder, count: usize) {
    let decoder = decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || decoder.wait_for_completed(count, PHASE_TIMEOUT))
            .await
            .expect("the mounted decode observer joins"),
        "mounted forced decode {count} completes"
    );
}

#[tokio::test]
async fn mounted_hypotheses_keep_revisable_forced_text_until_stop() {
    let interim = ScriptedDecoder::new();
    interim.push_text("word0010");
    interim.push_text("word0020");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text(live_prefix_text(0, 10));
    final_decoder.push_text(live_prefix_text(2, 20));
    final_decoder.push_text(live_prefix_text(12, 21));
    let service = speech_with_policy(&interim, Some(&final_decoder), 15, 100);
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

    append_audio(
        &mut socket,
        audio_samples(&vec![8_192; LIVE_PREFIX_STRIDE_SAMPLES]),
    )
    .await;
    wait_for_live_prefix_decodes(&final_decoder, 1).await;
    append_audio(
        &mut socket,
        audio_samples(&vec![8_192; LIVE_PREFIX_STRIDE_SAMPLES / 10]),
    )
    .await;
    let first = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(first["finalized"], "");
    assert_eq!(first["agreed"], live_prefix_text(0, 10));
    assert_eq!(first["tentative"], " word0010");
    assert_eq!(first["transcript"], live_prefix_text(0, 11));

    append_audio(
        &mut socket,
        audio_samples(&vec![8_192; LIVE_PREFIX_STRIDE_SAMPLES * 9 / 10]),
    )
    .await;
    wait_for_live_prefix_decodes(&final_decoder, 2).await;
    append_audio(
        &mut socket,
        audio_samples(&vec![8_192; LIVE_PREFIX_STRIDE_SAMPLES / 10]),
    )
    .await;
    let second = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(second["finalized"], live_prefix_text(0, 2));
    assert_eq!(
        second["agreed"],
        format!(" {}", live_prefix_text(2, 20))
    );
    assert_eq!(second["tentative"], " word0020");
    let visible_before_stop = live_prefix_text(0, 21);
    assert_eq!(second["transcript"], visible_before_stop);

    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    let completed = loop {
        let event = receive(&mut socket).await;
        match event["type"].as_str() {
            Some("conversation.item.input_audio_transcription.completed") => break event,
            Some(
                "input_audio_buffer.committed"
                | "conversation.item.created"
                | "conversation.item.input_audio_transcription.hypothesis",
            ) => {}
            other => panic!("unexpected mounted live-prefix event {other:?}: {event}"),
        }
    };
    assert_eq!(completed["transcript"], visible_before_stop);

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
