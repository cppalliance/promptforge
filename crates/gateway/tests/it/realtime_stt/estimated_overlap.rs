fn unaligned_window(window: usize, tokens: usize) -> String {
    (0..tokens)
        .map(|token| format!("w{window}t{token}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn append_owned_text(text: &mut String, piece: &str) {
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(piece);
}

fn assert_complete_estimated_hypothesis(
    hypothesis: &serde_json::Value,
    finalized: &str,
    pending: &str,
    tail: &str,
    item_id: &mut Option<String>,
) {
    let current_id = hypothesis["item_id"]
        .as_str()
        .expect("the live hypothesis names its one item");
    if let Some(expected_id) = item_id.as_ref() {
        assert_eq!(current_id, expected_id);
    } else {
        *item_id = Some(current_id.to_owned());
    }
    let mut complete = finalized.to_owned();
    append_owned_text(&mut complete, pending);
    append_owned_text(&mut complete, tail);
    assert_eq!(hypothesis["transcript"], complete, "{hypothesis}");
    assert_eq!(hypothesis["finalized"], finalized, "{hypothesis}");
    assert_eq!(
        hypothesis["agreed"],
        format!("{}{}", if finalized.is_empty() { "" } else { " " }, pending),
        "{hypothesis}"
    );
}

fn unaligned_decoders() -> (ScriptedDecoder, ScriptedDecoder, Vec<String>) {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    let mut windows = Vec::new();
    for window in 0..7 {
        interim.push_text(format!("tail{window}"));
        let tokens = match window {
            0 => 10,
            6 => 9,
            _ => 18,
        };
        let text = unaligned_window(window, tokens);
        final_decoder.push_text(&text);
        windows.push(text);
    }
    (interim, final_decoder, windows)
}

#[tokio::test]
async fn mounted_unaligned_windows_keep_complete_live_text_and_finish_once() {
    let (interim, final_decoder, windows) = unaligned_decoders();
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

    let mut finalized = String::new();
    let mut item_id = None;
    for window in 0..6 {
        let samples = if window == 0 {
            LIVE_PREFIX_STRIDE_SAMPLES
        } else {
            LIVE_PREFIX_STRIDE_SAMPLES * 9 / 10
        };
        append_audio(&mut socket, audio_samples(&vec![8_192; samples])).await;
        wait_for_live_prefix_decodes(&final_decoder, window + 1).await;
        if window > 0 {
            let owned_tokens = if window == 1 { 2 } else { 10 };
            append_owned_text(
                &mut finalized,
                &windows[window - 1]
                    .split_whitespace()
                    .take(owned_tokens)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        }
        append_audio(
            &mut socket,
            audio_samples(&vec![8_192; LIVE_PREFIX_STRIDE_SAMPLES / 10]),
        )
        .await;
        let hypothesis = expect_type(
            &mut socket,
            "conversation.item.input_audio_transcription.hypothesis",
        )
        .await;
        assert_complete_estimated_hypothesis(
            &hypothesis,
            &finalized,
            &windows[window],
            &format!("tail{window}"),
            &mut item_id,
        );
    }

    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    wait_for_live_prefix_decodes(&final_decoder, 7).await;
    append_owned_text(
        &mut finalized,
        &windows[5]
            .split_whitespace()
            .take(10)
            .collect::<Vec<_>>()
            .join(" "),
    );
    let mut expected_completion = finalized;
    append_owned_text(&mut expected_completion, &windows[6]);
    let mut committed = 0;
    let mut created = 0;
    let completed = loop {
        let event = receive(&mut socket).await;
        match event["type"].as_str() {
            Some("input_audio_buffer.committed") => committed += 1,
            Some("conversation.item.created") => created += 1,
            Some("conversation.item.input_audio_transcription.hypothesis") => {}
            Some("conversation.item.input_audio_transcription.completed") => break event,
            Some("conversation.item.input_audio_transcription.failed" | "error") => {
                panic!("healthy estimated take emitted an error: {event}")
            }
            other => panic!("unexpected estimated-overlap event {other:?}: {event}"),
        }
    };
    assert_eq!((committed, created), (1, 1));
    assert_eq!(completed["item_id"], item_id.expect("one live item was observed"));
    assert_eq!(completed["transcript"], expected_completion);
    assert!(!expected_completion.is_empty());

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
