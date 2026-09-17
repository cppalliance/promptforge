//! Tests for the gateway client: the client basics live here beside the
//! shared mock-server helper; the decoder, timeout, cache, and switch
//! areas each have their own submodule.

use super::*;

mod cache;
mod decoder;
mod progress;
mod switch;
mod timeouts;

/// Binds `app` on a free loopback port and returns its base URL.
pub(super) async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock gateway");
    let addr = listener.local_addr().expect("mock gateway address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock gateway serves");
    });
    format!("http://{addr}")
}

#[test]
fn trailing_slash_is_trimmed_from_base_url() {
    let client = GatewayClient::new("http://127.0.0.1:8081/", "k").expect("client builds");
    assert_eq!(client.base_url, "http://127.0.0.1:8081");
}

#[test]
fn debug_redacts_the_api_key() {
    let client = GatewayClient::new("http://127.0.0.1:8081", "secret-key").expect("client");
    let rendered = format!("{client:?}");
    assert!(!rendered.contains("secret-key"), "key leaked: {rendered}");
}
