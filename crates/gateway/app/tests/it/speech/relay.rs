//! The bounded background relay's terminal paths (byte ceiling, total lifetime,
//! upstream idle, blocked downstream, cancellation), each proving the permit
//! goes back by the admission of a later request.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::post;
use futures_util::StreamExt as _;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use super::{CANNED_AUDIO, bytes_within, spawn_speech, speech_body, speech_gateway};
use crate::support::{PHASE_TIMEOUT, join_within, send_within, spawn_backend};

mod backends;

use backends::{
    dripping_audio_backend, fixed_size_audio_backend, saturating_audio_backend,
    slow_headers_audio_backend, stalling_audio_backend,
};

// Bounded-relay boundary tests. The gateway builds this suite with its
// `test-fixtures` feature (the crate dev-depends on itself with it), which
// scales the relay's bounds down: byte ceiling 16 MiB, upstream read idle
// 200 ms, blocked downstream delivery 400 ms, total stream lifetime 2 s, and
// the profile-switch drain deadline 1 s. These mirrors name the same numbers
// so the boundary tests run in milliseconds; the relay's own constants are
// the source of truth.

/// The relay's test-scaled response byte ceiling.
const RELAY_BYTE_CEILING: u64 = 16 * 1024 * 1024;
/// A drip gap comfortably under the scaled 200 ms upstream-idle budget.
const DRIP_GAP: Duration = Duration::from_millis(60);
/// A header delay past the scaled idle budget but far under the
/// first-response budget: time-to-headers is never the relay's business.
const HEADER_DELAY: Duration = Duration::from_millis(300);

/// Waits for a backend arrival ping, bounded by [`PHASE_TIMEOUT`].
async fn next_ping(arrivals: &mut UnboundedReceiver<()>) {
    tokio::time::timeout(PHASE_TIMEOUT, arrivals.recv())
        .await
        .expect("timed out waiting for backend arrival")
        .expect("arrivals channel closed");
}

/// Reads a response body to its end or its failure, returning the bytes
/// that arrived and whether the read failed. A relay terminal path surfaces
/// over HTTP only as a failed read (never a clean EOF): hyper aborts the
/// response on a body-stream error, so the terminal item's message stays
/// server-side and the tests discriminate the bounds by stream shape and
/// timing instead. The whole read is bounded by [`PHASE_TIMEOUT`], so a
/// stream that never ends and never fails is a test failure, never a hang.
async fn read_to_end_or_error(response: reqwest::Response) -> (Vec<u8>, bool) {
    let mut response = response;
    let mut received = Vec::new();
    let deadline = tokio::time::Instant::now() + PHASE_TIMEOUT;
    loop {
        let item = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .expect("HTTP body read exceeded the phase timeout");
        match item {
            Ok(Some(chunk)) => received.extend_from_slice(&chunk),
            Ok(None) => return (received, false),
            Err(_) => return (received, true),
        }
    }
}

/// Proves the first stream's permit went back: under a one-slot dominion a
/// second request is admitted only once the relay holding the slot ended,
/// so its arrival at the backend is the release proof. The admitted request
/// is answered and then dropped mid-stream.
async fn assert_permit_released_by_admission(
    client: &reqwest::Client,
    url: &str,
    arrivals: &mut UnboundedReceiver<()>,
) {
    let second = spawn_speech(client, url);
    next_ping(arrivals).await;
    let second = join_within(second)
        .await
        .expect("the second request sends once the permit is free");
    assert_eq!(
        second.status().as_u16(),
        200,
        "a later request is admitted once the ended relay released the permit"
    );
    drop(second);
}

/// The relay's response-byte ceiling: a stream totaling exactly the ceiling
/// is accepted whole and ends cleanly.
#[tokio::test]
async fn relay_accepts_a_stream_at_the_exact_byte_ceiling() {
    let (backend, mut arrivals) = fixed_size_audio_backend(RELAY_BYTE_CEILING).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(!failed, "an exactly-at-ceiling stream ends cleanly");
    assert_eq!(
        u64::try_from(received.len()).unwrap(),
        RELAY_BYTE_CEILING,
        "every byte arrived"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// One byte past the ceiling fails the client's body read: the relay emits
/// its terminal error item instead of the crossing chunk, so no more than
/// the ceiling ever reaches the client and the read fails rather than
/// ending cleanly. Only the byte ceiling can produce that shape: the stream
/// is read eagerly (no idle, no blockage) and finishes far under the total
/// deadline. The lower bound is fuzzy by the chunks hyper had in flight
/// when the terminal item aborted the response. The permit goes back.
#[tokio::test]
async fn relay_fails_the_stream_one_byte_over_the_byte_ceiling() {
    let (backend, mut arrivals) = fixed_size_audio_backend(RELAY_BYTE_CEILING + 1).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(failed, "the read fails one byte over the ceiling");
    let received = u64::try_from(received.len()).unwrap();
    assert!(
        received <= RELAY_BYTE_CEILING,
        "the crossing chunk is never forwarded: {received} <= {RELAY_BYTE_CEILING}"
    );
    assert!(
        received + 1024 * 1024 >= RELAY_BYTE_CEILING,
        "the stream ran to the ceiling; only in-flight chunks were lost to the abort: {received}"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// The total-lifetime deadline ends a stream whose drip would otherwise run
/// forever. Only the deadline can fire here: the drip stays under the
/// per-read idle budget, the client reads eagerly, and the bytes are
/// nowhere near the ceiling. The permit goes back.
#[tokio::test]
async fn relay_ends_a_drip_at_the_total_lifetime_deadline() {
    let (backend, mut arrivals) = dripping_audio_backend(DRIP_GAP, None).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(!received.is_empty(), "the drip flowed before the deadline");
    assert!(
        failed,
        "the total deadline fails the read, never a clean EOF"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// A drip whose chunks arrive under the upstream-idle budget is healthy:
/// the idle budget is per-read, never a cap on the stream's length.
#[tokio::test]
async fn relay_tolerates_a_drip_under_the_idle_budget() {
    let (backend, _arrivals) = dripping_audio_backend(DRIP_GAP, Some(5)).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(!failed, "a sub-idle drip ends cleanly");
    assert_eq!(received, b"audio-drip;".repeat(5), "every drip arrived");
    gateway.shutdown().await;
}

/// An upstream that goes silent after headers trips the per-read idle
/// budget: the read fails well before the total-lifetime deadline (the only
/// other bound that could end a silent stream), and the permit goes back.
#[tokio::test]
async fn relay_fails_an_upstream_that_goes_idle_after_headers() {
    let (backend, mut arrivals) = stalling_audio_backend().await;
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
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "the first chunk arrived before the stall");
    let stalled = tokio::time::Instant::now();
    let (_rest, failed) = read_to_end_or_error(response).await;
    assert!(failed, "the idle budget fails the read, never a clean EOF");
    assert!(
        stalled.elapsed() < Duration::from_secs(1),
        "the idle budget fires well ahead of the 2 s total deadline"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// A client that stops reading backpressures the relay's bounded channel;
/// the blocked-delivery budget ends the stream and frees the permit while
/// the client still holds the unread response, and the client's eventual
/// read fails instead of seeing a clean EOF.
#[tokio::test]
async fn relay_fails_a_client_that_stops_reading() {
    let (backend, mut arrivals) = saturating_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let mut first = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(first.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, first.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(chunk.is_some(), "the stream started");

    // Stop reading. The relay fills its channel and the blocked-delivery
    // budget ends the stream; the permit release is observable as the second
    // request's admission under the one-slot dominion, well ahead of the 2 s
    // total deadline (the only other bound that could end this stream).
    let stalled = tokio::time::Instant::now();
    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    assert!(
        stalled.elapsed() < Duration::from_secs(1),
        "the blocked-delivery budget fires well ahead of the 2 s total deadline"
    );

    // The stalled client's resumed read drains the buffered chunks and then
    // fails on the terminal error item - never a clean EOF.
    let (_buffered, failed) = read_to_end_or_error(first).await;
    assert!(failed, "the stalled client's read fails, never a clean EOF");
    gateway.shutdown().await;
}

/// Headers arriving after the relay's (scaled) per-read idle budget but
/// within the first-response budget are accepted: the idle budget guards
/// only an opened body, never the wait for headers.
#[tokio::test]
async fn headers_delayed_past_the_idle_budget_are_accepted() {
    let backend = slow_headers_audio_backend(HEADER_DELAY).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(bytes_within(response).await, CANNED_AUDIO);
    gateway.shutdown().await;
}

/// An upstream body error after the permit is held propagates as exactly
/// one terminal error item, and the permit goes back: a later request is
/// admitted and answered under the one-slot dominion.
#[tokio::test]
async fn relay_releases_the_permit_after_an_upstream_body_error() {
    // The first request streams one chunk and then, once the test fires the
    // trigger (a rendezvous, so the failure lands after real bytes flowed),
    // fails; every later request streams the canned audio to a clean end.
    type FailWait = Arc<Mutex<Option<oneshot::Receiver<()>>>>;
    async fn speech(
        State((calls, wait_fail, arrivals)): State<(
            Arc<Mutex<usize>>,
            FailWait,
            UnboundedSender<()>,
        )>,
    ) -> Response {
        let _ = arrivals.send(());
        let call = {
            let mut calls = calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        let stream = if call == 1 {
            let wait = wait_fail
                .lock()
                .unwrap()
                .take()
                .expect("only the first request waits on the trigger");
            let first = futures_util::stream::once(async {
                Ok::<_, std::io::Error>(Bytes::from_static(b"audio-chunk-1;"))
            });
            let failure = futures_util::stream::once(async move {
                let _ = wait.await;
                Err(std::io::Error::other("upstream died mid-stream"))
            });
            Body::from_stream(first.chain(failure))
        } else {
            Body::from(CANNED_AUDIO)
        };
        let mut response = Response::new(stream);
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (fail, wait_fail) = oneshot::channel::<()>();
    let (arrivals, mut receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((
            Arc::new(Mutex::new(0_usize)),
            Arc::new(Mutex::new(Some(wait_fail))),
            arrivals,
        ));
    let backend = spawn_backend(router).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    next_ping(&mut receiver).await;
    let mut first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, first.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(
        chunk.is_some(),
        "real audio bytes flowed before the failure"
    );
    fail.send(()).expect("the backend is waiting on the signal");
    let (_rest, failed) = read_to_end_or_error(first).await;
    assert!(
        failed,
        "the upstream body error is the terminal item: the read fails, never a clean EOF"
    );

    assert_permit_released_by_admission(&client, &url, &mut receiver).await;
    gateway.shutdown().await;
}
