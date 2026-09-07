#[tokio::test]
async fn interim_scheduler_enforces_cadence_minimum_silence_and_coalescing() {
    let interim = ScriptedDecoder::new();
    interim.push_text("first window");
    interim.push_text("newest window");
    let service = speech_with_policy(&interim, Some(&ScriptedDecoder::new()), 15, 500);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    for _ in 0..4 {
        append_audio(&mut socket, audio()).await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        interim.requests().is_empty(),
        "sub-500 ms audio never enters the decoder"
    );

    interim.park_next();
    append_audio(&mut socket, audio()).await;
    let parked = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("park observer joins"),
        "the first eligible scheduled decode parks"
    );
    for _ in 0..5 {
        append_audio(&mut socket, audio()).await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        interim.requests().len(),
        1,
        "only one interim decode may be in flight"
    );

    interim.release();
    let coalesced = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || coalesced.wait_for_requests(2, PHASE_TIMEOUT))
            .await
            .expect("coalesced request observer joins"),
        "the newest eligible snapshot runs after release"
    );
    assert_eq!(interim.requests()[1].samples().len(), 16_000);

    interim.park_next();
    for _ in 0..5 {
        append_audio(&mut socket, audio()).await;
    }
    let canceled = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || canceled.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("cancellation park observer joins")
    );
    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.clear"}),
    )
    .await;
    expect_type(&mut socket, "input_audio_buffer.cleared").await;
    interim.release();
    let cleaned = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || cleaned.wait_for_completed(3, PHASE_TIMEOUT))
            .await
            .expect("canceled worker observer joins"),
        "cleared scheduled work releases its underlying worker job"
    );
    for _ in 0..5 {
        append_audio(&mut socket, audio_samples(&vec![0; 2_400])).await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        interim.requests().len(),
        3,
        "eligible silent windows are suppressed"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
#[tokio::test]
async fn completion_cadence_reaps_more_than_eight_canceled_interims() {
    let interim = ScriptedDecoder::new();
    let service = speech_with_policy(&interim, Some(&ScriptedDecoder::new()), 15, 50);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    for request_count in 1..=10 {
        interim.park_next();
        for _ in 0..5 {
            append_audio(&mut socket, audio()).await;
        }
        let parked = interim.clone();
        assert!(
            tokio::task::spawn_blocking(move || {
                parked.wait_for_requests(request_count, PHASE_TIMEOUT)
                    && parked.wait_until_parked(PHASE_TIMEOUT)
            })
            .await
            .expect("park observer joins"),
            "scheduled interim {request_count} reaches its worker"
        );
        send(
            &mut socket,
            serde_json::json!({"type": "input_audio_buffer.clear"}),
        )
        .await;
        assert_eq!(
            expect_type(&mut socket, "input_audio_buffer.cleared").await["type"],
            "input_audio_buffer.cleared",
            "completed canceled joins free bounded capacity before cycle {request_count}"
        );
        interim.release();
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

    final_decoder.park_next();
    append_audio(
        &mut socket,
        audio_samples(&[vec![0; 72_000], vec![8_192; 12_000]].concat()),
    )
    .await;
    let parked = final_decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || parked.wait_until_parked(PHASE_TIMEOUT))
            .await
            .expect("finalization park observer joins")
    );
    let pending = expect_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(pending["transcript"], "first phrase second phrase");
    assert_eq!(pending["finalized"], "");

    final_decoder.release();
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
