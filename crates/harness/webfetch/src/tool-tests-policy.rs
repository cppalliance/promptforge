//! Policy tests for [`WebFetch`]: a total timeout is a soft return, no
//! cookie, credential, or `Referer` is sent on any hop, a redirect to a
//! non-global address is refused before the target is contacted (through
//! the system resolver and an injected lookup alike), a policy-rejected URL
//! never reaches the network, and an HTTP error status is a soft return.

use super::*;

use promptforge_api_types::tools::ToolErrorKind;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_server_past_total_timeout_yields_timeout() {
    let (port, _recorded) = spawn_recording_server().await;
    let config = loopback_builder(port)
        .timeout(std::time::Duration::from_millis(200))
        .build()
        .expect("valid config");
    let tool = WebFetch::try_with_config(config).expect("client builds");

    let url = format!("http://localhost:{port}/slow");
    let result = tool
        .call(serde_json::json!({ "url": url }))
        .await
        .expect("a timeout is a soft (recoverable) return")
        .text()
        .to_owned();

    assert!(
        result.contains("timed out"),
        "expected a timeout message, got: {result}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_omits_the_cookie_and_authorization_headers() {
    let (port, recorded) = spawn_recording_server().await;
    let tool = loopback_tool(port);

    let url = format!("http://localhost:{port}/record");
    tool.call(serde_json::json!({ "url": url }))
        .await
        .expect("a loopback fetch through allow_exact must succeed");

    let recorded = recorded
        .lock()
        .expect("the recorded-headers mutex must not be poisoned");
    assert_eq!(recorded.len(), 1);
    let headers = &recorded[0];
    assert!(!headers.contains_key(axum::http::header::COOKIE));
    assert!(!headers.contains_key(axum::http::header::AUTHORIZATION));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_credential_or_referer_survives_a_redirect() {
    let (port, recorded) = spawn_recording_server().await;
    let tool = loopback_tool(port);

    // A query-bearing source URL redirecting to a distinct allowed path: the
    // target must receive no Cookie, Authorization, or Referer.
    let url = format!("http://localhost:{port}/redir-record?secret=leak-me");
    tool.call(serde_json::json!({ "url": url }))
        .await
        .expect("a redirect between loopback paths must succeed");

    let recorded = recorded
        .lock()
        .expect("the recorded-headers mutex must not be poisoned");
    assert_eq!(recorded.len(), 1);
    let headers = &recorded[0];
    assert!(!headers.contains_key(axum::http::header::COOKIE));
    assert!(!headers.contains_key(axum::http::header::AUTHORIZATION));
    assert!(
        !headers.contains_key(axum::http::header::REFERER),
        "no Referer may survive a redirect, got: {headers:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_returns_provenance_line_then_content() {
    let (port, _hits) = spawn_server().await;
    let tool = loopback_tool(port);

    let url = format!("http://localhost:{port}/");
    let out = tool
        .call(serde_json::json!({ "url": url }))
        .await
        .expect("a loopback fetch through allow_exact must succeed")
        .text()
        .to_owned();

    let expected = format!("url: http://localhost:{port}/");
    assert!(out.starts_with(&expected), "got: {out}");
    assert!(out.contains("substantial paragraph"), "got: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_to_internal_is_refused_and_target_untouched() {
    let (port, hits) = spawn_server().await;
    let tool = loopback_tool(port);

    let url = format!("http://localhost:{port}/redir");
    let result = tool
        .call(serde_json::json!({ "url": url }))
        .await
        .expect("a redirect-target policy refusal is a soft (recoverable) return")
        .text()
        .to_owned();

    assert!(
        result.contains("refused") && result.contains("127.0.0.1"),
        "got: {result}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_to_internal_via_injected_lookup_never_contacts_target() {
    let (port, hits) = spawn_server().await;
    let loopback: IpAddr = "127.0.0.1".parse().expect("loopback parses");
    let blocked: IpAddr = "10.0.0.1".parse().expect("private parses");
    // allowed.test reaches the loopback server and holds the only exact
    // exception; internal.test resolves only to a blocked address.
    let lookup = MapLookup {
        entries: vec![
            ("allowed.test".to_string(), loopback),
            ("internal.test".to_string(), blocked),
        ],
    };
    // The server redirects to internal.test, which resolves only to a
    // blocked address, so the redirected target is never contacted.
    let redir_state = AppState {
        port,
        hits: Arc::clone(&hits),
    };
    let redir_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a second loopback listener must succeed");
    let redir_port = redir_listener.local_addr().expect("local addr").port();
    let app = Router::new()
        .route(
            "/go",
            get(move || async move {
                Redirect::temporary(&format!("http://internal.test:{port}/target"))
            }),
        )
        .with_state(redir_state);
    spawn_tagged(mock_tag(), async move {
        axum::serve(redir_listener, app)
            .await
            .expect("the redirect server must serve");
    });
    // Reach the redirect server via allowed.test on its own port.
    let config = FetchConfig::builder()
        .allow_http(true)
        .allow_ports([redir_port, port])
        .allow_host_address("allowed.test", loopback)
        .build()
        .expect("valid config");
    let tool = WebFetch::with_lookup(config, lookup);

    let url = format!("http://allowed.test:{redir_port}/go");
    // The outcome may be a hard error (the redirect address is blocked) or a
    // soft return, but it must never include the internal target's body.
    if let Ok(output) = tool.call(serde_json::json!({ "url": url })).await {
        assert!(
            !output.text().contains("reached the internal target"),
            "the internal target body must never be returned"
        );
    }

    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the internal redirect target must never be contacted"
    );
}

#[tokio::test]
async fn call_rejects_bad_urls_before_network() {
    let tool = WebFetch::new();

    let hard_cases = [
        (
            "https://user:pass@example.com/",
            "url must not contain userinfo",
        ),
        ("https://example.com:8080/", "port not allowed: 8080"),
        ("https://0177.0.0.1/", "ip literal host not allowed"),
        ("https://2130706433/", "ip literal host not allowed"),
        ("https://[::1]/", "ip literal host not allowed"),
        ("https://127.1/", "ip literal host not allowed"),
    ];

    for (raw, reason) in hard_cases {
        let err = tool
            .call(serde_json::json!({ "url": raw }))
            .await
            .expect_err(&format!("expected {raw} to be refused before any network"));
        assert!(
            err.kind() == ToolErrorKind::InvalidArguments,
            "expected a policy rejection for {raw}, got: {err:?}"
        );
        assert!(
            err.to_string().contains(reason),
            "expected policy reason {reason:?} for {raw}, got: {err}"
        );
    }

    let soft = tool
        .call(serde_json::json!({ "url": "http://example.com/" }))
        .await
        .expect("blocked http scheme must be soft tool text")
        .text()
        .to_owned();
    assert!(
        soft.contains("scheme not allowed: http"),
        "expected soft scheme refusal, got: {soft}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_return_on_404() {
    let (port, _hits) = spawn_server().await;
    let tool = loopback_tool(port);

    let url = format!("http://localhost:{port}/notfound");
    let result = tool
        .call(serde_json::json!({ "url": url }))
        .await
        .expect("a 404 must be a soft return")
        .text()
        .to_owned();

    assert!(result.contains("404"), "got: {result}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soft_return_on_500() {
    let (port, _hits) = spawn_server().await;
    let tool = loopback_tool(port);

    let url = format!("http://localhost:{port}/error500");
    let result = tool
        .call(serde_json::json!({ "url": url }))
        .await
        .expect("a 500 must be a soft return")
        .text()
        .to_owned();

    assert!(result.contains("500"), "got: {result}");
}

#[tokio::test]
async fn blocked_url_still_hard_fails() {
    let tool = WebFetch::new();

    let err = tool
        .call(serde_json::json!({ "url": "https://1.2.3.4/secret" }))
        .await
        .expect_err("a bare IP literal URL must still be a hard error");

    assert!(
        err.kind() == ToolErrorKind::InvalidArguments && err.to_string().contains("ip literal"),
        "got: {err:?}"
    );
}
