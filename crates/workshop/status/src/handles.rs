//! The status subsystem's registration: the consumer-side push channel
//! every `/ws` session subscribes through, the producer-side sink
//! same-tier subsystems emit through, the bus itself as the subsystem's
//! state handle, and the progress renderer as its background task.

use std::sync::Arc;

use shared_progress::ProgressHub;

use workshop_registry::{
    BackgroundTaskAdapter, Registration, Registry, ShutdownHandle, StatusChannel,
    StatusChannelAdapter, StatusSink, StatusSinkAdapter,
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

/// Registers the subsystem's background task: the renderer that turns
/// the process progress hub's snapshots into the status bar's progress
/// indicator. The task spawns when the shell starts serving and stops
/// inside the graceful-shutdown signal. The returned guard keeps the
/// registration alive; the composition root holds it for the process
/// lifetime.
pub fn register_tasks(registry: &Registry, progress: Arc<ProgressHub>) -> Registration {
    registry.register_task(Arc::new(BackgroundTaskAdapter::new({
        let registry = registry.clone();
        move || {
            let renderer = crate::progress::spawn(Arc::clone(&progress), registry.push());
            ShutdownHandle::new(move || renderer.shutdown())
        }
    })))
}
