//! The caller-supplied [`ToolCatalog`] of tool descriptors and the catalog's
//! construction error.

use std::sync::Arc;

use super::descriptor::ToolDescriptor;
use super::ids::ToolId;

/// A catalog of the tools a run may offer, given as tool descriptors.
///
/// The caller builds the catalog from every Plugin it can serve, declared
/// by the prompt or not, and keeps the tool implementations itself. The
/// catalog lists every tool the run can use: the prompt's Lua calls one by
/// its [`ToolId`], and a section offers one to its model with
/// `tools.offer`.
///
/// Every tool in a catalog has a unique [`ToolId`]. Construction checks it,
/// and the [`get`](Self::get) lookup relies on that check.
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
    /// Builds a catalog from tool descriptors, keeping the order they are
    /// given in.
    ///
    /// This is the only place the catalog checks identities.
    ///
    /// # Errors
    /// Returns [`ToolCatalogError::DuplicateId`] if two descriptors share a
    /// [`ToolId`].
    pub fn new(tools: impl IntoIterator<Item = ToolDescriptor>) -> Result<Self, ToolCatalogError> {
        let tools: Vec<ToolDescriptor> = tools.into_iter().collect();
        let mut seen = std::collections::BTreeSet::new();
        for tool in &tools {
            if !seen.insert(&tool.id) {
                return Err(ToolCatalogError::DuplicateId {
                    id: tool.id.clone(),
                });
            }
        }
        Ok(Self {
            tools: Arc::from(tools),
        })
    }

    /// Returns the descriptor for `id`, if one is in the catalog.
    ///
    /// The lookup scans the descriptors one by one.
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
}

/// Reports why building a [`ToolCatalog`] failed.
///
/// Building fails when two tools share an identity. Call
/// [`kind`](Self::kind) to get a stable classification you can match on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolCatalogError {
    /// The same stable identity was supplied by more than one tool.
    #[non_exhaustive]
    DuplicateId {
        /// The stable identity supplied more than once.
        id: ToolId,
    },
}

impl ToolCatalogError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> ToolCatalogErrorKind {
        match self {
            ToolCatalogError::DuplicateId { .. } => ToolCatalogErrorKind::DuplicateId,
        }
    }

    /// Returns the duplicated identity when this is a [`Self::DuplicateId`].
    #[must_use]
    pub fn duplicate_id(&self) -> Option<&ToolId> {
        match self {
            ToolCatalogError::DuplicateId { id } => Some(id),
        }
    }
}

impl std::fmt::Display for ToolCatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolCatalogError::DuplicateId { id } => {
                write!(f, "duplicate tool identity `{id}` in the tool catalog")
            }
        }
    }
}

impl std::error::Error for ToolCatalogError {}
