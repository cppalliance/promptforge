//! The gateway subsystem's registration and any handle types. It
//! registers [`GatewayHandles`] - the replaceable endpoint binding and
//! the reachability flag - as its state handle set, and its background
//! tasks - the reachability heartbeat and the gateway progress
//! subscriber.

use std::sync::Arc;

use workshop_registry::{BackgroundTaskAdapter, Registration, Registry, ShutdownHandle};
use workshop_support::ReconnectBackoff;

use crate::binding::GatewayBinding;
use crate::heartbeat::{self, GatewayHealth};
use crate::progress;

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

/// The gateway subsystem's background-task registration guards: the
/// reachability heartbeat and the gateway progress subscriber. Dropping
/// them deregisters the tasks.
#[derive(Debug)]
#[must_use = "dropping the registrations deregisters the tasks"]
pub struct GatewayTaskRegistrations {
    /// The reachability heartbeat.
    pub heartbeat: Registration,
    /// The gateway progress subscriber.
    pub subscriber: Registration,
}

/// Registers the gateway subsystem's background tasks: the
/// reachability heartbeat and the gateway progress subscriber, both
/// reporting through the registry's push facade. The tasks spawn when
/// the server starts serving and stop inside the graceful-shutdown
/// signal. The returned guards keep the registrations alive; the
/// composition root holds them for the process lifetime.
pub fn register_tasks(
    registry: &Registry,
    handles: &GatewayHandles,
    backoff: ReconnectBackoff,
) -> GatewayTaskRegistrations {
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
        let registry = registry.clone();
        let binding = handles.binding().clone();
        let health = handles.health().clone();
        move || {
            let task = progress::spawn(binding.clone(), registry.push(), health.clone());
            ShutdownHandle::new(move || task.shutdown())
        }
    })));
    GatewayTaskRegistrations {
        heartbeat,
        subscriber,
    }
}
