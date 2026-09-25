//! Integration tests for `workshop-user-state`: the registration
//! contract - the `/user/state` routes and the store's state handle
//! served through the registry's contribution collections.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;
use workshop_registry::Registry;
use workshop_user_state::{UserStateStore, register};

#[tokio::test]
async fn the_registered_routes_write_through_to_the_registered_store() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let registry = Registry::new();
    let store = Arc::new(UserStateStore::new(dir.path()));
    let _guards = register(&registry, Arc::clone(&store));

    let registrars = registry.routes();
    assert_eq!(registrars.len(), 1, "the routes collection is registered");
    let request = Request::builder()
        .method("PUT")
        .uri("/user/state/zoom")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from("1.5"))
        .expect("static request parts are valid");
    let response = registrars[0]
        .routes()
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);

    let state = registry
        .state::<UserStateStore>()
        .expect("the store is registered as the state handle");
    assert!(
        Arc::ptr_eq(&state, &store),
        "the state handle is the caller's store"
    );
    assert_eq!(
        state.all().await.get("zoom").cloned().flatten(),
        Some(serde_json::json!(1.5)),
        "a put through the registered routes lands in the registered store"
    );
}

#[test]
fn dropping_the_guards_deregisters_every_contribution() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let registry = Registry::new();
    let guards = register(&registry, Arc::new(UserStateStore::new(dir.path())));
    assert!(!registry.routes().is_empty());
    assert!(registry.state::<UserStateStore>().is_some());
    drop(guards);
    assert!(registry.routes().is_empty());
    assert!(registry.state::<UserStateStore>().is_none());
}
