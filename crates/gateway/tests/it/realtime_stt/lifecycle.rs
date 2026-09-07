#[tokio::test]
async fn completion_cadence_reaps_more_than_eight_canceled_interims() {
    let interim = ScriptedDecoder::new();
    let service = speech_with_policy(&interim, Some(&ScriptedDecoder::new()), 15, 50);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    for request_count in 1..=10 {
        interim
            .with_next_decode_blocked(
                PHASE_TIMEOUT,
                || async {
                    for _ in 0..5 {
                        append_audio(&mut socket, audio()).await;
                    }
                    &mut socket
                },
                |socket| async {
                    assert_eq!(
                        interim.requests().len(),
                        request_count,
                        "scheduled interim {request_count} reaches its worker"
                    );
                    send(
                        socket,
                        serde_json::json!({"type": "input_audio_buffer.clear"}),
                    )
                    .await;
                    assert_eq!(
                        expect_type(socket, "input_audio_buffer.cleared").await["type"],
                        "input_audio_buffer.cleared",
                        "completed canceled joins free bounded capacity before cycle {request_count}"
                    );
                },
            )
            .await
            .unwrap_or_else(|| {
                panic!("scheduled interim {request_count} reaches the blocked scenario")
            });
        let completed = interim.clone();
        assert!(
            tokio::task::spawn_blocking(move || {
                completed.wait_for_completed(request_count, PHASE_TIMEOUT)
            })
            .await
            .expect("completion observer joins"),
            "underlying worker job {request_count} completes"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
#[tokio::test]
async fn consumed_boundary_rebases_before_delayed_finalization_completes() {
    let interim = ScriptedDecoder::new();
    for transcript in ["first phrase", "second phrase", "second phrase now"] {
        interim.push_text(transcript);
    }
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text("revised first");
    let service = speech_with_policy(&interim, Some(&final_decoder), 8, 500);
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

    for _ in 0..10 {
        append_audio(&mut socket, audio()).await;
    }
    let first = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(first["transcript"], "first phrase");

    final_decoder
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                append_audio(
                    &mut socket,
                    audio_samples(&[vec![0; 72_000], vec![8_192; 12_000]].concat()),
                )
                .await;
                &mut socket
            },
            |socket| async {
                let pending = expect_type(
                    socket,
                    "conversation.item.input_audio_transcription.hypothesis",
                )
                .await;
                assert_eq!(pending["transcript"], "first phrase second phrase");
                assert_eq!(pending["finalized"], "");
            },
        )
        .await
        .expect("delayed finalization reaches the blocked scenario");
    let finalized = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || finalized.wait_for_completed(1, PHASE_TIMEOUT))
            .await
            .expect("finalization completion observer joins")
    );
    append_audio(&mut socket, audio()).await;
    let revised = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(revised["finalized"], "revised first");
    assert_eq!(revised["transcript"], "revised first second phrase now");

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
#[tokio::test]
async fn stop_reconciles_an_accepted_word_from_a_skipped_short_final_range() {
    assert_stop_reconciles_skipped_range(7_200).await;
}
#[tokio::test]
async fn stop_reconciles_an_accepted_word_from_a_click_consumed_range() {
    assert_stop_reconciles_skipped_range(2_400).await;
}
#[tokio::test]
async fn same_range_divergent_final_text_overrides_the_accepted_hypothesis() {
    assert_same_range_final_authority("authoritative words", "authoritative words").await;
}
#[tokio::test]
async fn same_range_decoded_empty_remains_authoritative() {
    assert_same_range_final_authority("", "").await;
}
