//! The operator HTTP surface: the bearer-authed `POST /shutdown` route,
//! the loopback host-authority wall on a real listener, and the `/auth`
//! browser handoff with its ambient cookie.

use crate::support::{fake_backend, gateway_for, send_within};

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
async fn post_shutdown_rejects_a_missing_or_wrong_key_and_keeps_serving() {
    let server = gateway_for(fake_backend().await).await;
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
