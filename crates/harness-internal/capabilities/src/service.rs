//! Host services: typed objects the Host hands to the capabilities it
//! installs, each under a named id.
//!
//! A capability names the services it needs as [`ServiceId`]s in
//! [`Capability::needs`](crate::Capability::needs), and reads them at
//! activation through a [`ServiceKey`], which binds an id literal to the
//! provider's Rust type. The Host fills a [`HostServices`] map, and the
//! Harness hands it to each run in [`RunServices`](crate::RunServices).
//!
//! An id literal is a two-segment `namespace/name` in the capability id
//! grammar. [`HostServices::provide`] refuses one that does not parse.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;

use promptforge::capabilities::{CapabilityId, CapabilityIdError};

#[cfg(test)]
#[path = "service-tests.rs"]
mod tests;

/// The name of a Host service and the type its provider supplies.
///
/// Ids compare, hash, and display by their `namespace/name` literal
/// alone. Two keys with the same literal therefore name the same service,
/// whatever their types. The type matters only for whether a provider
/// satisfies the id; see [`HostServices::provides`].
#[derive(Clone, Copy)]
pub struct ServiceId {
    /// The `namespace/name` literal.
    literal: &'static str,
    /// The provider's type, as a function so the id builds in a `const`.
    type_id: fn() -> TypeId,
}

impl ServiceId {
    /// The provider type this id names.
    fn provider_type(self) -> TypeId {
        (self.type_id)()
    }
}

impl PartialEq for ServiceId {
    fn eq(&self, other: &ServiceId) -> bool {
        self.literal == other.literal
    }
}

impl Eq for ServiceId {}

impl Hash for ServiceId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.literal.hash(state);
    }
}

impl fmt::Debug for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ServiceId").field(&self.literal).finish()
    }
}

impl fmt::Display for ServiceId {
    /// Writes the `namespace/name` literal.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.literal)
    }
}

/// Binds the `namespace/name` id of a Host service to the type `T` its
/// provider supplies.
///
/// The crate that defines a service declares its key once, as a `const`.
/// The Host provides the service under that key, and a capability reads
/// the service through the same key.
pub struct ServiceKey<T: ?Sized + Send + Sync + 'static> {
    id: ServiceId,
    provider: PhantomData<fn() -> Arc<T>>,
}

impl<T: ?Sized + Send + Sync + 'static> ServiceKey<T> {
    /// Builds the key for the service named `literal`.
    ///
    /// Any literal builds a key. `HostServices::provide` checks the literal
    /// when a provider is supplied under this key.
    #[must_use]
    pub const fn new(literal: &'static str) -> ServiceKey<T> {
        ServiceKey {
            id: ServiceId {
                literal,
                type_id: TypeId::of::<T>,
            },
            provider: PhantomData,
        }
    }

    /// Returns the id this key names.
    #[must_use]
    pub const fn id(&self) -> ServiceId {
        self.id
    }
}

impl<T: ?Sized + Send + Sync + 'static> Clone for ServiceKey<T> {
    fn clone(&self) -> ServiceKey<T> {
        *self
    }
}

impl<T: ?Sized + Send + Sync + 'static> Copy for ServiceKey<T> {}

impl<T: ?Sized + Send + Sync + 'static> fmt::Debug for ServiceKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ServiceKey").field(&self.id.literal).finish()
    }
}

/// One provider in a [`HostServices`] map, with the type it was supplied
/// as.
#[derive(Clone)]
struct Entry {
    provider_type: TypeId,
    /// An `Arc<T>` for the `T` in `provider_type`.
    provider: Arc<dyn Any + Send + Sync>,
}

/// The services a Host provides, as a map from service id to provider.
///
/// The map records the type each provider was supplied as. Cloning the
/// map shares the providers.
#[derive(Clone, Default)]
pub struct HostServices {
    entries: BTreeMap<&'static str, Entry>,
}

impl HostServices {
    /// Builds an empty map.
    #[must_use]
    pub fn new() -> HostServices {
        HostServices::default()
    }

    /// Adds `provider` to the map under the id of `key`.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidId`] when the id literal of `key`
    /// fails to parse as a two-segment `namespace/name` id. Returns
    /// [`ServiceError::DuplicateId`] when the map already holds a provider
    /// under that literal, whatever the provider's type. The map stays as
    /// it was on either error.
    pub fn provide<T: ?Sized + Send + Sync + 'static>(
        &mut self,
        key: &ServiceKey<T>,
        provider: Arc<T>,
    ) -> Result<(), ServiceError> {
        let literal = key.id.literal;
        if let Err(source) = CapabilityId::parse(literal) {
            return Err(ServiceError::InvalidId {
                id: literal,
                source,
            });
        }
        if self.entries.contains_key(literal) {
            return Err(ServiceError::DuplicateId { id: literal });
        }
        self.insert(key, provider);
        Ok(())
    }

    /// Puts `provider` under `key`'s id, replacing any provider already
    /// there. The caller owns the literal's validity.
    fn insert<T: ?Sized + Send + Sync + 'static>(&mut self, key: &ServiceKey<T>, provider: Arc<T>) {
        self.entries.insert(
            key.id.literal,
            Entry {
                provider_type: key.id.provider_type(),
                provider: Arc::new(provider),
            },
        );
    }

    /// Returns the provider supplied under the id of `key`.
    ///
    /// Returns `None` when no provider is under that id, or when the
    /// provider was supplied as a type other than `T`.
    #[must_use]
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self, key: &ServiceKey<T>) -> Option<Arc<T>> {
        self.entries
            .get(key.id.literal)?
            .provider
            .downcast_ref::<Arc<T>>()
            .cloned()
    }

    /// Returns whether the map holds a provider under `id` that was
    /// supplied as the type `id` names.
    #[must_use]
    pub fn provides(&self, id: &ServiceId) -> bool {
        self.entries
            .get(id.literal)
            .is_some_and(|entry| entry.provider_type == id.provider_type())
    }
}

impl fmt::Debug for HostServices {
    /// Lists the provided ids, never the providers.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostServices")
            .field("services", &self.entries.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// The error [`HostServices::provide`] returns when it refuses a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServiceError {
    /// The id literal fails to parse as a two-segment `namespace/name` id.
    InvalidId {
        /// The id literal that was refused.
        id: &'static str,
        /// Why the literal fails to parse as an id.
        source: CapabilityIdError,
    },
    /// The map already holds a provider under the id.
    DuplicateId {
        /// The id literal that already has a provider.
        id: &'static str,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::InvalidId { id, .. } => {
                write!(f, "service id {id} is not a namespace/name id")
            }
            ServiceError::DuplicateId { id } => {
                write!(f, "a service with id {id} is already provided")
            }
        }
    }
}

impl std::error::Error for ServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ServiceError::InvalidId { source, .. } => Some(source),
            ServiceError::DuplicateId { .. } => None,
        }
    }
}
