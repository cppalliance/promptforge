//! Tests for the gateway client: the client basics live here beside the
//! shared mock-server helper; the progress, socket, switch, and timeout
//! areas each have their own submodule.

use super::*;

mod progress;
mod socket;
mod switch;
mod timeouts;

/// Binds `app` on a free loopback port and returns its base URL.
pub(super) async fn serve(app: axum::Router) -> String {
    let (addr, _handle) = workshop_support::fixtures::serve(app).await;
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

#[tokio::test]
async fn each_catalog_read_sends_the_bearer_to_its_own_route() {
    fn echo(route: &'static str) -> axum::routing::MethodRouter {
        axum::routing::get(move |headers: axum::http::HeaderMap| async move {
            let auth = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned();
            format!("{route} {auth}")
        })
    }
    let app = axum::Router::new()
        .route("/v1/models", echo("models"))
        .route("/admin/profiles", echo("profiles"))
        .route("/admin/status", echo("status"));
    let client = GatewayClient::new(&serve(app).await, "tok").expect("client builds in tests");

    for (read, expected) in [
        (client.list_models().await, "models Bearer tok"),
        (client.list_profiles().await, "profiles Bearer tok"),
        (client.profile_status().await, "status Bearer tok"),
    ] {
        let response = read.expect("the read completes");
        assert!(
            response.status.is_success(),
            "{expected}: {}",
            response.status
        );
        assert_eq!(String::from_utf8_lossy(&response.body), expected);
    }
}
