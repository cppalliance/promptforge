//! Integration tests for `workshop-registry`: the contribution
//! collection contract - routes and tasks as ordered vectors, state
//! handles and push sinks keyed by type.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::broadcast;

use workshop_protocol::{Activity, Severity, StatusBarUpdate};
use workshop_registry::{Registry, StatusChannel, StatusChannelAdapter};

/// The adapter's boxed subscribe closure type.
type Subscribe = Box<dyn Fn() -> broadcast::Receiver<StatusBarUpdate> + Send + Sync>;
/// The adapter's boxed latest-snapshot closure type.
type Latest = Box<dyn Fn() -> Option<StatusBarUpdate> + Send + Sync>;

/// A hand-rolled status bus plus its registration adapter: a broadcast
/// sender and a retained snapshot, mirroring the registrant's real bus.
struct TestBus {
    adapter: StatusChannelAdapter<Subscribe, Latest>,
    sender: broadcast::Sender<StatusBarUpdate>,
    latest: Arc<Mutex<Option<StatusBarUpdate>>>,
}

/// Builds a test bus with a fresh channel and no snapshot.
fn status_adapter() -> TestBus {
    let (sender, _) = broadcast::channel(4);
    let latest = Arc::new(Mutex::new(None));
    let subscribe: Subscribe = Box::new({
        let sender = sender.clone();
        move || sender.subscribe()
    });
    let latest_snapshot: Latest = Box::new({
        let latest = Arc::clone(&latest);
        move || {
            latest
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    });
    TestBus {
        adapter: StatusChannelAdapter::new(subscribe, latest_snapshot),
        sender,
        latest,
    }
}

/// A status update carrying only a label.
fn update(label: &str) -> StatusBarUpdate {
    StatusBarUpdate {
        label: label.to_string(),
        description: String::new(),
        progress: None,
        severity: Severity::Info,
        activity: Activity::General,
    }
}

#[test]
fn an_empty_registry_serves_no_contributions() {
    let registry = Registry::new();
    assert!(registry.routes().is_empty());
    assert!(registry.tasks().is_empty());
    assert!(registry.state::<String>().is_none());
    assert!(registry.state::<dyn StatusChannel>().is_none());
}

#[test]
fn a_registered_status_channel_serves_subscribe_and_latest() {
    let registry = Registry::new();
    let bus = status_adapter();
    let _registration = registry.register_state::<dyn StatusChannel>(Arc::new(bus.adapter));
    let channel = registry
        .state::<dyn StatusChannel>()
        .expect("the registered channel is served");
    *bus.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(update("Ready"));
    assert_eq!(
        channel.latest().map(|update| update.label),
        Some("Ready".to_string()),
        "the collection serves the registrant's retained snapshot"
    );
    let mut receiver = channel.subscribe();
    bus.sender
        .send(update("Working"))
        .expect("a receiver is subscribed");
    assert_eq!(
        receiver
            .try_recv()
            .expect("the send reaches the subscriber")
            .label,
        "Working"
    );
}

#[test]
fn dropping_the_guard_deregisters_the_contribution() {
    let registry = Registry::new();
    let bus = status_adapter();
    let registration = registry.register_state::<dyn StatusChannel>(Arc::new(bus.adapter));
    assert!(registry.state::<dyn StatusChannel>().is_some());
    drop(registration);
    assert!(
        registry.state::<dyn StatusChannel>().is_none(),
        "the key empties when the guard drops"
    );
}

#[test]
fn a_stale_guard_never_evicts_a_newer_occupant() {
    let registry = Registry::new();
    let stale = registry.register_state::<dyn StatusChannel>(Arc::new(status_adapter().adapter));
    let _current = registry.register_state::<dyn StatusChannel>(Arc::new(status_adapter().adapter));
    drop(stale);
    assert!(
        registry.state::<dyn StatusChannel>().is_some(),
        "the replacement survives the stale guard's drop"
    );
}

#[test]
fn registry_clones_share_the_same_collections() {
    let registry = Registry::new();
    let clone = registry.clone();
    let _registration =
        registry.register_state::<dyn StatusChannel>(Arc::new(status_adapter().adapter));
    assert!(
        clone.state::<dyn StatusChannel>().is_some(),
        "a registration through one handle is visible through every clone"
    );
}

#[test]
fn the_workspace_roots_handle_serves_the_registrants_grants() {
    use std::path::PathBuf;

    use workshop_registry::{WorkspaceRoots, WorkspaceRootsAdapter};

    let registry = Registry::new();
    assert!(
        registry.state::<dyn WorkspaceRoots>().is_none(),
        "an unregistered roots handle is a graceful no-op"
    );
    let registration =
        registry.register_state::<dyn WorkspaceRoots>(Arc::new(WorkspaceRootsAdapter::new(|| {
            vec![PathBuf::from("/granted")]
        })));
    let roots = registry
        .state::<dyn WorkspaceRoots>()
        .expect("the registered roots handle is served");
    assert_eq!(roots.granted_roots(), vec![PathBuf::from("/granted")]);
    drop(registration);
    assert!(
        registry.state::<dyn WorkspaceRoots>().is_none(),
        "the key empties when the guard drops"
    );
}

#[test]
fn route_registrants_build_their_routers_in_registration_order() {
    use workshop_registry::RouteRegistrarAdapter;

    let registry = Registry::new();
    assert!(registry.routes().is_empty());
    let first: Arc<dyn workshop_registry::RouteRegistrar> =
        Arc::new(RouteRegistrarAdapter::new(axum::Router::new));
    let second: Arc<dyn workshop_registry::RouteRegistrar> =
        Arc::new(RouteRegistrarAdapter::new(axum::Router::new));
    let first_guard = registry.register_routes(Arc::clone(&first));
    let _second_guard = registry.register_routes(Arc::clone(&second));
    let routes = registry.routes();
    assert_eq!(routes.len(), 2, "both registrants are served");
    assert!(
        Arc::ptr_eq(&routes[0], &first) && Arc::ptr_eq(&routes[1], &second),
        "registrants are served in registration order"
    );
    drop(first_guard);
    assert_eq!(
        registry.routes().len(),
        1,
        "a dropped guard removes only its own registrant"
    );
}

#[test]
fn a_registered_state_handle_downcasts_to_its_concrete_type() {
    let registry = Registry::new();
    assert!(registry.state::<String>().is_none());
    let _registration = registry.register_state(Arc::new("handles".to_string()));
    let handles = registry
        .state::<String>()
        .expect("the registered handle set is served");
    assert_eq!(handles.as_str(), "handles");
    assert!(
        registry.state::<u64>().is_none(),
        "state handles are keyed by type"
    );
}
