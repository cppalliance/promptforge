//! The operator HTTP surface: the bearer-authed `POST /shutdown` route,
//! keyless loopback access and its `trust_loopback = false` opt-out on a
//! real listener, the loopback host-authority wall, and the `/auth`
//! browser handoff with its ambient cookie.

use std::net::SocketAddr;

use gateway::{Config, Gateway, ProfilesContext};

use crate::support::{TestServer, fake_backend, gateway_for, send_within};

/// A gateway like [`gateway_for`] with `[server] trust_loopback = false`,
/// so every caller must present the bearer key.
async fn strict_gateway_for(backend: SocketAddr) -> TestServer {
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
trust_loopback = false

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
thinking = "never"
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

#[tokio::test]
async fn post_shutdown_answers_202_and_stops_the_server() {
    let server = gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    let response = send_within(
        client
            .post(format!("http://{}/shutdown", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    // No manual signal: only the route's own signal can end the serve task.
    server.join().await;
}

#[tokio::test]
async fn post_shutdown_rejects_a_wrong_key_even_from_loopback_and_keeps_serving() {
    let server = gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    let response = send_within(
        client
            .post(format!("http://{}/shutdown", server.addr))
            .bearer_auth("wrong"),
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a presented-but-wrong key is refused even though the peer is loopback"
    );
    let health = send_within(client.get(format!("http://{}/health", server.addr))).await;
    assert_eq!(
        health.status(),
        reqwest::StatusCode::OK,
        "a refused shutdown leaves the server up"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn post_shutdown_rejects_a_missing_key_when_loopback_trust_is_off() {
    let server = strict_gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    for key in [None, Some("wrong")] {
        let builder = client.post(format!("http://{}/shutdown", server.addr));
        let builder = match key {
            Some(key) => builder.bearer_auth(key),
            None => builder,
        };
        let response = send_within(builder).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "key {key:?}"
        );
    }
    let health = send_within(client.get(format!("http://{}/health", server.addr))).await;
    assert_eq!(
        health.status(),
        reqwest::StatusCode::OK,
        "a refused shutdown leaves the server up"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn a_keyless_loopback_client_reaches_the_inference_and_admin_surfaces() {
    let server = gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    for path in ["/v1/models", "/admin/status"] {
        let response = send_within(client.get(format!("http://{}{path}", server.addr))).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::OK,
            "GET {path} with no Authorization header from the loopback listener"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn trust_loopback_false_refuses_the_keyless_loopback_client() {
    let server = strict_gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    for path in ["/v1/models", "/admin/status"] {
        let response = send_within(client.get(format!("http://{}{path}", server.addr))).await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "GET {path} with no Authorization header under the opt-out"
        );
        let keyed = send_within(
            client
                .get(format!("http://{}{path}", server.addr))
                .bearer_auth("test-token"),
        )
        .await;
        assert_eq!(
            keyed.status(),
            reqwest::StatusCode::OK,
            "GET {path} with the key under the opt-out"
        );
    }
    server.shutdown().await;
}

#[tokio::test]
async fn get_shutdown_is_rejected() {
    let server = gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    let response = send_within(
        client
            .get(format!("http://{}/shutdown", server.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::METHOD_NOT_ALLOWED);
    server.shutdown().await;
}

#[tokio::test]
async fn the_host_wall_refuses_a_foreign_host_and_admits_the_bound_one() {
    let server = gateway_for(fake_backend().await).await;
    let client = reqwest::Client::new();
    let health = format!("http://{}/health", server.addr);
    // reqwest addresses the server by its bound socket, so the default
    // Host header passes the wall.
    let response = send_within(client.get(&health)).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let response = send_within(client.get(&health).header("host", "attacker.com")).await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "a rebound hostname is refused before routing"
    );
    let response = send_within(
        client
            .get(&health)
            .header("host", format!("localhost:{}", server.addr.port())),
    )
    .await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "localhost on the bound port names the bound socket"
    );
    server.shutdown().await;
}

#[cfg(feature = "config-ui")]
#[tokio::test]
async fn the_auth_handoff_sets_a_cookie_and_redirects_key_free() {
    let server = gateway_for(fake_backend().await).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let wrong = send_within(client.get(format!("http://{}/auth?key=wrong", server.addr))).await;
    assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);

    let response =
        send_within(client.get(format!("http://{}/auth?key=test-token", server.addr))).await;
    assert_eq!(response.status(), reqwest::StatusCode::FOUND);
    let location = response
        .headers()
        .get("location")
        .expect("a Location header");
    assert_eq!(location, "/config/", "the redirect target carries no key");
    let cookie = response
        .headers()
        .get("set-cookie")
        .expect("a Set-Cookie header")
        .to_str()
        .unwrap();
    assert!(
        cookie.starts_with("promptforge-gateway-session="),
        "the handoff cookie: {cookie}"
    );
    assert!(
        cookie.contains("HttpOnly"),
        "the cookie is HttpOnly: {cookie}"
    );
    assert!(
        cookie.contains("SameSite=Lax"),
        "the cookie is SameSite=Lax: {cookie}"
    );
    assert!(
        !cookie.contains("test-token"),
        "the cookie never carries the raw key: {cookie}"
    );

    // The cookie alone authenticates a subsequent config request from a
    // same-origin page (the fetch metadata a cross-origin page cannot
    // strip).
    let pair = cookie.split(';').next().unwrap();
    let status = send_within(
        client
            .get(format!("http://{}/admin/status", server.addr))
            .header("cookie", pair)
            .header("sec-fetch-site", "same-origin"),
    )
    .await;
    assert_eq!(status.status(), reqwest::StatusCode::OK);
    // The same cookie presented by a cross-origin rider is refused:
    // same-site is not same-origin, since ports are not part of a site.
    let status = send_within(
        client
            .get(format!("http://{}/admin/status", server.addr))
            .header("cookie", pair)
            .header("sec-fetch-site", "same-site"),
    )
    .await;
    assert_eq!(status.status(), reqwest::StatusCode::UNAUTHORIZED);

    server.shutdown().await;
}
