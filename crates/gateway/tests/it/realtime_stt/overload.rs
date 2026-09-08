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
async fn mounted_slower_than_capture_overload_preserves_the_committable_input() {
    let interim = ScriptedDecoder::new();
    let final_decoder = ScriptedDecoder::new();
    let service = speech(&interim, Some(&final_decoder));
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;
    let stride = audio_samples(&vec![8_192; 24_000 * 10]);

    final_decoder
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                append_audio(&mut socket, stride.clone()).await;
                (&mut socket, stride)
            },
            |(socket, stride)| async move {
                append_audio(socket, stride.clone()).await;
                send(
                    socket,
                    serde_json::json!({
                        "type": "input_audio_buffer.append",
                        "event_id": "capture_outpaced_final",
                        "audio": stride
                    }),
                )
                .await;
                expect_error(
                    socket,
                    "overload_error",
                    "too_much_unfinalized_audio",
                    "Unfinalized audio exceeds 30 seconds",
                    serde_json::json!("audio"),
                    "capture_outpaced_final",
                )
                .await;
                send(
                    socket,
                    serde_json::json!({
                        "type": "input_audio_buffer.commit",
                        "event_id": "commit_after_throughput_overload"
                    }),
                )
                .await;
                expect_type(socket, "input_audio_buffer.committed").await;
                expect_type(socket, "conversation.item.created").await;
            },
        )
        .await
        .expect("the mounted forced decoder reaches retained-budget overload");

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
