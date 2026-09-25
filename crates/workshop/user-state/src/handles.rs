//! The user-state subsystem's registration: its `/user/state` routes,
//! merged into the server's API router, and the store as the
//! subsystem's state handle, so the composition root fetches it by slot
//! instead of holding it by name.

use std::sync::Arc;

use workshop_registry::{Registration, Registry, RouteRegistrarAdapter};

use crate::handlers;
use crate::store::UserStateStore;

/// The user-state subsystem's registration guards: its routes and its
/// state handle. Dropping them deregisters the subsystem.
#[derive(Debug)]
#[must_use = "dropping the registrations deregisters the subsystem"]
pub struct UserStateRegistrations {
    /// The `/user/state` route registrar.
    pub routes: Registration,
    /// The store as the subsystem's state handle.
    pub state: Registration,
}

/// Registers the user-state subsystem into the registry: its
/// `/user/state` routes, merged into the server's API router, and the
/// store as the subsystem's state handle, so the composition root
/// fetches it by slot instead of holding it by name. The returned guards
/// keep the registrations alive; the composition root holds them for the
/// process lifetime.
pub fn register(registry: &Registry, store: Arc<UserStateStore>) -> UserStateRegistrations {
    let routes = registry.register_routes(Arc::new(RouteRegistrarAdapter::new({
        let store = Arc::clone(&store);
        move || handlers::routes(Arc::clone(&store))
    })));
    let state = registry.register_state::<UserStateStore>(store);
    UserStateRegistrations { routes, state }
}
