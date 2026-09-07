#[tokio::test]
async fn mounted_terminal_failures_preserve_their_typed_wire_reason() {
    let fixtures = canonical_sequences();
    let canonical_overload = canonical_server(
        &fixtures,
        "segment_admission_failure",
        "conversation.item.input_audio_transcription.failed",
    );
    for (overload, kind, code, message) in [
        (
            false,
            "server_error",
            "precommit_transcription_failed",
            "Accurate precommit transcription failed",
        ),
        (
            true,
            "overload_error",
            "final_segment_overload",
            "The authoritative segment could not be admitted",
        ),
    ] {
        let mut service = speech(&ScriptedDecoder::new(), Some(&ScriptedDecoder::new()));
        if overload {
            service.overload_realtime_final_segment();
        } else {
            service.fail_realtime_precommit();
        }
        let server = server(true, &service).await;
        let mut socket = connect(server.addr, Some("test-token"), None, None).await;
        expect_type(&mut socket, "session.created").await;
        send(
            &mut socket,
            serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": audio()
            }),
        )
        .await;
        send(
            &mut socket,
            serde_json::json!({"type": "input_audio_buffer.commit"}),
        )
        .await;
        expect_type(&mut socket, "input_audio_buffer.committed").await;
        expect_type(&mut socket, "conversation.item.created").await;
        let failed = expect_type(
            &mut socket,
            "conversation.item.input_audio_transcription.failed",
        )
        .await;
        assert_eq!(failed["error"]["type"], kind, "{failed}");
        assert_eq!(failed["error"]["code"], code, "{failed}");
        assert_eq!(failed["error"]["message"], message, "{failed}");
        assert!(failed["error"]["param"].is_null(), "{failed}");
        assert!(failed["error"].get("event_id").is_none(), "{failed}");
        if overload {
            assert_eq!(failed["error"], canonical_overload["error"]);
        }

        socket.close(None).await.expect("socket closes");
        drop(socket);
        server.shutdown().await;
    }
}
#[tokio::test]
async fn saturated_commit_preserves_the_canonical_input_for_retry() {
    let fixtures = canonical_sequences();
    let mut append = canonical_client(
        &fixtures,
        "saturated_commit_retry",
        "input_audio_buffer.append",
    );
    append["audio"] = serde_json::json!(audio());
    let commit = canonical_message(
        &fixtures,
        "saturated_commit_retry",
        "client",
        "input_audio_buffer.commit",
        0,
    );
    let retry = canonical_message(
        &fixtures,
        "saturated_commit_retry",
        "client",
        "input_audio_buffer.commit",
        1,
    );
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    final_decoder.park_next();
    for transcript in [
        "released",
        "existing two",
        "existing three",
        "existing four",
    ] {
        final_decoder.push_text(transcript);
    }
    final_decoder.push_text("retried canonical input");
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    let mut existing_items: Vec<String> = Vec::new();
    for _ in 0..4 {
        let item_id = commit_existing_item(&mut socket, &append, existing_items.last()).await;
        existing_items.push(item_id);
    }
    let parked = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("park observer joins"),
        "four committed items remain outstanding behind the parked final worker"
    );
    assert_eq!(
        final_decoder.requests().len(),
        1,
        "the serial final worker is parked while four items own finalization"
    );

    for _ in 0..5 {
        send(&mut socket, append.clone()).await;
    }
    send(&mut socket, commit).await;
    let saturated = expect_type(&mut socket, "error").await;
    let requests_at_saturation = final_decoder.requests().len();
    final_decoder.release();
    let expected_error = canonical_server(&fixtures, "saturated_commit_retry", "error");
    for field in ["type", "code", "message", "param", "event_id"] {
        assert_eq!(
            saturated["error"][field], expected_error["error"][field],
            "{field}: {saturated}"
        );
    }
    assert_eq!(
        requests_at_saturation, 1,
        "the rejected commit starts no fifth finalization"
    );

    let expected_release = canonical_server(
        &fixtures,
        "saturated_commit_retry",
        "conversation.item.input_audio_transcription.completed",
    );
    expect_existing_completions(&mut socket, &existing_items, &expected_release).await;
    expect_retried_item(&mut socket, retry, &existing_items).await;

    let final_requests = final_decoder.requests();
    assert_eq!(final_requests.len(), 5);
    assert_eq!(
        final_requests[4].samples(),
        final_requests[0].samples(),
        "retry finalizes exactly the same canonical audio as an accepted item"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
