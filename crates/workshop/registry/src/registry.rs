//! The central registry: four contribution collections - routes and
//! background tasks as ordered vectors of trait objects, state handles
//! and push sinks as maps keyed by `TypeId`. The single downcast lives
//! here; callers see typed `Option`s.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::push::Push;
use crate::traits::{BackgroundTask, RouteRegistrar};

/// Reads the lock, tolerating a poisoned guard: a panicking registrant
/// must not take down every consumer.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Writes the lock, tolerating a poisoned guard.
fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// A type-keyed contribution cell: one entry per Rust type, the `Arc<T>`
/// boxed so trait-object handle sets (`Arc<dyn Trait>`) store beside
/// concrete ones.
#[derive(Default)]
struct TypeMap {
    entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl TypeMap {
    /// The contribution registered under `T`, downcast back to its
    /// `Arc<T>`.
    fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.entries
            .get(&TypeId::of::<T>())?
            .downcast_ref::<Arc<T>>()
            .cloned()
    }

    /// Registers `contribution` under `T`, replacing any previous
    /// occupant.
    fn insert<T: ?Sized + Send + Sync + 'static>(&mut self, contribution: &Arc<T>) {
        self.entries
            .insert(TypeId::of::<T>(), Box::new(Arc::clone(contribution)));
    }

    /// Removes `contribution` from `T`'s key when it is still the
    /// occupant, so a stale guard never evicts a newer registration.
    fn remove<T: ?Sized + Send + Sync + 'static>(&mut self, contribution: &Arc<T>) {
        let current = self
            .entries
            .get(&TypeId::of::<T>())
            .and_then(|entry| entry.downcast_ref::<Arc<T>>())
            .is_some_and(|current| Arc::ptr_eq(current, contribution));
        if current {
            self.entries.remove(&TypeId::of::<T>());
        }
    }
}

impl fmt::Debug for TypeMap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypeMap")
            .field("registered", &self.entries.len())
            .finish()
    }
}

/// The central registry subsystems self-register into.
///
/// Clones are cheap (every collection is an `Arc`) and share the same
/// collections, so every handle the composition root hands out sees the
/// same registrations.
#[derive(Clone, Default)]
pub struct Registry {
    routes: Arc<RwLock<Vec<Arc<dyn RouteRegistrar>>>>,
    tasks: Arc<RwLock<Vec<Arc<dyn BackgroundTask>>>>,
    state: Arc<RwLock<TypeMap>>,
    sinks: Arc<RwLock<TypeMap>>,
}

impl fmt::Debug for Registry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Registry")
            .field("routes", &read(&self.routes).len())
            .field("tasks", &read(&self.tasks).len())
            .field("state", &read(&self.state))
            .field("sinks", &read(&self.sinks))
            .finish()
    }
}

impl Registry {
    /// An empty registry: every collection empty until its subsystems
    /// register.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a route contribution; the shell merges every
    /// registrant into its API router in registration order. The
    /// returned guard keeps the contribution alive: dropping it removes
    /// the registrant.
    pub fn register_routes(&self, registrar: Arc<dyn RouteRegistrar>) -> Registration {
        write(&self.routes).push(Arc::clone(&registrar));
        let routes = Arc::downgrade(&self.routes);
        Registration::new(move || {
            let Some(routes) = routes.upgrade() else {
                return;
            };
            write(&routes).retain(|current| !Arc::ptr_eq(current, &registrar));
        })
    }

    /// Registers a background task; the shell spawns every registrant
    /// with serving and stops each through its [`ShutdownHandle`](crate::ShutdownHandle)
    /// in the graceful-shutdown closure.
    pub fn register_task(&self, task: Arc<dyn BackgroundTask>) -> Registration {
        write(&self.tasks).push(Arc::clone(&task));
        let tasks = Arc::downgrade(&self.tasks);
        Registration::new(move || {
            let Some(tasks) = tasks.upgrade() else {
                return;
            };
            write(&tasks).retain(|current| !Arc::ptr_eq(current, &task));
        })
    }

    /// Registers a state handle set under its type; consumers read it
    /// back with [`Registry::state`]. Re-registering `T` replaces the
    /// previous occupant, whose stale guard then deregisters nothing.
    pub fn register_state<T>(&self, handles: Arc<T>) -> Registration
    where
        T: ?Sized + Send + Sync + 'static,
    {
        write(&self.state).insert(&handles);
        let state = Arc::downgrade(&self.state);
        Registration::new(move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            write(&state).remove(&handles);
        })
    }

    /// Registers a producer sink under its trait type; the [`Push`]
    /// facade resolves it with [`Registry::sink`].
    pub fn register_sink<T>(&self, sink: Arc<T>) -> Registration
    where
        T: ?Sized + Send + Sync + 'static,
    {
        write(&self.sinks).insert(&sink);
        let sinks = Arc::downgrade(&self.sinks);
        Registration::new(move || {
            let Some(sinks) = sinks.upgrade() else {
                return;
            };
            write(&sinks).remove(&sink);
        })
    }

    /// Every registered route contribution, in registration order.
    #[must_use]
    pub fn routes(&self) -> Vec<Arc<dyn RouteRegistrar>> {
        read(&self.routes).clone()
    }

    /// Every registered background task, in registration order.
    #[must_use]
    pub fn tasks(&self) -> Vec<Arc<dyn BackgroundTask>> {
        read(&self.tasks).clone()
    }

    /// The state handle set registered under `T`, or `None` while its
    /// subsystem has not registered - a graceful no-op the consumer
    /// branches on.
    #[must_use]
    pub fn state<T>(&self) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        read(&self.state).get::<T>()
    }

    /// The producer sink registered under `T`, or `None` while its
    /// subsystem has not registered.
    #[must_use]
    pub fn sink<T>(&self) -> Option<Arc<T>>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        read(&self.sinks).get::<T>()
    }

    /// [`Registry::state`] made total at the composition root: a missing
    /// contribution is a boot failure naming `T`, not a later panic.
    ///
    /// # Errors
    /// Returns [`MissingContribution`] naming `T` when no subsystem has
    /// registered it.
    pub fn require<T>(&self) -> Result<Arc<T>, MissingContribution>
    where
        T: ?Sized + Send + Sync + 'static,
    {
        self.state::<T>().ok_or(MissingContribution {
            contribution: std::any::type_name::<T>(),
        })
    }

    /// The intent-named push facade over the producer sink collection,
    /// for subsystems that report what happened without naming another
    /// subsystem's bus.
    #[must_use]
    pub fn push(&self) -> Push {
        Push::new(self.clone())
    }
}

/// A required contribution is absent from the registry: the composition
/// root failed to register a subsystem before sharing state.
#[derive(Debug)]
pub struct MissingContribution {
    contribution: &'static str,
}

impl MissingContribution {
    /// The fully qualified type name of the missing contribution.
    #[must_use]
    pub fn contribution(&self) -> &'static str {
        self.contribution
    }
}

impl fmt::Display for MissingContribution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "no subsystem registered `{}`", self.contribution)
    }
}

impl std::error::Error for MissingContribution {}

/// The registration guard: keeps the contribution registered while
/// held.
///
/// Dropping the guard removes the contribution from its collection,
/// unless the key has since been re-registered - a stale guard never
/// evicts a newer occupant.
#[must_use = "dropping the guard deregisters the contribution"]
pub struct Registration {
    remove: Option<Box<dyn FnOnce() + Send + Sync>>,
}

impl Registration {
    /// Builds the guard from its deregistration action.
    fn new(remove: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            remove: Some(Box::new(remove)),
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(remove) = self.remove.take() {
            remove();
        }
    }
}

impl fmt::Debug for Registration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Registration").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::{
        BackgroundTaskAdapter, RouteRegistrarAdapter, ShutdownHandle, StatusSink, StatusSinkAdapter,
    };

    #[test]
    fn registered_state_is_served_by_type() {
        let registry = Registry::new();
        let _registration = registry.register_state(Arc::new("handles".to_string()));
        let handles = registry
            .state::<String>()
            .expect("the registered handle set is served");
        assert_eq!(handles.as_str(), "handles");
    }

    #[test]
    fn state_of_an_unregistered_type_is_none() {
        let registry = Registry::new();
        let _registration = registry.register_state(Arc::new("handles".to_string()));
        assert!(
            registry.state::<u64>().is_none(),
            "a type no subsystem registered serves nothing"
        );
    }

    #[test]
    fn dropping_the_registration_empties_the_key() {
        let registry = Registry::new();
        let registration = registry.register_state(Arc::new("handles".to_string()));
        assert!(registry.state::<String>().is_some());
        drop(registration);
        assert!(
            registry.state::<String>().is_none(),
            "the key empties when the guard drops"
        );
    }

    #[test]
    fn route_registrants_are_served_in_registration_order() {
        let registry = Registry::new();
        let first: Arc<dyn RouteRegistrar> =
            Arc::new(RouteRegistrarAdapter::new(axum::Router::new));
        let second: Arc<dyn RouteRegistrar> =
            Arc::new(RouteRegistrarAdapter::new(axum::Router::new));
        let _first_guard = registry.register_routes(Arc::clone(&first));
        let _second_guard = registry.register_routes(Arc::clone(&second));
        let routes = registry.routes();
        assert_eq!(routes.len(), 2, "both registrants are served");
        assert!(
            Arc::ptr_eq(&routes[0], &first) && Arc::ptr_eq(&routes[1], &second),
            "registrants are served in registration order"
        );
    }

    #[test]
    fn a_registered_task_is_served_until_its_guard_drops() {
        let registry = Registry::new();
        let task: Arc<dyn BackgroundTask> = Arc::new(BackgroundTaskAdapter::new(|| {
            ShutdownHandle::new(|| async {})
        }));
        let registration = registry.register_task(Arc::clone(&task));
        let tasks = registry.tasks();
        assert_eq!(tasks.len(), 1, "the registered task is served");
        assert!(
            Arc::ptr_eq(&tasks[0], &task),
            "the served task is the registered one"
        );
        drop(registration);
        assert!(
            registry.tasks().is_empty(),
            "dropping the guard removes the task"
        );
    }

    #[test]
    fn require_on_a_missing_key_names_the_missing_type() {
        let registry = Registry::new();
        let error = registry
            .require::<String>()
            .expect_err("an unregistered type fails require");
        assert_eq!(error.contribution(), std::any::type_name::<String>());
        assert!(
            error.to_string().contains("String"),
            "the error message names the missing type: {error}"
        );
    }

    #[test]
    fn a_sink_round_trips_through_its_trait_type() {
        let registry = Registry::new();
        let _registration =
            registry.register_sink::<dyn StatusSink>(Arc::new(StatusSinkAdapter::new(|_| {})));
        assert!(
            registry.sink::<dyn StatusSink>().is_some(),
            "the sink collection serves the registered trait object"
        );
    }
}
