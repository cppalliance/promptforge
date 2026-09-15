//! Timeout tests: a gateway that accepts and never answers must trip the
//! client's bounds rather than hang its caller.

use super::*;

/// Binds a stub that completes TCP handshakes and then never answers,
/// modeling a gateway that is up but wedged.
async fn spawn_stalled_gateway() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stalled stub");
    let addr = listener.local_addr().expect("stalled stub address");
    tokio::spawn(async move {
        // Sockets are held open and never answered until the test's
        // runtime tears the task down.
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    format!("http://{addr}")
}

/// A client against `base_url` whose bounds are tight enough to trip
/// inside a test.
fn impatient_client(base_url: &str) -> GatewayClient {
    GatewayClient::new(base_url, "")
        .expect("client builds in tests")
        .with_timeouts_for_test(Duration::from_millis(100), Duration::from_millis(100))
}

#[tokio::test]
async fn a_stalled_gateway_trips_the_request_timeout() {
    let base_url = spawn_stalled_gateway().await;
    let error = impatient_client(&base_url)
        .list_models()
        .await
        .expect_err("a gateway that never answers must trip the request timeout");
    assert!(
        matches!(error, GatewayError::Transport(_)),
        "expected Transport, got {error:?}"
    );
}

#[tokio::test]
async fn a_stalled_gateway_trips_the_stream_header_bound() {
    let base_url = spawn_stalled_gateway().await;
    let error = impatient_client(&base_url)
        .cache_ensure("hf://example/blob")
        .await
        .expect_err("a gateway that never sends headers must trip the header bound");
    assert!(
        matches!(error, GatewayError::Transport(_)),
        "expected Transport, got {error:?}"
    );
}

#[tokio::test]
async fn a_stalled_gateway_probe_reads_unreachable() {
    let base_url = spawn_stalled_gateway().await;
    assert!(
        !impatient_client(&base_url).health().await,
        "a gateway that accepts but never answers must read unreachable"
    );
}
