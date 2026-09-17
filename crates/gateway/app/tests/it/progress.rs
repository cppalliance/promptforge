//! `GET /admin/progress`: bearer auth and the SSE content type. Event
//! ordering, snapshots, and detach silence are unit-tested beside the
//! response builder in the crate.

use reqwest::StatusCode;

use crate::support::{self, send_within};

#[tokio::test]
async fn progress_requires_the_bearer_token() {
    let backend = support::fake_backend().await;
    let server = support::gateway_for(backend).await;
    let http = reqwest::Client::new();

    // A wrong bearer is refused even from the loopback listener: presenting
    // a credential opts out of loopback trust.
    let refused = send_within(
        http.get(format!("http://{}/admin/progress", server.addr))
            .bearer_auth("wrong"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);

    let admitted = send_within(
        http.get(format!("http://{}/admin/progress", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(admitted.status(), StatusCode::OK);
    let content_type = admitted
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert_eq!(content_type, "text/event-stream");

    // The stream is open-ended; dropping the response disconnects.
    drop(admitted);
    server.shutdown().await;
}

/// A subscriber still attached to the open-ended progress stream - the
/// config SPA, the workshop - must not pin the graceful shutdown: the stream
/// ends when the shutdown signal fires, so the server exits within the phase
/// timeout while the client still holds the response.
#[tokio::test]
async fn shutdown_exits_while_a_progress_subscriber_is_attached() {
    let backend = support::fake_backend().await;
    let server = support::gateway_for(backend).await;
    let http = reqwest::Client::new();

    let mut subscriber = send_within(
        http.get(format!("http://{}/admin/progress", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(subscriber.status(), StatusCode::OK);

    // `shutdown` bounds the serve task by the phase timeout; an unended
    // stream fails it there.
    server.shutdown().await;

    // The subscriber sees a clean end of stream (the terminating chunk),
    // not a hung connection and not a reset.
    let end = tokio::time::timeout(support::PHASE_TIMEOUT, async {
        loop {
            match subscriber.chunk().await {
                Ok(Some(_frame)) => {}
                end => return end,
            }
        }
    })
    .await
    .expect("the progress stream ends with the server");
    assert!(
        matches!(end, Ok(None)),
        "the stream ends cleanly rather than by a reset: {end:?}"
    );
}
