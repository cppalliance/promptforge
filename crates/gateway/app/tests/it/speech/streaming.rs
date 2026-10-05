//! Speech stream lifetime: the permit is held until the audio stream ends, a
//! client disconnect aborts the upstream, and a mid-stream failure fails the read.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::post;
use futures_util::StreamExt as _;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;

use super::{bytes_within, gated_audio_backend, spawn_speech, speech_body, speech_gateway};
use crate::support::{PHASE_TIMEOUT, join_within, next_arrival, send_within, spawn_backend};

/// Bounded negative wait for a request that must not be admitted: long
/// enough that a released slot would deterministically let the request
/// reach the backend over loopback, and comfortably under the relay's
/// scaled 200 ms upstream-idle budget so the held stream's relay cannot
/// trip idle and free the slot mid-wait (the same ceiling [`DRIP_GAP`]
/// stays under).
const ADMISSION_GRACE: Duration = Duration::from_millis(100);

/// Under concurrency=1, a speech request holds the dominion queue permit
/// for the audio stream's whole lifetime: a second request is not admitted
/// until the first stream has ended.
#[tokio::test]
async fn stream_permit_is_held_until_the_audio_stream_ends() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // The second request cannot be admitted while the first stream holds the
    // only concurrency slot: a bounded negative wait proves no arrival, where
    // a bare try_recv would pass vacuously on a current-thread runtime that
    // never polled the spawned request.
    let second = spawn_speech(&client, &url);
    let arrived = tokio::time::timeout(ADMISSION_GRACE, arrivals.recv()).await;
    assert!(
        arrived.is_err(),
        "second request must not reach the backend while the stream is open"
    );

    let first_response = join_within(first).await.unwrap();
    assert_eq!(first_response.status().as_u16(), 200);
    release_first.send(()).unwrap();
    // Drain the body so the relay finishes and releases the permit.
    let body = bytes_within(first_response).await;
    assert_eq!(body, b"audio-chunk-1;audio-chunk-2;", "stream completed");

    // After the stream ends, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    let second = join_within(second).await.unwrap();
    assert_eq!(second.status().as_u16(), 200);
    let _ = bytes_within(second).await;
    gateway.shutdown().await;
}

/// A client disconnect mid-stream cancels the upstream stream: dropping the
/// response body drops the relay, which drops the gateway's upstream
/// connection, which the backend observes as its own response body being
/// dropped. Drop is the entire mechanism - there is no explicit cancel
/// path. The released permit admits a later request under concurrency=1.
#[tokio::test]
async fn client_disconnect_aborts_the_upstream_stream_and_releases_the_permit() {
    /// Signals once the backend's response body is dropped mid-stream.
    struct NotifyOnDrop(UnboundedSender<()>);
    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    let (dropped, mut observed) = mpsc::unbounded_channel::<()>();
    let backend = spawn_backend(Router::new().route(
        "/audio/speech",
        post(move || {
            let dropped = dropped.clone();
            async move {
                let first = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-chunk-1;"))
                });
                let rest = futures_util::stream::once(async move {
                    let _notify = NotifyOnDrop(dropped);
                    futures_util::future::pending::<()>().await;
                    unreachable!("the stream never yields a second chunk")
                });
                let mut response = Response::new(Body::from_stream(first.chain(rest)));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
                response
            }
        }),
    ))
    .await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let mut response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    // Read the first chunk so the stream is genuinely mid-flight, then hang up.
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "first chunk arrived");
    drop(response);

    tokio::time::timeout(PHASE_TIMEOUT, observed.recv())
        .await
        .expect("backend did not observe the disconnect within the phase timeout")
        .expect("disconnect notification channel closed");

    // The permit went back: a second request is admitted under concurrency=1.
    let mut second = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(second.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, second.chunk())
        .await
        .expect("second chunk read exceeded the phase timeout")
        .expect("second chunk read failed");
    assert!(
        chunk.is_some(),
        "the second request was admitted and answered after the disconnect released the permit"
    );
    drop(second);
    gateway.shutdown().await;
}

/// A mid-stream upstream failure after HTTP 200 cannot become an error
/// envelope - audio bytes already flowed - so the relay propagates the
/// failure and the client's body read fails on the truncation; the bytes
/// that did arrive are pure audio with no JSON spliced in.
#[tokio::test]
async fn mid_stream_upstream_error_fails_the_body_read_without_an_envelope() {
    const AUDIO_PREFIX: &[u8] = b"audio-so-far;";

    // The backend sends one chunk, then waits for the test to trigger the
    // failure, so the error is guaranteed to land after the client holds a
    // 200 and real audio bytes: a rendezvous, not a race.
    let (fail, wait_fail) = oneshot::channel::<()>();
    let wait_fail = Arc::new(Mutex::new(Some(wait_fail)));
    let backend = spawn_backend(Router::new().route(
        "/audio/speech",
        post(move || {
            let wait_fail = Arc::clone(&wait_fail);
            async move {
                let wait = wait_fail
                    .lock()
                    .unwrap()
                    .take()
                    .expect("the test sends one request");
                let first = futures_util::stream::once(async {
                    Ok::<_, std::io::Error>(Bytes::from_static(AUDIO_PREFIX))
                });
                let rest = futures_util::stream::once(async move {
                    let _ = wait.await;
                    Err(std::io::Error::other("upstream died mid-stream"))
                });
                let mut response = Response::new(Body::from_stream(first.chain(rest)));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
                response
            }
        }),
    ))
    .await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let mut response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the failure is mid-stream, after the 200"
    );
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed")
        .expect("the first audio chunk arrived");
    let mut received = first.to_vec();
    fail.send(()).expect("the backend is waiting on the signal");
    let failed = loop {
        match tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
            .await
            .expect("body read exceeded the phase timeout")
        {
            Ok(Some(chunk)) => received.extend_from_slice(&chunk),
            Ok(None) => break false,
            Err(_) => break true,
        }
    };
    assert!(
        failed,
        "the client's body read fails on a mid-stream upstream error"
    );
    assert_eq!(
        received, AUDIO_PREFIX,
        "only audio bytes arrived; no JSON envelope was spliced into the stream"
    );
    gateway.shutdown().await;
}
