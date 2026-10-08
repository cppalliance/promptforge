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

    interim
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                append_audio(&mut socket, audio()).await;
                &mut socket
            },
            |socket| async {
                for _ in 0..5 {
                    append_audio(socket, audio()).await;
                }
                tokio::time::sleep(Duration::from_millis(600)).await;
                assert_eq!(
                    interim.requests().len(),
                    1,
                    "only one interim decode may be in flight"
                );
            },
        )
        .await
        .expect("the first eligible scheduled decode reaches the blocked scenario");
    let coalesced = interim.clone();
    assert!(
        tokio::task::spawn_blocking(move || coalesced.wait_for_requests(2, PHASE_TIMEOUT))
            .await
            .expect("coalesced request observer joins"),
        "the newest eligible snapshot runs after release"
    );
    assert_eq!(interim.requests()[1].samples().len(), 16_000);

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
                send(
                    socket,
                    serde_json::json!({"type": "input_audio_buffer.clear"}),
                )
                .await;
                expect_type(socket, "input_audio_buffer.cleared").await;
            },
        )
        .await
        .expect("the canceled interim reaches the blocked scenario");
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
async fn interim_tick_skipped_by_a_slow_decode_runs_as_soon_as_that_decode_finishes() {
    const INTERVAL_MS: u64 = 1_000;
    let interval = Duration::from_millis(INTERVAL_MS);
    let interim = ScriptedDecoder::new();
    let service = speech_with_policy(&interim, Some(&ScriptedDecoder::new()), 15, INTERVAL_MS);
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;

    let released = interim
        .with_next_decode_blocked(
            PHASE_TIMEOUT,
            || async {
                for _ in 0..5 {
                    append_audio(&mut socket, audio()).await;
                }
                &mut socket
            },
            |socket| async {
                append_audio(socket, audio()).await;
                // The decode parked just after the tick that started it, so
                // this outlasts the following tick and releases the decode
                // three quarters of an interval before the one after that.
                tokio::time::sleep(interval + interval / 4).await;
                std::time::Instant::now()
            },
        )
        .await
        .expect("the first eligible interim reaches the blocked scenario");
    let caught_up = interim.clone();
    let started_after = tokio::task::spawn_blocking(move || {
        caught_up
            .wait_for_requests(2, PHASE_TIMEOUT)
            .then(|| released.elapsed())
    })
    .await
    .expect("catch-up request observer joins")
    .expect("an interim decode follows the released one");
    assert!(
        started_after < interval / 4,
        "the tick skipped mid-decode starts its decode when the slow decode is reaped, \
         not at the next tick: it started {started_after:?} after release"
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
