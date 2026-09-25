//! Integration tests for `workshop-status`: the registration contract -
//! the push channel, the producer sink, and the bus's state handle
//! served through the registry's contribution collections.

use workshop_protocol::{Activity, Severity, StatusBarUpdate};
use workshop_registry::{Registry, StatusChannel, StatusSink};
use workshop_status::{StatusBus, register};

/// A non-busy, info-severity update carrying only a label.
fn update(label: &str) -> StatusBarUpdate {
    StatusBarUpdate {
        label: label.to_owned(),
        description: String::new(),
        busy: false,
        severity: Severity::Info,
        activity: Activity::General,
    }
}

#[tokio::test]
async fn the_registered_sink_and_channel_serve_the_registered_bus() {
    let registry = Registry::new();
    let bus = StatusBus::new();
    let _guards = register(&registry, &bus);

    let channel = registry
        .state::<dyn StatusChannel>()
        .expect("the push channel is registered");
    let mut receiver = channel.subscribe();
    registry
        .sink::<dyn StatusSink>()
        .expect("the producer sink is registered")
        .emit(update("Loading"));

    let emitted = receiver.recv().await.expect("the sink emits onto the bus");
    assert_eq!(
        emitted.label, "Loading",
        "the channel sees what the sink emits"
    );
    assert_eq!(
        channel.latest().map(|latest| latest.label).as_deref(),
        Some("Loading"),
        "the channel's snapshot is the bus's retained update"
    );
    let state = registry
        .state::<StatusBus>()
        .expect("the bus is registered as the state handle");
    assert_eq!(
        state.latest().map(|latest| latest.label).as_deref(),
        Some("Loading"),
        "the state handle shares the registered bus"
    );
    assert_eq!(
        bus.latest().map(|latest| latest.label).as_deref(),
        Some("Loading"),
        "the caller's bus is the one the registrations drive"
    );
}

#[test]
fn dropping_the_guards_deregisters_every_contribution() {
    let registry = Registry::new();
    let guards = register(&registry, &StatusBus::new());
    assert!(registry.state::<dyn StatusChannel>().is_some());
    assert!(registry.sink::<dyn StatusSink>().is_some());
    assert!(registry.state::<StatusBus>().is_some());
    drop(guards);
    assert!(registry.state::<dyn StatusChannel>().is_none());
    assert!(registry.sink::<dyn StatusSink>().is_none());
    assert!(registry.state::<StatusBus>().is_none());
}
