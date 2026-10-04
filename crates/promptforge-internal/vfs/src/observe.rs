//! The operation-observation seam: [`Origin`], [`OpEvent`], and the op
//! sink.
//!
//! A handle with an installed sink fires it on every admitted operation -
//! after policy and claims pass, before the backend executes - with the
//! op kind, the canonical path, and the caller-supplied [`Origin`]. The
//! seam is fire-and-forget: no outcome flows back, and a policy-denied
//! operation never fires. Claims still key on the internal
//! [`ExecId`](crate::ExecId); the origin is observability, never
//! identity. The deferred consumers - the bounded event log, the Lua
//! pull query, and enrichment policies - subscribe through this seam in
//! later steps.

use std::panic::Location;
use std::sync::Arc;

use crate::path::VfsPath;
use crate::traits::Op;

/// Who asked for a file operation: a label and the most precise source
/// position the caller knows.
///
/// An origin is for observation only. It never decides whether an
/// operation is allowed, and it never appears in a claim.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Origin {
    /// The most specific label the caller has: a section name for a
    /// chain, a tool id for a tool, a fixture name for a test.
    pub label: String,
    /// The file or document that `line` refers to: the Rust source file
    /// for [`Origin::new`], or the name the caller passes to
    /// [`Origin::at`], such as a prompt's name.
    pub file: String,
    /// The 1-based line within `file`.
    pub line: u32,
}

impl Origin {
    /// Creates an origin with the given label, positioned at the Rust call
    /// site.
    ///
    /// The file and line come from [`Location::caller`], so callers and
    /// tests get their position automatically. Use the most specific
    /// label available, such as a section name for a chain, a tool id for
    /// a tool, or a fixture name for a test.
    #[must_use]
    #[track_caller]
    pub fn new(label: impl Into<String>) -> Origin {
        let caller = Location::caller();
        Origin {
            label: label.into(),
            file: caller.file().to_owned(),
            line: caller.line(),
        }
    }

    /// Creates an origin with the given label at an explicit file and line.
    ///
    /// Use it when the caller knows a more precise position than the Rust
    /// call site, such as a line in a prompt, so each event carries the
    /// most precise position available. Choose the label as for
    /// [`Origin::new`].
    #[must_use]
    pub fn at(label: impl Into<String>, file: impl Into<String>, line: u32) -> Origin {
        Origin {
            label: label.into(),
            file: file.into(),
            line,
        }
    }
}

/// One file operation that passed the policy and claim checks, as handed
/// to the installed sink.
///
/// The event fires before the backend runs the operation. It borrows its
/// values from the `Access` capability that admitted the operation, so
/// firing is allocation-free. A sink that keeps events must clone the
/// values it needs.
#[derive(Debug)]
pub struct OpEvent<'a> {
    pub(crate) op: Op,
    pub(crate) path: &'a VfsPath,
    pub(crate) origin: &'a Origin,
}

impl<'a> OpEvent<'a> {
    /// The operation kind.
    #[must_use]
    pub fn op(&self) -> Op {
        self.op
    }

    /// The canonical path the operation acts on. Two-path operations
    /// (rename, copy) fire one event per path.
    #[must_use]
    pub fn path(&self) -> &'a VfsPath {
        self.path
    }

    /// The origin of the `Access` capability that admitted the operation.
    #[must_use]
    pub fn origin(&self) -> &'a Origin {
        self.origin
    }
}

/// A callback that receives an `OpEvent` for each admitted file
/// operation.
///
/// The sink must be cheap. Store operations call it from the blocking
/// pool, inline with the operation.
pub type OpSink = Arc<dyn Fn(OpEvent<'_>) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::Origin;

    #[test]
    fn origin_new_stamps_the_callers_file_and_line() {
        let origin = Origin::new("the fixture");
        assert_eq!(origin.line, line!() - 1, "the call site's line");
        assert!(
            origin.file.ends_with("observe.rs"),
            "the call site's file: {}",
            origin.file
        );
        assert_eq!(origin.label, "the fixture");
    }

    #[test]
    fn origin_at_holds_the_explicit_position() {
        let origin = Origin::at("the section", "the prompt", 42);
        assert_eq!(origin.label, "the section");
        assert_eq!(origin.file, "the prompt");
        assert_eq!(origin.line, 42);
    }
}
