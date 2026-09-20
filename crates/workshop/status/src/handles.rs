//! The status subsystem's registration: the consumer-side push channel
//! every `/ws` session subscribes through, the producer-side sink
//! same-tier subsystems emit through, and the bus itself as the
//! subsystem's state handle.

use std::sync::Arc;

use workshop_registry::{
    Registration, Registry, StatusChannel, StatusChannelAdapter, StatusSink, StatusSinkAdapter,
};

use crate::StatusBus;

/// Registers the status subsystem into the registry: the consumer-side
/// push channel every `/ws` session subscribes through, the
/// producer-side sink same-tier subsystems emit through, and the bus
/// itself as the subsystem's state handle. The returned guards keep the
/// registrations alive; the composition root holds them for the process
/// lifetime.
pub fn register(
    registry: &Registry,
    bus: &StatusBus,
) -> (Registration, Registration, Registration) {
    let channel =
        registry.register_state::<dyn StatusChannel>(Arc::new(StatusChannelAdapter::new(
            {
                let bus = bus.clone();
                move || bus.subscribe()
            },
            {
                let bus = bus.clone();
                move || bus.latest()
            },
        )));
    let sink = registry.register_sink::<dyn StatusSink>(Arc::new(StatusSinkAdapter::new({
        let bus = bus.clone();
        move |update| bus.emit(update)
    })));
    let state = registry.register_state::<StatusBus>(Arc::new(bus.clone()));
    (channel, sink, state)
}
