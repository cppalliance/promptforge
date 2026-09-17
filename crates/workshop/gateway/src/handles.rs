//! The gateway subsystem's registration: the replaceable endpoint
//! binding and the reachability flag as its state handle set, and its
//! background tasks - the reachability heartbeat and the gateway
//! progress subscriber.

use std::sync::Arc;

use shared_progress::ProgressHub;

use workshop_registry::{BackgroundTaskAdapter, Registration, Registry, ShutdownHandle};
use workshop_support::ReconnectBackoff;

use crate::gateway_binding::GatewayBinding;
use crate::gateway_progress;
use crate::heartbeat::{self, GatewayHealth};

/// The gateway subsystem's shared handles: the atomically replaceable
/// endpoint binding every gateway call snapshots, and the heartbeat's
/// reachability flag the gateway-dependent routes short-circuit on.
#[derive(Debug, Clone)]
pub struct GatewayHandles {
    binding: GatewayBinding,
    health: GatewayHealth,
}

impl GatewayHandles {
    /// Bundles the binding and the health flag for registration.
    #[must_use]
    pub fn new(binding: GatewayBinding, health: GatewayHealth) -> Self {
        Self { binding, health }
    }

    /// The replaceable endpoint binding.
    #[must_use]
    pub fn binding(&self) -> &GatewayBinding {
        &self.binding
    }

    /// The heartbeat's shared reachability flag.
    #[must_use]
    pub fn health(&self) -> &GatewayHealth {
        &self.health
    }
}

/// Registers the gateway subsystem's state handles into the registry.
/// The returned guard keeps the registration alive; the composition
/// root holds it for the process lifetime.
pub fn register(registry: &Registry, handles: GatewayHandles) -> Registration {
    registry.register_state::<GatewayHandles>(Arc::new(handles))
}

/// Registers the gateway subsystem's background tasks: the
/// reachability heartbeat and the gateway progress subscriber. The
/// tasks spawn when the shell starts serving and stop inside the
/// graceful-shutdown signal. The returned guards keep the registrations
/// alive; the composition root holds them for the process lifetime.
pub fn register_tasks(
    registry: &Registry,
    handles: &GatewayHandles,
    progress: Arc<ProgressHub>,
    backoff: ReconnectBackoff,
) -> (Registration, Registration) {
    let heartbeat = registry.register_task(Arc::new(BackgroundTaskAdapter::new({
        let registry = registry.clone();
        let binding = handles.binding().clone();
        let health = handles.health().clone();
        move || {
            let task = heartbeat::spawn(
                binding.clone(),
                registry.push(),
                health.clone(),
                heartbeat::HEARTBEAT_INTERVAL,
                backoff.clone(),
            );
            ShutdownHandle::new(move || task.shutdown())
        }
    })));
    let subscriber = registry.register_task(Arc::new(BackgroundTaskAdapter::new({
        let binding = handles.binding().clone();
        let health = handles.health().clone();
        move || {
            let task =
                gateway_progress::spawn(binding.clone(), Arc::clone(&progress), health.clone());
            ShutdownHandle::new(move || task.shutdown())
        }
    })));
    (heartbeat, subscriber)
}
