//! Generation ownership integration tests: request and worker-job ownership
//! for the one published runtime.

#![expect(
    clippy::expect_used,
    reason = "integration tests panic with the failed ownership invariant"
)]

use std::time::Duration;

use gateway_stt::SpeechService;
use gateway_stt::test_fixtures::{
    ScriptedDecoder, ScriptedModelFactory, generation_counts, generation_ownership,
    scripted_service,
};

use crate::common::transcribe_batch;

const WAIT: Duration = Duration::from_secs(2);

fn factory(decoder: &ScriptedDecoder) -> ScriptedModelFactory {
    ScriptedModelFactory::new(decoder.clone())
}

fn service(decoder: &ScriptedDecoder) -> SpeechService {
    scripted_service(factory(decoder), 15, 500).expect("scripted generation starts")
}

#[test]
fn request_and_worker_job_ownership_are_counted_independently() {
    let decoder = ScriptedDecoder::new();
    let service = service(&decoder);
    let request = generation_ownership(&service).expect("open generation admits a request");
    assert_eq!(generation_counts(&service), Some((1, 0)));

    let job = request
        .own_worker_job()
        .expect("the admitted request owns a worker job");
    assert_eq!(generation_counts(&service), Some((1, 1)));

    drop(request);
    assert_eq!(
        generation_counts(&service),
        Some((0, 1)),
        "request cancellation cannot report false worker idleness"
    );
    drop(job);
    assert_eq!(generation_counts(&service), Some((0, 0)));
    service.shutdown();
}

#[tokio::test]
async fn canceled_request_keeps_its_worker_job_owned_until_decode_returns() {
    let decoder = ScriptedDecoder::new();
    let service = service(&decoder);
    decoder
        .with_next_decode_blocked(
            WAIT,
            || {
                let request_service = service.clone();
                async move {
                    (tokio::spawn(async move {
                        transcribe_batch(request_service, "scripted-interim", &[0.25; 16]).await
                    }),)
                }
            },
            |(request,)| async {
                assert_eq!(generation_counts(&service), Some((1, 1)));
                request.abort();
                assert!(
                    request
                        .await
                        .expect_err("request is canceled")
                        .is_cancelled()
                );
                tokio::time::timeout(WAIT, async {
                    while generation_counts(&service) != Some((0, 1)) {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("request ownership drops while worker ownership remains");
                assert_eq!(generation_counts(&service), Some((0, 1)));
            },
        )
        .await
        .expect("decode reaches the blocked native-equivalent scenario");
    tokio::time::timeout(WAIT, async {
        while generation_counts(&service) != Some((0, 0)) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("worker ownership drains after native decode returns");
    service.shutdown();
}
