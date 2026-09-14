//! The explicit host-built capability registry: [`CapabilityRegistry`].
//!
//! Linking a capability crate alone registers nothing: a host constructs one
//! registry, registers each installed capability by hand, and hands the
//! registry to the [`Environment`](crate::execute::Environment). v1 is
//! unversioned - one capability per id - so a duplicate registration is
//! rejected rather than shadowing the installed capability.
//!
//! Registration runs an advisory near-duplicate lint over capability
//! descriptions through the picker (the engine behind fuzzy tool slots):
//! two installed capabilities whose descriptions are near-verbatim copies
//! almost certainly overlap in what they offer, so the lint logs a warning
//! naming both. The lint never fails a registration. The tool
//! prefix-containment check is not here: tools exist only after
//! [`Capability::create`], so containment is checked when a run's catalog
//! is assembled, not at registration.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//!
//! use promptforge_api::capabilities::{CapabilityRegistry, RegistryErrorKind};
//! use shared_promptforge_api::capabilities::{
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

use promptforge_tool_picker::{Catalog, Config, ToolDescriptor, ToolId, ToolPicker};
use shared_promptforge_api::capabilities::{Capability, CapabilityId};

#[cfg(test)]
mod tests;

// The first-party capability rides the facade so hosts never name the
// internal pack crate (the one-door rule).
pub use promptforge_web::Web;

/// The synthetic third segment keying a capability in the lint catalog.
///
/// The picker's catalog speaks three-segment tool ids, so each capability
/// is keyed `<namespace>/<pack>/capability`; stripping the segment maps a
/// lint pair back to the capability it names.
const LINT_KEY_SEGMENT: &str = "capability";

/// An explicit host-built registry of installed capabilities.
///
/// See the [module documentation](self) for the registration rules and the
/// near-duplicate description lint.
pub struct CapabilityRegistry {
    /// The installed capabilities, keyed by their stable ids.
    capabilities: BTreeMap<CapabilityId, Arc<dyn Capability>>,
    /// The lint index over the registered descriptions, rebuilt at each
    /// registration. `ToolPicker::empty` skips the embedding-model load, so
    /// the lint runs on the picker's deterministic fallback embeddings.
    lint: ToolPicker,
}

impl CapabilityRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> CapabilityRegistry {
        CapabilityRegistry {
            capabilities: BTreeMap::new(),
            lint: ToolPicker::empty(Config::default()),
        }
    }

    /// Registers an installed capability.
    ///
    /// The near-duplicate description lint runs after the insert; a lint
    /// hit logs a warning and does not fail the registration.
    ///
    /// # Errors
    /// Returns [`RegistryError`] with [`RegistryErrorKind::DuplicateId`]
    /// when a capability with the same id is already registered; the
    /// registry keeps the first registration.
    pub fn register(&mut self, capability: Arc<dyn Capability>) -> Result<(), RegistryError> {
        let id = capability.id().clone();
        if self.capabilities.contains_key(&id) {
            return Err(RegistryError {
                kind: RegistryErrorKind::DuplicateId,
                id,
            });
        }
        self.capabilities.insert(id.clone(), capability);
        self.lint_new_registration(&id);
        Ok(())
    }

    /// Returns the capability registered under `id`, when present.
    #[must_use]
    pub fn get(&self, id: &CapabilityId) -> Option<&Arc<dyn Capability>> {
        self.capabilities.get(id)
    }

    /// Rebuilds the lint index over every registered description and warns
    /// on each near-duplicate pair involving the capability just
    /// registered, so a pair is reported exactly once, at the registration
    /// that created it. Lint machinery failures degrade to a warning:
    /// registration itself never fails on the lint.
    fn lint_new_registration(&mut self, new_id: &CapabilityId) {
        let descriptors = self
            .capabilities
            .values()
            .map(|capability| {
                ToolDescriptor::new(
                    lint_key(capability.id()),
                    capability.description(),
                    serde_json::json!({}),
                )
            })
            .collect::<Vec<_>>();
        let picker = match self.lint.rebuild(Catalog::new(descriptors)) {
            Ok(picker) => picker,
            Err(error) => {
                tracing::warn!(%error, "capability near-duplicate lint skipped: index rebuild failed");
                return;
            }
        };
        let keys = picker
            .iter()
            .map(|descriptor| descriptor.id().clone())
            .collect::<Vec<_>>();
        let new_key = lint_key(new_id);
        match picker.near_duplicates(&keys) {
            Ok(pairs) => {
                for pair in &pairs {
                    if pair.first().id() == &new_key || pair.second().id() == &new_key {
                        tracing::warn!(
                            first = %pair.first().id().capability(),
                            second = %pair.second().id().capability(),
                            similarity = pair.similarity(),
                            "registered capability descriptions are near-duplicates"
                        );
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "capability near-duplicate lint skipped: analysis failed");
            }
        }
        self.lint = picker;
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

/// Maps a capability id onto its lint-catalog key. Valid by construction:
/// a capability id's two segments and the key segment are all valid
/// global-name segments.
fn lint_key(id: &CapabilityId) -> ToolId {
    ToolId::from_validated(&format!("{id}/{LINT_KEY_SEGMENT}"))
}

/// A stable, matchable classification of a [`RegistryError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryErrorKind {
    /// A capability with the same id was already registered.
    DuplicateId,
}

/// The reason a capability registration was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a capability with id {id} is already registered")]
#[non_exhaustive]
pub struct RegistryError {
    /// A stable classification of the rejection.
    kind: RegistryErrorKind,
    /// The id whose registration was rejected.
    id: CapabilityId,
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
}
