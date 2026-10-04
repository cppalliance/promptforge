//! The Harness-supplied [`ToolCatalog`] of tool descriptors and the catalog's
//! construction error.

use std::sync::Arc;

use super::descriptor::ToolDescriptor;
use super::ids::{ToolId, validate_identifier};

/// A catalog of the tools a run may bind, given as tool descriptors.
///
/// The caller builds the catalog and keeps the tool implementations itself.
/// The Engine fills a prompt's tool slots from these descriptors and never
/// holds an implementation.
///
/// Every tool in a catalog has a unique [`ToolId`] and a wire name that is
/// not empty and contains no `/` and no control character. Construction
/// checks both, so the [`get`](Self::get) lookup does not check them again.
///
/// Cloning is cheap, because all clones share one reference-counted list of
/// descriptors.
#[derive(Clone, Default)]
#[non_exhaustive]
pub struct ToolCatalog {
    tools: Arc<[ToolDescriptor]>,
}

impl std::fmt::Debug for ToolCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolCatalog")
            .field(
                "ids",
                &self.tools.iter().map(|tool| &tool.id).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ToolCatalog {
    /// Builds a catalog from tool descriptors.
    ///
    /// This is the only place the catalog checks identities and wire names.
    ///
    /// # Errors
    /// Returns [`ToolCatalogError::DuplicateId`] if two descriptors share a
    /// [`ToolId`], or [`ToolCatalogError::InvalidWireName`] if a
    /// descriptor's wire name is empty or contains a `/` separator or a
    /// control character.
    pub fn new(tools: &[ToolDescriptor]) -> Result<Self, ToolCatalogError> {
        let mut seen = std::collections::BTreeSet::new();
        for tool in tools {
            // The catalog is the transport boundary: reject a wire name that
            // is empty or holds a separator/control character.
            if let Err(error) = validate_identifier("wire name", &tool.wire_name) {
                return Err(ToolCatalogError::InvalidWireName {
                    wire_name: tool.wire_name.clone(),
                    reason: error.reason(),
                });
            }
            if !seen.insert(tool.id.clone()) {
                return Err(ToolCatalogError::DuplicateId {
                    id: tool.id.clone(),
                });
            }
        }
        Ok(Self {
            // `Arc::<[T]>::from(&[T])` clones each element straight into the
            // ref-counted slice; no intermediate owned `Vec` is allocated first.
            tools: Arc::from(tools),
        })
    }

    /// Returns the descriptor for `id`, if one is in the catalog.
    ///
    /// The lookup scans the descriptors one by one and keeps no index. It
    /// runs only while a run binds its tools, once per declared tool slot.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|tool| tool.id == *id)
    }

    /// Returns the catalog's descriptors in supplied order.
    #[must_use]
    pub fn tools(&self) -> &[ToolDescriptor] {
        &self.tools
    }
}

/// A stable, matchable classification of a [`ToolCatalogError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolCatalogErrorKind {
    /// Two supplied tools shared a stable [`ToolId`].
    DuplicateId,
    /// A supplied descriptor's [`wire_name`](ToolDescriptor::wire_name) was
    /// empty or contained a `/` separator or a control character.
    InvalidWireName,
}

/// The error returned when a [`ToolCatalog`] cannot be built from the
/// supplied tools.
///
/// Building fails when two tools share an identity, or when a descriptor's
/// [`wire_name`](ToolDescriptor::wire_name) is empty or contains a `/`
/// separator or a control character. Call [`kind`](Self::kind) to get a
/// stable classification you can match on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ToolCatalogError {
    /// The same stable identity was supplied by more than one tool.
    #[error("duplicate tool identity {id:?} in the tool catalog")]
    #[non_exhaustive]
    DuplicateId {
        /// The stable identity supplied more than once.
        id: ToolId,
    },
    /// A tool's wire name was empty or contained a `/` separator or a
    /// control character.
    #[error("invalid tool wire name {wire_name:?}: {reason}")]
    #[non_exhaustive]
    InvalidWireName {
        /// The rejected wire name.
        wire_name: String,
        /// Why it was rejected.
        reason: &'static str,
    },
}

impl ToolCatalogError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> ToolCatalogErrorKind {
        match self {
            ToolCatalogError::DuplicateId { .. } => ToolCatalogErrorKind::DuplicateId,
            ToolCatalogError::InvalidWireName { .. } => ToolCatalogErrorKind::InvalidWireName,
        }
    }

    /// Returns the duplicated identity when this is a [`Self::DuplicateId`].
    #[must_use]
    pub fn duplicate_id(&self) -> Option<&ToolId> {
        match self {
            ToolCatalogError::DuplicateId { id } => Some(id),
            ToolCatalogError::InvalidWireName { .. } => None,
        }
    }
}
