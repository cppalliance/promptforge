//! The explicit host-built capability registry: [`CapabilityRegistry`].
//!
//! Linking a capability crate alone registers nothing: a host constructs one
//! registry, registers each installed capability by hand, and hands the
//! registry to the [`Environment`](crate::execute::Environment). v1 is
//! unversioned - one capability per id - so a duplicate registration is
//! rejected rather than shadowing the installed capability, and an id
//! differing from a registered id only by `-`/`_`/`.` punctuation is
//! rejected as a normalization collision: punctuation twins would be
//! indistinguishable to a model reading a catalog.
//!
//! The tool prefix-containment check is not here: tools exist only after
//! [`Capability::create`], so containment is checked when a run's catalog
//! is assembled, not at registration.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//!
//! use promptforge_api_runtime::capabilities::{CapabilityRegistry, RegistryErrorKind};
//! use promptforge_api_types::capabilities::{
//!     Capability, CapabilityError, CapabilityId, Contribution, RunServices,
//! };
//!
//! struct Web {
//!     id: CapabilityId,
//! }
//!
//! impl Capability for Web {
//!     fn id(&self) -> &CapabilityId {
//!         &self.id
//!     }
//!     fn description(&self) -> &str {
//!         "Web fetch and search tools."
//!     }
//!     fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
//!         let _ = services;
//!         Ok(Contribution::default())
//!     }
//! }
//!
//! let mut registry = CapabilityRegistry::new();
//! let id = CapabilityId::parse("promptforge/web")?;
//! registry.register(Arc::new(Web { id: id.clone() }))?;
//! assert!(registry.get(&id).is_some());
//!
//! let duplicate = registry.register(Arc::new(Web { id: id.clone() }));
//! assert_eq!(
//!     duplicate.map(|_| ()).unwrap_err().kind(),
//!     RegistryErrorKind::DuplicateId
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use promptforge_api_types::capabilities::{Capability, CapabilityId};

#[cfg(test)]
#[path = "capabilities-tests.rs"]
mod tests;

// The first-party capability rides the facade so hosts never name the
// internal pack crate (the one-door rule).
pub use promptforge_web::Web;

/// An explicit host-built registry of installed capabilities.
///
/// See the [module documentation](self) for the registration rules.
pub struct CapabilityRegistry {
    /// The installed capabilities, keyed by their stable ids.
    capabilities: BTreeMap<CapabilityId, Arc<dyn Capability>>,
}

impl CapabilityRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> CapabilityRegistry {
        CapabilityRegistry {
            capabilities: BTreeMap::new(),
        }
    }

    /// Registers an installed capability.
    ///
    /// # Errors
    /// Returns [`RegistryError`] with [`RegistryErrorKind::DuplicateId`]
    /// when a capability with the same id is already registered; the
    /// registry keeps the first registration. Returns [`RegistryError`]
    /// with [`RegistryErrorKind::NormalizationCollision`] when the id
    /// differs from an existing registration only by `-`/`_`/`.`
    /// punctuation.
    pub fn register(&mut self, capability: Arc<dyn Capability>) -> Result<(), RegistryError> {
        let id = capability.id().clone();
        if self.capabilities.contains_key(&id) {
            return Err(RegistryError {
                kind: RegistryErrorKind::DuplicateId,
                id,
                collides_with: None,
            });
        }
        let normalized = normalize_id(&id);
        if let Some(existing) = self
            .capabilities
            .keys()
            .find(|existing| normalize_id(existing) == normalized)
        {
            return Err(RegistryError {
                kind: RegistryErrorKind::NormalizationCollision,
                id,
                collides_with: Some(existing.clone()),
            });
        }
        self.capabilities.insert(id, capability);
        Ok(())
    }

    /// Returns the capability registered under `id`, when present.
    #[must_use]
    pub fn get(&self, id: &CapabilityId) -> Option<&Arc<dyn Capability>> {
        self.capabilities.get(id)
    }
}

impl Default for CapabilityRegistry {
    fn default() -> CapabilityRegistry {
        CapabilityRegistry::new()
    }
}

impl fmt::Debug for CapabilityRegistry {
    /// Reports the registered ids, never the capabilities themselves.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityRegistry")
            .field(
                "capabilities",
                &self.capabilities.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

/// A stable, matchable classification of a [`RegistryError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryErrorKind {
    /// A capability with the same id was already registered.
    DuplicateId,
    /// The id differs from an existing registration only by `-`/`_`/`.`
    /// punctuation: punctuation twins would be indistinguishable to a
    /// model reading a catalog, so the second one is rejected.
    NormalizationCollision,
}

/// The reason a capability registration was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegistryError {
    /// A stable classification of the rejection.
    kind: RegistryErrorKind,
    /// The id whose registration was rejected.
    id: CapabilityId,
    /// The registered id a punctuation twin collides with.
    collides_with: Option<CapabilityId>,
}

impl RegistryError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> RegistryErrorKind {
        self.kind
    }

    /// Returns the id whose registration was rejected.
    #[must_use]
    pub fn id(&self) -> &CapabilityId {
        &self.id
    }

    /// Returns the registered id the rejected id collides with, when the
    /// rejection is a [`RegistryErrorKind::NormalizationCollision`].
    #[must_use]
    pub fn collides_with(&self) -> Option<&CapabilityId> {
        self.collides_with.as_ref()
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            RegistryErrorKind::DuplicateId => {
                write!(
                    formatter,
                    "a capability with id {} is already registered",
                    self.id
                )
            }
            RegistryErrorKind::NormalizationCollision => match &self.collides_with {
                Some(existing) => write!(
                    formatter,
                    "capability id {} was rejected: it differs from the registered id {existing} only by '-', '_' or '.' punctuation",
                    self.id
                ),
                None => write!(
                    formatter,
                    "capability id {} was rejected: it differs from a registered id only by '-', '_' or '.' punctuation",
                    self.id
                ),
            },
        }
    }
}

impl std::error::Error for RegistryError {}

/// Normalizes a capability id for the punctuation-twin check: each
/// separator byte (`-`, `_`, `.`) maps to one canonical byte, so two ids
/// differing only in separator choice compare equal. The global-name
/// charset is lowercase-only, so case needs no handling.
fn normalize_id(id: &CapabilityId) -> (String, String) {
    (
        normalize_segment(id.namespace()),
        normalize_segment(id.pack()),
    )
}

/// Maps every separator byte in a segment to the canonical `-`.
fn normalize_segment(segment: &str) -> String {
    segment
        .chars()
        .map(|c| if matches!(c, '-' | '_' | '.') { '-' } else { c })
        .collect()
}
