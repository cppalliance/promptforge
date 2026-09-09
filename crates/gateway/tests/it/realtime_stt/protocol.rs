#[tokio::test]
async fn producer_snapshots_partition_finalized_agreed_and_tentative_text() {
    let interim = ScriptedDecoder::new();
    for transcript in [
        "Why is it",
        "Why is it",
        "Why is this",
        "is this working now",
    ] {
        interim.push_text(transcript);
    }
    let final_decoder = ScriptedDecoder::new();
    let service = speech_with_policy(&interim, Some(&final_decoder), 1, 500);
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

    let mut hypotheses = Vec::new();
    for _ in 0..4 {
        for _ in 0..5 {
            append_audio(&mut socket, audio()).await;
        }
        hypotheses.push(
            expect_type(
                &mut socket,
                "conversation.item.input_audio_transcription.hypothesis",
            )
            .await,
        );
    }

    assert_eq!(hypotheses[0]["transcript"], "Why is it");
    assert_eq!(hypotheses[1]["agreed"], "Why is it");
    assert_eq!(
        hypotheses[2]["transcript"], "Why is this",
        "a whole-window revision retracts its former promoted suffix"
    );
    assert_eq!(hypotheses[2]["audio_start_ms"], 500);
    assert_eq!(hypotheses[2]["audio_end_ms"], 1_500);
    assert_eq!(
        hypotheses[3]["transcript"], "Why is this working now",
        "the sliding window retains only the prefix before explicit overlap"
    );
    assert_eq!(hypotheses[3]["audio_start_ms"], 1_000);
    assert_eq!(hypotheses[3]["audio_end_ms"], 2_000);

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
#[tokio::test]
#[ignore = "requires packaged whisper.dll, ggml-tiny.en.bin, and jfk.wav fixtures"]
async fn realtime_stt_native_incremental() {
    for (variable, name) in [
        ("PROMPTFORGE_WHISPER_LIBRARY", "whisper.dll"),
        ("PROMPTFORGE_WHISPER_MODEL", "ggml-tiny.en.bin"),
        ("PROMPTFORGE_WHISPER_AUDIO", "jfk.wav"),
    ] {
        let _fixture = require_fixture(variable, &native_fixture_root(), name);
    }
    let service = native_speech_service();
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

    let samples = native_jfk_24khz();
    let mut cursor = 0;
    let mut spans = Vec::new();
    for chunk_samples in [48_000, 24_000, 24_000, 24_000] {
        let end = (cursor + chunk_samples).min(samples.len());
        append_audio(&mut socket, audio_samples(&samples[cursor..end])).await;
        cursor = end;
        let event = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                let event = receive_within(&mut socket, Duration::from_secs(90)).await;
                if event["type"] == "conversation.item.input_audio_transcription.hypothesis" {
                    return event;
                }
            }
        })
        .await
        .expect("native hypothesis arrives before its decode deadline");
        spans.push((
            event["audio_start_ms"]
                .as_u64()
                .expect("native start offset is unsigned"),
            event["audio_end_ms"]
                .as_u64()
                .expect("native end offset is unsigned"),
            event["transcript"]
                .as_str()
                .expect("native transcript is text")
                .to_owned(),
        ));
    }
    assert_native_incremental_spans(&spans);

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
    tokio::task::spawn_blocking(move || service.shutdown())
        .await
        .expect("native shutdown thread joins");
}
#[tokio::test]
async fn mounted_route_drives_scripted_wire_ownership_errors_and_privacy() {
    let interim = ScriptedDecoder::new();
    interim.push_text("provisional transcript");
    interim.push_text("provisional transcript");
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("authoritative transcript");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;

    let created = expect_type(&mut socket, "session.created").await;
    assert_eq!(created["session"]["type"], "transcription");
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "event_id": "private-client-update",
            "session": {
                "type": "transcription",
                "audio": {"input": {"transcription": {"prompt": "private prompt"}}},
                "include": []
            }
        }),
    )
    .await;
    let updated = expect_type(&mut socket, "session.updated").await;
    assert_eq!(
        updated["session"]["audio"]["input"]["transcription"]["prompt"],
        "private prompt"
    );

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "bad-audio",
            "audio": 7
        }),
    )
    .await;
    let error = expect_type(&mut socket, "error").await;
    assert_eq!(error["error"]["event_id"], "bad-audio");
    assert!(
        !error.to_string().contains(&audio()),
        "errors never echo buffered audio"
    );

    for pass in 1..=2 {
        for _ in 0..5 {
            append_audio(&mut socket, audio()).await;
        }
        let completed = interim.clone();
        assert!(
            tokio::task::spawn_blocking(move || {
                completed.wait_for_completed(pass, PHASE_TIMEOUT)
            })
            .await
            .expect("interim completion observer joins")
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.commit",
            "event_id": "commit-one"
        }),
    )
    .await;
    let committed = expect_type(&mut socket, "input_audio_buffer.committed").await;
    let item_id = committed["item_id"]
        .as_str()
        .expect("commit owns an item")
        .to_owned();
    assert_eq!(committed["item_id"], item_id);
    assert!(committed["previous_item_id"].is_null());
    let item = expect_type(&mut socket, "conversation.item.created").await;
    assert_eq!(item["item"]["id"], item_id);
    let delta = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.delta",
    )
    .await;
    assert_eq!(delta["item_id"], item_id);
    assert_eq!(delta["delta"], "provisional transcript");
    let complete = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;
    assert_eq!(complete["item_id"], item_id);
    assert_eq!(complete["transcript"], "authoritative transcript");

    let interim_requests = interim.requests();
    assert_eq!(interim_requests.len(), 2);
    assert_eq!(interim_requests[0].guidance(), ["private prompt"]);
    assert_eq!(final_decoder.requests().len(), 1);

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
#[tokio::test]
async fn mounted_session_errors_keep_canonical_codes_parameters_and_correlation() {
    let interim = ScriptedDecoder::new();
    interim.push_error("scripted interim failure");
    let service = speech(&interim, Some(&ScriptedDecoder::new()));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "event_id": "invalid-audio",
            "audio": "***"
        }),
    )
    .await;
    expect_error(
        &mut socket,
        "invalid_request_error",
        "invalid_base64_audio",
        "Audio must be valid Base64",
        serde_json::json!("audio"),
        "invalid-audio",
    )
    .await;

    for _ in 0..5 {
        append_audio(&mut socket, audio()).await;
    }
    let inference = expect_type(&mut socket, "error").await;
    assert_eq!(inference["error"]["type"], "server_error");
    assert_eq!(inference["error"]["code"], "internal_error");
    assert_eq!(inference["error"]["message"], "Transcription failed");
    assert!(inference["error"]["param"].is_null());
    assert!(
        inference["error"]["event_id"].is_null(),
        "scheduled inference failure is not attributed to one append"
    );

    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.clear"}),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.cleared").await;
    let short = base64::engine::general_purpose::STANDARD.encode([0_u8, 0]);
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": short
        }),
    )
    .await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.commit",
            "event_id": "short-commit"
        }),
    )
    .await;
    expect_error(
        &mut socket,
        "invalid_request_error",
        "audio_too_short",
        "A commit requires at least 100 ms of audio",
        serde_json::json!("audio"),
        "short-commit",
    )
    .await;

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
#[tokio::test]
async fn standard_interims_emit_only_appendable_agreed_deltas() {
    let interim = ScriptedDecoder::new();
    for transcript in ["Hello there", "Hello world", "Hello world again"] {
        interim.push_text(transcript);
    }
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("Hello world again");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    for pass in 1..=3 {
        for _ in 0..5 {
            append_audio(&mut socket, audio()).await;
        }
        let completed = interim.clone();
        assert!(
            tokio::task::spawn_blocking(move || {
                completed.wait_for_completed(pass, PHASE_TIMEOUT)
            })
            .await
            .expect("interim completion observer joins")
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.committed").await;
    expect_type(&mut socket, "conversation.item.created").await;
    let first = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.delta",
    )
    .await;
    let second = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.delta",
    )
    .await;
    assert_eq!(first["delta"], "Hello");
    assert_eq!(second["delta"], " world");
    assert_eq!(
        format!(
            "{}{}",
            first["delta"].as_str().expect("first delta is text"),
            second["delta"].as_str().expect("second delta is text")
        ),
        "Hello world"
    );
    expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.completed",
    )
    .await;

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
