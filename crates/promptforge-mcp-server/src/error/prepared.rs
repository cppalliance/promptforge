//! The tool-environment preparation failure and its classification.

use std::fmt;

/// Preparing the immutable tool environment (`PreparedTools::load`) failed.
///
/// The gateway model catalog being unreachable is not one of these: it is a
/// degraded-but-serving condition, logged and tolerated. These are the ways the
/// boot cannot proceed at all. Each variant wraps a private dependency source,
/// so no dependency's error type reaches this crate's public surface; a caller
/// classifies with [`PreparedToolsError::kind`] and reads causes through
/// [`std::error::Error::source`].
///
/// Opaque: the representation is private, so a caller classifies with
/// [`PreparedToolsError::kind`] and reads the underlying cause through
/// [`std::error::Error::source`], rather than matching a variant or reading a
/// public field. No dependency error type reaches this crate's public surface.
///
/// # Examples
/// ```
/// use promptforge_mcp_server::{PreparedToolsError, PreparedToolsErrorKind};
///
/// // A caller classifies with `kind` and walks the cause through `source`,
/// // never matching a private variant.
/// fn describe(err: &PreparedToolsError) -> PreparedToolsErrorKind {
///     if let Some(cause) = std::error::Error::source(err) {
///         eprintln!("prepare tools failed: {err}: {cause}");
///     }
///     err.kind()
/// }
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct PreparedToolsError {
    repr: PreparedToolsErrorRepr,
}

/// The private representation of a [`PreparedToolsError`]. Kept out of the
/// public surface so no dependency error type is exposed and the shape stays
/// free to change behind [`PreparedToolsError::kind`].
#[derive(Debug)]
enum PreparedToolsErrorRepr {
    /// The live tool registry could not be assembled. Carries the underlying
    /// error erased as its source.
    Tools {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The tool picker index could not be built. Carries the underlying error
    /// erased as its source.
    Picker {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The tool picker index could not be rebuilt over a new tool set. Carries
    /// the underlying error erased as its source. Only the test-only
    /// `PreparedTools::rebuild` raises it, so it is compiled only under `test`.
    #[cfg(test)]
    Index {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl PreparedToolsError {
    /// The live tool registry could not be assembled.
    pub(crate) fn tools(
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> PreparedToolsError {
        PreparedToolsError {
            repr: PreparedToolsErrorRepr::Tools {
                source: source.into(),
            },
        }
    }

    /// The tool picker index could not be built.
    pub(crate) fn picker(
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> PreparedToolsError {
        PreparedToolsError {
            repr: PreparedToolsErrorRepr::Picker {
                source: source.into(),
            },
        }
    }

    /// The tool picker index could not be rebuilt over a new tool set. Only the
    /// test-only `PreparedTools::rebuild` raises it.
    #[cfg(test)]
    pub(crate) fn index(
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> PreparedToolsError {
        PreparedToolsError {
            repr: PreparedToolsErrorRepr::Index {
                source: source.into(),
            },
        }
    }
}

impl fmt::Display for PreparedToolsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            PreparedToolsErrorRepr::Tools { .. } => f.write_str("assemble the live tool registry"),
            PreparedToolsErrorRepr::Picker { .. } => f.write_str("build the tool picker index"),
            #[cfg(test)]
            PreparedToolsErrorRepr::Index { .. } => f.write_str("rebuild the tool picker index"),
        }
    }
}

impl std::error::Error for PreparedToolsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            PreparedToolsErrorRepr::Tools { source }
            | PreparedToolsErrorRepr::Picker { source } => Some(source.as_ref()),
            #[cfg(test)]
            PreparedToolsErrorRepr::Index { source } => Some(source.as_ref()),
        }
    }
}

/// A stable, dependency-free classification of a [`PreparedToolsError`].
///
/// # Examples
/// ```
/// use promptforge_mcp_server::PreparedToolsErrorKind;
///
/// // A plain `Copy` value a caller can match or store.
/// fn is_picker_failure(kind: PreparedToolsErrorKind) -> bool {
///     matches!(kind, PreparedToolsErrorKind::Picker)
/// }
/// assert!(is_picker_failure(PreparedToolsErrorKind::Picker));
/// assert_ne!(PreparedToolsErrorKind::Tools, PreparedToolsErrorKind::Picker);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PreparedToolsErrorKind {
    /// The live tool registry could not be assembled.
    Tools,
    /// The tool picker index could not be built.
    Picker,
    /// The tool picker index could not be rebuilt.
    Index,
}

impl PreparedToolsError {
    /// Classifies the failure without exposing the error's representation.
    ///
    /// # Examples
    /// ```
    /// use promptforge_mcp_server::{PreparedToolsError, PreparedToolsErrorKind};
    ///
    /// fn registry_failed(err: &PreparedToolsError) -> bool {
    ///     err.kind() == PreparedToolsErrorKind::Tools
    /// }
    /// ```
    #[must_use]
    pub fn kind(&self) -> PreparedToolsErrorKind {
        match &self.repr {
            PreparedToolsErrorRepr::Tools { .. } => PreparedToolsErrorKind::Tools,
            PreparedToolsErrorRepr::Picker { .. } => PreparedToolsErrorKind::Picker,
            #[cfg(test)]
            PreparedToolsErrorRepr::Index { .. } => PreparedToolsErrorKind::Index,
        }
    }
}
