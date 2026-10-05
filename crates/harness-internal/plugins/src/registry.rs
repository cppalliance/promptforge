//! The explicit Host-built Plugin registry: [`PluginRegistry`].
//!
//! Linking a Plugin crate alone registers nothing: the Host builds one
//! registry, registers each installed Plugin by hand, and hands it to
//! the Harness, which passes it to [`activate`](crate::activate) for each
//! run. The registry holds one Plugin per id, so a duplicate registration
//! is rejected rather than shadowing the installed Plugin, and an id
//! differing from a registered id only by `-`/`_`/`.` punctuation is
//! rejected as a normalization collision: punctuation twins would be
//! indistinguishable to a model reading a catalog.
//!
//! Tools exist only after [`Plugin::create`], so tool
//! prefix-containment is checked when a run's catalog is assembled, not
//! at registration.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use promptforge::plugins::PluginId;

use crate::plugin::Plugin;

#[cfg(test)]
#[path = "registry-tests.rs"]
mod tests;

/// A registry of the Plugins a Host has installed.
///
/// The Host registers each installed Plugin by hand and hands the
/// registry to the Harness, which passes it to
/// [`activate`](crate::activate) for each run.
///
/// The registry holds one Plugin per id. Registering a second
/// Plugin under an id that is already registered fails, and the first
/// registration stays in place. Registering an id that differs from a
/// registered id only by `-`, `_`, or `.` punctuation also fails, as a
/// normalization collision.
///
/// A clone shares the Plugins registered so far. After that, each
/// copy takes its own registrations.
#[derive(Clone)]
pub struct PluginRegistry {
    /// The installed Plugins, keyed by their stable ids.
    plugins: BTreeMap<PluginId, Arc<dyn Plugin>>,
}

impl PluginRegistry {
    /// Builds an empty registry.
    #[must_use]
    pub fn new() -> PluginRegistry {
        PluginRegistry {
            plugins: BTreeMap::new(),
        }
    }

    /// Registers an installed Plugin.
    ///
    /// # Errors
    /// Returns [`RegistryError`] with [`RegistryErrorKind::DuplicateId`]
    /// when a Plugin with the same id is already registered. The
    /// registry keeps the first registration. Returns [`RegistryError`]
    /// with [`RegistryErrorKind::NormalizationCollision`] when the id
    /// differs from a registered id only by `-`, `_`, or `.`
    /// punctuation.
    pub fn register(&mut self, plugin: Arc<dyn Plugin>) -> Result<(), RegistryError> {
        let id = plugin.id().clone();
        if self.plugins.contains_key(&id) {
            return Err(RegistryError {
                kind: RegistryErrorKind::DuplicateId,
                id,
                collides_with: None,
            });
        }
        let normalized = normalize_id(&id);
        if let Some(existing) = self
            .plugins
            .keys()
            .find(|existing| normalize_id(existing) == normalized)
        {
            return Err(RegistryError {
                kind: RegistryErrorKind::NormalizationCollision,
                id,
                collides_with: Some(existing.clone()),
            });
        }
        self.plugins.insert(id, plugin);
        Ok(())
    }

    /// Returns the Plugin registered under `id`, when present.
    #[must_use]
    pub fn get(&self, id: &PluginId) -> Option<&Arc<dyn Plugin>> {
        self.plugins.get(id)
    }
}

impl Default for PluginRegistry {
    fn default() -> PluginRegistry {
        PluginRegistry::new()
    }
}

impl fmt::Debug for PluginRegistry {
    /// Reports only the registered ids.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginRegistry")
            .field("plugins", &self.plugins.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// The kind of a [`RegistryError`], as a stable value that code can match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegistryErrorKind {
    /// A Plugin with the same id was already registered.
    DuplicateId,
    /// The id differs from a registered id only by `-`, `_`, or `.`
    /// punctuation. Such ids are indistinguishable to a model reading a
    /// run's tool catalog, so the registry rejects the second one.
    NormalizationCollision,
}

/// The reason a Plugin registration was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegistryError {
    /// A stable classification of the rejection.
    kind: RegistryErrorKind,
    /// The id whose registration was rejected.
    id: PluginId,
    /// The registered id a punctuation twin collides with.
    collides_with: Option<PluginId>,
}

impl RegistryError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> RegistryErrorKind {
        self.kind
    }

    /// Returns the id whose registration was rejected.
    #[must_use]
    pub fn id(&self) -> &PluginId {
        &self.id
    }

    /// Returns the registered id the rejected id collides with, when the
    /// rejection is a [`RegistryErrorKind::NormalizationCollision`].
    #[must_use]
    pub fn collides_with(&self) -> Option<&PluginId> {
        self.collides_with.as_ref()
    }
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            RegistryErrorKind::DuplicateId => {
                write!(
                    formatter,
                    "a Plugin with id {} is already registered",
                    self.id
                )
            }
            RegistryErrorKind::NormalizationCollision => match &self.collides_with {
                Some(existing) => write!(
                    formatter,
                    "Plugin id {} was rejected: it differs from the registered id {existing} only by '-', '_' or '.' punctuation",
                    self.id
                ),
                None => write!(
                    formatter,
                    "Plugin id {} was rejected: it differs from a registered id only by '-', '_' or '.' punctuation",
                    self.id
                ),
            },
        }
    }
}

impl std::error::Error for RegistryError {}

/// Normalizes a Plugin id for the punctuation-twin check: each
/// separator byte (`-`, `_`, `.`) maps to one canonical byte, so two ids
/// differing only in separator choice compare equal. The global-name
/// charset is lowercase-only, so case needs no handling.
fn normalize_id(id: &PluginId) -> (String, String) {
    (
        normalize_segment(id.namespace()),
        normalize_segment(id.name()),
    )
}

/// Maps every separator byte in a segment to the canonical `-`.
fn normalize_segment(segment: &str) -> String {
    segment
        .chars()
        .map(|c| if matches!(c, '-' | '_' | '.') { '-' } else { c })
        .collect()
}
