use gateway_stt::test_fixtures::{
    HourSimulationProbe, hour_marker_input, hour_simulation_service,
};

const HOUR_STRIDES: usize = 360;
const INPUT_STRIDE_SAMPLES: usize = 24_000 * 10;
const OUTPUT_STRIDE_SAMPLES: u64 = 16_000 * 10;
const HOUR_CHUNKS: [usize; 6] = [1, 23_999, 72_000, 17, 47_983, 96_000];

fn expected_hour_text() -> String {
    (0..3_600)
        .map(|second| format!("word{second:04}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::test]
async fn mounted_hour_stream_keeps_one_item_commit_and_completion() {
    let probe = HourSimulationProbe::new();
    let service =
        hour_simulation_service(probe.clone()).expect("marked hour speech service starts");
    let server = server(true, &service).await;
    let mut socket = connect(server.addr, Some("test-token"), None, None).await;
    expect_type(&mut socket, "session.created").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "session.update",
            "session": {
                "type": "transcription",
                "include": []
            }
        }),
    )
    .await;
    expect_type(&mut socket, "session.updated").await;

    for stride in 0..HOUR_STRIDES {
        let mut offset = 0_usize;
        for chunk in 0..HOUR_CHUNKS.len() {
            let count = HOUR_CHUNKS[(stride + chunk) % HOUR_CHUNKS.len()];
            let start = u64::try_from(stride * INPUT_STRIDE_SAMPLES + offset)
                .expect("hour input offset fits");
            append_audio(
                &mut socket,
                audio_samples(&hour_marker_input(start, count)),
            )
            .await;
            offset += count;
        }
        assert_eq!(offset, INPUT_STRIDE_SAMPLES);
        let deadline = std::time::Instant::now() + PHASE_TIMEOUT;
        while probe.final_decode_count() < stride + 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "mounted forced decode {stride} completes"
            );
            tokio::task::yield_now().await;
        }
    }

    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.commit",
            "event_id": "hour_commit"
        }),
    )
    .await;
    let mut committed = Vec::new();
    let mut created = Vec::new();
    let completed = loop {
        let event = receive(&mut socket).await;
        match event["type"].as_str() {
            Some("input_audio_buffer.committed") => {
                committed.push(event["item_id"].as_str().expect("commit has item").to_owned());
            }
            Some("conversation.item.created") => {
                created.push(event["item"]["id"].as_str().expect("item has id").to_owned());
            }
            Some("conversation.item.input_audio_transcription.completed") => break event,
            Some("conversation.item.input_audio_transcription.delta") => {}
            other => panic!("unexpected mounted hour event {other:?}: {event}"),
        }
    };

    assert_eq!(committed.len(), 1);
    assert_eq!(created, committed);
    assert_eq!(completed["item_id"], committed[0]);
    assert_eq!(completed["usage"]["seconds"], 3_600.0);
    assert_eq!(completed["transcript"], expected_hour_text());
    assert_eq!(probe.final_decode_count(), HOUR_STRIDES);
    assert_eq!(
        probe.gap_free_coverage_samples(),
        OUTPUT_STRIDE_SAMPLES * HOUR_STRIDES as u64
    );

    socket.close(None).await.expect("socket closes");
    drop(socket);
    server.shutdown().await;
}
