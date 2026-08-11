//! The filesystem-watch start failure and its classification.

use std::fmt;
use std::path::PathBuf;

/// A filesystem watch that could not be established.
///
/// Only starting the watcher fails this way. Once it is running, a reload that
/// cannot re-resolve the catalog keeps the previous one and logs why, because a
/// typo in one file must not take the running service down with it.
///
/// Opaque: the representation is private, so a caller classifies with
/// [`WatchError::kind`], reads the failing path (when one is known) through
/// [`WatchError::path`], and reads the underlying cause through
/// [`std::error::Error::source`], rather than matching a variant or reading a
/// public field. No `String` payload or dependency error type reaches this
/// crate's public surface.
///
/// # Examples
/// ```
/// use promptforge_mcp_server::{WatchError, WatchErrorKind};
///
/// // A caller classifies with `kind`, reads the failing path when one is
/// // known, and walks the cause through `source`.
/// fn describe(err: &WatchError) -> WatchErrorKind {
///     if let Some(path) = err.path() {
///         eprintln!("could not watch {}", path.display());
///     }
///     let _ = std::error::Error::source(err);
///     err.kind()
/// }
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct WatchError {
    repr: WatchErrorRepr,
}

/// The private representation of a [`WatchError`]. Kept out of the public
/// surface so no `String` payload or dependency error type is exposed and the
/// shape stays free to change behind [`WatchError::kind`].
#[derive(Debug)]
enum WatchErrorRepr {
    /// The platform watcher could not be created. Carries the underlying error
    /// erased as its source.
    Create {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// A path could not be watched. Carries the path losslessly and the
    /// underlying error erased as its source.
    Watch {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The watcher was started outside a Tokio runtime, so its debounce task
    /// had nowhere to run.
    Runtime,
}

impl WatchError {
    /// The platform watcher could not be created.
    pub(crate) fn create(
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> WatchError {
        WatchError {
            repr: WatchErrorRepr::Create {
                source: source.into(),
            },
        }
    }

    /// A path could not be watched.
    pub(crate) fn watch(
        path: PathBuf,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> WatchError {
        WatchError {
            repr: WatchErrorRepr::Watch {
                path,
                source: source.into(),
            },
        }
    }

    /// The watcher was started with no Tokio runtime to run its task on.
    pub(crate) fn runtime() -> WatchError {
        WatchError {
            repr: WatchErrorRepr::Runtime,
        }
    }

    /// The path a watch failure names, losslessly, when the failure is about a
    /// specific path. `None` otherwise.
    ///
    /// # Examples
    /// ```
    /// use promptforge_mcp_server::WatchError;
    ///
    /// fn failing_path(err: &WatchError) -> Option<&std::path::Path> {
    ///     err.path()
    /// }
    /// ```
    #[must_use]
    pub fn path(&self) -> Option<&std::path::Path> {
        match &self.repr {
            WatchErrorRepr::Watch { path, .. } => Some(path.as_path()),
            _ => None,
        }
    }
}

impl fmt::Display for WatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            WatchErrorRepr::Create { .. } => f.write_str("create the filesystem watcher"),
            WatchErrorRepr::Watch { path, .. } => write!(f, "watch {}", path.display()),
            WatchErrorRepr::Runtime => {
                f.write_str("start a filesystem watch outside a tokio runtime")
            }
        }
    }
}

impl std::error::Error for WatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.repr {
            WatchErrorRepr::Create { source } | WatchErrorRepr::Watch { source, .. } => {
                Some(source.as_ref())
            }
            WatchErrorRepr::Runtime => None,
        }
    }
}

/// A stable, dependency-free classification of a [`WatchError`].
///
/// # Examples
/// ```
/// use promptforge_mcp_server::WatchErrorKind;
///
/// // A plain `Copy` value a caller can match or store.
/// fn names_a_path(kind: WatchErrorKind) -> bool {
///     matches!(kind, WatchErrorKind::Watch)
/// }
/// assert!(names_a_path(WatchErrorKind::Watch));
/// assert_ne!(WatchErrorKind::Create, WatchErrorKind::Runtime);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WatchErrorKind {
    /// The platform watcher could not be created.
    Create,
    /// A path could not be watched.
    Watch,
    /// The watcher was started outside a Tokio runtime.
    Runtime,
}

impl WatchError {
    /// Classifies the failure without exposing the error's representation.
    ///
    /// # Examples
    /// ```
    /// use promptforge_mcp_server::{WatchError, WatchErrorKind};
    ///
    /// fn could_not_create(err: &WatchError) -> bool {
    ///     err.kind() == WatchErrorKind::Create
    /// }
    /// ```
    #[must_use]
    pub fn kind(&self) -> WatchErrorKind {
        match &self.repr {
            WatchErrorRepr::Create { .. } => WatchErrorKind::Create,
            WatchErrorRepr::Watch { .. } => WatchErrorKind::Watch,
            WatchErrorRepr::Runtime => WatchErrorKind::Runtime,
        }
    }
}
