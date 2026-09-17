//! Integration tests for `workshop-workspace`: the registration
//! contract - routes and the granted-roots handle served through the
//! registry's contribution collections.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;
use workshop_registry::{Registry, WorkspaceRoots};
use workshop_workspace::{Workspace, register};

#[tokio::test]
async fn the_registered_routes_serve_the_workspace_api() {
    let registry = Registry::new();
    let workspace = Workspace::new();
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("notes.txt"), "hello").expect("seed a file");
    let _guards = register(&registry, &workspace);

    let registrars = registry.routes();
    assert_eq!(registrars.len(), 1, "the routes collection is registered");
    let router = registrars[0].routes();

    // The roots listing answers through the registered routes.
    let request = Request::builder()
        .uri("/workspace/tree")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);

    // A grant over HTTP becomes visible through the registered roots
    // handle immediately.
    let body = serde_json::json!({ "path": dir.path() }).to_string();
    let request = Request::builder()
        .method("POST")
        .uri("/workspace/grant")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("static request parts are valid");
    let response = router
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    let roots = registry
        .state::<dyn WorkspaceRoots>()
        .expect("the roots handle is registered")
        .granted_roots();
    assert_eq!(roots.len(), 1, "the grant reached the shared state");
    assert!(
        roots[0].ends_with(dir.path().file_name().expect("a named tempdir")),
        "the granted root is the dropped directory: {roots:?}"
    );
}

#[tokio::test]
async fn dropping_the_guards_deregisters_every_contribution() {
    let registry = Registry::new();
    let workspace = Workspace::new();
    let guards = register(&registry, &workspace);
    assert!(!registry.routes().is_empty());
    assert!(registry.state::<Workspace>().is_some());
    assert!(registry.state::<dyn WorkspaceRoots>().is_some());
    drop(guards);
    assert!(registry.routes().is_empty());
    assert!(registry.state::<Workspace>().is_none());
    assert!(registry.state::<dyn WorkspaceRoots>().is_none());
}
