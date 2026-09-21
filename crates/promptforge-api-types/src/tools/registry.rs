//! The host-supplied [`ToolCatalog`] of tool descriptors and the catalog's
//! construction error.

use std::sync::Arc;

use super::descriptor::ToolDescriptor;
use super::ids::{ToolId, validate_identifier};

/// The host-supplied catalog of the tools a run may bind, as descriptors.
///
/// The host assembles the catalog from its activated capabilities and keeps
/// the implementations in a table of its own: the engine fills its tool
/// slots against the descriptors and never holds an implementation.
/// Construction rejects a repeated [`ToolId`] or a transport-illegal wire
/// name, so the bind-phase [`get`](Self::get) lookup trusts the invariant
/// without rescanning. Cloning is cheap: the descriptors live behind one
/// refcounted slice.
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
    /// Validates identity uniqueness and wire-name legality once, here, so
    /// [`Self::get`] can trust the invariant without rescanning.
    ///
    /// # Errors
    /// Returns [`ToolCatalogError::DuplicateId`] if two descriptors share a
    /// [`ToolId`], or [`ToolCatalogError::InvalidWireName`] if a
    /// descriptor's wire name is empty or contains a `/` separator or a
    /// control character.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::tools::ToolCatalog;
    ///
    /// let catalog = ToolCatalog::new(&[])?;
    /// assert!(catalog.tools().is_empty());
    /// # Ok::<(), promptforge_api_types::tools::ToolCatalogError>(())
    /// ```
    pub fn new(tools: &[ToolDescriptor]) -> Result<Self, ToolCatalogError> {
        let mut seen = std::collections::BTreeSet::new();
        for tool in tools {
            // The catalog is the transport boundary: reject a wire name that
            // is empty or carries a separator/control character (tools.rs F4).
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
    /// This is the bind-time lookup, a cold path run once per declared
    /// slot, so it scans linearly rather than keeping a cached-identity
    /// index.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::tools::{ToolCatalog, ToolId};
    ///
    /// let catalog = ToolCatalog::new(&[])?;
    /// let missing = ToolId::parse("promptforge/tools/missing")?;
    /// assert!(catalog.get(&missing).is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<&ToolDescriptor> {
        self.tools.iter().find(|tool| tool.id == *id)
    }

    /// Returns the catalog's descriptors in supplied order.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::tools::ToolCatalog;
    ///
    /// let catalog = ToolCatalog::new(&[])?;
    /// assert!(catalog.tools().is_empty());
    /// # Ok::<(), promptforge_api_types::tools::ToolCatalogError>(())
    /// ```
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
    /// not transport-legal.
    InvalidWireName,
}

/// A [`ToolCatalog`] could not be built from the supplied tools.
///
/// This classifying error supersedes the design's `DuplicateToolId` name
/// (DESIGN-2.4): the catalog is the schema/transport boundary, so besides
/// rejecting a repeated identity it also rejects a descriptor whose
/// [`wire_name`](ToolDescriptor::wire_name) is empty or contains a
/// separator or control character (tools.rs F4). It exposes a stable
/// [`kind`](Self::kind) classifier (DESIGN-5).
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
    /// A tool's transport wire name was not a legal identifier.
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
    /// Returns the stable classification of this error (DESIGN-5).
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
