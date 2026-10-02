//! Generation ownership integration tests: request and worker-job ownership
//! for the one published runtime.

#![expect(
    clippy::expect_used,
    reason = "integration tests panic with the failed ownership invariant"
)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
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

#[tokio::test]
async fn shutdown_ends_a_decode_that_watches_its_cancellation_flag() {
    let decoder = ScriptedDecoder::new();
    decoder.park_next_until_cancelled(Duration::from_secs(10));
    let service = service(&decoder);
    let request_service = service.clone();
    let request = tokio::spawn(async move {
        transcribe_batch(request_service, "scripted-interim", &[0.25; 16]).await
    });
    let observer = decoder.clone();
    assert!(
        tokio::task::spawn_blocking(move || observer.wait_for_requests(1, WAIT))
            .await
            .expect("request waiter does not panic"),
        "the batch decode enters the decoder"
    );

    let shutdown_service = service.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::task::spawn_blocking(move || shutdown_service.shutdown()),
    )
    .await
    .expect("shutdown returns once the parked decode sees its flag")
    .expect("shutdown does not panic");
    assert!(
        decoder.observed_cancellation(),
        "the parked decode saw the epoch's flag set"
    );
    let (status, _) = tokio::time::timeout(WAIT, request)
        .await
        .expect("the cancelled request settles")
        .expect("request task joins");
    assert_ne!(
        status,
        StatusCode::OK,
        "a cancelled decode is not a success"
    );
}

#[tokio::test]
async fn batch_decodes_carry_the_epoch_cancellation_flag() {
    let decoder = ScriptedDecoder::new();
    decoder.push_text("done");
    let service = service(&decoder);
    let (status, _) = transcribe_batch(service.clone(), "scripted-interim", &[0.25; 16]).await;
    assert_eq!(status, StatusCode::OK);

    let requests = decoder.requests();
    assert_eq!(requests.len(), 1);
    let flag = Arc::clone(
        requests[0]
            .cancellation()
            .expect("a batch decode carries the epoch's flag"),
    );
    assert!(
        !flag.load(Ordering::Acquire),
        "an open epoch's flag is unset"
    );
    service.shutdown_admission();
    assert!(
        flag.load(Ordering::Acquire),
        "closing admission sets the flag the decode carried"
    );
    service.shutdown();
}
