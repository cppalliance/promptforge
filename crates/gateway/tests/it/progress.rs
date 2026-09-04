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
