use super::*;

fn start_decode(
    engine: Arc<SttEngine>,
) -> tokio::task::JoinHandle<Result<String, TranscribeError>> {
    tokio::spawn(async move {
        engine
            .decode(request(DecodeMode::Interim, vec![0.25], Vec::new(), ""))
            .await
    })
}

#[tokio::test]
async fn blocked_decode_scenario_releases_after_normal_return() {
    let decoder = ScriptedDecoder::new();
    let engine = Arc::new(
        SttEngine::new(ScriptedModelFactory::new(decoder.clone()), policy())
            .expect("scripted worker starts"),
    );
    run_blocked_decode(&decoder, &engine, "released").await;
    engine.shutdown().expect("worker joins");
}

#[tokio::test]
async fn canceled_decode_scenario_releases_and_permits_a_follow_up() {
    let decoder = ScriptedDecoder::new();
    decoder.push_text("released after cancellation");
    let engine = Arc::new(
        SttEngine::new(ScriptedModelFactory::new(decoder.clone()), policy())
            .expect("scripted worker starts"),
    );
    let scenario_decoder = decoder.clone();
    let scenario_engine = Arc::clone(&engine);
    let (decode_tx, decode_rx) = tokio::sync::oneshot::channel();
    let (parked_tx, parked_rx) = tokio::sync::oneshot::channel();
    let scenario = tokio::spawn(async move {
        scenario_decoder
            .with_next_decode_blocked(
                WAIT,
                || async move {
                    drop(decode_tx.send(start_decode(scenario_engine)));
                },
                |()| async move {
                    let _ = parked_tx.send(());
                    std::future::pending::<()>().await;
                },
            )
            .await
    });

    tokio::time::timeout(WAIT, parked_rx)
        .await
        .expect("decode parks before cancellation")
        .expect("park observer remains live");
    scenario.abort();
    assert!(
        scenario
            .await
            .expect_err("scenario is canceled")
            .is_cancelled()
    );
    assert_eq!(
        tokio::time::timeout(WAIT, decode_rx.await.expect("decode handle is published"))
            .await
            .expect("cancellation releases the decode")
            .expect("decode task joins")
            .expect("released decode succeeds"),
        "released after cancellation"
    );
    run_blocked_decode(&decoder, &engine, "follow-up after cancellation").await;
    engine.shutdown().expect("worker joins");
}

#[tokio::test]
async fn decode_rendezvous_timeout_releases_a_late_arrival_and_permits_a_follow_up() {
    let decoder = ScriptedDecoder::new();
    decoder.push_text("late arrival");
    let engine = Arc::new(
        SttEngine::new(ScriptedModelFactory::new(decoder.clone()), policy())
            .expect("scripted worker starts"),
    );
    // The rendezvous timeout is a real-time condvar wait while the paused
    // clock auto-advances when the runtime idles, so a timer-based late
    // arrival would race the rendezvous. Arriving only after the timeout
    // keeps the lateness deterministic.
    let result = decoder
        .with_next_decode_blocked(Duration::from_millis(10), || async {}, |()| async {})
        .await;
    assert!(result.is_none(), "the rendezvous must time out");
    let delayed_engine = Arc::clone(&engine);
    let decode = tokio::spawn(async move {
        start_decode(delayed_engine)
            .await
            .expect("nested decode task joins")
    });
    assert_eq!(
        tokio::time::timeout(WAIT, decode)
            .await
            .expect("late decode is not stranded")
            .expect("late decode task joins")
            .expect("late decode succeeds"),
        "late arrival"
    );
    run_blocked_decode(&decoder, &engine, "follow-up after timeout").await;
    engine.shutdown().expect("worker joins");
}

#[tokio::test]
async fn panicked_decode_scenario_releases_and_permits_a_follow_up() {
    let decoder = ScriptedDecoder::new();
    decoder.push_text("released after panic");
    let engine = Arc::new(
        SttEngine::new(ScriptedModelFactory::new(decoder.clone()), policy())
            .expect("scripted worker starts"),
    );
    let scenario_decoder = decoder.clone();
    let scenario_engine = Arc::clone(&engine);
    let (decode_tx, decode_rx) = tokio::sync::oneshot::channel();
    let scenario = tokio::spawn(async move {
        scenario_decoder
            .with_next_decode_blocked(
                WAIT,
                || async move {
                    drop(decode_tx.send(start_decode(scenario_engine)));
                },
                |()| async move {
                    panic!("decode scenario panic sentinel");
                },
            )
            .await
    });
    assert!(scenario.await.expect_err("scenario panics").is_panic());
    assert_eq!(
        tokio::time::timeout(WAIT, decode_rx.await.expect("decode handle is published"))
            .await
            .expect("unwind releases the decode")
            .expect("decode task joins")
            .expect("released decode succeeds"),
        "released after panic"
    );
    run_blocked_decode(&decoder, &engine, "follow-up after panic").await;
    engine.shutdown().expect("worker joins");
}
