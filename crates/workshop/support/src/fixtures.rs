//! A mock HTTP server for the dependent crates' tests: binds the caller's
//! router on a free loopback port and serves it in a task.

// An `allow` rather than an `expect`: whether the lint fires here depends
// on the build's cfg permutation (clippy's allow-expect-in-tests covers
// only `#[cfg(test)]` code, not this `test-fixtures`-gated module), so an
// expectation would be unfulfilled in some builds and fail the -D warnings
// gate.
#![allow(
    clippy::expect_used,
    reason = "test fixtures fail by panicking with the invariant named"
)]

use std::net::SocketAddr;

use axum::Router;
use tokio::task::JoinHandle;

/// Binds `router` on a free loopback port and serves it in a task,
/// returning the bound address and the task handle. Drop the handle to
/// leave the server running for the test's lifetime, or abort it to stop
/// the server early.
///
/// # Panics
/// Panics when the loopback bind fails or the bound address cannot be
/// read.
pub async fn serve(router: Router) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("mock server address");
    let handle = tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("mock server serves");
    });
    (addr, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::routing::get;

    /// Fetches the body of a GET to `url`, panicking on transport or
    /// status errors.
    async fn get_text(url: &str) -> String {
        let response = reqwest::get(url)
            .await
            .expect("the request reaches the mock server");
        response.text().await.expect("the response body reads")
    }

    #[tokio::test]
    async fn a_mock_server_serves_its_router_on_the_reported_address() {
        let (addr, _handle) = serve(Router::new().route("/probe", get(|| async { "pong" }))).await;
        assert!(addr.ip().is_loopback(), "the bound address is loopback");
        assert_ne!(addr.port(), 0, "the bound address has a real port");
        let body = get_text(&format!("http://{addr}/probe")).await;
        assert_eq!(body, "pong", "the served router answers the request");
    }

    #[tokio::test]
    async fn the_returned_handle_stops_the_server_when_aborted() {
        let (_addr, handle) = serve(Router::new().route("/", get(|| async { "up" }))).await;
        handle.abort();
        assert!(
            handle.await.unwrap_err().is_cancelled(),
            "the task handle stops the server when aborted"
        );
    }
}
