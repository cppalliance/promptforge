//! Fake speech backends shaped for the relay's terminal paths: fixed-size,
//! dripping, stalling, saturating, and slow-header streams.

use std::net::SocketAddr;
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

use crate::speech::CANNED_AUDIO;
use crate::support::spawn_backend;

/// A fake speech backend streaming exactly `total` bytes in 64 KiB chunks,
/// pinging the arrivals channel per request so a test can prove a later
/// request was admitted after the first stream's relay ended.
pub(super) async fn fixed_size_audio_backend(total: u64) -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(State((total, arrivals)): State<(u64, UnboundedSender<()>)>) -> Response {
        let _ = arrivals.send(());
        let stream = futures_util::stream::unfold(0_u64, move |sent| async move {
            let remaining = total - sent;
            if remaining == 0 {
                return None;
            }
            let len = usize::try_from(remaining.min(64 * 1024)).unwrap();
            let chunk = Bytes::from(vec![0xAB; len]);
            Some((
                Ok::<_, std::convert::Infallible>(chunk),
                sent + u64::try_from(len).unwrap(),
            ))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((total, arrivals));
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend dripping one small chunk every `gap`: `Some(n)`
/// chunks then a clean end, or `None` chunks forever. Arrivals are
/// signalled per request.
pub(super) async fn dripping_audio_backend(
    gap: Duration,
    chunks: Option<usize>,
) -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(
        State((gap, chunks, arrivals)): State<(Duration, Option<usize>, UnboundedSender<()>)>,
    ) -> Response {
        let _ = arrivals.send(());
        let stream = futures_util::stream::unfold(chunks, move |remaining| async move {
            if remaining == Some(0) {
                return None;
            }
            tokio::time::sleep(gap).await;
            let remaining = remaining.map(|left| left - 1);
            Some((
                Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-drip;")),
                remaining,
            ))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((gap, chunks, arrivals));
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend that sends one audio chunk and then pends forever,
/// so the relay's upstream-idle budget is the only thing that can end the
/// stream. Arrivals are signalled per request.
pub(super) async fn stalling_audio_backend() -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(State(arrivals): State<UnboundedSender<()>>) -> Response {
        let _ = arrivals.send(());
        let first = futures_util::stream::once(async {
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-chunk-1;"))
        });
        let mut response = Response::new(Body::from_stream(
            first.chain(futures_util::stream::pending()),
        ));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend pouring unbounded 64 KiB chunks as fast as the
/// connection takes them, so a client that stops reading backpressures the
/// relay's bounded channel. Arrivals are signalled per request.
pub(super) async fn saturating_audio_backend() -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(State(arrivals): State<UnboundedSender<()>>) -> Response {
        let _ = arrivals.send(());
        let stream = futures_util::stream::unfold((), |()| async {
            Some((
                Ok::<_, std::convert::Infallible>(Bytes::from(vec![0xCD; 64 * 1024])),
                (),
            ))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend that waits `delay` before sending any headers, then
/// answers with the canned audio: time-to-headers is the first-response
/// budget's business, never the relay's per-read idle budget.
pub(super) async fn slow_headers_audio_backend(delay: Duration) -> SocketAddr {
    async fn speech(State(delay): State<Duration>) -> Response {
        tokio::time::sleep(delay).await;
        let mut response = Response::new(Body::from(CANNED_AUDIO));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }
    spawn_backend(
        Router::new()
            .route("/audio/speech", post(speech))
            .with_state(delay),
    )
    .await
}
