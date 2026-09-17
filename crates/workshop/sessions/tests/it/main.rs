//! Integration tests for `workshop-sessions`: the registration
//! contract - routes and the agent-session state handle served through
//! the registry's contribution collections.

// clippy.toml's allow-expect-in-tests covers #[test] functions and
// #[cfg(test)] modules only, not integration-test helpers; failing a test
// by panicking with the invariant named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tower::ServiceExt as _;
use workshop_gateway::{GatewayBinding, GatewayHandles, GatewayHealth};
use workshop_menu::{CatalogBus, MenuBus};
use workshop_registry::{Registration, Registry};
use workshop_sessions::{AgentSessions, SessionHost, SessionsState, register, routes};
use workshop_support::ReconnectBackoff;

/// A wired composition: the menu and gateway handle sets registered
/// into the registry the sessions state reads them through, with the
/// guards and the state directory held for the test's duration.
struct Wired {
    registry: Registry,
    state: SessionsState,
    agents: AgentSessions,
    _guards: Vec<Registration>,
    _dir: tempfile::TempDir,
}

/// Builds the sessions route state against a stub gateway address, the
/// state directory a fresh tempdir returned alongside so it outlives the
/// test. The origin policy is injected exactly as the shell injects its
/// own.
fn wired_for(base_url: &str, origin_allowed: fn(&axum::http::HeaderMap) -> bool) -> Wired {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let registry = Registry::new();
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let gateway = GatewayBinding::new(base_url, "test-key").expect("the binding builds");
    let mut guards = Vec::new();
    let (catalog_sink, menu_sink, menu_state) = workshop_menu::register(&registry, &catalog, &menu);
    guards.extend([catalog_sink, menu_sink, menu_state]);
    guards.push(workshop_gateway::register(
        &registry,
        GatewayHandles::new(gateway.clone(), GatewayHealth::new()),
    ));
    let host = SessionHost::new(registry.clone(), ReconnectBackoff::new(), menu, catalog);
    let agents = AgentSessions::new(
        dir.path().join("agents"),
        dir.path().join("sessions"),
        gateway,
        host,
    );
    let state = SessionsState::new(registry.clone(), origin_allowed);
    Wired {
        registry,
        state,
        agents,
        _guards: guards,
        _dir: dir,
    }
}

#[tokio::test]
async fn the_registered_routes_serve_the_sessions_api() {
    let wired = wired_for("http://127.0.0.1:1", |_| true);
    let _guards = register(&wired.registry, &wired.state, &wired.agents);

    let registrars = wired.registry.routes();
    assert_eq!(registrars.len(), 1, "the routes collection is registered");
    let router = registrars[0].routes();

    // A plain GET to `/ws` without upgrade headers is rejected with 400,
    // which proves the route is mounted; the socket flows are pinned end
    // to end by the shell's integration binary over live sockets.
    let request = Request::builder()
        .uri("/ws")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // The excised buffered chat endpoint is gone: a `POST /chat` answers
    // 404, not a relay response.
    let request = Request::builder()
        .method("POST")
        .uri("/chat")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#,
        ))
        .expect("static request parts are valid");
    let response = router
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // The agent-session registry is served as the state handle.
    let agents = wired
        .registry
        .state::<AgentSessions>()
        .expect("the sessions handle is registered");
    assert_eq!(agents.discover(), vec!["chat".to_string()]);
}

#[tokio::test]
async fn a_foreign_origin_is_refused_on_both_upgrades() {
    // The shell's loopback policy, in miniature: an `Origin` header, when
    // present, must be a loopback origin.
    fn loopback_only(headers: &axum::http::HeaderMap) -> bool {
        headers
            .get(axum::http::header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|origin| origin.starts_with("http://127.0.0.1"))
    }
    let wired = wired_for("http://127.0.0.1:1", loopback_only);
    let router = routes(wired.state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the test server");
    let addr = listener.local_addr().expect("the test server address");
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("the test server serves");
    });
    for path in ["/ws", "/agents/ws"] {
        let mut request = format!("ws://{addr}{path}")
            .into_client_request()
            .expect("the handshake request builds");
        request.headers_mut().insert(
            axum::http::header::ORIGIN,
            "https://evil.example"
                .parse()
                .expect("a valid header value"),
        );
        let outcome = tokio_tungstenite::connect_async(request).await;
        let Err(tokio_tungstenite::tungstenite::Error::Http(response)) = outcome else {
            panic!("a foreign origin must fail the handshake: {path}");
        };
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "for {path}");
        let json: serde_json::Value =
            serde_json::from_slice(response.body().as_deref().expect("the refusal has a body"))
                .expect("the refusal is the envelope");
        assert_eq!(json["error"]["code"], "cross_site", "for {path}");
    }
}
